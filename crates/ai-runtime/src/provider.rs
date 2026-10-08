//! Provider 抽象 (RFC-0010 §3)
//!
//! 所有 AI 模型实现统一的 AiProvider trait。
//! GPT / Claude / Gemini / DeepSeek / Ollama 都是不同的 Provider。
//! 密钥从环境变量读取，不进配置文件。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// AiProvider trait
// ============================================================================

/// 所有 AI Provider 的统一接口 (RFC-0010 §3.1)
pub trait AiProvider: Send + Sync + std::fmt::Debug {
    /// Provider 标识（如 "openai"、"anthropic"、"ollama"）
    fn id(&self) -> &str;

    /// 模型名（账本展示用；自定义 provider 未实现时为空）
    fn model(&self) -> &str {
        ""
    }

    /// 发送请求，返回生成的文本
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError>;

    /// 流式生成 — 通过回调发送每个增量事件
    ///
    /// 默认实现：调用 `generate()` 后一次性返回全部文本。
    /// 支持流式的 Provider 应覆写此方法。
    fn stream_generate(
        &self,
        request: &GenerateRequest,
        on_event: &(dyn Fn(StreamEvent) + Send + Sync),
    ) -> Result<(), AiError> {
        match self.generate(request) {
            Ok(resp) => {
                on_event(StreamEvent::Delta {
                    delta: resp.text.clone(),
                });
                on_event(StreamEvent::Done {
                    full_text: resp.text,
                    usage: resp.usage,
                    stop_reason: Some(resp.stop_reason),
                });
                Ok(())
            }
            Err(e) => {
                on_event(StreamEvent::Error {
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }
}

// ============================================================================
// 请求 / 响应
// ============================================================================

/// 生成请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// 系统指令
    pub system: String,
    /// 用户消息（含上下文 + 任务描述）
    pub messages: Vec<Message>,
    /// 模型参数
    pub model: String,
    /// 温度
    pub temperature: f32,
    /// 最大 token 数
    pub max_tokens: usize,
    /// 可用工具定义（Agent 模式下使用）
    #[serde(default)]
    pub tools: Vec<ToolDef>,
}

impl GenerateRequest {
    pub fn new(system: impl Into<String>, user_prompt: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            messages: vec![Message::user(user_prompt)],
            model: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            tools: Vec::new(),
        }
    }
}

/// 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// 工具调用（当 role=Assistant 且 AI 要调用工具时）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// 工具调用 ID（当 role=Tool 时，关联到对应的 tool_use）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: String, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id),
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }
}

/// 消息角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    /// 工具调用结果
    Tool,
}

/// 工具定义（给 LLM 的函数签名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 对象，描述参数
    pub parameters: serde_json::Value,
}

/// 工具调用（LLM 请求执行某个函数）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider 分配的唯一 ID（Anthropic: tool_use_id, OpenAI: tool_call_id）
    pub id: String,
    /// 工具名
    pub name: String,
    /// 参数（JSON 对象）
    pub arguments: serde_json::Value,
    /// Gemini 思考签名（OpenAI 兼容端点在 tool_calls[].extra_content.google
    /// 里返回）。多轮工具调用回放 assistant 历史时必须原样带回，
    /// 否则 Gemini 2.5+/3 思考模型会返回 400
    /// "Function call is missing a thought_signature in functionCall parts"。
    /// 并行工具调用时 Gemini 只给首个调用签名，其余保持 None 即可。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// 响应终止原因
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StopReason {
    /// 模型自然结束
    #[default]
    EndTurn,
    /// 模型请求调用工具
    ToolUse,
    /// 达到 max_tokens 限制
    MaxTokens,
    /// 其他停止序列
    Stop,
}

/// 生成响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    /// AI 生成的文本
    pub text: String,
    /// token 消耗
    pub usage: TokenUsage,
    /// 终止原因
    #[serde(default)]
    pub stop_reason: StopReason,
    /// 工具调用请求（当 stop_reason=ToolUse 时）
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

