//! Agent 入口：工具注册表、会话存储、系统提示、会话化/非会话化运行入口。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use story_ai_runtime::{
    current_chat, Agent, AgentConfig, AgentFactory, AgentStep, AgentTrace, AiProvider,
    DeltaCallback, SessionStore, StepCallback, Tool, ToolDef, ToolRegistry,
};

use crate::error::StudioError;

// 引入所有子模块的 Tool 实现，供 create_tool_registry 注册。
use super::*;

// ============================================================================
// AskUserTool — 向用户提问，等待回答后继续
// ============================================================================

/// prompt_id 自增计数器（与时间戳拼接，保证同一会话内多次提问的唯一性）
static ASK_PROMPT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 向用户提问的工具。
///
/// 工具自身无状态、不依赖项目文件，因此不需要 `ToolContext`。
/// 它在 `execute` 内通过 thread-local [`current_chat`] 取到当前 chat 上下文
/// （session_id、cancel_token、on_step），然后调用 [`SessionStore::request_input`]
/// 同步阻塞等待用户回答。回答（或超时/取消信息）作为 `ToolResult` 回传 LLM。
pub struct AskUserTool;

impl Tool for AskUserTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "ask_user".into(),
            description: "向用户提出一个问题并等待回答。用于关键创作方向决策、风格偏好、\
                重大剧情走向、用户意图不明确或风险操作前的澄清。\
                可在 options 中提供自拟选项（用户点选其一，也可忽略选项自行输入）。\
                切勿滥用：常规创作决策应自主完成。"
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "要问用户的问题。清晰、具体、一次只问一个核心问题。"
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "可选：你建议的候选答案。用户可任选其一，也可手动输入。无需选项时省略或留空数组。"
                    }
                },
                "required": ["question"]
            }),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let question = arguments
            .get("question")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少参数: question".to_string())?;

        let options: Vec<String> = arguments
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // 从 thread-local 取当前 chat 上下文
        let ctx = current_chat().ok_or_else(|| {
            "无法向用户提问：当前未在 Agent 会话上下文中执行（仅项目模式可用）".to_string()
        })?;

        // 生成唯一 prompt_id
        let seq = ASK_PROMPT_SEQ.fetch_add(1, Ordering::Relaxed);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let prompt_id = format!("ask_{ts}_{seq}");

        let on_step = ctx
            .on_step
            .ok_or_else(|| "无法向用户提问：缺少 step 回调".to_string())?;

        let store = session_store();
        let session_id = ctx.session_id.clone();
        let result = store.request_input(
            &session_id,
            &prompt_id,
            &question,
            &options,
            &ctx.cancel_token,
            &on_step,
        );

        match result {
            Ok(answer) => {
                // 答案回传给 LLM
                Ok(format!("用户的回答：{answer}"))
            }
            Err(msg) => {
                // 取消/超时作为工具结果（非硬错误），让 LLM 知道用户没回答
                Ok(format!("（{msg}）"))
            }
        }
    }
}

