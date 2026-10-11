//! ServerHandler 实现 — ToolRegistry 到 MCP 的适配层

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use story_ai_runtime::{clamp_tool_result, ToolDef, ToolRegistry};
use story_studio_lib::{create_provider_for_purpose, create_tool_registry, purpose, StudioError};

use crate::McpServerOptions;

/// MCP 环境下不暴露的工具。
/// ask_user 依赖桌面端的线程局部交互上下文（等待宿主 UI 答复），
/// MCP 客户端没有宿主 UI，外部 Agent 应直接询问自己的用户。
const HIDDEN_TOOLS: &[&str] = &["ask_user"];

/// 只读模式白名单：仅暴露不修改项目的查询 / 分析类工具。
/// 刻意用白名单而非黑名单——registry 新增工具默认不出现在只读模式，
/// 直到确认无副作用才手工加入。
pub const READ_ONLY_TOOLS: &[&str] = &[
    // 结构 / 实体 / 知识 / 线索 / 正文 / 大纲 / 世界观查询
    "get_structure",
    "get_project_stats",
    "search_entities",
    "get_entity_detail",
    "profile_check",
    "get_relations",
    "query_assertions",
    "get_character_state",
    "find_mentions",
    "search_text",
    "get_scene_context",
    "get_threads",
    "get_thread_detail",
    "read_fragment_text",
    "read_scene_full_text",
    "list_fragment_versions",
    "get_outline",
    "get_chapter_outline",
    "get_worldview",
    "get_writing_guide",
    "prepare_writing",
    "list_unresolved_refs",
    // 校验 / 分析（不改项目，但 ai_check_* / get_story_recap 会调用 AI 服务）
    "compile_check",
    "ai_check_fragment",
    "ai_check_scene",
    "ai_check_chapter",
    "ai_check_pov",
    "get_story_recap",
];

/// 工具在当前模式下是否暴露
pub fn is_exposed(name: &str, read_only: bool) -> bool {
    if HIDDEN_TOOLS.contains(&name) {
        return false;
    }
    if read_only {
        READ_ONLY_TOOLS.contains(&name)
    } else {
        true
    }
}

/// 内部 ToolDef 列表 → MCP 工具描述符列表（按模式过滤）
pub fn build_tool_descriptors(defs: Vec<ToolDef>, read_only: bool) -> Vec<Tool> {
    defs.into_iter()
        .filter(|d| is_exposed(&d.name, read_only))
        .map(mcp_tool_from_def)
        .collect()
}

/// ToolDef → MCP Tool。两者形状同构（name + description + JSON Schema），
/// 只做容器转换。
fn mcp_tool_from_def(def: ToolDef) -> Tool {
    let schema = match def.parameters {
        serde_json::Value::Object(map) => map,
        // 缺 schema 时退回空对象（{} 是合法的"任意参数"schema）
        _ => JsonObject::new(),
    };
    Tool::new(
        Cow::Owned(def.name),
        Cow::Owned(def.description),
        Arc::new(schema),
    )
}

/// MCP 工具调用的失败分类
#[derive(Debug)]
enum CallFailure {
    /// 工具执行失败 — 以 isError 结果返回，让调用方模型看到并自行纠偏
    ToolError(String),
    /// 协议层参数错误 — 以 JSON-RPC error 返回
    InvalidParams(String),
}

