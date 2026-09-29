//! Anthropic Provider — Claude (RFC-0010 §3)
//!
//! API: https://docs.anthropic.com/en/api/messages
//! 特点：system prompt 独立于 messages（与我们的 GenerateRequest 结构对齐）。
//! 认证：x-api-key 头 + anthropic-version 头。
//! API Key 从 ANTHROPIC_API_KEY 环境变量读取。

use crate::provider::{
    AiError, AiProvider, GenerateRequest, GenerateResponse, Role, StopReason, StreamEvent,
    TokenUsage, ToolCall,
};
use crate::providers::{
    api_key_from_env, round_temperature, HttpClient, HttpStreamClient, ProviderAdapter,
    ThinkingConfig,
};

// ============================================================================
// AnthropicProvider
// ============================================================================

#[derive(Debug)]
pub struct AnthropicProvider {
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
    /// 思考强度（映射为 thinking.budget_tokens；智谱兼容层只发 thinking.type）
    thinking: Option<ThinkingConfig>,
    /// 单次请求超时（秒）
    timeout_secs: u64,
}

impl AnthropicProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<String>,
        api_url: Option<String>,
        api_url_full: bool,
        extra_body: Option<serde_json::Value>,
        extra_headers: Vec<(String, String)>,
        thinking: Option<ThinkingConfig>,
        timeout_secs: Option<u64>,
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
            timeout_secs: crate::providers::normalize_timeout_secs(timeout_secs),
        }
    }

    /// 从 Provider 构造 adapter（共用逻辑，供 generate / stream_generate 调用）
    fn build_adapter(&self, request: &GenerateRequest) -> AnthropicAdapter {
        AnthropicAdapter {
            model: if request.model.is_empty() {
                self.model.clone()
            } else {
                request.model.clone()
            },
            api_key: self.api_key.clone(),
            api_url: self.api_url.clone(),
            api_url_full: self.api_url_full,
            extra_body: self.extra_body.clone(),
            extra_headers: self.extra_headers.clone(),
            thinking: self.thinking.clone(),
            timeout_secs: self.timeout_secs,
        }
    }
}

