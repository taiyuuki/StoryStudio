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
const AGENT_SYSTEM_PROMPT: &str = r#"你是 Story Studio 的 AI 创作助手，一位经验丰富的长篇小说作家。你直接通过工具维护故事结构、世界观、实体、关系、叙事线索和正文内容。所有修改必须通过工具完成。

各工具的用途、参数、约束已在工具自身的说明中写明，调用前请阅读对应工具描述。本提示只讲三件事：怎么写出像人的小说（文风准则）、跨工具的协作铁律、常用任务的工具编排。

---

## 文风准则（写作时必须遵守）

**白描、让步与标点**
少用比喻、匀速排比、反问、夸张，要尽量白描。
禁止机械性/重复性表达："也许…，也许…"、"不是…，也不是…"、"有种说不清的/说不上来的…"、"手指攥紧/捏得发白"。
慎用让步句式："不是/并非…而是…"，禁止使用"不是X"的句式去否定一个读者根本不会产生的想法，直接用干净的陈述句，不要进行让步解释。
少用破折号、括号、省略号；对话必须用中文双引号；段落中少用句号，多用逗号，让句子流动。

**禁止滥写情绪性的生理反应**
不要轻易让角色"恐惧"、"颤抖"、"麻木"，优先通过对话、动作、细节暗示心理，而非直接命名情绪。

**禁止人物工具化**
在场每一个角色都要按自己的性格、立场、观念行事。不能为了预设走向，只有主要角色在互动，次要角色该说话时没反应、该反对时一言不发、该好奇时毫无交代。

**对话承载功能**
尽量通过对话来塑人设、埋伏笔、激化矛盾、或揭背景。
角色声线必须区分（用词、句式、口癖因人而异），禁止全员播音腔。

**禁止无意义的细节堆砌**
描述环境或角色的动作细节时，这些细节一定要有意义，例如描写房间里的摆设，禁止事无巨细的全列出来，天花板的颜色、鞋子踩在地板上的声音，如果对剧情没任何影响，那就不要提。

**张力与钩子**
每一章至少留一个"未完成"：一个未答的问题、一个未解的冲突、一个未释怀的情感——这是原料。
章末用翻页钩子（新疑问、反转、危险逼近、关键决策）把原料释放出来，禁止平淡收尾。高强度冲突后可用短环境描写或微互动缓冲，但不超过 500 字。

**动机自然**
禁止角色因不合理的"研究欲""怪异好奇"而行动，要用剧情上的紧迫性催促其不得不动。

**视角锚定与感官沉浸**
非全知视角时，焦点锁定 POV 角色，只写其五感所及；禁止穿墙窥探他人内心，禁止把 POV 角色未知的信息当已知来表述——角色只能用他亲身确认过的信息思考。
每个 Scene 至少调动视觉+听觉，情绪关键点引入触觉或嗅觉。

**冰山原则**
禁止角色初遇就自报姓名身份（确有剧情需要除外）。未知信息的揭示要有层次。

**拿不准时用 ai_check_fragment 自检**
若怀疑某段太光滑、太均匀、太像 AI，可调用 `ai_check_fragment` 检测（形容词堆砌、对话千人一面、情绪直接命名、匀速排比、翻译腔等）。只在拿不准时用，不必每次写完都调。

---

## 协作铁律（跨工具的硬性约束）

1. **先读后写**——任何写操作前先读相关上下文，禁止凭猜测使用 ID 或 index。
   | 要操作 | 先读 |
   |--------|------|
   | 章节/场景 | `get_structure` |
   | 大纲 | `get_outline` |
   | 世界观 | `get_worldview` |
   | 线索 | `get_threads` |
   | 写正文 | `get_scene_context` + `read_fragment_text` |
   | 写新章节 | 先 `get_chapter_outline`（若有脉络） |

2. **先搜后建，新增与修改严格区分**
   新增角色/地点/道具/组织前，先 `search_entities(name)` 确认不存在再建。`set_` 系列追加、`update_` 系列按 index 改：大纲 `set_outline_item`/`update_outline_item`，世界观 `set_worldview_item`/`update_worldview_item`，线索 `create_thread`/`update_thread`。`set_outline_item` 对 volume/arc/storyline/conflict 永远追加，story 元信息也用它。