/// 工具执行闸门：读共享、写排他（进程级全局——HTTP 模式每会话一个
/// server 实例，闸门必须跨会话共享）。
///
/// rmcp 对每个请求并发派发，工具若并行执行会发生两类数据损坏：
/// - 同连接流水线调用的乱序（改名跑在建档之前）；
/// - ID 分配竞态：并发的 create_* 扫号互不可见，多个调用分到同一 ID、
///   写同一文件互相覆盖，先写的数据静默丢失。
///
/// 变更类工具整体持写锁（扫描→分配→落盘因此原子），查询类共享读锁
/// 不互相阻塞。中毒不扩散（工具 panic 后服务继续可用）。
static TOOL_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// 持闸执行工具：查询类（[`READ_ONLY_TOOLS`] 白名单）读锁，其余写锁。
/// 未列入白名单的新工具默认按变更处理（安全默认）。
///
/// 两级闸门：进程内 [`TOOL_GATE`]（RwLock）+ 跨进程项目锁
/// （std::fs::File 原生 `lock`/`lock_shared`，1.89+ 稳定；macOS 为 flock、
/// Linux 为 OFD fcntl，均属 open file description——同进程独立打开的
/// 描述符也互斥；`gate` 为锁文件路径。同一项目可能被多个进程同时服务
/// ——App 内嵌 HTTP 与外部 stdio CLI 并存时，进程内锁管不到对方的
/// ID 分配竞态）。每次调用独立打开锁文件、drop 自动释放、进程崩溃即
/// 解锁；锁文件不可用时退化为仅进程内闸门（best-effort）。
fn execute_gated(
    registry: &ToolRegistry,
    read_only: bool,
    name: &str,
    arguments: &serde_json::Value,
    gate: Option<&Path>,
) -> Result<String, CallFailure> {
    if HIDDEN_TOOLS.contains(&name) {
        return Err(CallFailure::InvalidParams(format!(
            "工具 {name} 在 MCP 服务中不可用（依赖桌面端交互）"
        )));
    }
    // 只读模式在调用层复核：即使客户端绕过 tools/list 直接调用
    if read_only && !READ_ONLY_TOOLS.contains(&name) {
        return Err(CallFailure::InvalidParams(format!(
            "服务器以 --read-only 模式启动，{name} 会修改项目，不可用"
        )));
    }
    // 跨进程项目锁（读写语义与进程内闸门一致）；获取顺序恒为
    // 进程内 → 跨进程，无死锁环。锁文件只作锁载体，永不写入（truncate(false)）
    let flock = gate.and_then(|p| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(p)
            .ok()
    });
    if let Some(f) = flock.as_ref() {
        let is_read = READ_ONLY_TOOLS.contains(&name);
        let res = if is_read { f.lock_shared() } else { f.lock() };
        if let Err(e) = res {
            eprintln!("[mcp] 项目锁获取失败（退化为仅进程内闸门）：{e}");
        }
    }
    // 读共享 / 写排他（两种 guard 无公共类型，用 Option 对各持其一）。
    // 锁中毒（持锁工具 panic）时强制恢复而非拒服务：
    // 临界区内没有需要守护的不变量（工具自己保证文件一致性）
    let _read_guard = if READ_ONLY_TOOLS.contains(&name) {
        Some(TOOL_GATE.read().unwrap_or_else(std::sync::PoisonError::into_inner))
    } else {
        None
    };
    let _write_guard = if _read_guard.is_none() {
        Some(TOOL_GATE.write().unwrap_or_else(std::sync::PoisonError::into_inner))
    } else {
        None
    };
    match registry.execute(name, arguments) {
        Some(Ok(text)) => Ok(clamp_tool_result(&text)),
        Some(Err(e)) => Err(CallFailure::ToolError(e)),
        None => Err(CallFailure::InvalidParams(format!("未知工具：{name}"))),
    }
}

pub(crate) struct StoryMcpServer {
    registry: Arc<ToolRegistry>,
    tools: Vec<Tool>,
    instructions: String,
    read_only: bool,
    /// 跨进程项目锁文件路径（.novelstudio 不可写时为 None = 禁用）
    gate_path: Option<PathBuf>,
}

impl StoryMcpServer {
    pub(crate) fn new(options: McpServerOptions) -> Result<Self, StudioError> {
        let dir = options.project_dir;
        if !dir.join("project.toml").is_file() {
            return Err(StudioError::Config(format!(
                "目录不是有效的 Story Studio 项目（缺少 project.toml）：{}",
                dir.display()
            )));
        }
        // 与桌面端妙笔一致的 provider 解析：agent 用途档案 → 激活档案 →
        // 旧字段 / 环境变量。LLM 工具（ai_check_* 等）实际调用发生在
        // 工具执行时，启动阶段无需 API Key。
        let provider = Arc::from(create_provider_for_purpose(
            purpose::AGENT,
            "anthropic",
            None,
        )?);
        let registry = Arc::new(create_tool_registry(&dir, provider));
        let tools = build_tool_descriptors(registry.definitions(), options.read_only);
        let instructions = build_instructions(&dir, options.read_only);
        // 跨进程项目锁：同一项目目录可能同时被本进程（App 内嵌 HTTP）与
        // 其他进程（stdio CLI）服务，flock 兜底跨进程的 ID 分配竞态
        let gate_path = std::fs::create_dir_all(dir.join(".novelstudio"))
            .ok()
            .map(|_| dir.join(".novelstudio").join("tool-gate.lock"));
        Ok(Self {
            registry,
            tools,
            instructions,
            read_only: options.read_only,
            gate_path,
        })
    }