impl AiProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
        let adapter = self.build_adapter(request);
        self.client.send(&adapter, request)
    }

    fn stream_generate(
        &self,
        request: &GenerateRequest,
        on_event: &(dyn Fn(StreamEvent) + Send + Sync),
    ) -> Result<(), AiError> {
        let adapter = self.build_adapter(request);

        let full_text = std::sync::Mutex::new(String::new());
        let final_usage = std::sync::Mutex::new(TokenUsage::default());

        // Use a local callback that captures full_text accumulation
        // 跟踪是否已收到带 stop_reason 的 Done（来自 SSE 的 message_delta 事件）。
        // 避免在流末尾重复发送 Done —— 旧实现会再发一个 stop_reason: None 的 Done，
        // 其 usage 可能不完整却覆盖掉正确的值，且语义混乱。
        let done_received = std::sync::Mutex::new(false);
        let local_on = |event: StreamEvent| match event {
            StreamEvent::Delta { ref delta } => {
                if let Ok(mut t) = full_text.lock() {
                    t.push_str(delta);
                }
                on_event(StreamEvent::Delta {
                    delta: delta.clone(),
                });
            }
            StreamEvent::Done {
                ref usage,
                ref stop_reason,
                ..
            } => {
                if let Ok(mut u) = final_usage.lock() {
                    *u = usage.clone();
                }
                // 只转发带 stop_reason 的 Done；无 stop_reason 的末尾 Done 不转发，
                // 由下面的兜底逻辑在流结束后补发一次（带 stop_reason，若曾收到过）。
                if stop_reason.is_some() {
                    if let Ok(mut d) = done_received.lock() {
                        *d = true;
                    }
                    on_event(event);
                }
            }
            // tool_call 增量直接转发，由 stream_with_cancel 累积
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
        let stream_fn = || async { HttpStreamClient::stream(&adapter, request, &local_on).await };

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
                // 若流中途异常（未收到 message_delta），补发一次 Done 让消费者能结束。
                // 已正常收到过 Done 时不再重复发送。
                let already_done = done_received.lock().map(|d| *d).unwrap_or(false);
                if !already_done {
                    let full = full_text.lock().unwrap().clone();
                    let usage = final_usage.lock().unwrap().clone();
                    on_event(StreamEvent::Done {
                        full_text: full,
                        usage,
                        stop_reason: None,
                    });
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

// ============================================================================
// AnthropicAdapter — 请求/响应转换
// ============================================================================

#[derive(Default)]
struct AnthropicAdapter {
    model: String,
    api_key: Option<String>,
    api_url: Option<String>,
    api_url_full: bool,
    extra_body: Option<serde_json::Value>,
    extra_headers: Vec<(String, String)>,
    thinking: Option<ThinkingConfig>,
    timeout_secs: u64,
}

impl ProviderAdapter for AnthropicAdapter {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn endpoint(&self) -> String {
        if let Some(ref url) = self.api_url {
            // 完整 URL 模式：直接使用，不做任何处理
            if self.api_url_full {
                return url.clone();
            }
            let base = url.trim_end_matches('/');
            // 兜底：用户已提供完整路径则直通
            if base.ends_with("/messages") || base.ends_with("/v1/messages") {
                return base.to_string();
            }
            // base 已带 /v1 先剥掉，避免拼出 /v1/v1/ 双段
            let base = base.strip_suffix("/v1").unwrap_or(base);
            return format!("{base}/v1/messages");
        }
        "https://api.anthropic.com/v1/messages".to_string()
    }

    fn api_key(&self) -> Result<String, AiError> {
        if let Some(ref key) = self.api_key {
            return Ok(key.clone());
        }
        api_key_from_env("ANTHROPIC_API_KEY")
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        let key = self.api_key();
        vec![
            ("x-api-key".to_string(), key.unwrap_or_default()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ]
    }

    fn custom_headers(&self) -> &[(String, String)] {
        &self.extra_headers
    }

    fn custom_body(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.extra_body.as_ref().and_then(|v| v.as_object())
    }

    fn thinking_config(&self) -> Option<&ThinkingConfig> {
        self.thinking.as_ref()
    }

    // 智谱的 Anthropic 兼容层按 URL 识别：开思考时只发 thinking.type，不带 budget_tokens
    fn thinking_dialect(&self) -> crate::providers::ThinkingDialect {
        crate::providers::thinking_dialect_for_url(self.api_url.as_deref())
    }

    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    fn to_request_body(&self, request: &GenerateRequest) -> serde_json::Value {
        // Anthropic 的消息规则：
        // 1. assistant 消息含 tool_use 时，content 必须是 [text_block?, tool_use_block...] 数组
        // 2. tool_use 后面必须紧跟**一条** user 消息，包含所有对应的 tool_result block
        //    （不能拆成多条独立消息，否则报 400）
        //
        // 因此需要将连续的 Role::Tool 消息合并为单条 user 消息。
        let mut messages: Vec<serde_json::Value> = Vec::new();
        let mut tool_results_buffer: Vec<serde_json::Value> = Vec::new();

        for msg in &request.messages {
            // 如果当前不是 Tool 消息，先 flush 缓冲的 tool_results
            if msg.role != Role::Tool && !tool_results_buffer.is_empty() {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": tool_results_buffer,
                }));
                tool_results_buffer = Vec::new();
            }

            match msg.role {
                Role::System | Role::User => {
                    // 过滤空 user 消息
                    if !msg.content.is_empty() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": msg.content,
                        }));
                    }
                }
                Role::Assistant => {
                    if !msg.tool_calls.is_empty() {
                        // Anthropic 要求 content 是 text block + tool_use block 的数组
                        let mut content_blocks: Vec<serde_json::Value> = Vec::new();
                        if !msg.content.is_empty() {
                            content_blocks.push(serde_json::json!({
                                "type": "text",
                                "text": msg.content,
                            }));
                        }
                        for tc in &msg.tool_calls {
                            content_blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content_blocks,
                        }));
                    } else {
                        // 纯文本 assistant 消息
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content,
                        }));
                    }
                }
                Role::Tool => {
                    // 缓冲 tool_result，等遇到非 Tool 消息时合并 flush
                    if let Some(ref tc_id) = msg.tool_call_id {
                        tool_results_buffer.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tc_id,
                            "content": msg.content,
                        }));
                    }
                }
            }
        }

        // flush 尾部剩余的 tool_results
        if !tool_results_buffer.is_empty() {
            messages.push(serde_json::json!({
                "role": "user",
                "content": tool_results_buffer,
            }));
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "temperature": round_temperature(request.temperature),
            "system": request.system,
            "messages": messages,
        });

        // Add tools if present
        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        body
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<GenerateResponse, AiError> {
        let content = body
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| AiError::Parse(format!("无法解析 Anthropic 响应: {body}")))?;

        // Collect text from text blocks
        let mut text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in content {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                "tool_use" => {
                    if let (Some(id), Some(name), Some(input)) = (
                        block.get("id").and_then(|i| i.as_str()),
                        block.get("name").and_then(|n| n.as_str()),
                        block.get("input").cloned(),
                    ) {
                        tool_calls.push(ToolCall {
                            id: id.to_string(),
                            name: name.to_string(),
                            arguments: input,
                        });
                    }
                }
                _ => {}
            }
        }

        let stop_reason = match body
            .get("stop_reason")
            .and_then(|r| r.as_str())
            .unwrap_or("end_turn")
        {
            "tool_use" => StopReason::ToolUse,
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

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

        let event_type = value.get("type")?.as_str()?;

        match event_type {
            "content_block_start" => {
                // tool_use 块开始：{"type":"content_block_start","index":N,
                //   "content_block":{"type":"tool_use","id":"toolu_...","name":"tool_name"}}
                let block = value.get("content_block")?;
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !id.is_empty() && !name.is_empty() {
                        return Some(StreamEvent::ToolUseStart { index, id, name });
                    }
                }
                None
            }
            "content_block_delta" => {
                let delta = value.get("delta")?;
                let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        // 文本增量
                        let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if text.is_empty() {
                            None
                        } else {
                            Some(StreamEvent::Delta {
                                delta: text.to_string(),
                            })
                        }
                    }
                    "input_json_delta" => {
                        // tool 参数 JSON 增量：index 关联到 content_block_start 的 tool_use 块
                        let index =
                            value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let partial = delta
                            .get("partial_json")
                            .and_then(|p| p.as_str())
                            .unwrap_or("");
                        Some(StreamEvent::ToolUseDelta {
                            index,
                            partial_json: partial.to_string(),
                        })
                    }
                    _ => None,
                }
            }
            "message_delta" => {
                // 最终消息：含 stop_reason 和 usage
                let usage = value.get("usage");
                let output_tokens = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as usize;
                let input_tokens = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0) as usize;
                // stop_reason 在 delta.stop_reason 里（如 "tool_use"/"end_turn"/"max_tokens"）
                let stop_reason = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                    .and_then(parse_anthropic_stop_reason);
                Some(StreamEvent::Done {
                    full_text: String::new(),
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens,
                    },
                    stop_reason,
                })
            }
            "message_stop" => None, // stream end marker
            "ping" => None,
            _ => None,
        }
    }
}