// ============================================================================
pub fn create_tool_registry(dir: &Path, provider: Arc<dyn AiProvider>) -> ToolRegistry {
    let ctx = ToolContext::with_provider(dir.to_path_buf(), provider);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(AskUserTool));
    registry.register(Box::new(PrepareWritingTool::new(ctx.clone())));
    registry.register(Box::new(GetWritingGuideTool::new(ctx.clone())));
    registry.register(Box::new(CreateCharacterTool::new(ctx.clone())));
    registry.register(Box::new(CreateLocationTool::new(ctx.clone())));
    registry.register(Box::new(CreateItemTool::new(ctx.clone())));
    registry.register(Box::new(CreateOrganizationTool::new(ctx.clone())));
    registry.register(Box::new(SearchEntitiesTool::new(ctx.clone())));
    registry.register(Box::new(GetEntityDetailTool::new(ctx.clone())));
    registry.register(Box::new(ProfileCheckTool::new(ctx.clone())));
    registry.register(Box::new(DeleteEntityTool::new(ctx.clone())));
    registry.register(Box::new(MergeEntitiesTool::new(ctx.clone())));
    registry.register(Box::new(RevealCharacterNameTool::new(ctx.clone())));
    registry.register(Box::new(GetStoryRecapTool::new(ctx.clone())));
    registry.register(Box::new(GetRelationsTool::new(ctx.clone())));
    registry.register(Box::new(QueryAssertionsTool::new(ctx.clone())));
    registry.register(Box::new(GetCharacterStateTool::new(ctx.clone())));
    registry.register(Box::new(FindMentionsTool::new(ctx.clone())));
    registry.register(Box::new(SearchTextTool::new(ctx.clone())));
    registry.register(Box::new(GetProjectStatsTool::new(ctx.clone())));
    registry.register(Box::new(GetSceneContextTool::new(ctx.clone())));
    registry.register(Box::new(CompileCheckTool::new(ctx.clone())));
    registry.register(Box::new(GetStructureTool::new(ctx.clone())));
    registry.register(Box::new(CreateSceneTool::new(ctx.clone())));
    registry.register(Box::new(CreateChapterTool::new(ctx.clone())));
    registry.register(Box::new(UpdateSceneTool::new(ctx.clone())));
    registry.register(Box::new(SaveSceneSummariesTool::new(ctx.clone())));
    registry.register(Box::new(RepairProjectTool::new(ctx.clone())));
    registry.register(Box::new(DeleteSceneTool::new(ctx.clone())));
    registry.register(Box::new(UpdateChapterTool::new(ctx.clone())));
    registry.register(Box::new(DeleteChapterTool::new(ctx.clone())));
    registry.register(Box::new(AddAssertionTool::new(ctx.clone())));
    registry.register(Box::new(RetireAssertionTool::new(ctx.clone())));
    registry.register(Box::new(DeleteAssertionTool::new(ctx.clone())));
    registry.register(Box::new(RefreshEvidenceTool::new(ctx.clone())));
    registry.register(Box::new(AddRelationTool::new(ctx.clone())));
    registry.register(Box::new(RemoveRelationTool::new(ctx.clone())));
    registry.register(Box::new(UpdateRelationTool::new(ctx.clone())));
    registry.register(Box::new(CreateThreadTool::new(ctx.clone())));
    registry.register(Box::new(GetThreadsTool::new(ctx.clone())));
    registry.register(Box::new(GetThreadDetailTool::new(ctx.clone())));
    registry.register(Box::new(UpdateThreadTool::new(ctx.clone())));
    registry.register(Box::new(AdvanceThreadTool::new(ctx.clone())));
    registry.register(Box::new(SetThreadKnowledgeTool::new(ctx.clone())));
    registry.register(Box::new(DeleteThreadTool::new(ctx.clone())));
    registry.register(Box::new(WriteFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(EditFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(ReadFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(ReadSceneFullTextTool::new(ctx.clone())));
    registry.register(Box::new(AppendFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(DeleteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(RewriteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(SplitFragmentTool::new(ctx.clone())));
    registry.register(Box::new(MergeFragmentsTool::new(ctx.clone())));
    registry.register(Box::new(MoveFragmentTool::new(ctx.clone())));
    registry.register(Box::new(SetFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(RemoveFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(AnchorFragmentsTool::new(ctx.clone())));
    registry.register(Box::new(ListUnresolvedRefsTool::new(ctx.clone())));
    registry.register(Box::new(ResolveUnresolvedRefTool::new(ctx.clone())));
    registry.register(Box::new(AiCheckTool::new(ctx.clone())));
    registry.register(Box::new(AiCheckScopeTool::chapter(ctx.clone())));
    registry.register(Box::new(AiCheckScopeTool::scene(ctx.clone())));
    registry.register(Box::new(UpdateEntityTool::new(ctx.clone())));
    registry.register(Box::new(GetOutlineTool::new(ctx.clone())));
    registry.register(Box::new(SetOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(UpdateOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(DeleteOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(DesignChaptersTool::new(ctx.clone())));
    registry.register(Box::new(GetChapterOutlineTool::new(ctx.clone())));
    registry.register(Box::new(UpdateChapterOutlineTool::new(ctx.clone())));
    registry.register(Box::new(DeleteChapterOutlineTool::new(ctx.clone())));
    registry.register(Box::new(GetWorldviewTool::new(ctx.clone())));
    registry.register(Box::new(SetWorldviewItemTool::new(ctx.clone())));
    registry.register(Box::new(UpdateWorldviewItemTool::new(ctx.clone())));
    registry.register(Box::new(DeleteWorldviewItemTool::new(ctx.clone())));
    registry
}

// ============================================================================
// SessionStore — 进程级会话存储（跨调用保持 Agent 状态）
// ============================================================================

/// 全局 SessionStore 实例（进程级，线程安全）
static SESSION_STORE: OnceLock<SessionStore> = OnceLock::new();

/// 获取全局 SessionStore
pub fn session_store() -> &'static SessionStore {
    SESSION_STORE.get_or_init(SessionStore::new)
}

/// Agent 工厂（项目模式）— 创建带项目工具的 Agent
fn project_agent_factory(dir: PathBuf, provider: Arc<dyn AiProvider>) -> AgentFactory {
    let tools = Arc::new(create_tool_registry(&dir, provider.clone()));
    Box::new(move || {
        Agent::new(
            // 每轮重建都重新解析项目覆盖，改提示词对下一轮对话生效
            crate::prompt_custom::resolve(
                &crate::prompt_custom::AGENT_PROJECT,
                Some(dir.as_path()),
            ),
            default_agent_config(),
            provider.clone(),
            tools.clone(),
        )
    })
}

/// Agent 工厂（全局模式）— 创建无工具的 Agent
fn global_agent_factory(provider: Arc<dyn AiProvider>) -> AgentFactory {
    let tools = Arc::new(ToolRegistry::new());
    Box::new(move || {
        Agent::new(
            crate::prompts::AGENT_GLOBAL,
            global_agent_config(),
            provider.clone(),
            tools.clone(),
        )
    })
}

/// 妙笔每轮对话默认工具调用轮数上限（连续请求次数）
const DEFAULT_MAX_TOOL_TURNS: usize = 40;
/// 用户可调的硬边界：通读全书、批量改稿等长任务可调高，
/// 但过高意味着 token 消耗失控与纠偏困难
const MAX_TOOL_TURNS_LIMIT: u32 = 200;

/// 解析配置中的工具轮数上限：None → 默认 40；有值则 clamp 到 1–200。
fn resolve_max_tool_turns(configured: Option<u32>) -> usize {
    configured
        .map(|n| n.clamp(1, MAX_TOOL_TURNS_LIMIT) as usize)
        .unwrap_or(DEFAULT_MAX_TOOL_TURNS)
}

fn default_agent_config() -> AgentConfig {
    let max_tool_turns = resolve_max_tool_turns(
        crate::ai::read_global_ai_config()
            .ok()
            .and_then(|c| c.agent_max_tool_turns),
    );
    AgentConfig {
        model: String::new(),
        temperature: 0.7,
        // 思考型模型（GLM-5.2 / Claude with thinking）的 reasoning token
        // 计入 max_tokens，平均消耗 30k+。8192 连思考都装不下会导致空输出截断。
        // 65536 是 GLM-5.2 官方示例值，覆盖绝大多数思考场景。
        max_tokens: 65536,
        max_tool_turns,
        max_context_tokens: profile_context_budget(),
        ..AgentConfig::default()
    }
}

fn global_agent_config() -> AgentConfig {
    AgentConfig {
        model: String::new(),
        temperature: 0.7,
        max_tokens: 65536,
        max_tool_turns: 1,
        max_context_tokens: profile_context_budget(),
        ..AgentConfig::default()
    }
}

/// 按妙笔档案声明的「长上下文」调整上下文预算（压缩阈值 = 预算 × 0.7）：
/// - Some(true)：模型支持 100 万 token → 90 万，用满大窗口、减少过早压缩；
/// - Some(false)：明确不支持 → 12 万（128K 窗口模型的保守值，避免超出窗口报错）；
/// - None：未声明 → 同样按 12 万保守处理。多数模型窗口是 128K，未声明时
///   按大窗口（50 万）赌一把会在请求超窗时直接报错；大窗口模型请在档案里
///   勾选「长上下文」获得 90 万预算。
fn profile_context_budget() -> usize {
    const DEFAULT_BUDGET: usize = 120_000;
    let Ok(config) = crate::ai::read_global_ai_config() else {
        return DEFAULT_BUDGET;
    };
    match config
        .profile_for_purpose(crate::ai::purpose::AGENT)
        .and_then(|p| p.long_context)
    {
        Some(true) => 900_000,
        Some(false) | None => DEFAULT_BUDGET,
    }
}

// ============================================================================
// 会话化 Agent 入口（推荐）— 跨调用保持记忆
// ============================================================================

/// 会话化 Agent 对话（项目模式，流式）
///
/// 按 session_id 复用 Agent 实例，保持跨调用的对话历史与记忆。
/// 通过 `on_step` 回调实时推送每个步骤（含工具调用、推理等）。
pub fn agent_chat_streaming(
    dir: &Path,
    session_id: &str,
    provider: Arc<dyn AiProvider>,
    user_input: &str,
    on_step: StepCallback,
) -> Result<AgentTrace, StudioError> {
    let provider_id = provider.id().to_string();
    let provider_model = provider.model().to_string();
    let started = std::time::Instant::now();
    let store = session_store();
    let factory = project_agent_factory(dir.to_path_buf(), provider);
    let on_delta = on_step_to_delta(on_step.clone());
    let result = store
        .chat(
            session_id,
            factory,
            user_input,
            Some(on_step),
            Some(on_delta),
        )
        .map_err(|e| StudioError::Ai(e.to_string()));
    // AI 账本（best-effort，不影响对话结果）
    match &result {
        Ok(trace) => crate::ai_ledger::record(
            dir,
            "妙笔对话",
            &provider_id,
            &provider_model,
            trace.total_tokens as u64,
            started.elapsed().as_millis() as u64,
            true,
            None,
        ),
        Err(e) => crate::ai_ledger::record(
            dir,
            "妙笔对话",
            &provider_id,
            &provider_model,
            0,
            started.elapsed().as_millis() as u64,
            false,
            Some(&e.to_string()),
        ),
    }
    result
}

/// 会话化 Agent 对话（全局模式，流式）
///
/// 无项目上下文，纯对话模式，按 session_id 保持记忆。
pub fn agent_chat_global_streaming(
    session_id: &str,
    provider: Arc<dyn AiProvider>,
    user_input: &str,
    on_step: StepCallback,
) -> Result<AgentTrace, StudioError> {
    let factory = global_agent_factory(provider);
    let on_delta = on_step_to_delta(on_step.clone());
    session_store()
        .chat(
            session_id,
            factory,
            user_input,
            Some(on_step),
            Some(on_delta),
        )
        .map_err(|e| StudioError::Ai(e.to_string()))
}

/// 取消指定会话的当前 Agent 运行
pub fn cancel_agent(session_id: &str) -> bool {
    session_store().cancel(session_id)
}

/// 提交用户对当前提问的回答（唤醒阻塞的 ask_user 工具）
///
/// 返回 `true` 表示存在待回答的提问并已提交；`false` 表示当前无待回答的提问
/// （可能已超时、已取消或已回答）。
pub fn respond_agent(session_id: &str, answer: &str) -> bool {
    session_store().respond(session_id, answer.to_string())
}

/// 列出所有活跃会话 ID
pub fn list_sessions() -> Vec<String> {
    session_store().list_session_ids()
}

/// 删除指定会话（清理记忆）
pub fn drop_session(session_id: &str) {
    session_store().drop_session(session_id);
}

/// 将 step 回调适配为 delta 回调（AiText 增量透传）
fn on_step_to_delta(on_step: StepCallback) -> DeltaCallback {
    Arc::new(move |delta: String| {
        on_step(AgentStep::AiText { content: delta });
    })
}

// ============================================================================
// 向后兼容入口（非会话化，单次调用）
// ============================================================================

/// 运行 Agent（非流式，无会话记忆）
///
/// 向后兼容入口。推荐使用 `agent_chat_streaming` 以获得会话记忆。
pub fn run_agent(
    dir: &Path,
    provider: Arc<dyn AiProvider>,
    user_input: &str,
) -> Result<AgentTrace, StudioError> {
    let tools = Arc::new(create_tool_registry(dir, provider.clone()));
    let config = default_agent_config();
    let mut agent = Agent::new(crate::prompts::AGENT_PROJECT, config, provider, tools);
    agent
        .chat(user_input, None, None)
        .map_err(|e| StudioError::Ai(e.to_string()))
}

/// 运行 Agent（流式，无会话记忆）
///
/// 向后兼容入口。推荐使用 `agent_chat_streaming` 以获得会话记忆。
pub fn run_agent_streaming(
    dir: &Path,
    provider: Arc<dyn AiProvider>,
    user_input: &str,
    on_step: &(dyn Fn(AgentStep) + Send + Sync),
) -> Result<AgentTrace, StudioError> {
    let tools = Arc::new(create_tool_registry(dir, provider.clone()));
    let config = default_agent_config();
    let mut agent = Agent::new(crate::prompts::AGENT_PROJECT, config, provider, tools);

    let trace = agent
        .chat(user_input, None, None)
        .map_err(|e| StudioError::Ai(e.to_string()))?;

    // 发送完整步骤记录（含工具调用等）
    for step in &trace.steps {
        on_step(step.clone());
    }
    Ok(trace)
}

// ============================================================================
// 全局 Agent（无项目上下文）— 向后兼容入口
// ============================================================================

/// 运行全局 Agent（无项目，无工具，非流式）
pub fn run_agent_global(
    provider: Arc<dyn AiProvider>,
    user_input: &str,
) -> Result<AgentTrace, StudioError> {
    let tools = Arc::new(ToolRegistry::new());
    let config = global_agent_config();
    let mut agent = Agent::new(crate::prompts::AGENT_GLOBAL, config, provider, tools);
    agent
        .chat(user_input, None, None)
        .map_err(|e| StudioError::Ai(e.to_string()))
}

/// 运行全局 Agent（无项目，流式）
pub fn run_agent_global_streaming(
    provider: Arc<dyn AiProvider>,
    user_input: &str,
    on_step: &(dyn Fn(AgentStep) + Send + Sync),
) -> Result<AgentTrace, StudioError> {
    let tools = Arc::new(ToolRegistry::new());
    let config = global_agent_config();
    let mut agent = Agent::new(crate::prompts::AGENT_GLOBAL, config, provider, tools);

    let trace = agent
        .chat(user_input, None, None)
        .map_err(|e| StudioError::Ai(e.to_string()))?;
    for step in &trace.steps {
        on_step(step.clone());
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_max_tool_turns_defaults_and_clamps() {
        assert_eq!(resolve_max_tool_turns(None), 40, "未配置用默认值");
        assert_eq!(resolve_max_tool_turns(Some(80)), 80, "范围内原样生效");
        assert_eq!(resolve_max_tool_turns(Some(1)), 1, "下边界");
        assert_eq!(resolve_max_tool_turns(Some(200)), 200, "上边界");
        assert_eq!(resolve_max_tool_turns(Some(0)), 1, "0 拉回下边界");
        assert_eq!(resolve_max_tool_turns(Some(9999)), 200, "超上限拉回 200");
    }
}