    /// 暴露的工具数（HTTP 句柄 / Tauri 状态用）
    pub(crate) fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

impl ServerHandler for StoryMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "story-studio",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(self.instructions.clone())
    }

    /// 只协商到 ping 仍是标准请求的协议版本。
    ///
    /// 2026-07-28（无状态修订）移除了 ping 请求：协商到该版本的客户端
    /// （如 ZCode）保活探测会报 "Method 'ping' is not supported by the
    /// negotiated protocol version"，连接被反复判 stale 重连。本服务
    /// 用不到该版本的特性，默认版本表里去掉它即可。
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[
            ProtocolVersion::V_2024_11_05,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
        ])
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let registry = self.registry.clone();
        let read_only = self.read_only;
        let gate_path = self.gate_path.clone();

        // 工具全部是同步阻塞实现（LLM 工具含网络调用），放到阻塞线程池，
        // 不占用 tokio worker。变更类工具经两级闸门（进程内 RwLock +
        // 跨进程 flock）串行（读共享），并发调用的 ID 分配与流水线依赖
        // 因此安全——包括 App 内嵌 HTTP 与外部 stdio CLI 同时服务同一
        // 项目的场景
        let outcome = tokio::task::spawn_blocking(move || {
            execute_gated(&registry, read_only, &name, &arguments, gate_path.as_deref())
        })
        .await
        .map_err(|e| McpError::internal_error(format!("工具执行线程异常：{e}"), None))?;

        match outcome {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into()),
            Err(CallFailure::ToolError(e)) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(e)]).into())
            }
            Err(CallFailure::InvalidParams(msg)) => Err(McpError::invalid_params(msg, None)),
        }
    }
}

