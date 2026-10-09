//! Gemini Provider — Google Gemini 原生 API (RFC-0010 §3)
//!
//! API: https://ai.google.dev/api/generate-content
//! 端点：`POST /v1beta/models/{model}:generateContent`（非流式）
//!       `POST /v1beta/models/{model}:streamGenerateContent?alt=sse`（流式 SSE）
//! 认证：`x-goog-api-key` 头。API Key 从 GEMINI_API_KEY / GOOGLE_API_KEY 环境变量读取。
//!
//! 与 OpenAI 兼容端点的差异（为什么要原生 provider）：
//! - 思考参数：generationConfig.thinkingConfig（3.x → thinkingLevel，2.5 → thinkingBudget）
//! - 工具调用回放必须原样带回 thoughtSignature（否则 400）
//! - functionResponse 按 name 匹配（服务端下发的 id 才需要回传）
//! - maxOutputTokens 包含思考 token，开启思考时需抬高配额

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::provider::{
    AiError, AiProvider, GenerateRequest, GenerateResponse, Role, StopReason, StreamEvent,
    TokenUsage, ToolCall,
};
use crate::providers::{
    api_key_from_env, round_temperature, HttpClient, HttpStreamClient, ProviderAdapter,
    ThinkingConfig,
};

/// Gemini 默认 API base
const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";

// ============================================================================
// GeminiProvider
// ============================================================================

#[derive(Debug)]
pub struct GeminiProvider {
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
    /// 思考强度（3.x → thinkingLevel；2.5 → thinkingBudget）
    thinking: Option<ThinkingConfig>,
    /// 单次请求超时（秒）
    timeout_secs: u64,
    /// 档案级输出上限（含思考 token）；None = 用请求自带值
    max_tokens: Option<usize>,
}

impl GeminiProvider {
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
            timeout_secs: crate::providers::normalize_timeout_secs(timeout_secs),
            max_tokens,
        }
    }

    /// 从 Provider 构造 adapter（共用逻辑，供 generate / stream_generate 调用）
    fn build_adapter(&self, request: &GenerateRequest) -> GeminiAdapter {
        GeminiAdapter {
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
            // 跨 SSE chunk 递增的工具调用序号：agent 按 index 聚合，
            // 两次相邻 chunk 各带一个 functionCall 时不能都从 0 开始
            call_index: AtomicUsize::new(0),
        }
    }
}

impl AiProvider for GeminiProvider {
    fn id(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
        let request = super::with_profile_max_tokens(self.max_tokens, request);
        let adapter = self.build_adapter(&request);
        self.client.send(&adapter, &request)
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
        // Gemini 的 Done 只在带 finishReason 的最终 chunk 发出（stop_reason 恒 Some）；
        // 流中途异常收不到时由末尾兜底补发一次
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
            StreamEvent::Done { ref usage, .. } => {
                if let Ok(mut u) = final_usage.lock() {
                    *u = usage.clone();
                }
                on_event(event);
                if let Ok(mut d) = done_received.lock() {
                    *d = true;
                }
            }
            StreamEvent::ToolUseStart { .. } | StreamEvent::ToolUseDelta { .. } => {
                on_event(event);
            }
            StreamEvent::Error { .. } => {
                on_event(event);
            }
        };

