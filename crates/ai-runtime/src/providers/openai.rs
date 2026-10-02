//! OpenAI Provider — GPT (RFC-0010 §3)
//!
//! API: https://platform.openai.com/docs/api-reference/chat
//! 特点：system prompt 是 messages 数组的第一项（role=system）。
//! 认证：Authorization: Bearer 头。
//! API Key 从 OPENAI_API_KEY 环境变量读取。

use crate::provider::{
    AiError, AiProvider, GenerateRequest, GenerateResponse, Role, StopReason, StreamEvent,
    TokenUsage, ToolCall,
};
use crate::providers::{
    api_key_from_env, round_temperature, HttpClient, HttpStreamClient, ProviderAdapter,
    ThinkingConfig,
};

// ============================================================================
// ApiMode — Chat Completions vs Responses API
// ============================================================================

/// OpenAI Provider 的 API 模式（Profile 级配置）
///
/// - `Chat` → 经典 `POST /v1/chat/completions`（默认，最大兼容性）
/// - `Responses` → 新版 `POST /v1/responses`（GPT-5 / o 系列原生接口，
///   支持更丰富的内置工具与 reasoning）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiMode {
    #[default]
    Chat,
    Responses,
}

// ============================================================================
// OpenAiProvider
// ============================================================================

#[derive(Debug)]
pub struct OpenAiProvider {
    client: HttpClient,
    model: String,
    api_key: Option<String>,
    api_url: Option<String>,
    /// 为 true 时直接使用 api_url 原样，不追加路径也不做格式检测
    api_url_full: bool,
    /// 自定义请求体字段（深合并到请求 JSON）
    extra_body: Option<serde_json::Value>,
    /// 自定义 HTTP 头（追加到请求头）
    extra_headers: Vec<(String, String)>,
    /// 思考强度（Chat → reasoning_effort；Responses → reasoning.effort；
    /// 智谱/通义按 URL 识别为原生开关参数）
    thinking: Option<ThinkingConfig>,
    /// API 模式：Chat Completions 或 Responses
    api_mode: ApiMode,
    /// 单次请求超时（秒）
    timeout_secs: u64,
    /// 档案级输出上限（含思考 token）；None = 用请求自带值
    max_tokens: Option<usize>,
}

impl OpenAiProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        api_url: Option<String>,
        api_url_full: bool,
        extra_body: Option<serde_json::Value>,
        extra_headers: Vec<(String, String)>,
        thinking: Option<ThinkingConfig>,
        api_mode: ApiMode,
        timeout_secs: Option<u64>,
        max_tokens: Option<usize>,
    ) -> Self {
        Self {
            client: HttpClient::new(),
            model: model.into(),
            api_key,
            api_url,
            api_url_full,
            extra_body,
            extra_headers,
            thinking,
            api_mode,
            timeout_secs: super::normalize_timeout_secs(timeout_secs),
            max_tokens,
        }
    }

    /// 从 Provider 构造 adapter（共用逻辑，供 generate / stream_generate 调用）
    ///
    /// 按 `api_mode` 分流到 Chat Completions 或 Responses 适配器。
    fn build_adapter(&self, request: &GenerateRequest) -> Box<dyn ProviderAdapter + Sync> {
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
        let shared = AdapterShared {
            model,
            api_key: self.api_key.clone(),
            api_url: self.api_url.clone(),
            api_url_full: self.api_url_full,
            extra_body: self.extra_body.clone(),
            extra_headers: self.extra_headers.clone(),
            thinking: self.thinking.clone(),
            timeout_secs: self.timeout_secs,
        };
        match self.api_mode {
            ApiMode::Chat => Box::new(OpenAiAdapter { shared }),
            ApiMode::Responses => Box::new(OpenAiResponsesAdapter { shared }),
        }
    }
}

