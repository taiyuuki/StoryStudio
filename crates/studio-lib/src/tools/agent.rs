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
// 工具并发闸门（读共享 / 写排他）
// ============================================================================

// 读 / 写分类的唯一真源是注册点：ToolRegistry::register（默认按会写处理）
// 与 ToolRegistry::register_readonly（不改动项目文档树）。此前这里维护一份
// 名字白名单，注册表新增或改了工具名时没有任何机制会发现，MCP 只读模式的
// 暴露表与工具闸门的读锁都会跟着失真。
//
// 判定口径是"是否改动项目文档树"，不是"是否有任何副作用"：
// - ai_check_* 会写 AI 台账（旁路日志，写入自身有原子性与回读校验），
//   但不碰稿件——仍算只读。否则一次整章审校要独占写锁数分钟，把所有读写
//   都堵住（代价仅是极端并发下可能丢一条台账记录）；
// - get_story_recap 会把现场生成的场景摘要写回 structure.toml，属于项目
//   SoT 的读-改-写，必须持写锁，因此不是只读；
// - ask_user 不改项目，但会阻塞等待用户回答，绝不能持写锁。

/// 带项目闸门的工具包装：查询类共享读锁，其余（默认）整体持写锁——
/// 扫描→分配→落盘因此原子，并发的 ID 分配竞态（多路 create_* 分到同一
/// ID 互相覆盖）与流水线乱序（改名跑在建档之前）都被闸住。
///
/// 闸门只覆盖工具执行本身；Agent 的 LLM 思考阶段不持锁，妙笔长对话
/// 不会把整个项目的写入口堵死。
///
/// 回执接力：execute 成功后把待送达的后台锚定结果 / 在途状态追加到
/// 回执尾部（见 `anchoring::AnchorDeliveryState`）。MCP 等请求-响应
/// 调用方没有服务端推送通道，这是超时转后台的锚定结果唯一的确定性
/// 送达路径——任何工具的下一次调用都会带回。
struct GatedTool {
    inner: Box<dyn Tool>,
    lock: crate::project_lock::ProjectLock,
    is_read: bool,
    anchor_delivery: Arc<std::sync::Mutex<crate::tools::anchoring::AnchorDeliveryState>>,
    receipt_notes: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Tool for GatedTool {
    fn definition(&self) -> ToolDef {
        self.inner.definition()
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let _read = if self.is_read {
            Some(self.lock.read())
        } else {
            None
        };
        let _write = if _read.is_none() {
            Some(self.lock.write())
        } else {
            None
        };
        let mut result = self.inner.execute(arguments);
        // 失败的回执不附送达段（保持错误聚焦；结果留到下一次成功调用）
        if let Ok(text) = &mut result {
            let note = crate::tools::anchoring::take_anchor_delivery_note(&self.anchor_delivery);
            if !note.is_empty() {
                text.push_str(&note);
            }
            for note in crate::tools::take_receipt_notes_from(&self.receipt_notes) {
                text.push_str(&note);
            }
        }
        result
    }
}

// ============================================================================
/// 构建完整工具注册表，并给每个工具套上项目闸门。
///
/// **每个工具必须在注册点声明读写分类**：改动项目文档树的用
/// [`ToolRegistry::register`]（默认），不碰稿件的用
/// [`ToolRegistry::register_readonly`]。这一处声明同时决定两件事——
/// 工具执行期间持读锁还是写锁，以及 MCP `--read-only` 模式是否暴露它。
/// 漏标的代价是"少一点并发"（默认按会写处理），不会反向出错。
pub fn create_tool_registry(dir: &Path, provider: Arc<dyn AiProvider>) -> ToolRegistry {
    let ctx = ToolContext::with_provider(dir.to_path_buf(), provider);
    // GatedTool 回执接力与锚定 worker 共用同一送达队列（ctx 内 Arc）
    let anchor_delivery = ctx.anchor_delivery().clone();
    let receipt_notes = ctx.receipt_notes().clone();
    let mut registry = ToolRegistry::new();
    registry.register_readonly(Box::new(AskUserTool));
    registry.register_readonly(Box::new(PrepareWritingTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetWritingGuideTool::new(ctx.clone())));
    registry.register(Box::new(CreateEntityTool::new(ctx.clone())));
    registry.register_readonly(Box::new(SearchEntitiesTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetEntityDetailTool::new(ctx.clone())));
    registry.register_readonly(Box::new(ProfileCheckTool::new(ctx.clone())));
    registry.register(Box::new(DeleteEntityTool::new(ctx.clone())));
    registry.register(Box::new(MergeEntitiesTool::new(ctx.clone())));
    registry.register(Box::new(RevealCharacterNameTool::new(ctx.clone())));
    // 会写回 structure.toml（现场生成的场景摘要落盘）——必须持写锁
    registry.register(Box::new(GetStoryRecapTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetRelationsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(QueryAssertionsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetCharacterStateTool::new(ctx.clone())));
    registry.register_readonly(Box::new(FindMentionsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(SearchTextTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetProjectStatsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetSceneContextTool::new(ctx.clone())));
    registry.register_readonly(Box::new(CompileCheckTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetStructureTool::new(ctx.clone())));
    registry.register(Box::new(CreateSceneTool::new(ctx.clone())));
    registry.register(Box::new(CreateChapterTool::new(ctx.clone())));
    registry.register(Box::new(UpdateSceneTool::new(ctx.clone())));
    registry.register(Box::new(SaveSceneSummariesTool::new(ctx.clone())));
    registry.register(Box::new(RepairProjectTool::new(ctx.clone())));
    registry.register(Box::new(DeleteSceneTool::new(ctx.clone())));
    registry.register(Box::new(SplitSceneTool::new(ctx.clone())));
    registry.register(Box::new(MergeScenesTool::new(ctx.clone())));
    registry.register(Box::new(UpdateChapterTool::new(ctx.clone())));
    registry.register(Box::new(DeleteChapterTool::new(ctx.clone())));
    registry.register(Box::new(AddAssertionTool::new(ctx.clone())));
    registry.register(Box::new(RetireAssertionTool::new(ctx.clone())));
    registry.register(Box::new(RefreshEvidenceTool::new(ctx.clone())));
    registry.register(Box::new(AddRelationTool::new(ctx.clone())));
    registry.register(Box::new(RemoveRelationTool::new(ctx.clone())));
    registry.register(Box::new(UpdateRelationTool::new(ctx.clone())));
    registry.register(Box::new(CreateThreadTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetThreadsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetThreadDetailTool::new(ctx.clone())));
    registry.register(Box::new(UpdateThreadTool::new(ctx.clone())));
    registry.register(Box::new(AdvanceThreadTool::new(ctx.clone())));
    registry.register(Box::new(SetThreadKnowledgeTool::new(ctx.clone())));
    registry.register(Box::new(DeleteThreadTool::new(ctx.clone())));
    registry.register(Box::new(WriteFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(EditFragmentTextTool::new(ctx.clone())));
    registry.register_readonly(Box::new(ReadFragmentTextTool::new(ctx.clone())));
    registry.register_readonly(Box::new(ReadSceneFullTextTool::new(ctx.clone())));
    registry.register(Box::new(AppendFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(DeleteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(RewriteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(SplitFragmentTool::new(ctx.clone())));
    registry.register(Box::new(MergeFragmentsTool::new(ctx.clone())));
    registry.register(Box::new(MoveFragmentTool::new(ctx.clone())));
    registry.register_readonly(Box::new(ListFragmentVersionsTool::new(ctx.clone())));
    registry.register(Box::new(RestoreFragmentVersionTool::new(ctx.clone())));
    registry.register(Box::new(SetFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(RemoveFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(AnchorFragmentsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(ListUnresolvedRefsTool::new(ctx.clone())));
    registry.register(Box::new(ResolveUnresolvedRefTool::new(ctx.clone())));
    registry.register_readonly(Box::new(AiCheckTool::new(ctx.clone())));
    registry.register_readonly(Box::new(AiCheckPovTool::new(ctx.clone())));
    registry.register(Box::new(UpdateEntityTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetOutlineTool::new(ctx.clone())));
    registry.register(Box::new(SetOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(UpdateOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(DeleteOutlineItemTool::new(ctx.clone())));
    registry.register(Box::new(DesignChaptersTool::new(ctx.clone())));
    registry.register(Box::new(PopulateScenesFromBeatsTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetChapterOutlineTool::new(ctx.clone())));
    registry.register(Box::new(UpdateChapterOutlineTool::new(ctx.clone())));
    registry.register(Box::new(DeleteChapterOutlineTool::new(ctx.clone())));
    registry.register_readonly(Box::new(GetWorldviewTool::new(ctx.clone())));
    registry.register(Box::new(SetWorldviewItemTool::new(ctx.clone())));
    registry.register(Box::new(SetTimelineErasTool::new(ctx.clone())));
    registry.register(Box::new(UpdateWorldviewItemTool::new(ctx.clone())));
    registry.register(Box::new(DeleteWorldviewItemTool::new(ctx.clone())));
    // 全部工具套项目闸门：桌面端妙笔、MCP（stdio / 内嵌 HTTP）、CLI
    // 的每一次工具调用都经同一把 ProjectLock（进程内 RwLock + 跨进程
    // 文件锁）排队。读写分类由注册点声明、经 map_tools 透传给闸门——
    // 闸门不再自己去翻白名单，分类不可能与注册表脱节
    let lock = crate::project_lock::ProjectLock::for_root(dir);
    registry.map_tools(move |t, is_read| {
        Box::new(GatedTool {
            inner: t,
            lock: lock.clone(),
            is_read,
            anchor_delivery: anchor_delivery.clone(),
            receipt_notes: receipt_notes.clone(),
        })
    })
}

/// 签名专用占位 provider：[`unbound_tool_registry`] 只取工具定义不执行，
/// 该 provider 永不应被调用。
#[derive(Debug)]
struct DefinitionOnlyProvider;

impl AiProvider for DefinitionOnlyProvider {
    fn id(&self) -> &str {
        "definition-only"
    }

    fn generate(
        &self,
        _request: &story_ai_runtime::provider::GenerateRequest,
    ) -> Result<story_ai_runtime::provider::GenerateResponse, story_ai_runtime::provider::AiError>
    {
        Err(story_ai_runtime::provider::AiError::Unavailable(
            "unbound_tool_registry 只取工具定义，不会调用 provider".into(),
        ))
    }
}

/// 构建不绑定具体项目的"签名用"工具注册表。
///
/// 工具名、参数 schema 与读写分类都不随项目变化；需要 tools/list 描述符
/// 却没有项目可绑的场景（如 MCP 工作区模式启动时尚未绑定项目）用它。
/// 实现上在临时空项目上走一次 [`create_tool_registry`]：保证工具清单、
/// 分类与真实注册点永远同源，不会另养一份名单失真。注册表构造是纯
/// 内存操作（仅 ProjectLock 预备锁目录），不解析项目、不产生 LLM 调用；
/// 通过它 execute 没有意义（项目目录随临时目录一起销毁）。
pub fn unbound_tool_registry() -> Result<ToolRegistry, StudioError> {
    let tmp = tempfile::tempdir().map_err(|e| StudioError::Io(format!("创建临时目录失败：{e}")))?;
    story_project_format::init::init_project(tmp.path(), "definitions")
        .map_err(|e| StudioError::Io(format!("初始化临时项目失败：{e}")))?;
    Ok(create_tool_registry(
        tmp.path(),
        Arc::new(DefinitionOnlyProvider),
    ))
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

    /// 回执接力：GatedTool 把待送达的后台锚定结果追加到成功回执尾部，
    /// 一次送达即清空；失败回执不带走结果，留到下一次成功调用
    #[test]
    fn gated_tool_relays_anchor_delivery_note() {
        struct EchoTool;
        impl story_ai_runtime::Tool for EchoTool {
            fn definition(&self) -> story_ai_runtime::ToolDef {
                story_ai_runtime::ToolDef {
                    name: "echo".into(),
                    description: "测试桩".into(),
                    parameters: serde_json::json!({}),
                }
            }
            fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
                Ok("echo-ok".to_string())
            }
        }
        struct FailTool;
        impl story_ai_runtime::Tool for FailTool {
            fn definition(&self) -> story_ai_runtime::ToolDef {
                story_ai_runtime::ToolDef {
                    name: "fail".into(),
                    description: "测试桩".into(),
                    parameters: serde_json::json!({}),
                }
            }
            fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
                Err("工具失败".to_string())
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "relay-test").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        // 塞一条待送达的后台锚定结果
        crate::tools::anchoring::finish_deferred_anchor(
            ctx.anchor_delivery(),
            "f_0001",
            &Ok(crate::tools::anchoring::AnchorOutcome {
                anchored: vec![(
                    "c_0001".to_string(),
                    "referenced".to_string(),
                    "他".to_string(),
                )],
                unresolved: vec![("灰袍女人".to_string(), "疑似新角色".to_string())],
            }),
        );

        let make_gated = |inner: Box<dyn story_ai_runtime::Tool>| GatedTool {
            inner,
            lock: crate::project_lock::ProjectLock::for_root(tmp.path()),
            is_read: true,
            anchor_delivery: ctx.anchor_delivery().clone(),
            receipt_notes: ctx.receipt_notes().clone(),
        };

        // 成功回执：尾部追加送达段（结果 + 处置引导）
        let out = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("echo-ok"), "{out}");
        assert!(out.contains("后台锚定完成"), "{out}");
        assert!(out.contains("f_0001"), "{out}");
        assert!(out.contains("c_0001"), "{out}");
        assert!(out.contains("处置方式"), "{out}");

        // 一次送达即清空
        let out2 = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(!out2.contains("后台锚定完成"), "{out2}");

        // 失败回执不带走待送达结果（空结果不入队，此处给非空结果）
        crate::tools::anchoring::finish_deferred_anchor(
            ctx.anchor_delivery(),
            "f_0002",
            &Ok(crate::tools::anchoring::AnchorOutcome {
                anchored: vec![(
                    "c_0002".to_string(),
                    "appears".to_string(),
                    "那人".to_string(),
                )],
                unresolved: Vec::new(),
            }),
        );
        let err = make_gated(Box::new(FailTool))
            .execute(&serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, "工具失败");

        // 结果留到下一次成功调用送达
        let out3 = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out3.contains("f_0002"), "{out3}");
    }

    /// 回执接力（通用队列）：后台任务（如超时转后台的 AI 检测）的待送达
    /// 回执同样随成功回执送达、一次取空；失败回执不带走
    #[test]
    fn gated_tool_relays_receipt_notes() {
        struct EchoTool;
        impl story_ai_runtime::Tool for EchoTool {
            fn definition(&self) -> story_ai_runtime::ToolDef {
                story_ai_runtime::ToolDef {
                    name: "echo".into(),
                    description: "测试桩".into(),
                    parameters: serde_json::json!({}),
                }
            }
            fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
                Ok("echo-ok".to_string())
            }
        }
        struct FailTool;
        impl story_ai_runtime::Tool for FailTool {
            fn definition(&self) -> story_ai_runtime::ToolDef {
                story_ai_runtime::ToolDef {
                    name: "fail".into(),
                    description: "测试桩".into(),
                    parameters: serde_json::json!({}),
                }
            }
            fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
                Err("工具失败".to_string())
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "relay-note-test").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let make_gated = |inner: Box<dyn story_ai_runtime::Tool>| GatedTool {
            inner,
            lock: crate::project_lock::ProjectLock::for_root(tmp.path()),
            is_read: true,
            anchor_delivery: ctx.anchor_delivery().clone(),
            receipt_notes: ctx.receipt_notes().clone(),
        };

        ctx.push_receipt_note(
            "\n📬 后台 AI 检测完成（此前超时转后台的检测，结果送达）：报告内容".to_string(),
        );

        // 成功回执：尾部追加送达段，一次送达即清空
        let out = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("echo-ok"), "{out}");
        assert!(out.contains("后台 AI 检测完成"), "{out}");
        let out2 = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(!out2.contains("后台 AI 检测完成"), "{out2}");

        // 失败回执不带走待送达回执，留到下一次成功调用
        ctx.push_receipt_note("\n📬 后台任务结果".to_string());
        let err = make_gated(Box::new(FailTool))
            .execute(&serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, "工具失败");
        let out3 = make_gated(Box::new(EchoTool))
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out3.contains("后台任务结果"), "{out3}");
    }
}

#[cfg(test)]
mod registry_size_probe {
    use story_ai_runtime::provider::{GenerateRequest, GenerateResponse};

    #[derive(Debug)]
    struct NullProvider;
    impl story_ai_runtime::AiProvider for NullProvider {
        fn id(&self) -> &str {
            "null"
        }
        fn generate(
            &self,
            _req: &GenerateRequest,
        ) -> Result<GenerateResponse, story_ai_runtime::AiError> {
            Err(story_ai_runtime::AiError::Unavailable("null".into()))
        }
    }

    /// 工具预算守卫：数量与定义体积超预算即失败。
    ///
    /// 2026-10 精简（84 → 77，见 tool-consolidation 分支）后钉住两条线：
    /// - 数量 ≤ 80：近重复入口会让模型在相似工具间产生选择歧义；
    ///   新增工具前先考虑并入现有入口（kind/scope/op 参数化是既有模式）
    /// - 定义体积 ≤ 48000 字符：全量定义随每次请求发送，体积直接吃上下文
    ///
    /// 预算不是硬上限——确有充分理由时可上调，但必须是有意识的决策，
    /// 不允许工具集无感膨胀。
    #[test]
    fn registry_stays_within_tool_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let provider: std::sync::Arc<dyn story_ai_runtime::AiProvider> =
            std::sync::Arc::new(NullProvider);
        let registry = super::create_tool_registry(tmp.path(), provider);
        let defs = registry.definitions();
        let json = serde_json::to_string(&defs).unwrap();
        let chars = json.chars().count();

        assert!(
            defs.len() <= 80,
            "工具数 {} 超过预算 80：新增前先考虑并入现有入口（kind/scope/op 参数化）",
            defs.len()
        );
        assert!(
            chars <= 48_000,
            "工具定义体积 {} 字符超过预算 48000：精简描述或合并工具",
            chars
        );
    }
}