        // 与 Anthropic 相同的 runtime 复用策略：内部 tokio → block_in_place，外部 → 临时 runtime
        let stream_fn = || async { HttpStreamClient::stream(&adapter, &request, &local_on).await };

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
// GeminiAdapter — 请求/响应转换
// ============================================================================

/// 内部合成 id 的分隔符：服务端不下发 functionCall.id 时按 `{name}#{序号}` 合成，
/// 回放时据此区分「合成的（只回 name）」与「服务端下发的（id 必须回传）」
const SYNTHETIC_ID_SEP: char = '#';

fn is_synthetic_id(id: &str) -> bool {
    id.contains(SYNTHETIC_ID_SEP)
}

#[derive(Default)]
struct GeminiAdapter {
    model: String,
    api_key: Option<String>,
    api_url: Option<String>,
    api_url_full: bool,
    extra_body: Option<serde_json::Value>,
    extra_headers: Vec<(String, String)>,
    thinking: Option<ThinkingConfig>,
    timeout_secs: u64,
    /// 流式工具调用序号（跨 chunk 递增；AtomicUsize 以满足 Adapter + Sync 约束）
    call_index: AtomicUsize,
}

impl GeminiAdapter {
    /// 构建指定方法的完整 URL：`{base}/v1beta/models/{model}:{method}`
    fn method_url(&self, method: &str) -> String {
        let url = match self.api_url.as_deref() {
            Some(url) => url.trim_end_matches('/'),
            None => return format!("{DEFAULT_BASE}/v1beta/models/{}:{method}", self.model),
        };

        // 用户已给完整方法路径则直通（含 :generateContent / :streamGenerateContent）
        if url.ends_with(":generateContent") || url.ends_with(":streamGenerateContent") {
            return url.to_string();
        }

        // 模型集合端点（…/v1beta/models）：是列表 URL 而非可 POST 的方法端点，
        // 原样使用会 405 —— 完整 URL 模式也不例外，统一补 /{model}:{method}
        //（版本段保持用户原样，/v1beta、/v1 均可）
        if url.ends_with("/models") {
            return format!("{url}/{}:{method}", self.model);
        }

        // 完整 URL 模式的其它任意端点：原样使用（非标准路径的代理）
        if self.api_url_full {
            return url.to_string();
        }

        // base 模式：剥掉已有 /v1beta（或 /v1）尾巴再拼，避免双段
        let base = url
            .strip_suffix("/v1beta")
            .or_else(|| url.strip_suffix("/v1"))
            .unwrap_or(url);
        format!("{base}/v1beta/models/{}:{method}", self.model)
    }
}

/// Gemini finishReason → StopReason。
/// 注意 Gemini 对「请求调用工具」同样返回 STOP（靠 functionCall part 区分），
/// 由调用方在有工具调用时改判 ToolUse。
fn parse_gemini_finish_reason(s: &str) -> Option<StopReason> {
    match s {
        "STOP" => Some(StopReason::EndTurn),
        "MAX_TOKENS" => Some(StopReason::MaxTokens),
        "STOP_SEQUENCE" => Some(StopReason::Stop),
        _ => None,
    }
}

/// 安全策略类 finishReason：正文为空时应该作为错误 surfaced 给用户
fn is_safety_block(finish_reason: &str) -> bool {
    matches!(
        finish_reason,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "OTHER"
    )
}

/// usageMetadata → TokenUsage（思考 token 计入输出，与计费口径一致）
fn parse_usage_metadata(body: &serde_json::Value) -> TokenUsage {
    let usage = body.get("usageMetadata");
    let num = |key: &str| {
        usage
            .and_then(|u| u.get(key))
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as usize
    };
    TokenUsage {
        input_tokens: num("promptTokenCount"),
        output_tokens: num("candidatesTokenCount") + num("thoughtsTokenCount"),
    }
}

/// 从响应 parts 提取文本 / 工具调用（非流式 parse_response 与流式 chunk 共用）。
///
/// `next_index` 供给合成 id / ToolUseStart index 的递增序号（流式跨 chunk 唯一）。
fn extract_parts(
    parts: &[serde_json::Value],
    next_index: &mut usize,
) -> (String, Vec<ToolCall>) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for part in parts {
        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
            text.push_str(t);
            continue;
        }
        if let Some(call) = part.get("functionCall") {
            let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            // 服务端下发 id 则透传（回放时必须原样带回）；
            // 没有则按 name#序号 合成，仅用于内部关联
            let id = match call.get("id").and_then(|i| i.as_str()) {
                Some(server_id) => server_id.to_string(),
                None => format!("{name}{}{next_index}", SYNTHETIC_ID_SEP),
            };
            let args = call
                .get("args")
                .cloned()
                .filter(|a| a.is_object())
                .unwrap_or_else(|| serde_json::json!({}));
            tool_calls.push(ToolCall {
                id,
                name: name.to_string(),
                arguments: args,
                thought_signature: part
                    .get("thoughtSignature")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string()),
            });
            *next_index += 1;
        }
        // 仅含 thoughtSignature 的独立 part：签名主要随 functionCall part 到达，
        // 独立帧（如文本 part 的签名）没有承载结构，跳过
    }
    (text, tool_calls)
}