/// Chat / Responses 两个适配器共享的配置字段（两者仅请求/响应格式不同）。
#[derive(Default)]
struct AdapterShared {
    model: String,
    api_key: Option<String>,
    api_url: Option<String>,
    api_url_full: bool,
    extra_body: Option<serde_json::Value>,
    extra_headers: Vec<(String, String)>,
    thinking: Option<ThinkingConfig>,
    timeout_secs: u64,
}

impl AiProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
        let request = super::with_profile_max_tokens(self.max_tokens, request);
        let adapter = self.build_adapter(&request);
        self.client.send(&*adapter, &request)
    }

    fn stream_generate(
        &self,
        request: &GenerateRequest,
        on_event: &(dyn Fn(StreamEvent) + Send + Sync),
    ) -> Result<(), AiError> {
        let request = super::with_profile_max_tokens(self.max_tokens, request);
        let adapter = self.build_adapter(&request);

        let full_text = std::sync::Mutex::new(String::new());
        let final_usage = std::sync::Mutex::new(TokenUsage::default());

        // Local callback that accumulates text
        let local_on = |event: StreamEvent| match event {
            StreamEvent::Delta { ref delta } => {
                if let Ok(mut t) = full_text.lock() {
                    t.push_str(delta);
                }
                on_event(StreamEvent::Delta {
                    delta: delta.clone(),
                });
            }
            StreamEvent::Done { ref usage, .. } => {
                if let Ok(mut u) = final_usage.lock() {
                    *u = usage.clone();
                }
                on_event(event);
            }
            StreamEvent::ToolUseStart { .. } | StreamEvent::ToolUseDelta { .. } => {
                on_event(event);
            }
            StreamEvent::Error { .. } => {
                on_event(event);
            }
        };

        // Avoid "Cannot start a runtime from within a runtime" panic:
        // - Inside a tokio runtime → use block_in_place before block_on
        // - Outside → create a fresh temporary runtime
        let stream_fn =
            || async { HttpStreamClient::stream(&*adapter, &request, &local_on).await };

        let result = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(stream_fn())),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| AiError::Network(format!("tokio runtime: {e}")))?;
                rt.block_on(stream_fn())
            }
        };

        match result {
            Ok(()) => {
                let full = full_text.lock().unwrap().clone();
                let usage = final_usage.lock().unwrap().clone();
                on_event(StreamEvent::Done {
                    full_text: full,
                    usage,
                    stop_reason: None,
                });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

// ============================================================================
// OpenAiAdapter — Chat Completions 请求/响应转换
// ============================================================================

#[derive(Default)]
struct OpenAiAdapter {
    shared: AdapterShared,
}

impl ProviderAdapter for OpenAiAdapter {
    fn id(&self) -> &str {
        "openai"
    }

    fn endpoint(&self) -> String {
        if let Some(ref url) = self.shared.api_url {
            // 完整 URL 模式：直接使用，不做任何处理
            if self.shared.api_url_full {
                return url.clone();
            }
            let base = url.trim_end_matches('/');
            // 兜底：用户已提供完整路径则直通
            if base.ends_with("/chat/completions") || base.ends_with("/v1/chat/completions") {
                return base.to_string();
            }
            // base 已带 /v1（常见手填写法）先剥掉，避免拼出 /v1/v1/ 双段
            let base = base.strip_suffix("/v1").unwrap_or(base);
            return format!("{base}/v1/chat/completions");
        }
        "https://api.openai.com/v1/chat/completions".to_string()
    }

    fn api_key(&self) -> Result<String, AiError> {
        if let Some(ref key) = self.shared.api_key {
            return Ok(key.clone());
        }
        api_key_from_env("OPENAI_API_KEY")
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn custom_headers(&self) -> &[(String, String)] {
        &self.shared.extra_headers
    }

    fn custom_body(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.shared.extra_body.as_ref().and_then(|v| v.as_object())
    }

    fn thinking_config(&self) -> Option<&ThinkingConfig> {
        self.shared.thinking.as_ref()
    }

    // Chat 模式按 URL 识别方言（智谱 → thinking.type；通义 → enable_thinking）
    fn thinking_dialect(&self) -> super::ThinkingDialect {
        super::thinking_dialect_for_url(self.shared.api_url.as_deref())
    }

    fn timeout_secs(&self) -> u64 {
        self.shared.timeout_secs
    }

    fn to_request_body(&self, request: &GenerateRequest) -> serde_json::Value {
        let mut messages: Vec<serde_json::Value> = Vec::new();

        // system message（非空时插入）
        if !request.system.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": request.system,
            }));
        }

        // 消息转换
        for msg in &request.messages {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let mut msg_obj = serde_json::json!({ "role": role });

            // Handle tool calls in assistant messages
            if !msg.tool_calls.is_empty() {
                let oai_calls: Vec<serde_json::Value> = msg
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            }
                        })
                    })
                    .collect();
                msg_obj["tool_calls"] = serde_json::Value::Array(oai_calls);
                if !msg.content.is_empty() {
                    msg_obj["content"] = serde_json::Value::String(msg.content.clone());
                }
            } else if let Some(ref tc_id) = msg.tool_call_id {
                // Tool result message
                msg_obj["tool_call_id"] = serde_json::Value::String(tc_id.clone());
                msg_obj["content"] = serde_json::Value::String(msg.content.clone());
            } else {
                msg_obj["content"] = serde_json::Value::String(msg.content.clone());
            }

            messages.push(msg_obj);
        }

        let mut body = serde_json::json!({
            "model": self.shared.model,
            "messages": messages,
            "temperature": round_temperature(request.temperature),
            "max_tokens": request.max_tokens,
        });

        // Add tools if present
        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        body
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<GenerateResponse, AiError> {
        let choices = body
            .get("choices")
            .and_then(|c| c.as_array())
            .ok_or_else(|| AiError::Parse(format!("无法解析 OpenAI 响应: {body}")))?;

        let first = choices.first();
        let message = first.and_then(|c| c.get("message"));

        // Text from content field
        let text = message
            .and_then(|m| m.get("content"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        // Tool calls from tool_calls field
        let tool_calls: Vec<ToolCall> = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let id = tc.get("id")?.as_str()?.to_string();
                        let func = tc.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let args_str = func.get("arguments")?.as_str().unwrap_or("{}");
                        let arguments: serde_json::Value =
                            serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                        Some(ToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = first
            .and_then(|c| c.get("finish_reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("stop");

        let stop_reason = match finish_reason {
            "tool_calls" => StopReason::ToolUse,
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let usage = body.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|t| t.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|t| t.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);

        Ok(GenerateResponse {
            text,
            usage: TokenUsage {
                input_tokens,
                output_tokens,
            },
            stop_reason,
            tool_calls,
        })
    }

    fn parse_stream_chunk(&self, line: &str) -> Option<StreamEvent> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return None,
        };

        let choices = value.get("choices")?.as_array()?;
        let first_choice = choices.first()?;

        // finish_reason 出现在最后一个 chunk（delta 可能为空对象）
        let finish_reason = first_choice.get("finish_reason").and_then(|r| r.as_str());
        if let Some(fr) = finish_reason {
            let usage = value.get("usage");
            let output_tokens = usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as usize;
            let stop_reason = match fr {
                "tool_calls" => Some(StopReason::ToolUse),
                "stop" => Some(StopReason::EndTurn),
                "length" => Some(StopReason::MaxTokens),
                _ => Some(StopReason::EndTurn),
            };
            return Some(StreamEvent::Done {
                full_text: String::new(),
                usage: TokenUsage {
                    input_tokens: usage
                        .and_then(|u| u.get("prompt_tokens"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0) as usize,
                    output_tokens,
                },
                stop_reason,
            });
        }

        let delta = first_choice.get("delta")?;

        // tool_calls 增量（首帧含 index+id+name，后续帧含 index+arguments 片段）
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                // 首帧：function.name + id
                if let (Some(id), Some(name)) = (
                    tc.get("id").and_then(|i| i.as_str()),
                    tc.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str()),
                ) {
                    return Some(StreamEvent::ToolUseStart {
                        index,
                        id: id.to_string(),
                        name: name.to_string(),
                    });
                }
                // 后续帧：function.arguments 片段
                if let Some(args) = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    if !args.is_empty() {
                        return Some(StreamEvent::ToolUseDelta {
                            index,
                            partial_json: args.to_string(),
                        });
                    }
                }
            }
        }

        // Regular content delta
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                return Some(StreamEvent::Delta {
                    delta: content.to_string(),
                });
            }
        }

        None
    }
}