/// initialize 响应里的 instructions — 给外部 Agent 的使用引导
fn build_instructions(dir: &Path, read_only: bool) -> String {
    let mut s = format!(
        "Story Studio 长篇小说项目工具集。当前项目：{}\n\
         建议：先用 get_structure / get_project_stats 了解项目结构，\
         用 search_text / search_entities 检索内容，\
         读正文用 read_fragment_text / read_scene_full_text，\
         动笔前用 prepare_writing 聚合上下文。\n\
         项目对象（卷 / 章 / 场景 / 片段 / 实体 / 线索等）一律用稳定 ID 引用，先查后用，\
         不要凭空猜测 ID。\n\
         修改类工具（create_* / update_* / write_fragment_text 等）会直接改写项目文件；\
         compile_check 可随时校验改动的一致性。",
        dir.display()
    );
    if read_only {
        s.push_str("\n本服务以只读模式启动：仅暴露查询类工具，修改类工具不可用。");
    } else {
        s.push_str(
            "\n部分工具（ai_check_* / get_story_recap / design_chapters 等）会调用配置好的 \
             AI 服务，耗时与费用由该 AI 档案决定。",
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 协商版本表不得包含 2026-07-28（该版本移除 ping，客户端保活会失败）
    #[test]
    fn negotiated_versions_exclude_pingless_era() {
        let versions = ServerHandler::supported_protocol_versions(&StoryMcpServer {
            registry: Arc::new(ToolRegistry::new()),
            tools: Vec::new(),
            instructions: String::new(),
            read_only: false,
            gate_path: None,
        });
        assert!(!versions.contains(&ProtocolVersion::V_2026_07_28));
        assert!(versions.contains(&ProtocolVersion::V_2025_11_25));
    }

    /// 闸门语义：读锁共享、写锁排他
    #[test]
    fn tool_gate_shares_reads_and_exclusives_writes() {
        let r1 = TOOL_GATE.read().unwrap();
        let r2 = TOOL_GATE.read().unwrap(); // 读共享：两把读锁并存
        drop((r1, r2));

        let w = TOOL_GATE.write().unwrap();
        assert!(
            TOOL_GATE.try_read().is_err(),
            "写锁持有期间读应被拒绝"
        );
        assert!(TOOL_GATE.try_write().is_err(), "写锁不可重入");
        drop(w);
        assert!(TOOL_GATE.try_read().is_ok(), "释放后应恢复");
    }

    /// 无副作用测试桩 provider（LLM 工具不会被本组测试触发）
    #[derive(Debug)]
    struct StubProvider;
    impl story_ai_runtime::provider::AiProvider for StubProvider {
        fn id(&self) -> &str {
            "stub-mcp-test"
        }
        fn generate(
            &self,
            _request: &story_ai_runtime::provider::GenerateRequest,
        ) -> Result<story_ai_runtime::provider::GenerateResponse, story_ai_runtime::provider::AiError>
        {
            Err(story_ai_runtime::provider::AiError::Unavailable(
                "测试桩不可调用".into(),
            ))
        }
    }

    /// 回归：rmcp 并发派发下，并发的 create_* 曾扫号互不可见、分到同一 ID、
    /// 写同一文件互相覆盖（多份"成功"回执只剩一份数据）。
    /// 经两级闸门（进程内 RwLock + 跨进程 flock）串行后，N 路并发创建
    /// 应得到 N 个互不相同的 ID。
    #[test]
    fn execute_gated_serializes_concurrent_creates() {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "gate-test").unwrap();
        let registry = Arc::new(story_studio_lib::create_tool_registry(
            tmp.path(),
            Arc::new(StubProvider),
        ));
        let gate = tmp.path().join(".novelstudio/tool-gate.lock");

        const N: usize = 6;
        let mut handles = Vec::new();
        for i in 0..N {
            let reg = registry.clone();
            let gate = gate.clone();
            handles.push(std::thread::spawn(move || {
                execute_gated(
                    &reg,
                    false,
                    "create_character",
                    &serde_json::json!({ "name": format!("并发{i}号"), "role": "probe" }),
                    Some(&gate),
                )
                .unwrap()
            }));
        }
        let mut ids = Vec::new();
        for h in handles {
            let text = h.join().unwrap();
            let id = text
                .split("id=")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .trim()
                .to_string();
            assert!(id.starts_with("c_"), "回执应含实体 ID: {text}");
            ids.push(id);
        }
        let distinct: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), N, "并发创建不应撞 ID: {ids:?}");

        // 落盘文件数与 ID 数一致（无互相覆盖）
        let files: Vec<_> = std::fs::read_dir(tmp.path().join("entities/characters"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("c_")
            })
            .collect();
        assert_eq!(files.len(), N, "每个实体应有自己的文件: {files:?}");

        // 只读模式下变更工具仍被拒（execute_gated 同时承担该复核）
        let err = execute_gated(
            &registry,
            true,
            "create_character",
            &serde_json::json!({ "name": "只读探针" }),
            Some(&gate),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::InvalidParams(_)));
    }

    /// 跨进程项目锁语义：锁属 open file description——同进程两次独立
    /// 打开也互斥；读共享、写排他；释放后可重取。
    #[test]
    fn project_flock_excludes_and_shares() {
        let open = |path: &Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .unwrap()
        };
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gate.lock");

        let w1 = open(&path);
        w1.lock().unwrap();
        let w2 = open(&path);
        assert!(w2.try_lock().is_err(), "独立打开的描述符应互斥");
        drop(w1);
        w2.lock().unwrap(); // 释放后可获取

        // 读共享：须先释放写锁，否则共享获取会阻塞（写排他语义）
        w2.unlock().unwrap();
        let r1 = open(&path);
        r1.lock_shared().unwrap();
        let r2 = open(&path);
        r2.lock_shared().unwrap(); // 读共享：两把并存
        let w3 = open(&path);
        assert!(w3.try_lock().is_err(), "读持有期间写应被拒");
        drop((r1, r2, w3, w2));
    }
}