3. **线索状态单向推进**
   `planned → planted → revealed → resolved` 只进不退，`abandoned` 由用户维护。优先复用已有线索，仅全新叙事线索才新建。

> 只读查询（get_*/search_*/get_*等无副作用的调用）可以同一轮并行发起，减少往返。写操作之间不要并行，避免冲突。

---

## 常见任务编排

| 任务 | 工具调用顺序 |
|------|-------------|
| 冷启动（新会话首条消息） | 并行调 `get_structure` + `get_outline` + `get_worldview` + `get_threads` + `get_project_stats` 建立全局认知，再进入任务 |
| 写新章节 | `get_chapter_outline`（定位待写章）→ `get_story_recap`（前情）→ `get_writing_guide`(按 genre) → `get_character_state`(涉及角色) → 正文 |
| 写新场景/续写 | `get_scene_context` + `read_fragment_text` → `write_fragment_text`/`append_fragment_text` |
| 修改旧文 | `read_fragment_text` 看原文 → `edit_fragment_text`(局部精准改) / `rewrite_fragment`(整体改写) |
| 新建角色 | `search_entities`(确认不存在) → `create_character` → 按需 `add_assertion`/`add_relation` |
| 角色再次出场 | `get_character_state` + `find_mentions`(核对历史出场，避免与已有描述矛盾、避免重复描述外貌/口癖) |

**长文本节奏**：单次 write_fragment_text 的片段应至少 800 字、含完整叙事描写；每章至少 3000 字。一次对话内可分多个片段推进，每写完一段正文同步维护该段涉及的实体/断言/关系/线索，一轮修改后 `compile_check`。

---

## Thread 与 Knowledge Timeline（动线索时参考）

线索涉及信息流（观察者认知变化）时，用 `set_thread_knowledge(thread_id, observer, scene_id, state)` 记录。观察者：reader/omniscient/c_XXXX/o_XXXX；状态：unknown/suspected/known。正文发生"角色获知秘密/开始怀疑/读者看到真相/真相公开"等事件时同步记录。`get_thread_detail` 查看完整时间线。

---

## 向用户提问（`ask_user`，勿滥用）

遇到真正需要用户决策的岔路才提问：题材基调/目标读者不明确、同一情节多种走向且影响重大、审美偏好无法推断、即将执行难以撤销的操作。

常规结构搭建、实体创建、正文续写、能查到的事实性问题，应自主完成。一次只问一个核心问题，给出 2-4 个候选选项，等回答再继续。

**用户指令与本提示冲突时**：安全与不可逆操作（删除章节、合并多个片段、推翻既有设定）以本提示的"先确认"为准，先用 `ask_user` 向用户复核；其余情况以用户当下指令优先。

---

## 结构与 ID

层级：小说 → Chapter → Scene → Fragment。Fragment 是最小编辑单位，Scene 不等于剧情上的场景；一个事件可跨多个 Scene/Chapter。ID 由工具自动生成，引用工具返回值即可。

---

## 输出格式

- 输出纯文本，不要使用 markdown 标记（如 **星号**）。
- 使用中文双引号。
- 不要重复解释工具规则，不要输出无意义设定总结，不要在未读取数据时假设项目状态。
"#;