// ============================================================================
// OpenAiResponsesAdapter — Responses API 请求/响应转换
// ============================================================================
//
// OpenAI Responses API（POST /v1/responses）与 Chat Completions 的主要差异：
// - system prompt → 顶层 `instructions` 字段（而非 messages 首项）
// - 消息 → `input` 数组，工具调用/结果是独立的 item（而非嵌在 message 内）
// - 工具定义扁平：`{type:"function", name, description, parameters}`
//   （Chat 是嵌套 `{type:"function", function:{...}}`）
// - token 上限字段为 `max_output_tokens`（Chat 是 `max_tokens`）
// - reasoning 模型用嵌套 `reasoning:{effort:...}`（Chat 是顶层 `reasoning_effort`）
// - 响应的文本/工具调用在 `output[]` 数组里，按 item 类型区分
// - usage 字段名为 `input_tokens`/`output_tokens`（Chat 是 prompt/completion）

#[derive(Default)]
struct OpenAiResponsesAdapter {
    shared: AdapterShared,
}

impl ProviderAdapter for OpenAiResponsesAdapter {
    fn id(&self) -> &str {
        "openai-responses"
    }

    fn endpoint(&self) -> String {
        if let Some(ref url) = self.shared.api_url {
            // 完整 URL 模式：直接使用，不做任何处理
            if self.shared.api_url_full {
                return url.clone();
            }
            let base = url.trim_end_matches('/');
            // 兜底：用户已提供完整路径则直通
            if base.ends_with("/responses") || base.ends_with("/v1/responses") {
                return base.to_string();
            }
            let base = base.strip_suffix("/v1").unwrap_or(base);
            return format!("{base}/v1/responses");
        }
        "https://api.openai.com/v1/responses".to_string()
    }