impl ProviderAdapter for GeminiAdapter {
    fn id(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> String {
        self.method_url("generateContent")
    }

    /// 流式走独立方法路径 + alt=sse（Gemini 不用 body 里的 stream 字段，
    /// 且未知字段会被 400 拒绝，见 stream_flag）
    fn stream_endpoint(&self) -> String {
        // method_url 已归一化全部自定义 URL 形态；把 generateContent 方法换掉
        let url = self.method_url("generateContent");
        if let Some(s) = url.strip_suffix(":generateContent") {
            return format!("{s}:streamGenerateContent?alt=sse");
        }
        // 用户给的 URL 不以 :generateContent 结尾（完整 URL 模式的任意端点直通）
        let joiner = if url.contains('?') { '&' } else { '?' };
        format!("{url}{joiner}alt=sse")
    }

    /// Gemini 靠 URL 区分流式，请求体不接受 stream 字段（未知字段 400）
    fn stream_flag(&self) -> Option<(String, serde_json::Value)> {
        None
    }

    fn api_key(&self) -> Result<String, AiError> {
        if let Some(ref key) = self.api_key {
            return Ok(key.clone());
        }
        // AI Studio 惯用 GEMINI_API_KEY，gcloud 生态惯用 GOOGLE_API_KEY
        api_key_from_env("GEMINI_API_KEY")
            .or_else(|_| api_key_from_env("GOOGLE_API_KEY"))
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        let key = self.api_key();
        vec![("x-goog-api-key".to_string(), key.unwrap_or_default())]
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

    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    fn to_request_body(&self, request: &GenerateRequest) -> serde_json::Value {
        // Gemini 的消息规则：
        // 1. role 只有 user / model（system 走独立的 systemInstruction）
        // 2. assistant 工具调用 = model 消息的 functionCall part（含 thoughtSignature）
        // 3. 工具结果 = user 消息的 functionResponse part，按 **name** 匹配
        //    （服务端下发过 functionCall.id 的才需要回传 id）
        // 4. 连续的 tool 结果合并进同一条 user 消息（多个 functionResponse part）
        let mut contents: Vec<serde_json::Value> = Vec::new();
        let mut tool_responses: Vec<serde_json::Value> = Vec::new();
        // 上一条 assistant 消息的 tool_call id → name 映射，
        // Tool 消息只带 id，functionResponse 需要 name
        let mut last_call_names: HashMap<String, String> = HashMap::new();
        // messages 里的 System 消息并入 systemInstruction（正常流程 system 走独立字段）
        let mut system_parts: Vec<serde_json::Value> = Vec::new();
        if !request.system.is_empty() {
            system_parts.push(serde_json::json!({ "text": request.system }));
        }

        for msg in &request.messages {
            if msg.role != Role::Tool && !tool_responses.is_empty() {
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": tool_responses,
                }));
                tool_responses = Vec::new();
            }

            match msg.role {
                Role::System => {
                    if !msg.content.is_empty() {
                        system_parts.push(serde_json::json!({ "text": msg.content }));
                    }
                }
                Role::User => {
                    if !msg.content.is_empty() {
                        contents.push(serde_json::json!({
                            "role": "user",
                            "parts": [{ "text": msg.content }],
                        }));
                    }
                }
                Role::Assistant => {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    if !msg.content.is_empty() {
                        parts.push(serde_json::json!({ "text": msg.content }));
                    }
                    for tc in &msg.tool_calls {
                        last_call_names.insert(tc.id.clone(), tc.name.clone());
                        let mut part = serde_json::json!({
                            "functionCall": {
                                "name": tc.name,
                                "args": tc.arguments,
                            }
                        });
                        // 服务端下发的 id 才回传；合成 id（name#序号）服务端不认识
                        if !tc.id.is_empty() && !is_synthetic_id(&tc.id) {
                            part["functionCall"]["id"] = serde_json::Value::String(tc.id.clone());
                        }
                        // Gemini 3+ 的思考签名必须随 functionCall part 原样带回，否则 400
                        if let Some(sig) = tc.thought_signature.as_deref().filter(|s| !s.is_empty())
                        {
                            part["thoughtSignature"] = serde_json::Value::String(sig.to_string());
                        }
                        parts.push(part);
                    }
                    if !parts.is_empty() {
                        contents.push(serde_json::json!({
                            "role": "model",
                            "parts": parts,
                        }));
                    }
                }
                Role::Tool => {
                    if let Some(ref tc_id) = msg.tool_call_id {
                        let name = last_call_names
                            .get(tc_id)
                            .cloned()
                            .unwrap_or_default();
                        let mut resp = serde_json::json!({
                            "functionResponse": {
                                // id 缺失时按 name 匹配；合成 id 不回传
                                "name": name,
                                "response": { "result": msg.content },
                            }
                        });
                        if !is_synthetic_id(tc_id) {
                            resp["functionResponse"]["id"] =
                                serde_json::Value::String(tc_id.clone());
                        }
                        tool_responses.push(resp);
                    }
                }
            }
        }