/// Anthropic stop_reason 字符串 → StopReason
fn parse_anthropic_stop_reason(s: &str) -> Option<StopReason> {
    match s {
        "tool_use" => Some(StopReason::ToolUse),
        "end_turn" => Some(StopReason::EndTurn),
        "max_tokens" => Some(StopReason::MaxTokens),
        "stop_sequence" => Some(StopReason::Stop),
        _ => None,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    #[test]
    fn build_request_body() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
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

        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["system"], "你是小说作者");
        assert_eq!(body["max_tokens"], 4096);
        assert!((body["temperature"].as_f64().unwrap() - 0.8).abs() < 0.001);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "续写故事");
    }

    #[test]
    fn build_request_body_with_tool_calls() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };

        // 模拟一轮工具调用：user → assistant(tool_use + reasoning) → tool(result)
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("根据大纲创建角色"),
                Message::assistant_with_tools(
                    "让我先查看大纲".to_string(),
                    vec![ToolCall {
                        id: "toolu_01".to_string(),
                        name: "get_outline".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                ),
                Message::tool(
                    "toolu_01".to_string(),
                    "## 故事概要\n标题: 魔法纪元\n类型: 奇幻",
                ),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3, "应有 3 条消息");

        // [0] user
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "根据大纲创建角色");

        // [1] assistant — 必须同时包含 text block 和 tool_use block
        assert_eq!(messages[1]["role"], "assistant");
        let content = messages[1]["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            2,
            "assistant 消息应有 text + tool_use 两个 block"
        );
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "让我先查看大纲");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_01");
        assert_eq!(content[1]["name"], "get_outline");

        // [2] tool result（role=user，content 是 tool_result block）
        assert_eq!(messages[2]["role"], "user");
        let tr_content = messages[2]["content"].as_array().unwrap();
        assert_eq!(tr_content[0]["type"], "tool_result");
        assert_eq!(tr_content[0]["tool_use_id"], "toolu_01");
        assert!(tr_content[0]["content"]
            .as_str()
            .unwrap()
            .contains("魔法纪元"));
    }

    #[test]
    fn build_request_body_multiple_tool_results_merged() {
        // 关键测试：assistant 一次调用多个工具时，
        // 所有 tool_result 必须合并到**一条** user 消息中。
        // 否则 Anthropic 报 400: "tool_use ids found without tool_result blocks immediately after"
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };

        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("根据大纲创建角色"),
                // assistant 一次请求调用两个工具
                Message::assistant_with_tools(
                    "让我查看大纲和结构".to_string(),
                    vec![
                        ToolCall {
                            id: "toolu_01".to_string(),
                            name: "get_outline".to_string(),
                            arguments: serde_json::json!({}),
                        },
                        ToolCall {
                            id: "toolu_02".to_string(),
                            name: "get_structure".to_string(),
                            arguments: serde_json::json!({}),
                        },
                    ],
                ),
                // 两个独立的 Tool 消息（agent 的内部表示）
                Message::tool("toolu_01".to_string(), "标题: 魔法纪元\n类型: 奇幻"),
                Message::tool("toolu_02".to_string(), "书籍: 魔法纪元\n  章节: 第一章"),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = adapter.to_request_body(&request);
        let messages = body["messages"].as_array().unwrap();

        // 应只有 3 条消息：user, assistant(tool_use x2), user(tool_result x2)
        // 两个 tool_result 合并到一条 user 消息，而不是拆成两条
        assert_eq!(
            messages.len(),
            3,
            "应为 3 条消息（两个 tool_result 合并为一条），实际: {}",
            messages.len()
        );

        // [1] assistant 有 1 text + 2 tool_use = 3 blocks
        assert_eq!(messages[1]["role"], "assistant");
        let asst_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(asst_content.len(), 3, "text + 2 tool_use = 3 blocks");
        assert_eq!(asst_content[0]["type"], "text");
        assert_eq!(asst_content[1]["type"], "tool_use");
        assert_eq!(asst_content[1]["id"], "toolu_01");
        assert_eq!(asst_content[2]["type"], "tool_use");
        assert_eq!(asst_content[2]["id"], "toolu_02");

        // [2] 合并后的 user 消息，含 2 个 tool_result block
        assert_eq!(messages[2]["role"], "user");
        let tr_content = messages[2]["content"].as_array().unwrap();
        assert_eq!(
            tr_content.len(),
            2,
            "应有 2 个 tool_result block 合并在一条消息中"
        );
        assert_eq!(tr_content[0]["type"], "tool_result");
        assert_eq!(tr_content[0]["tool_use_id"], "toolu_01");
        assert_eq!(tr_content[1]["type"], "tool_result");
        assert_eq!(tr_content[1]["tool_use_id"], "toolu_02");
    }

    #[test]
    fn parse_response_success() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };

        let response = serde_json::json!({
            "content": [
                { "type": "text", "text": "林风推开门。" }
            ],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        });

        let result = adapter.parse_response(&response).unwrap();

        assert_eq!(result.text, "林风推开门。");
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 50);
    }

    #[test]
    fn parse_response_error() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };

        let bad_response = serde_json::json!({ "error": "something" });

        let result = adapter.parse_response(&bad_response);
        assert!(result.is_err());
    }

    #[test]
    fn endpoint_custom_url_appends_v1() {
        // DeepSeek 等 Anthropic 兼容端点：只提供 base URL 时应补全 /v1/messages
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            api_url: Some("https://api.deepseek.com/anthropic".to_string()),
            ..Default::default()
        };
        assert_eq!(
            adapter.endpoint(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn endpoint_custom_url_trailing_slash() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            api_url: Some("https://api.deepseek.com/anthropic/".to_string()),
            ..Default::default()
        };
        assert_eq!(
            adapter.endpoint(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn endpoint_custom_url_full_path_passthrough() {
        // 用户已填完整路径（含 /v1/messages）则直通
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            api_url: Some("https://custom.example.com/v1/messages".to_string()),
            ..Default::default()
        };
        assert_eq!(adapter.endpoint(), "https://custom.example.com/v1/messages");
    }

    #[test]
    fn endpoint_default() {
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };
        assert_eq!(adapter.endpoint(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn endpoint_full_url_mode_verbatim() {
        // 完整 URL 模式：不追加路径，不做格式检测，原样使用
        let adapter = AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            api_url: Some("https://custom-proxy.example.com/api/chat".to_string()),
            api_url_full: true,
            ..Default::default()
        };
        assert_eq!(
            adapter.endpoint(),
            "https://custom-proxy.example.com/api/chat"
        );
    }

    // —— 流式 tool_call 解析测试 ——

    fn stream_adapter() -> AnthropicAdapter {
        AnthropicAdapter {
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_text_delta() {
        let adapter = stream_adapter();
        let line = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::Delta { delta } => assert_eq!(delta, "你好"),
            other => panic!("应为 Delta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_start() {
        let adapter = stream_adapter();
        let line = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01A","name":"write_fragment_text"}}"#;
        let event = adapter.parse_stream_chunk(line).unwrap();
        match event {
            StreamEvent::ToolUseStart { index, id, name } => {
                assert_eq!(index, 1);
                assert_eq!(id, "toolu_01A");
                assert_eq!(name, "write_fragment_text");
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let adapter = stream_adapter();
        // input_json_delta 分多次到达，index 关联到 content_block_start
        let line1 = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"fragment_id\":"}}"#;
        let line2 = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"f_0001\",\"text\":\"正文\"}"}}"#;

        let e1 = adapter.parse_stream_chunk(line1).unwrap();
        let e2 = adapter.parse_stream_chunk(line2).unwrap();
        match e1 {
            StreamEvent::ToolUseDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 1);
                assert!(partial_json.contains("fragment_id"));
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
        match e2 {
            StreamEvent::ToolUseDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 1);
                assert!(partial_json.contains("正文"));
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_with_stop_reason() {
        let adapter = stream_adapter();
        // message_delta 携带 stop_reason
        let line_tool = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":50}}"#;
        let event = adapter.parse_stream_chunk(line_tool).unwrap();
        match event {
            StreamEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, Some(StopReason::ToolUse));
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }

        let line_end = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50}}"#;
        let event = adapter.parse_stream_chunk(line_end).unwrap();
        match event {
            StreamEvent::Done { stop_reason, .. } => {
                assert_eq!(stop_reason, Some(StopReason::EndTurn));
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_content_block_start_text_ignored() {
        // content_block_start 但 type=text（不是 tool_use）→ 应返回 None
        let adapter = stream_adapter();
        let line =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert!(adapter.parse_stream_chunk(line).is_none());
    }
}