/// token 消耗统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}

impl TokenUsage {
    pub fn total(&self) -> usize {
        self.input_tokens + self.output_tokens
    }
}

// ============================================================================
// StreamEvent — 流式生成事件
// ============================================================================

/// 流式生成过程中的单个事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    /// 文本增量 — 流式过程中到达的每个 chunk
    Delta { delta: String },
    /// 流式完成 — 包含完整文本和 token 统计
    Done {
        full_text: String,
        usage: TokenUsage,
        /// 本轮停止原因（如 tool_use / end_turn）。None 时由消费者兜底为 EndTurn。
        stop_reason: Option<StopReason>,
    },
    /// 流式出错
    Error { message: String },
    /// tool_use 块开始 —— Anthropic 的 content_block_start(tool_use) 或 OpenAI 的 delta.tool_calls 首帧。
    /// `index` 标识该 tool_call 在本轮中的序号，用于关联后续的 ToolUseDelta。
    /// `signature` 为 Gemini 兼容端点随首帧返回的 thought_signature（可能为 None，
    /// 也可能延后到后续 Delta 帧才到达）。
    ToolUseStart {
        index: usize,
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// tool 参数 JSON 增量 —— 拼接所有同 index 的 partial_json 得到完整 arguments。
    /// `signature` 同 ToolUseStart：签名可能出现在任意一帧（partial_json 可为空串）。
    ToolUseDelta {
        index: usize,
        partial_json: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

// ============================================================================
// AiError
// ============================================================================

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum AiError {
    #[error("网络错误: {0}")]
    Network(String),

    #[error("API 错误 ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("认证失败: 缺少 API Key")]
    Auth,

    #[error("Provider 不可用: {0}")]
    Unavailable(String),

    #[error("响应解析失败: {0}")]
    Parse(String),

    #[error("用户取消了执行")]
    Cancelled,
}

// ============================================================================
// ProviderRegistry
// ============================================================================

/// Provider 注册表 (RFC-0010 §3.2)
#[derive(Debug, Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Box<dyn AiProvider>>,
    /// 默认 Provider ID
    default_provider: Option<String>,
    /// 默认模型
    default_model: String,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Box<dyn AiProvider>) {
        let id = provider.id().to_string();
        if self.default_provider.is_none() {
            self.default_provider = Some(id.clone());
        }
        self.providers.insert(id, provider);
    }

    pub fn set_default(&mut self, provider_id: &str, model: &str) {
        self.default_provider = Some(provider_id.to_string());
        self.default_model = model.to_string();
    }

    pub fn get(&self, id: &str) -> Option<&dyn AiProvider> {
        self.providers.get(id).map(|b| b.as_ref())
    }

    pub fn default_provider(&self) -> Option<(&dyn AiProvider, &str)> {
        let id = self.default_provider.as_ref()?;
        let provider = self.providers.get(id)?.as_ref();
        Some((provider, &self.default_model))
    }

    /// 使用默认 Provider 生成
    pub fn generate(&self, request: &mut GenerateRequest) -> Result<GenerateResponse, AiError> {
        let (provider, model) = self
            .default_provider()
            .ok_or(AiError::Unavailable("未注册任何 Provider".to_string()))?;
        if request.model.is_empty() {
            request.model = model.to_string();
        }
        provider.generate(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的 Mock Provider
    #[derive(Debug)]
    pub struct MockProvider {
        id: String,
        response: String,
    }

    impl MockProvider {
        pub fn new(id: &str, response: &str) -> Self {
            Self {
                id: id.to_string(),
                response: response.to_string(),
            }
        }
    }

    impl AiProvider for MockProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    #[test]
    fn registry_default_provider() {
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(MockProvider::new("mock", "hello")));
        reg.set_default("mock", "mock-model");

        let mut req = GenerateRequest::new("system", "user");
        let resp = reg.generate(&mut req).unwrap();
        assert_eq!(resp.text, "hello");
        assert_eq!(req.model, "mock-model"); // 自动填充
    }
}