        if !tool_responses.is_empty() {
            contents.push(serde_json::json!({
                "role": "user",
                "parts": tool_responses,
            }));
        }

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "temperature": round_temperature(request.temperature),
                "maxOutputTokens": request.max_tokens,
            }
        });

        if !system_parts.is_empty() {
            body["systemInstruction"] = serde_json::json!({ "parts": system_parts });
        }

        if !request.tools.is_empty() {
            let decls: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{ "functionDeclarations": decls }]);
        }

        body
    }

    fn parse_response(&self, body: &serde_json::Value) -> Result<GenerateResponse, AiError> {
        // promptFeedback.blockReason → 请求被输入侧拦截，candidates 可能缺失
        if let Some(block) = body
            .pointer("/promptFeedback/blockReason")
            .and_then(|b| b.as_str())
        {
            return Err(AiError::Parse(format!("Gemini 请求被安全策略拦截: {block}")));
        }

        let candidate = body
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| AiError::Parse(format!("无法解析 Gemini 响应: {body}")))?;

        let parts: Vec<serde_json::Value> = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();

        let mut index = 0usize;
        let (text, tool_calls) = extract_parts(&parts, &mut index);
        let usage = parse_usage_metadata(body);

        let finish_reason = candidate
            .get("finishReason")
            .and_then(|r| r.as_str())
            .unwrap_or("STOP");

        // 安全拦截 / 空结果要有可读错误，否则上游把空正文当成功
        if text.is_empty() && tool_calls.is_empty() && is_safety_block(finish_reason) {
            return Err(AiError::Parse(format!(
                "Gemini 生成被拦截（finishReason={finish_reason}）"
            )));
        }

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            parse_gemini_finish_reason(finish_reason).unwrap_or(StopReason::EndTurn)
        };

        Ok(GenerateResponse {
            text,
            usage,
            stop_reason,
            tool_calls,
        })
    }

    fn parse_stream_chunks(&self, line: &str) -> Vec<StreamEvent> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let mut events = Vec::new();
        let Some(candidate) = value
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        else {
            return Vec::new();
        };

        if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
            for part in parts {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        events.push(StreamEvent::Delta {
                            delta: t.to_string(),
                        });
                    }
                    continue;
                }
                if let Some(call) = part.get("functionCall") {
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    let index = self.call_index.fetch_add(1, Ordering::SeqCst);
                    let id = match call.get("id").and_then(|i| i.as_str()) {
                        Some(server_id) => server_id.to_string(),
                        None => format!("{name}{}{index}", SYNTHETIC_ID_SEP),
                    };
                    let signature = part
                        .get("thoughtSignature")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    let args = call
                        .get("args")
                        .cloned()
                        .filter(|a| a.is_object())
                        .unwrap_or_else(|| serde_json::json!({}));
                    // Gemini 的 functionCall 整帧到达（不分片），Start 后立即补一条完整参数 Delta
                    events.push(StreamEvent::ToolUseStart {
                        index,
                        id,
                        name: name.to_string(),
                        signature: signature.clone(),
                    });
                    events.push(StreamEvent::ToolUseDelta {
                        index,
                        partial_json: args.to_string(),
                        signature: None,
                    });
                }
            }
        }

        // 最终 chunk：finishReason + usageMetadata 一起到达 → Done
        if let Some(finish) = candidate.get("finishReason").and_then(|r| r.as_str()) {
            let usage = parse_usage_metadata(&value);
            // 流式路径的 stop_reason 只能判 MAX_TOKENS/STOP；
            // 是否 ToolUse 由 agent 侧按累积的 tool_calls 改判（与聚合层约定一致）
            let stop_reason = parse_gemini_finish_reason(finish);
            events.push(StreamEvent::Done {
                full_text: String::new(),
                usage,
                stop_reason,
            });
        }

        events
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Message, ToolDef};

    fn adapter() -> GeminiAdapter {
        GeminiAdapter {
            model: "gemini-3.8-flash".to_string(),
            ..Default::default()
        }
    }

    // ── 端点构造 ──────────────────────────────────────────

    #[test]
    fn endpoint_default() {
        assert_eq!(
            adapter().endpoint(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent"
        );
        assert_eq!(
            adapter().stream_endpoint(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn endpoint_custom_base_appends_v1beta() {
        // 代理只给 base：拼 /v1beta/models/{model}:{method}
        let a = GeminiAdapter {
            model: "gemini-2.5-pro".to_string(),
            api_url: Some("https://gemini-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(
            a.endpoint(),
            "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(
            a.stream_endpoint(),
            "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn endpoint_base_with_v1beta_not_duplicated() {
        // base 已带 /v1beta：剥掉再拼，避免 /v1beta/v1beta
        let a = GeminiAdapter {
            model: "gemini-2.5-pro".to_string(),
            api_url: Some("https://gemini-proxy.example.com/v1beta/".to_string()),
            ..Default::default()
        };
        assert_eq!(
            a.endpoint(),
            "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn endpoint_full_method_url_passthrough() {
        // 用户已填完整方法路径（含 :generateContent）则直通，流式端点换方法
        let a = GeminiAdapter {
            model: "gemini-2.5-pro".to_string(),
            api_url: Some(
                "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:generateContent"
                    .to_string(),
            ),
            ..Default::default()
        };
        assert_eq!(
            a.endpoint(),
            "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
        assert_eq!(
            a.stream_endpoint(),
            "https://gemini-proxy.example.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn endpoint_models_collection_url_appends_method() {
        // 回归：完整 URL 模式填了模型集合端点 …/v1beta/models（列表 URL）——
        // 原样 POST 会 405，应自动补 /{model}:{method}；版本段保持用户原样
        for full in [true, false] {
            let a = GeminiAdapter {
                model: "gemini-2.5-pro".to_string(),
                api_url: Some("http://127.0.0.1:8045/v1beta/models".to_string()),
                api_url_full: full,
                ..Default::default()
            };
            assert_eq!(
                a.endpoint(),
                "http://127.0.0.1:8045/v1beta/models/gemini-2.5-pro:generateContent",
                "api_url_full={full} 时集合端点都应补方法路径"
            );
            assert_eq!(
                a.stream_endpoint(),
                "http://127.0.0.1:8045/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
            );
        }
        // 不带版本段的集合端点同样容错
        let a = GeminiAdapter {
            model: "gemini-2.5-pro".to_string(),
            api_url: Some("http://127.0.0.1:8045/models/".to_string()),
            api_url_full: true,
            ..Default::default()
        };
        assert_eq!(
            a.endpoint(),
            "http://127.0.0.1:8045/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn endpoint_full_url_mode_arbitrary_verbatim() {
        // 完整 URL 模式的任意非标准端点仍原样直通（仅流式追加 alt=sse）
        let a = GeminiAdapter {
            model: "gemini-2.5-pro".to_string(),
            api_url: Some("https://my-proxy.example.com/api/chat?token=x".to_string()),
            api_url_full: true,
            ..Default::default()
        };
        assert_eq!(a.endpoint(), "https://my-proxy.example.com/api/chat?token=x");
        assert_eq!(
            a.stream_endpoint(),
            "https://my-proxy.example.com/api/chat?token=x&alt=sse"
        );
    }

    #[test]
    fn stream_flag_is_none() {
        // Gemini 不接受 body 里的 stream 字段（未知字段 400）
        assert!(adapter().stream_flag().is_none());
    }

    #[test]
    fn api_key_env_fallback_chain() {
        // 档案未配置 Key 时回退 GEMINI_API_KEY，再回退 GOOGLE_API_KEY
        // （无 Key 环境下测试报 Auth 而非 panic）
        let a = GeminiAdapter::default();
        let _ = a.api_key();
        assert_eq!(a.extra_headers()[0].0, "x-goog-api-key");
    }

    // ── 请求体构造 ────────────────────────────────────────

    #[test]
    fn build_request_body_basic() {
        let a = adapter();
        let request = GenerateRequest {
            system: "你是小说作者".to_string(),
            messages: vec![Message::user("续写故事"), Message::assistant("林风推开门。")],
            model: String::new(),
            temperature: 0.8,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = a.to_request_body(&request);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "你是小说作者");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
        assert!(
            (body["generationConfig"]["temperature"].as_f64().unwrap() - 0.8).abs() < 0.001
        );
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "续写故事");
        assert_eq!(body["contents"][1]["role"], "model");
        assert_eq!(body["contents"][1]["parts"][0]["text"], "林风推开门。");
        // 模型名走 URL，不在 body（Gemini 拒绝未知字段）
        assert!(body.get("model").is_none());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn build_request_body_with_tools() {
        let a = adapter();
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![Message::user("查大纲")],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 2048,
            tools: vec![ToolDef {
                name: "get_outline".to_string(),
                description: "读取大纲".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            }],
        };

        let body = a.to_request_body(&request);
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "get_outline");
        assert_eq!(
            body["tools"][0]["functionDeclarations"][0]["parameters"]["type"],
            "object"
        );
        // 空 system 不发 systemInstruction
        assert!(body.get("systemInstruction").is_none());
    }

    #[test]
    fn build_request_body_tool_call_replay_with_signature() {
        // Gemini 3 的 thoughtSignature 必须随 functionCall part 原样回放
        let a = adapter();
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("根据大纲创建角色"),
                Message::assistant_with_tools(
                    "让我先查看大纲".to_string(),
                    vec![ToolCall {
                        // 合成 id（服务端没下发）：不回传 id，只按 name 匹配
                        id: "get_outline#0".to_string(),
                        name: "get_outline".to_string(),
                        arguments: serde_json::json!({}),
                        thought_signature: Some("CqQBAXJ2...".to_string()),
                    }],
                ),
                Message::tool("get_outline#0".to_string(), "标题: 魔法纪元"),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = a.to_request_body(&request);
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);

        // [1] model 消息：text + functionCall（带签名、不带合成 id）
        assert_eq!(contents[1]["role"], "model");
        let parts = contents[1]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["text"], "让我先查看大纲");
        assert_eq!(parts[1]["functionCall"]["name"], "get_outline");
        assert!(
            parts[1]["functionCall"].get("id").is_none(),
            "合成 id 不应回传"
        );
        assert_eq!(parts[1]["thoughtSignature"], "CqQBAXJ2...");

        // [2] user 消息：functionResponse 按 name 匹配（无 id）
        assert_eq!(contents[2]["role"], "user");
        let resp = &contents[2]["parts"][0]["functionResponse"];
        assert_eq!(resp["name"], "get_outline");
        assert_eq!(resp["response"]["result"], "标题: 魔法纪元");
        assert!(resp.get("id").is_none(), "合成 id 不应回传");
    }

    #[test]
    fn build_request_body_server_issued_id_replayed() {
        // 服务端下发的 functionCall.id 必须在 functionCall/functionResponse 两侧回传
        let a = adapter();
        let request = GenerateRequest {
            system: String::new(),
            messages: vec![
                Message::user("并行读两段"),
                Message::assistant_with_tools(
                    String::new(),
                    vec![
                        ToolCall {
                            id: "fc-abc123".to_string(),
                            name: "read_fragment".to_string(),
                            arguments: serde_json::json!({"id": "f_001"}),
                            thought_signature: None,
                        },
                        ToolCall {
                            id: "fc-def456".to_string(),
                            name: "read_fragment".to_string(),
                            arguments: serde_json::json!({"id": "f_002"}),
                            thought_signature: None,
                        },
                    ],
                ),
                Message::tool("fc-abc123".to_string(), "第一段正文"),
                Message::tool("fc-def456".to_string(), "第二段正文"),
            ],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let body = a.to_request_body(&request);
        let contents = body["contents"].as_array().unwrap();
        // 连续 tool 结果合并进同一条 user 消息（两个 functionResponse part）
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[1]["parts"][0]["functionCall"]["id"], "fc-abc123");
        assert_eq!(contents[1]["parts"][1]["functionCall"]["id"], "fc-def456");
        let resp_parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(resp_parts.len(), 2);
        assert_eq!(resp_parts[0]["functionResponse"]["id"], "fc-abc123");
        assert_eq!(resp_parts[0]["functionResponse"]["name"], "read_fragment");
        assert_eq!(resp_parts[1]["functionResponse"]["id"], "fc-def456");
        assert_eq!(resp_parts[1]["functionResponse"]["name"], "read_fragment");
    }

    #[test]
    fn build_request_body_system_role_folded_into_instruction() {
        // messages 内的 System 消息并入 systemInstruction，不占 contents
        let a = adapter();
        let request = GenerateRequest {
            system: "主指令".to_string(),
            messages: vec![Message::system("补充指令"), Message::user("正文")],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        };
        let body = a.to_request_body(&request);
        let parts = body["systemInstruction"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "主指令");
        assert_eq!(parts[1]["text"], "补充指令");
        assert_eq!(body["contents"].as_array().unwrap().len(), 1);
    }

    // ── 响应解析 ──────────────────────────────────────────

    #[test]
    fn parse_response_text() {
        let a = adapter();
        let response = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "林风推开门。"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 50,
                "thoughtsTokenCount": 200,
                "totalTokenCount": 350
            }
        });
        let result = a.parse_response(&response).unwrap();
        assert_eq!(result.text, "林风推开门。");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        // 思考 token 计入输出
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 250);
    }

    #[test]
    fn parse_response_function_calls() {
        // 无 id 的 functionCall：合成 name#序号 id；有 finishReason=STOP 也判 ToolUse
        let a = adapter();
        let response = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "我先看看大纲。"},
                    {"functionCall": {"name": "get_outline", "args": {"scope": "all"}}}
                ]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5}
        });
        let result = a.parse_response(&response).unwrap();
        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "get_outline");
        assert_eq!(result.tool_calls[0].id, "get_outline#0");
        assert_eq!(result.tool_calls[0].arguments["scope"], "all");
    }

    #[test]
    fn parse_response_function_call_with_server_id_and_signature() {
        let a = adapter();
        let response = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"functionCall": {"id": "fc-x1", "name": "write_fragment", "args": {}},
                     "thoughtSignature": "CqQBAXJ2sig=="}
                ]},
                "finishReason": "STOP"
            }]
        });
        let result = a.parse_response(&response).unwrap();
        assert_eq!(result.tool_calls[0].id, "fc-x1");
        assert_eq!(
            result.tool_calls[0].thought_signature.as_deref(),
            Some("CqQBAXJ2sig==")
        );
    }

    #[test]
    fn parse_response_safety_block_is_error() {
        // 空正文 + SAFETY 类 finishReason → 可读错误（而非静默成功）
        let a = adapter();
        let response = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": []},
                "finishReason": "SAFETY"
            }]
        });
        let err = a.parse_response(&response).unwrap_err();
        assert!(err.to_string().contains("拦截"));
    }

    #[test]
    fn parse_response_prompt_feedback_block() {
        let a = adapter();
        let response = serde_json::json!({
            "promptFeedback": {"blockReason": "SAFETY"}
        });
        assert!(a.parse_response(&response).is_err());
    }

    #[test]
    fn parse_response_max_tokens() {
        let a = adapter();
        let response = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "写到一半"}]},
                "finishReason": "MAX_TOKENS"
            }]
        });
        let result = a.parse_response(&response).unwrap();
        assert_eq!(result.stop_reason, StopReason::MaxTokens);
    }

    // ── 流式解析 ──────────────────────────────────────────

    #[test]
    fn parse_stream_text_delta() {
        let a = adapter();
        let line = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"你好"}]},"index":0}]}"#;
        let events = a.parse_stream_chunks(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::Delta { delta } => assert_eq!(delta, "你好"),
            other => panic!("应为 Delta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_function_call_full_frame() {
        // functionCall 整帧到达：Start + 完整参数 Delta，签名随 Start
        let a = adapter();
        let line = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"update_thread","args":{"thread_id":"t_007"}},"thoughtSignature":"sig=="}]},"index":0}]}"#;
        let events = a.parse_stream_chunks(line);
        assert_eq!(events.len(), 2);
        match &events[0] {
            StreamEvent::ToolUseStart { index, id, name, signature } => {
                assert_eq!(*index, 0);
                assert_eq!(id, "update_thread#0");
                assert_eq!(name, "update_thread");
                assert_eq!(signature.as_deref(), Some("sig=="));
            }
            other => panic!("应为 ToolUseStart，实际: {other:?}"),
        }
        match &events[1] {
            StreamEvent::ToolUseDelta { index, partial_json, .. } => {
                assert_eq!(*index, 0);
                let args: serde_json::Value = serde_json::from_str(partial_json).unwrap();
                assert_eq!(args["thread_id"], "t_007");
            }
            other => panic!("应为 ToolUseDelta，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_index_increments_across_chunks() {
        // 关键回归：两个 chunk 各带一个 functionCall，index 必须跨 chunk 递增
        // （agent 按 index 聚合，重复 0 会把两次调用合并成一次）
        let a = GeminiAdapter {
            model: "gemini-3.8-flash".to_string(),
            call_index: AtomicUsize::new(0),
            ..Default::default()
        };
        let line1 = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_a","args":{}}}]}}]}"#;
        let line2 = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_b","args":{}}}]}}]}"#;
        let e1 = a.parse_stream_chunks(line1);
        let e2 = a.parse_stream_chunks(line2);
        match (&e1[0], &e2[0]) {
            (
                StreamEvent::ToolUseStart { index: i1, id: id1, .. },
                StreamEvent::ToolUseStart { index: i2, id: id2, .. },
            ) => {
                assert_eq!((*i1, *i2), (0, 1));
                assert_eq!((id1.as_str(), id2.as_str()), ("read_a#0", "read_b#1"));
            }
            other => panic!("应为 ToolUseStart 对，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_final_chunk_done() {
        let a = adapter();
        let line = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"完"}]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"thoughtsTokenCount":7}}"#;
        let events = a.parse_stream_chunks(line);
        // Delta("完") + Done
        assert_eq!(events.len(), 2);
        match &events[1] {
            StreamEvent::Done { usage, stop_reason, .. } => {
                assert_eq!(*stop_reason, Some(StopReason::EndTurn));
                assert_eq!(usage.output_tokens, 10);
            }
            other => panic!("应为 Done，实际: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_function_call_finish_maps_tool_use() {
        // 流式最终 chunk：STOP + functionCall → Done 后由聚合层按 tool_calls 改判 ToolUse；
        // stop_reason 至少不能误报 MaxTokens
        let a = adapter();
        let line = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_scene","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}}"#;
        let events = a.parse_stream_chunks(line);
        let done = events.iter().find_map(|e| match e {
            StreamEvent::Done { stop_reason, .. } => Some(stop_reason),
            _ => None,
        });
        assert_eq!(done, Some(&Some(StopReason::EndTurn)));
    }

    #[test]
    fn parse_stream_ignores_garbage() {
        let a = adapter();
        assert!(a.parse_stream_chunks("not json").is_empty());
        assert!(a.parse_stream_chunks(r#"{"candidates":[]}"#).is_empty());
    }
}