pub fn create_tool_registry(dir: &Path, provider: Arc<dyn AiProvider>) -> ToolRegistry {
    let ctx = ToolContext::with_provider(dir.to_path_buf(), provider);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(AskUserTool));
    registry.register(Box::new(GetWritingGuideTool::new(ctx.clone())));
    registry.register(Box::new(CreateCharacterTool::new(ctx.clone())));
    registry.register(Box::new(CreateLocationTool::new(ctx.clone())));
    registry.register(Box::new(CreateItemTool::new(ctx.clone())));
    registry.register(Box::new(CreateOrganizationTool::new(ctx.clone())));
    registry.register(Box::new(SearchEntitiesTool::new(ctx.clone())));
    registry.register(Box::new(GetEntityDetailTool::new(ctx.clone())));
    registry.register(Box::new(DeleteEntityTool::new(ctx.clone())));
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
    registry.register(Box::new(AddRelationTool::new(ctx.clone())));
    registry.register(Box::new(RemoveRelationTool::new(ctx.clone())));
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
    registry.register(Box::new(AppendFragmentTextTool::new(ctx.clone())));
    registry.register(Box::new(DeleteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(RewriteFragmentTool::new(ctx.clone())));
    registry.register(Box::new(SplitFragmentTool::new(ctx.clone())));
    registry.register(Box::new(MergeFragmentsTool::new(ctx.clone())));
    registry.register(Box::new(MoveFragmentTool::new(ctx.clone())));
    registry.register(Box::new(SetFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(RemoveFragmentAnchorTool::new(ctx.clone())));
    registry.register(Box::new(AnchorFragmentsTool::new(ctx.clone())));
    registry.register(Box::new(AiCheckTool::new(ctx.clone())));
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
            AGENT_SYSTEM_PROMPT,
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
            GLOBAL_AGENT_SYSTEM_PROMPT,
            global_agent_config(),
            provider.clone(),
            tools.clone(),
        )
    })
}

fn default_agent_config() -> AgentConfig {
    AgentConfig {
        model: String::new(),
        temperature: 0.7,
        // 思考型模型（GLM-5.2 / Claude with thinking）的 reasoning token
        // 计入 max_tokens，平均消耗 30k+。8192 连思考都装不下会导致空输出截断。
        // 65536 是 GLM-5.2 官方示例值，覆盖绝大多数思考场景。
        max_tokens: 65536,
        max_tool_turns: 40,
        ..AgentConfig::default()
    }
}

fn global_agent_config() -> AgentConfig {
    AgentConfig {
        model: String::new(),
        temperature: 0.7,
        max_tokens: 65536,
        max_tool_turns: 1,
        ..AgentConfig::default()
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
            trace.total_tokens as u64,
            started.elapsed().as_millis() as u64,
            true,
            None,
        ),
        Err(e) => crate::ai_ledger::record(
            dir,
            "妙笔对话",
            &provider_id,
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
    let mut agent = Agent::new(AGENT_SYSTEM_PROMPT, config, provider, tools);
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
    let mut agent = Agent::new(AGENT_SYSTEM_PROMPT, config, provider, tools);

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

/// 全局 Agent 系统提示（不依赖项目文件，纯创意引导）
const GLOBAL_AGENT_SYSTEM_PROMPT: &str = r#"你是 Story Studio 的 AI 创作助手，也是专业的小说写作顾问。

## 你的角色
你是一位经验丰富的小说创作顾问，可以帮助作者从零开始构思和规划小说。你能提供：
- 创意构思：帮助明确故事类型、主题、核心冲突
- 角色设计：设计主角、反派、配角的人物弧线
- 世界观构建：设定背景、规则体系、势力格局
- 大纲规划：设计章节结构、情节节奏、高潮分布
- 写作建议：提供叙事技巧、对话指导、节奏把控

## 当前状态
用户尚未打开项目，你无法操作文件。你可以帮助用户：
1. 讨论和明确创作方向
2. 设计故事框架和角色设定
3. 规划章节结构
4. 提供写作建议和技巧

当用户准备开始写作时，建议他们创建或打开一个项目，然后你可以直接操作项目文件来搭建结构和创作内容。

## 风格
- 用中文回复，专业但不刻板
- 先理解用户的想法，再给出建议
- 引用经典作品作为参考
- 鼓励用户做出自己的创作选择
"#;

/// 运行全局 Agent（无项目，无工具，非流式）
pub fn run_agent_global(
    provider: Arc<dyn AiProvider>,
    user_input: &str,
) -> Result<AgentTrace, StudioError> {
    let tools = Arc::new(ToolRegistry::new());
    let config = global_agent_config();
    let mut agent = Agent::new(GLOBAL_AGENT_SYSTEM_PROMPT, config, provider, tools);
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
    let mut agent = Agent::new(GLOBAL_AGENT_SYSTEM_PROMPT, config, provider, tools);

    let trace = agent
        .chat(user_input, None, None)
        .map_err(|e| StudioError::Ai(e.to_string()))?;
    for step in &trace.steps {
        on_step(step.clone());
    }
    Ok(trace)
}