    fn api_key(&self) -> Result<String, AiError> {
        if let Some(ref key) = self.shared.api_key {
            return Ok(key.clone());
        }
        api_key_from_env("OPENAI_API_KEY")
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn custom_headers(&self) -> &[(String, String)] {
        &self.shared.extra_headers
    }

    fn custom_body(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.shared.extra_body.as_ref().and_then(|v| v.as_object())
    }

    fn thinking_config(&self) -> Option<&ThinkingConfig> {
        self.shared.thinking.as_ref()
    }

    // Responses API 是 OpenAI 原生接口，始终用 reasoning.effort（默认方言）
    fn timeout_secs(&self) -> u64 {
        self.shared.timeout_secs
    }

    fn to_request_body(&self, request: &GenerateRequest) -> serde_json::Value {
        let mut input: Vec<serde_json::Value> = Vec::new();

        // 消息 → input items
        for msg in &request.messages {
            match msg.role {
                Role::User => {
                    input.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content,
                    }));
                }
                Role::Assistant => {
                    // 助手文本（即使有 tool_calls，content 非空时也先放文本 item）
                    if !msg.content.is_empty() {
                        input.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content,
                        }));
                    }
                    // 工具调用：每个 tool_call 是一个独立 function_call item
                    for tc in &msg.tool_calls {
                        input.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                        }));
                    }
                }
                Role::Tool => {
                    // 工具结果：function_call_output item
                    // call_id 关联到上一步的 function_call
                    let call_id = msg.tool_call_id.clone().unwrap_or_default();
                    input.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": msg.content,
                    }));
                }
                Role::System => {
                    // 罕见：消息流中夹杂的 system 角色 → 映射为 developer item
                    input.push(serde_json::json!({
                        "role": "developer",
                        "content": msg.content,
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.shared.model,
            "input": input,
            "temperature": round_temperature(request.temperature),
            "max_output_tokens": request.max_tokens,
        });

        // system prompt → 顶层 instructions（Responses API 的原生位置）
        if !request.system.is_empty() {
            body["instructions"] = serde_json::Value::String(request.system.clone());
        }

        // 工具定义（扁平结构：name/description/parameters 不嵌套在 function 下）
        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        body
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<GenerateResponse, AiError> {
        let output = body
            .get("output")
            .and_then(|o| o.as_array())
            .ok_or_else(|| AiError::Parse(format!("无法解析 Responses 响应: {body}")))?;

        let mut text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        // 遍历 output 数组，按 item 类型提取文本与工具调用
        for item in output {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    // message item 的 content 是数组，其中 output_text 项的 text 拼接成完整文本
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for part in content {
                            let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if part_type == "output_text" {
                                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                                    text.push_str(t);
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    // function_call item：call_id / name / arguments（JSON 字符串）
                    let id = item
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args_str = item
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}");
                    let arguments: serde_json::Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }

        // stop_reason：按 status 与是否含工具调用推断
        let status = body.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let stop_reason = if status == "incomplete" {
            StopReason::MaxTokens
        } else if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        // usage（Responses 用 input_tokens / output_tokens）
        let usage = body.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);

        Ok(GenerateResponse {
            text,
            usage: TokenUsage {
                input_tokens,
                output_tokens,
            },
            stop_reason,
            tool_calls,
        })
    }

    fn parse_stream_chunk(&self, line: &str) -> Option<StreamEvent> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return None,
        };

        // Responses 流式事件在 data JSON 内用 type 字段标识事件类型
        let event_type = value.get("type").and_then(|t| t.as_str())?;

        match event_type {
            // 文本增量
            "response.output_text.delta" => {
                let delta = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !delta.is_empty() {
                    return Some(StreamEvent::Delta {
                        delta: delta.to_string(),
                    });
                }
                None
            }
            // 新 output item（function_call 在此声明 call_id + name）
            "response.output_item.added" => {
                let item = value.get("item")?;
                let item_type = item.get("type").and_then(|t| t.as_str())?;
                if item_type == "function_call" {
                    let index = value
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .unwrap_or(0) as usize;
                    let id = item
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Some(StreamEvent::ToolUseStart { index, id, name });
                }
                None
            }
            // function_call arguments 增量
            "response.function_call_arguments.delta" => {
                let index = value
                    .get("output_index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(0) as usize;
                let delta = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !delta.is_empty() {
                    return Some(StreamEvent::ToolUseDelta {
                        index,
                        partial_json: delta.to_string(),
                    });
                }
                None
            }
            // 流结束：携带完整 response（含 usage）
            "response.completed" => {
                let response = value.get("response").unwrap_or(&value);
                let usage = response.get("usage");
                let input_tokens = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as usize;
                let output_tokens = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as usize;

                // 按 status + output 项推断 stop_reason
                let status = response
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                let has_function_call = response
                    .get("output")
                    .and_then(|o| o.as_array())
                    .map(|arr| {
                        arr.iter().any(|i| {
                            i.get("type").and_then(|t| t.as_str()) == Some("function_call")
                        })
                    })
                    .unwrap_or(false);
                let stop_reason = if status == "incomplete" {
                    Some(StopReason::MaxTokens)
                } else if has_function_call {
                    Some(StopReason::ToolUse)
                } else {
                    Some(StopReason::EndTurn)
                };

                Some(StreamEvent::Done {
                    full_text: String::new(),
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens,
                    },
                    stop_reason,
                })
            }
            // 其余事件（response.created / response.in_progress / response.output_item.done /
            // response.output_text.done / response.function_call_arguments.done / *.failed 等）
            // 不携带需要转发的增量，忽略。
            _ => None,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Message, ToolDef};

    #[test]
    fn build_request_body_with_system() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };

        let request = GenerateRequest {
            system: "你是小说作者".to_string(),
            messages: vec![Message::user("续写故事")],
            model: String::new(),
            temperature: 0.8,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["max_tokens"], 4096);
        // system 应该是 messages 第一项
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "你是小说作者");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "续写故事");
    }

    #[test]
    fn build_request_body_no_system() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };

        let request = GenerateRequest {
            system: String::new(),
            messages: vec![Message::user("续写")],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);

        // 无 system 时，messages 只有 user
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn parse_response_success() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };

        let response = serde_json::json!({
            "choices": [
                {
                    "message": { "role": "assistant", "content": "林风推开门。" },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        });

        let result = adapter.parse_response(&response).unwrap();

        assert_eq!(result.text, "林风推开门。");
        assert_eq!(result.usage.input_tokens, 50);
        assert_eq!(result.usage.output_tokens, 30);
    }

    #[test]
    fn parse_response_error() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };

        let bad_response = serde_json::json!({ "error": { "message": "bad request" } });

        let result = adapter.parse_response(&bad_response);
        assert!(result.is_err());
    }

    #[test]
    fn endpoint_custom_url_appends_v1() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "deepseek-chat".to_string(),
                api_url: Some("https://api.deepseek.com".to_string()),
                ..Default::default()
            },
        };
        assert_eq!(
            adapter.endpoint(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_custom_url_full_path_passthrough() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "deepseek-chat".to_string(),
                api_url: Some("https://custom.example.com/v1/chat/completions".to_string()),
                ..Default::default()
            },
        };
        assert_eq!(
            adapter.endpoint(),
            "https://custom.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_default() {
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };
        assert_eq!(
            adapter.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_full_url_mode_verbatim() {
        // 完整 URL 模式：不追加路径，不做格式检测，原样使用
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "deepseek-chat".to_string(),
                api_url: Some("https://custom-proxy.example.com/api/chat".to_string()),
                api_url_full: true,
                ..Default::default()
            },
        };
        assert_eq!(
            adapter.endpoint(),
            "https://custom-proxy.example.com/api/chat"
        );
    }

    // —— 流式 tool_call 解析测试 ——

    fn stream_adapter() -> OpenAiAdapter {
        OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn parse_tool_calls_first_chunk() {
        // OpenAI tool_calls 首帧：含 index + id + function.name
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"write_fragment_text","arguments":""}}]}}]}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseStart { index, id, name } => {
                assert_eq!(index, 0);
                assert_eq!(id, "call_abc");
                assert_eq!(name, "write_fragment_text");
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_tool_calls_arguments_delta() {
        // OpenAI tool_calls 后续帧：只有 arguments 片段
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"text\":\"正文\""}}]}}]}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 0);
                assert!(partial_json.contains("正文"));
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_finish_reason_tool_calls() {
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::Done {
                stop_reason, usage, ..
            } => {
                assert_eq!(stop_reason, Some(StopReason::ToolUse));
                assert_eq!(usage.output_tokens, 20);
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }
    }

    // ========================================================================
    // OpenAiResponsesAdapter 测试
    // ========================================================================

    fn responses_adapter() -> OpenAiResponsesAdapter {
        OpenAiResponsesAdapter {
            shared: AdapterShared {
                model: "gpt-5".to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn responses_endpoint_default() {
        let adapter = responses_adapter();
        assert_eq!(adapter.endpoint(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn responses_endpoint_custom_base_appends_v1() {
        let adapter = OpenAiResponsesAdapter {
            shared: AdapterShared {
                model: "gpt-5".to_string(),
                api_url: Some("https://api.openai.com".to_string()),
                ..Default::default()
            },
        };
        assert_eq!(adapter.endpoint(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn responses_endpoint_full_path_passthrough() {
        let adapter = OpenAiResponsesAdapter {
            shared: AdapterShared {
                model: "gpt-5".to_string(),
                api_url: Some("https://custom.example.com/v1/responses".to_string()),
                ..Default::default()
            },
        };
        assert_eq!(
            adapter.endpoint(),
            "https://custom.example.com/v1/responses"
        );
    }

    #[test]
    fn responses_endpoint_full_url_mode_verbatim() {
        let adapter = OpenAiResponsesAdapter {
            shared: AdapterShared {
                model: "gpt-5".to_string(),
                api_url: Some("https://proxy.example.com/relay".to_string()),
                api_url_full: true,
                ..Default::default()
            },
        };
        assert_eq!(adapter.endpoint(), "https://proxy.example.com/relay");
    }

    #[test]
    fn responses_build_request_body_basic() {
        let adapter = responses_adapter();
        let request = GenerateRequest {
            system: "你是小说作者".to_string(),
            messages: vec![Message::user("续写故事")],
            model: String::new(),
            temperature: 0.8,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);

        // system → 顶层 instructions（而非 messages 首项）
        assert_eq!(body["instructions"], "你是小说作者");
        assert_eq!(body["model"], "gpt-5");
        // max_output_tokens 而非 max_tokens
        assert_eq!(body["max_output_tokens"], 4096);
        assert!(body.get("max_tokens").is_none());
        // input 数组含一条 user item
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "续写故事");
    }

    #[test]
    fn responses_build_request_body_with_tools() {
        let adapter = responses_adapter();
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![Message::user("写一段")],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: vec![ToolDef {
                name: "write_fragment".to_string(),
                description: "写入正文".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        };

        let body = adapter.to_request_body(&request);

        // 工具扁平结构：name/description/parameters 不嵌套在 function 下
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "write_fragment");
        assert_eq!(body["tools"][0]["description"], "写入正文");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
        // 不应有嵌套的 function 字段
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn responses_build_request_body_tool_history() {
        let adapter = responses_adapter();
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("调用工具"),
                // 助手发起工具调用（带 content 文本 + tool_calls）
                Message::assistant_with_tools(
                    "正在调用".to_string(),
                    vec![ToolCall {
                        id: "call_123".to_string(),
                        name: "write_fragment".to_string(),
                        arguments: serde_json::json!({"text": "正文"}),
                    }],
                ),
                // 工具返回结果
                Message::tool("call_123".to_string(), "写入成功"),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);
        let input = body["input"].as_array().unwrap();

        // user → {role:"user"}
        assert_eq!(input[0]["role"], "user");
        // 助手文本 → {role:"assistant"}
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "正在调用");
        // 助手工具调用 → 独立 function_call item（call_id + name + arguments 字符串）
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_123");
        assert_eq!(input[2]["name"], "write_fragment");
        assert_eq!(input[2]["arguments"], r#"{"text":"正文"}"#);
        // 工具结果 → function_call_output item
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_123");
        assert_eq!(input[3]["output"], "写入成功");
    }

    #[test]
    fn responses_parse_response_text() {
        let adapter = responses_adapter();
        let response = serde_json::json!({
            "id": "resp_abc",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "林风推开门。" }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30
            }
        });

        let result = adapter.parse_response(&response).unwrap();

        assert_eq!(result.text, "林风推开门。");
        assert_eq!(result.usage.input_tokens, 50);
        assert_eq!(result.usage.output_tokens, 30);
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert!(result.tool_calls.is_empty());
    }

    #[test]
    fn responses_parse_response_function_call() {
        let adapter = responses_adapter();
        let response = serde_json::json!({
            "id": "resp_abc",
            "status": "completed",
            "output": [
                {
                    "type": "function_call",
                    "call_id": "call_xyz",
                    "name": "write_fragment",
                    "arguments": r#"{"text":"正文内容"}"#
                }
            ],
            "usage": {
                "input_tokens": 40,
                "output_tokens": 20
            }
        });

        let result = adapter.parse_response(&response).unwrap();

        assert!(result.text.is_empty());
        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].id, "call_xyz");
        assert_eq!(result.tool_calls[0].name, "write_fragment");
        assert_eq!(result.tool_calls[0].arguments["text"], "正文内容");
    }

    #[test]
    fn responses_parse_response_incomplete_status() {
        let adapter = responses_adapter();
        let response = serde_json::json!({
            "id": "resp_abc",
            "status": "incomplete",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "被截断的文本" }
                    ]
                }
            ],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });

        let result = adapter.parse_response(&response).unwrap();
        // incomplete → MaxTokens
        assert_eq!(result.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn responses_parse_stream_text_delta() {
        let adapter = responses_adapter();
        let line = r#"{"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"林风"}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::Delta { delta } => assert_eq!(delta, "林风"),
            other => panic!("应为 Delta，实际: {other:?}"),
        }
    }

    #[test]
    fn responses_parse_stream_function_call_start() {
        let adapter = responses_adapter();
        let line = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_abc","name":"write_fragment","arguments":""}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseStart { index, id, name } => {
                assert_eq!(index, 0);
                assert_eq!(id, "call_abc");
                assert_eq!(name, "write_fragment");
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
    }

    #[test]
    fn responses_parse_stream_function_call_arguments_delta() {
        let adapter = responses_adapter();
        let line = r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"text\":\"正文\""}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 0);
                assert!(partial_json.contains("正文"));
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
    }

    #[test]
    fn responses_parse_stream_completed() {
        let adapter = responses_adapter();
        let line = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"你好"}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::Done {
                stop_reason, usage, ..
            } => {
                assert_eq!(stop_reason, Some(StopReason::EndTurn));
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }
    }

    #[test]
    fn responses_parse_stream_completed_with_function_call() {
        let adapter = responses_adapter();
        let line = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"function_call","call_id":"c1","name":"f","arguments":"{}"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, Some(StopReason::ToolUse));
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }
    }

    #[test]
    fn responses_parse_stream_ignores_other_events() {
        let adapter = responses_adapter();
        // response.created / response.in_progress 等事件应返回 None
        assert!(adapter
            .parse_stream_chunk(r#"{"type":"response.created","response":{"id":"resp_1"}}"#)
            .is_none());
        assert!(adapter
            .parse_stream_chunk(r#"{"type":"response.output_text.done","output_index":0}"#)
            .is_none());
    }
}

#[cfg(test)]
mod v1_normalize_tests {
    use super::*;

    #[test]
    fn chat_endpoint_strips_trailing_v1() {
        let provider = OpenAiProvider::new(
            "m",
            None,
            Some("https://api.deepseek.com/v1".to_string()),
            false,
            None,
            vec![],
            None,
            ApiMode::Chat,
            None,
            None,
        );
        let adapter = provider.build_adapter(&GenerateRequest::new("s", "u"));
        assert_eq!(
            adapter.endpoint(),
            "https://api.deepseek.com/v1/chat/completions",
            "base 带 /v1 不应拼出 /v1/v1/"
        );
    }

    #[test]
    fn responses_endpoint_strips_trailing_v1() {
        let provider = OpenAiProvider::new(
            "m",
            None,
            Some("https://api.openai.com/v1".to_string()),
            false,
            None,
            vec![],
            None,
            ApiMode::Responses,
            None,
            None,
        );
        let adapter = provider.build_adapter(&GenerateRequest::new("s", "u"));
        assert_eq!(adapter.endpoint(), "https://api.openai.com/v1/responses");
    }
}
