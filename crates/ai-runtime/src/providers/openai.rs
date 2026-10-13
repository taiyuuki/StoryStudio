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
        let stream_fn = || async { HttpStreamClient::stream(&*adapter, &request, &local_on).await };

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
// Gemini thought_signature 提取 / 注入（OpenAI 兼容端点）
// ============================================================================
//
// Gemini 2.5+/3 思考模型通过 OpenAI 兼容端点返回工具调用时，会在
// tool_calls[] 上附带非标准字段 extra_content.google.thought_signature
// （部分 LiteLLM 风格网关放在顶层 thought_signature）。
// 多轮工具调用回放 assistant 历史时必须原样带回，否则 API 返回 400：
// "Function call is missing a thought_signature in functionCall parts"。

/// 从 OpenAI 格式的 tool_call JSON 中提取 thought_signature
fn extract_thought_signature(tc: &serde_json::Value) -> Option<String> {
    let sig = tc
        .pointer("/extra_content/google/thought_signature")
        .or_else(|| tc.get("thought_signature"))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty());
    sig.map(|s| s.to_string())
}

/// 把 thought_signature 注入 OpenAI 格式的 tool_call JSON
/// （写在文档规定的 extra_content.google 路径，保留兄弟字段）
fn inject_thought_signature(call: &mut serde_json::Value, signature: &str) {
    if call
        .pointer_mut("/extra_content/google")
        .and_then(|g| g.as_object_mut())
        .is_some()
    {
        call["extra_content"]["google"]["thought_signature"] =
            serde_json::Value::String(signature.to_string());
        return;
    }
    // extra_content 不存在或不是预期结构 → 原样保留兄弟键后重建
    let mut extra = call
        .get("extra_content")
        .and_then(|e| e.as_object())
        .cloned()
        .unwrap_or_default();
    extra.insert(
        "google".to_string(),
        serde_json::json!({ "thought_signature": signature }),
    );
    call["extra_content"] = serde_json::Value::Object(extra);
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
                        let mut call = serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            }
                        });
                        // Gemini thought_signature 回放：有则原样带回
                        if let Some(ref sig) = tc.thought_signature {
                            inject_thought_signature(&mut call, sig);
                        }
                        call
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
                        // arguments 兼容两种形态：JSON 字符串（OpenAI 标准）或
                        // 直接内联对象（部分网关），否则对象形态会被静默丢成 {}
                        let args_val = func.get("arguments");
                        let arguments = args_val
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .or_else(|| args_val.filter(|v| v.is_object()).cloned())
                            .unwrap_or(serde_json::json!({}));
                        Some(ToolCall {
                            id,
                            name,
                            arguments,
                            thought_signature: extract_thought_signature(tc),
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

    fn parse_stream_chunks(&self, line: &str) -> Vec<StreamEvent> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let Some(first_choice) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        else {
            return Vec::new();
        };

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
            return vec![StreamEvent::Done {
                full_text: String::new(),
                usage: TokenUsage {
                    input_tokens: usage
                        .and_then(|u| u.get("prompt_tokens"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0) as usize,
                    output_tokens,
                },
                stop_reason,
            }];
        }

        let Some(delta) = first_choice.get("delta") else {
            return Vec::new();
        };

        let mut events: Vec<StreamEvent> = Vec::new();

        // tool_calls 增量。OpenAI 原生把首帧（index+id+name，arguments 空）与
        // 后续 arguments 片段分开；Gemini 兼容端点则常把 id/name/完整 arguments
        // 放在同一帧、并行调用也可能挤在一个 delta 里——逐条完整解析，
        // 不能只处理数组首个后提前 return（会丢参数或丢并行调用）。
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let signature = extract_thought_signature(tc);
                let func = tc.get("function");
                // arguments 兼容两种形态：字符串片段（OpenAI 标准）或完整对象
                // （部分网关单帧调用直接给对象）
                let args_str = func
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str().map(str::to_string))
                    .or_else(|| {
                        func.and_then(|f| f.get("arguments"))
                            .filter(|a| a.is_object())
                            .map(|a| a.to_string())
                    });

                // 首帧：function.name + id；同帧若已带完整/部分 arguments，
                // 紧跟一条 Delta 补发（Gemini 单帧调用否则参数会被丢弃）
                let is_start = matches!(
                    (
                        tc.get("id").and_then(|i| i.as_str()),
                        func.and_then(|f| f.get("name")).and_then(|n| n.as_str()),
                    ),
                    (Some(_), Some(_))
                );
                if is_start {
                    events.push(StreamEvent::ToolUseStart {
                        index,
                        id: tc
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: func
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        signature: signature.clone(),
                    });
                }
                // arguments 片段（首帧同带的完整参数也走这里）
                if let Some(args) = args_str.filter(|a| !a.is_empty()) {
                    events.push(StreamEvent::ToolUseDelta {
                        index,
                        partial_json: args,
                        signature: signature.clone(),
                    });
                } else if !is_start && signature.is_some() {
                    // 签名单独成帧（partial_json 为空、仅携带 signature）
                    events.push(StreamEvent::ToolUseDelta {
                        index,
                        partial_json: String::new(),
                        signature,
                    });
                }
            }
        }

        // Regular content delta（与 tool_calls 同帧时两者都保留）
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                events.push(StreamEvent::Delta {
                    delta: content.to_string(),
                });
            }
        }

        events
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
                        thought_signature: None,
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

    fn parse_stream_chunks(&self, line: &str) -> Vec<StreamEvent> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        // Responses 流式事件在 data JSON 内用 type 字段标识事件类型
        let Some(event_type) = value.get("type").and_then(|t| t.as_str()) else {
            return Vec::new();
        };

        match event_type {
            // 文本增量
            "response.output_text.delta" => {
                let delta = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !delta.is_empty() {
                    vec![StreamEvent::Delta {
                        delta: delta.to_string(),
                    }]
                } else {
                    Vec::new()
                }
            }
            // 新 output item（function_call 在此声明 call_id + name）
            "response.output_item.added" => {
                let Some(item) = value.get("item") else {
                    return Vec::new();
                };
                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
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
                    vec![StreamEvent::ToolUseStart {
                        index,
                        id,
                        name,
                        signature: None,
                    }]
                } else {
                    Vec::new()
                }
            }
            // function_call arguments 增量
            "response.function_call_arguments.delta" => {
                let index = value
                    .get("output_index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(0) as usize;
                let delta = value.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !delta.is_empty() {
                    vec![StreamEvent::ToolUseDelta {
                        index,
                        partial_json: delta.to_string(),
                        signature: None,
                    }]
                } else {
                    Vec::new()
                }
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

                vec![StreamEvent::Done {
                    full_text: String::new(),
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens,
                    },
                    stop_reason,
                }]
            }
            // 其余事件（response.created / response.in_progress / response.output_item.done /
            // response.output_text.done / response.function_call_arguments.done / *.failed 等）
            // 不携带需要转发的增量，忽略。
            _ => Vec::new(),
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
            StreamEvent::ToolUseStart {
                index, id, name, ..
            } => {
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
                ..
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

    // —— Gemini thought_signature 测试 ——

    #[test]
    fn parse_stream_tool_call_first_chunk_with_signature() {
        // Gemini OpenAI 兼容端点：首帧 tool_call 附带 extra_content.google.thought_signature
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"get_project_stats","arguments":""},"extra_content":{"google":{"thought_signature":"CrICAdHtim"}}}]}}]}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseStart {
                index,
                id,
                name,
                signature,
            } => {
                assert_eq!(index, 0);
                assert_eq!(id, "call_abc");
                assert_eq!(name, "get_project_stats");
                assert_eq!(signature.as_deref(), Some("CrICAdHtim"));
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_tool_call_signature_only_frame() {
        // 签名可独立成帧（无 id/name，arguments 为空）
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"extra_content":{"google":{"thought_signature":"CrICAd"}}}]}}]}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseDelta {
                index,
                partial_json,
                signature,
            } => {
                assert_eq!(index, 0);
                assert_eq!(partial_json, "");
                assert_eq!(signature.as_deref(), Some("CrICAd"));
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_tool_call_top_level_signature_fallback() {
        // LiteLLM 风格网关把签名放在 tool_call 顶层
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"f","arguments":""},"thought_signature":"TopLevel"}]}}]}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseStart { signature, .. } => {
                assert_eq!(signature.as_deref(), Some("TopLevel"));
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_response_tool_call_with_signature() {
        // 非流式响应：tool_calls[].extra_content.google.thought_signature
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gemini-3-pro".to_string(),
                ..Default::default()
            },
        };
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_project_stats", "arguments": "{}"},
                        "extra_content": {"google": {"thought_signature": "CrICAdHtim"}}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = adapter.parse_response(&response).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(
            result.tool_calls[0].thought_signature.as_deref(),
            Some("CrICAdHtim")
        );
    }

    // —— 参数丢失回归（P0：Gemini 兼容端点单帧完整工具调用） ——

    #[test]
    fn parse_stream_single_frame_complete_tool_call() {
        // Gemini OpenAI 兼容端点：id/name/完整 arguments 同在一帧。
        // 旧实现首帧分支只取 id/name 后提前 return，同帧 arguments 被丢弃，
        // 导致所有工具调用参数变成空 {}（「缺少参数」批量报错）。
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"update_thread","arguments":"{\"thread_id\":\"t_007\",\"scope\":\"pov\"}"}}]}}]}"#;
        let events = adapter.parse_stream_chunks(line);
        assert_eq!(events.len(), 2, "应产生 Start + Delta 两个事件: {events:?}");
        match (&events[0], &events[1]) {
            (
                StreamEvent::ToolUseStart { index, name, .. },
                StreamEvent::ToolUseDelta {
                    index: i2,
                    partial_json,
                    ..
                },
            ) => {
                assert_eq!(*index, 0);
                assert_eq!(name, "update_thread");
                assert_eq!(*i2, 0);
                let args: serde_json::Value = serde_json::from_str(partial_json).unwrap();
                assert_eq!(args["thread_id"], "t_007");
                assert_eq!(args["scope"], "pov");
            }
            other => panic!("应为 Start + Delta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_parallel_tool_calls_in_one_chunk() {
        // 同一 delta 携带多个并行 tool_calls：逐条解析，不能只留第一个
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"call_a","type":"function","function":{"name":"get_structure","arguments":"{\"chapter_id\":\"ch_03\"}"}},
            {"index":1,"id":"call_b","type":"function","function":{"name":"get_threads","arguments":"{}"}}
        ]}}]}"#;
        let events = adapter.parse_stream_chunks(line);
        let starts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolUseStart { index, name, .. } => Some((*index, name.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 2, "两个并行调用都应解析: {events:?}");
        assert_eq!(starts[0], (0, "get_structure".to_string()));
        assert_eq!(starts[1], (1, "get_threads".to_string()));
        // index 0 的完整参数以 Delta 补发
        assert!(events.iter().any(|e| matches!(e,
            StreamEvent::ToolUseDelta { index: 0, partial_json, .. }
                if partial_json.contains("ch_03"))));
    }

    #[test]
    fn parse_stream_tool_call_object_arguments() {
        // 部分网关单帧调用的 arguments 直接给 JSON 对象（而非字符串）
        let adapter = stream_adapter();
        let line = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"search_entities","arguments":{"query":"苏格"}}}]}}]}"#;
        let events = adapter.parse_stream_chunks(line);
        assert!(
            events.iter().any(|e| matches!(e,
            StreamEvent::ToolUseDelta { partial_json, .. }
                if partial_json.contains("苏格"))),
            "对象形态参数应转成 Delta: {events:?}"
        );
    }

    #[test]
    fn parse_response_object_arguments() {
        // 非流式：arguments 为内联对象时不应被静默丢成 {}
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_x",
                        "type": "function",
                        "function": {"name": "search_entities", "arguments": {"query": "苏格"}}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = adapter.parse_response(&response).unwrap();
        assert_eq!(result.tool_calls[0].arguments["query"], "苏格");
    }

    #[test]
    fn parse_response_missing_arguments_field_yields_empty_object() {
        // 无参工具的 arguments 字段缺失：仍应保留该调用（空参数）而非整个丢弃
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_x",
                        "type": "function",
                        "function": {"name": "get_project_stats"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = adapter.parse_response(&response).unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_project_stats");
        assert!(result.tool_calls[0].arguments.is_object());
    }

    #[test]
    fn build_request_body_replays_thought_signature() {
        // 回放历史：assistant tool_calls 里的签名必须原样带回 extra_content.google
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gemini-3-pro".to_string(),
                ..Default::default()
            },
        };
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("调用工具"),
                Message::assistant_with_tools(
                    String::new(),
                    vec![ToolCall {
                        id: "call_123".to_string(),
                        name: "get_project_stats".to_string(),
                        arguments: serde_json::json!({}),
                        thought_signature: Some("CrICAdHtim".to_string()),
                    }],
                ),
                Message::tool("call_123".to_string(), "{}"),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);
        let assistant = &body["messages"][1];
        assert_eq!(
            assistant["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
            "CrICAdHtim"
        );
    }

    #[test]
    fn build_request_body_without_signature_stays_standard() {
        // 无签名（非 Gemini provider / 并行调用的无签名兄弟项）时
        // 保持 OpenAI 标准结构，不注入任何非标准字段
        let adapter = OpenAiAdapter {
            shared: AdapterShared {
                model: "gpt-4o".to_string(),
                ..Default::default()
            },
        };
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![Message::assistant_with_tools(
                String::new(),
                vec![ToolCall {
                    id: "call_123".to_string(),
                    name: "f".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                }],
            )],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);
        let call = &body["messages"][0]["tool_calls"][0];
        assert!(call.get("extra_content").is_none());
        assert_eq!(call["function"]["name"], "f");
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
                        thought_signature: None,
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
            StreamEvent::ToolUseStart {
                index, id, name, ..
            } => {
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
                ..
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
