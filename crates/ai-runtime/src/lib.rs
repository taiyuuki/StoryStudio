//! AI Runtime — Provider 抽象 + Task 模型 + ReAct Agent (RFC-0010)
//!
//! AI Runtime 是数据流中唯一允许非确定性的层。
//! 它管理 Agent 的执行、上下文构建、Change 生成与提交——
//! 是"AI 生成 → 确定性验证"这条链的编排者。
//!
//! 核心设计（RFC-0010）：
//! - AI 输出文本，系统用 Parser 解析为 Change——AI 不输出 JSON
//! - 统一 Task 抽象：StatelessTask / ConversationalTask
//! - ReAct Agent：感知上下文 → 规划行动 → 调用工具 → 整合反馈 → 更新状态
//! - 非确定性隔离：AI 生成允许不确定，Dry-Run 验证后提交才确定

pub mod agent;
pub mod cancel;
pub mod context;
pub mod conversation;
pub mod interactive;
pub mod pipeline;
pub mod prompts;
pub mod provider;
pub mod providers;
pub mod session;
pub mod stream_cancel;
pub mod task;

pub use agent::{
    clamp_tool_result, is_retryable_error, Agent, AgentConfig, AgentState, AgentStep, AgentTrace,
    Scratchpad, SubTask, TaskPlan, Tool, ToolRegistry,
};
pub use cancel::CancelToken;
pub use context::ContextBuilder;
pub use conversation::{send_with_retry, CommitResult, Conversation, ConversationTurn};
pub use interactive::{
    current_chat, current_session_id, ChatContext, InteractiveStore, PendingSlot,
};
pub use pipeline::{run_stateless, OutputPipeline, PipelineError, PipelineResult};
pub use provider::{
    AiError, AiProvider, GenerateRequest, GenerateResponse, Message, ProviderRegistry, Role,
    StopReason, StreamEvent, TokenUsage, ToolCall, ToolDef,
};
pub use providers::{AnthropicProvider, ApiMode, GeminiProvider, OpenAiProvider, ThinkingConfig};
pub use session::{AgentFactory, DeltaCallback, SessionHandle, SessionStore, StepCallback};
pub use stream_cancel::stream_generate_with_cancel;
pub use task::{
    parse_fragment_draft, AssertionDraft, ChapterMaterials, ExtractAssertionsTask, FragmentDraft,
    ModelConfig, Task, TaskContext, TaskError, TaskKind, TaskMode, TaskResult, WriteChapterTask,
    WriteFragmentTask,
};

/// crate 对应的 RFC 版本
pub const RFC_VERSION: &str = "0.1";
