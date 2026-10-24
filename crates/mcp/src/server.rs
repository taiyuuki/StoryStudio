//! ServerHandler 实现 — ToolRegistry 到 MCP 的适配层

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use story_ai_runtime::{clamp_tool_result, AiProvider, ToolDef, ToolRegistry};
use story_studio_lib::{create_tool_registry, purpose, StudioError};

use crate::McpServerOptions;

/// MCP 环境下不暴露的工具。
/// ask_user 依赖桌面端的线程局部交互上下文（等待宿主 UI 答复），
/// MCP 客户端没有宿主 UI，外部 Agent 应直接询问自己的用户。
pub const HIDDEN_TOOLS: &[&str] = &["ask_user"];

/// 只读模式的暴露规则：以**注册表自己的分类**为准
/// （`ToolRegistry::register_readonly`，与工具并发闸门同一来源）。
///
/// 此前 MCP 另存一份名字白名单，与注册表是两份事实，改名或新增工具时
/// 只读模式会静默失真；现在分类只有注册点一个真源。
///
/// 工具在当前模式下是否暴露
pub fn is_exposed(registry: &ToolRegistry, name: &str, read_only: bool) -> bool {
    if HIDDEN_TOOLS.contains(&name) {
        return false;
    }
    if read_only {
        registry.is_read_only(name)
    } else {
        true
    }
}

/// 注册表 → MCP 工具描述符列表（按模式过滤）
pub fn build_tool_descriptors(registry: &ToolRegistry, read_only: bool) -> Vec<Tool> {
    registry
        .definitions()
        .into_iter()
        .filter(|d| is_exposed(registry, &d.name, read_only))
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

// ============================================================================
// 元工具 — 工作区级项目管理（创建 / 列出 / 切换 / 查看当前）
// ============================================================================
//
// 这四个工具操作的是"当前项目绑定"本身，不绑单个项目目录，因此不进
// studio-lib 的项目工具注册表，在 MCP 层直接分发。无头场景（外部 Agent
// 自主工作）由此获得完整的选书能力：起服务不需要已有项目。

/// 元工具规格（静态描述 + 只读模式可用性）
struct MetaToolSpec {
    name: &'static str,
    description: &'static str,
    /// JSON Schema（参数声明）
    parameters: serde_json::Value,
    /// 只读模式下是否可用（不写任何文件的操作为 true）
    read_only_ok: bool,
}

fn meta_tool_specs() -> Vec<MetaToolSpec> {
    let no_params = || serde_json::json!({"type": "object", "properties": {}, "required": []});
    vec![
        MetaToolSpec {
            name: "list_projects",
            description: "列出工作区内所有小说项目（目录名、书名、章/场景/片段数、
                正文总字数），并标记当前绑定的项目。",
            parameters: no_params(),
            read_only_ok: true,
        },
        MetaToolSpec {
            name: "create_project",
            description: "在工作区内从零创建一个新小说项目（目录骨架 + project.toml +
                structure.toml），成功后自动切换为当前项目。参数 name=书名
                （目录名由拼音 slug 自动生成，冲突自动加后缀）。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "书名（中文即可）" }
                },
                "required": ["name"]
            }),
            read_only_ok: false,
        },
        MetaToolSpec {
            name: "switch_project",
            description: "切换当前操作的项目：参数 project 填工作区内的项目目录名
                （见 list_projects），或项目的绝对路径。切换后本会话的项目工具
                （get_structure / prepare_writing / create_* 等）全部指向新书，
                各项目解析缓存独立、互不污染。",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "工作区内的项目目录名，或项目绝对路径"
                    }
                },
                "required": ["project"]
            }),
            read_only_ok: true,
        },
        MetaToolSpec {
            name: "get_current_project",
            description: "查看当前绑定的项目：书名、目录、章/场景/片段数、
                正文与今日字数、目标字数。未绑定时给出创建/切换指引。",
            parameters: no_params(),
            read_only_ok: true,
        },
    ]
}

/// 元工具 → MCP 描述符（只读模式下滤掉 create_project）
fn meta_tool_descriptors(read_only: bool) -> Vec<Tool> {
    meta_tool_specs()
        .into_iter()
        .filter(|s| !read_only || s.read_only_ok)
        .map(|s| {
            let schema = match s.parameters {
                serde_json::Value::Object(map) => map,
                _ => JsonObject::new(),
            };
            Tool::new(
                Cow::Owned(s.name.to_string()),
                Cow::Owned(s.description.to_string()),
                Arc::new(schema),
            )
        })
        .collect()
}

// ============================================================================
// 服务核心 — 不变配置 + 可变的项目绑定
// ============================================================================

/// 服务核心。HTTP 传输每个会话独立构建一个 StoryMcpServer（各自解析
/// 缓存），核心随之按会话隔离——A 会话 switch_project 不影响 B 会话；
/// stdio 单实例单核心。
pub(crate) struct ServerCore {
    /// 工作区目录（绝对路径；list_projects / create_project 的范围）
    workspace: PathBuf,
    read_only: bool,
    /// 动态档案 provider（调用时解析，跨项目共享——AI 档案是全局配置）
    provider: Arc<dyn AiProvider>,
    /// 启动时绑定的项目（HTTP 句柄 / 桌面端幂等检查用）
    initial_project: Option<PathBuf>,
    /// 当前项目绑定；None = 未绑定（工作区模式起步 / 项目被切换走前不存在）
    binding: std::sync::RwLock<Option<Arc<ProjectBinding>>>,
    /// 元工具变更串行化：create/switch 的"扫目录→建→换绑"不在并发下交错
    meta_gate: std::sync::Mutex<()>,
}

/// 一次项目绑定：目录 + 绑定在其上的工具注册表。
/// 切换 = 原子换 Arc：在途调用持旧 Arc 跑完旧项目，无撕裂；旧注册表的
/// 解析缓存随最后一个引用释放。
struct ProjectBinding {
    /// 规范化项目根
    dir: PathBuf,
    registry: ToolRegistry,
}

/// 为目录构建项目绑定（校验 project.toml + 构建工具注册表）
fn build_binding(core: &ServerCore, dir: &Path) -> Result<Arc<ProjectBinding>, String> {
    if !dir.join("project.toml").is_file() {
        return Err(format!(
            "目录不是有效的 Story Studio 项目（缺少 project.toml）：{}",
            dir.display()
        ));
    }
    let registry = create_tool_registry(dir, core.provider.clone());
    Ok(Arc::new(ProjectBinding {
        dir: dir.to_path_buf(),
        registry,
    }))
}

/// 当前绑定快照（读锁内克隆 Arc，立即放锁）
fn current_binding(core: &ServerCore) -> Option<Arc<ProjectBinding>> {
    core.binding
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 换绑并返回旧绑定
fn set_binding(core: &ServerCore, next: Arc<ProjectBinding>) -> Option<Arc<ProjectBinding>> {
    core.binding
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(next)
}

fn execute_meta(core: &ServerCore, name: &str, args: &serde_json::Value) -> Result<String, String> {
    match name {
        "list_projects" => meta_list_projects(core),
        "create_project" => meta_create_project(core, args),
        "switch_project" => meta_switch_project(core, args),
        "get_current_project" => meta_get_current(core),
        _ => unreachable!("dispatch 已按 META 名称过滤：{name}"),
    }
}

/// 读取必填字符串参数（元工具参数口径与项目工具一致：非空）
fn meta_str_arg(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少参数: {key}"))
}

fn meta_list_projects(core: &ServerCore) -> Result<String, String> {
    let current = current_binding(core).map(|b| b.dir.clone());
    let mut roots: Vec<PathBuf> = match std::fs::read_dir(&core.workspace) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("project.toml").is_file())
            .collect(),
        Err(_) => Vec::new(), // 工作区目录不存在 = 空工作区（从零场景）
    };
    if roots.is_empty() {
        return Ok(format!(
            "工作区 {} 内暂无项目。用 create_project 从零创建一本新书。",
            core.workspace.display()
        ));
    }
    roots.sort();
    // 当前绑定目录总是规范化的；read_dir 出来的根可能带符号链接
    // （如 macOS 的 /var → /private/var），对比时规范化再比
    let is_current = |root: &Path| -> bool {
        current
            .as_deref()
            .is_some_and(|cur| cur == root || std::fs::canonicalize(root).is_ok_and(|c| c == cur))
    };
    let mut out = format!(
        "工作区 {} 共 {} 个项目：\n",
        core.workspace.display(),
        roots.len()
    );
    for (i, root) in roots.iter().enumerate() {
        let dir_name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mark = if is_current(root) { "  ← 当前" } else { "" };
        match story_studio_lib::peek(root) {
            Ok(p) => out.push_str(&format!(
                "  {}. {}/ 《{}》 {} 章 · {} 场景 · {} 片段 · 正文 {} 字{mark}\n",
                i + 1,
                dir_name,
                p.name,
                p.chapter_count,
                p.scene_count,
                p.fragment_count,
                p.word_count
            )),
            Err(_) => out.push_str(&format!(
                "  {}. {}/（project.toml 损坏，无法读取）{mark}\n",
                i + 1,
                dir_name
            )),
        }
    }
    out.push_str("切换：switch_project（参数用上面的目录名，或项目绝对路径）。");
    Ok(out)
}

fn meta_create_project(core: &ServerCore, args: &serde_json::Value) -> Result<String, String> {
    let name = meta_str_arg(args, "name")?;
    let _gate = core
        .meta_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 从零场景：工作区目录可以尚不存在，按需创建
    std::fs::create_dir_all(&core.workspace)
        .map_err(|e| format!("创建工作区目录失败（{}）：{e}", core.workspace.display()))?;
    let root =
        story_studio_lib::create_project(&core.workspace, &name).map_err(|e| format!("{e}"))?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let binding = build_binding(core, &root)?;
    let previous = set_binding(core, binding);
    let mut out = format!(
        "✓ 已创建并切换到新项目：{}\n  书名《{name}》（目录名由拼音 slug 生成）",
        root.display()
    );
    match previous {
        Some(old) => out.push_str(&format!("\n  原项目：{}", old.dir.display())),
        None => out.push_str("\n  （此前未绑定项目）"),
    }
    out.push_str(
        "\n  下一步：create_entity 建角色，create_chapter / create_scene 搭结构，\
         或 design_chapters 让 AI 设计章节脉络。",
    );
    Ok(out)
}

fn meta_switch_project(core: &ServerCore, args: &serde_json::Value) -> Result<String, String> {
    let target = meta_str_arg(args, "project")?;
    let raw = PathBuf::from(&target);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        core.workspace.join(raw)
    };
    let dir = std::fs::canonicalize(&candidate).map_err(|_| {
        format!(
            "项目目录不存在：{target}（既不是绝对路径，也不在工作区 {} 内）",
            core.workspace.display()
        )
    })?;
    let _gate = core
        .meta_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 幂等：已绑定同一项目时直接回报，不重建注册表（保住解析缓存）
    if current_binding(core).is_some_and(|b| b.dir == dir) {
        return Ok(format!(
            "当前已绑定该项目：{}\n（未发生变化）",
            dir.display()
        ));
    }
    let binding = build_binding(core, &dir)?;
    let previous = set_binding(core, binding);
    let mut out = match story_studio_lib::peek(&dir) {
        Ok(p) => format!(
            "✓ 已切换到《{}》：{}\n  {} 章 · {} 场景 · {} 片段 · 正文 {} 字（今日 {} 字）",
            p.name,
            dir.display(),
            p.chapter_count,
            p.scene_count,
            p.fragment_count,
            p.word_count,
            p.today.total()
        ),
        Err(_) => format!("✓ 已切换到项目：{}", dir.display()),
    };
    if let Some(old) = previous {
        out.push_str(&format!("\n  原项目：{}", old.dir.display()));
    }
    out.push_str("\n  后续项目工具（get_structure / prepare_writing 等）都已指向新书。");
    Ok(out)
}

fn meta_get_current(core: &ServerCore) -> Result<String, String> {
    let Some(dir) = current_binding(core).map(|b| b.dir.clone()) else {
        return Ok(format!(
            "当前未绑定项目。list_projects 查看工作区（{}）内的项目，\
             create_project 新建，或 switch_project 切换。",
            core.workspace.display()
        ));
    };
    match story_studio_lib::peek(&dir) {
        Ok(p) => Ok(format!(
            "当前项目：《{}》\n  目录：{}\n  {} 章 · {} 场景 · {} 片段 · 正文 {} 字（今日 {} 字）\n  目标字数：{}",
            p.name,
            dir.display(),
            p.chapter_count,
            p.scene_count,
            p.fragment_count,
            p.word_count,
            p.today.total(),
            p.target_words
                .map(|w| w.to_string())
                .unwrap_or_else(|| "未设置".into())
        )),
        Err(e) => Ok(format!(
            "当前项目目录：{}\n  读取概览失败：{e}",
            dir.display()
        )),
    }
}

/// MCP 工具调用的失败分类
#[derive(Debug)]
enum CallFailure {
    /// 工具执行失败 — 以 isError 结果返回，让调用方模型看到并自行纠偏
    ToolError(String),
    /// 工具不可调用（未知 / 隐藏 / 只读模式下被拒）— 以 JSON-RPC -32601
    /// 返回：语义是"该方法在此服务上不可用"，而非参数无效（-32602）
    MethodNotFound(String),
}

/// 工具调用闸门：读共享、写排他。
///
/// rmcp 对每个请求并发派发，工具若并行执行会发生两类数据损坏：
/// - 同连接流水线调用的乱序（改名跑在建档之前）；
/// - ID 分配竞态：并发的 create_* 扫号互不可见，多个调用分到同一 ID、
///   写同一文件互相覆盖，先写的数据静默丢失。
///
/// 闸门本体在 studio-lib 的 ProjectLock（进程内 RwLock + 跨进程文件锁
/// `.novelstudio/tool-gate.lock`），由 create_tool_registry 给每个工具
/// 包装——本服务与桌面端、外部 stdio CLI 三方共闸。这里只保留 MCP 的
/// 暴露策略（隐藏工具 / 只读模式复核）。
fn execute_gated(
    registry: &ToolRegistry,
    read_only: bool,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<String, CallFailure> {
    if HIDDEN_TOOLS.contains(&name) {
        return Err(CallFailure::MethodNotFound(format!(
            "工具 {name} 在 MCP 服务中不可用（依赖桌面端交互）"
        )));
    }
    // 只读模式在调用层复核：即使客户端绕过 tools/list 直接调用
    if read_only && !registry.is_read_only(name) {
        return Err(CallFailure::MethodNotFound(format!(
            "服务器以 --read-only 模式启动，{name} 会修改项目，不可用"
        )));
    }
    match registry.execute(name, arguments) {
        Some(Ok(text)) => Ok(clamp_tool_result(&text)),
        Some(Err(e)) => Err(CallFailure::ToolError(e)),
        None => Err(CallFailure::MethodNotFound(format!("未知工具：{name}"))),
    }
}

pub(crate) struct StoryMcpServer {
    core: Arc<ServerCore>,
    tools: Vec<Tool>,
    instructions: String,
}

/// 相对路径 → 绝对路径（不要求目标存在：从零场景允许工作区先有路径后有目录）
fn absolutize(path: PathBuf) -> Result<PathBuf, StudioError> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir()
        .map_err(|e| StudioError::Config(format!("获取当前目录失败：{e}")))?;
    Ok(cwd.join(path))
}

impl StoryMcpServer {
    pub(crate) fn new(options: McpServerOptions) -> Result<Self, StudioError> {
        let read_only = options.read_only;
        // 初始项目：显式指定时必须已是有效项目（快速失败，兼容旧 -d 用法）
        let initial_dir = match options.project_dir {
            Some(dir) => {
                if !dir.join("project.toml").is_file() {
                    return Err(StudioError::Config(format!(
                        "目录不是有效的 Story Studio 项目（缺少 project.toml）：{}",
                        dir.display()
                    )));
                }
                Some(std::fs::canonicalize(&dir).unwrap_or(dir))
            }
            None => None,
        };
        // 工作区：显式指定 > 初始项目父级 > 配置错误
        let workspace = match options.workspace {
            Some(w) => absolutize(w)?,
            None => initial_dir
                .as_ref()
                .and_then(|d| d.parent())
                .map(|p| p.to_path_buf())
                .ok_or_else(|| {
                    StudioError::Config(
                        "未指定工作区：需要 -w <工作区目录>，或 -d <项目目录>（工作区取其父级）"
                            .into(),
                    )
                })?,
        };
        let workspace = if workspace.exists() {
            std::fs::canonicalize(&workspace).unwrap_or(workspace)
        } else {
            workspace
        };

        // 与桌面端妙笔一致的 provider 解析：agent 用途档案 → 激活档案 →
        // 旧字段 / 环境变量。LLM 工具（ai_check_* 等）实际调用发生在
        // 工具执行时，启动阶段无需 API Key。用动态包装器承载：外部
        // Agent 会话生命周期远长于一次档案切换，静态 provider 会把旧
        // 档案冻结在会话里，切档后仍报旧档案的错。provider 跨项目共享
        // （档案是全局配置，换绑不必重建）。
        let provider: Arc<dyn AiProvider> =
            Arc::from(story_studio_lib::create_dynamic_provider_for_purpose(
                purpose::AGENT,
                "anthropic",
                None,
            )?);
        let core = Arc::new(ServerCore {
            workspace,
            read_only,
            provider,
            initial_project: initial_dir.clone(),
            binding: std::sync::RwLock::new(None),
            meta_gate: std::sync::Mutex::new(()),
        });
        if let Some(dir) = &initial_dir {
            let binding = build_binding(&core, dir).map_err(StudioError::Config)?;
            *core
                .binding
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(binding);
        }
        // 工具描述符 = 元工具 + 项目工具。项目工具的签名与读写分类
        // 不随项目变化：未绑定时取无绑定注册表（与真实注册点同源），
        // 保证 tools/list 在两种模式下形状一致
        let mut tools = meta_tool_descriptors(read_only);
        match current_binding(&core) {
            Some(b) => tools.extend(build_tool_descriptors(&b.registry, read_only)),
            None => {
                let scratch = story_studio_lib::unbound_tool_registry()?;
                tools.extend(build_tool_descriptors(&scratch, read_only));
            }
        }
        let instructions = build_instructions(&core.workspace, initial_dir.as_deref(), read_only);
        Ok(Self {
            core,
            tools,
            instructions,
        })
    }

    /// 暴露的工具数（HTTP 句柄 / Tauri 状态用）
    pub(crate) fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// 工作区目录（HTTP 句柄用）
    pub(crate) fn workspace(&self) -> &Path {
        &self.core.workspace
    }

    /// 启动时绑定的项目（HTTP 句柄 / 桌面端幂等检查用）
    pub(crate) fn initial_project(&self) -> Option<&Path> {
        self.core.initial_project.as_deref()
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
        let core = Arc::clone(&self.core);

        // 工具全部是同步阻塞实现（LLM 工具含网络调用），放到阻塞线程池，
        // 不占用 tokio worker。并发闸门（读共享 / 写排他）在工具内部
        // （ProjectLock），与桌面端、外部 CLI 三方共闸
        let outcome = tokio::task::spawn_blocking(move || dispatch(&core, &name, &arguments))
            .await
            .map_err(|e| McpError::internal_error(format!("工具执行线程异常：{e}"), None))?;

        match outcome {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into()),
            Err(CallFailure::ToolError(e)) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(e)]).into())
            }
            Err(CallFailure::MethodNotFound(msg)) => {
                Err(McpError::new(ErrorCode::METHOD_NOT_FOUND, msg, None))
            }
        }
    }
}

/// 工具调用总分发：元工具（工作区级）→ 项目工具（当前绑定）。
fn dispatch(
    core: &ServerCore,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<String, CallFailure> {
    if let Some(spec) = meta_tool_specs().into_iter().find(|s| s.name == name) {
        // 只读模式在调用层复核（与项目工具同策略：绕过 tools/list 直调也拦住）
        if core.read_only && !spec.read_only_ok {
            return Err(CallFailure::MethodNotFound(format!(
                "服务器以 --read-only 模式启动，{name} 会创建项目文件，不可用"
            )));
        }
        return execute_meta(core, name, arguments)
            .map(|text| clamp_tool_result(&text))
            .map_err(CallFailure::ToolError);
    }
    let Some(binding) = current_binding(core) else {
        return Err(CallFailure::MethodNotFound(format!(
            "当前未绑定项目，{name} 不可用；先 create_project 新建或 \
             switch_project 切换（list_projects 可列工作区项目）"
        )));
    };
    execute_gated(&binding.registry, core.read_only, name, arguments)
}

/// initialize 响应里的 instructions — 给外部 Agent 的使用引导
///
/// 在 initialize 时发送一次；switch_project 后正文中会明确指出新书
/// （模型以工具回执为准，无需求动态刷新本段）。
fn build_instructions(workspace: &Path, current: Option<&Path>, read_only: bool) -> String {
    let current_line = match current {
        Some(dir) => format!("当前项目：{}\n", dir.display()),
        None => "当前项目：未绑定（先 create_project 新建，或 list_projects + switch_project \
            选择工作区内的书）\n"
            .to_string(),
    };
    let mut s = format!(
        "Story Studio 长篇小说工作区工具集。\n\
         工作区：{}\n{current_line}\
         项目管理：list_projects 列书，create_project 从零建书（建完自动切换），\
         switch_project 切换，get_current_project 查看当前。\n\
         绑定项目后：先用 get_structure / get_project_stats 了解项目结构，\
         用 search_text / search_entities 检索内容，\
         读正文用 read_fragment_text / read_scene_full_text，\
         动笔前用 prepare_writing 聚合上下文。\n\
         项目对象（卷 / 章 / 场景 / 片段 / 实体 / 线索等）一律用稳定 ID 引用，先查后用，\
         不要凭空猜测 ID。\n\
         修改类工具（create_* / update_* / write_fragment_text 等）会直接改写项目文件；\
         compile_check 可随时校验改动的一致性。",
        workspace.display()
    );
    if read_only {
        s.push_str(
            "\n本服务以只读模式启动：仅暴露查询类工具（list/switch/get_current 可用，\
             create_project 与修改类工具不可用）。",
        );
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
            core: test_core(Path::new("/nonexistent"), false),
            tools: Vec::new(),
            instructions: String::new(),
        });
        assert!(!versions.contains(&ProtocolVersion::V_2026_07_28));
        assert!(versions.contains(&ProtocolVersion::V_2025_11_25));
    }

    /// 测试用服务核心（StubProvider，不触网）
    fn test_core(workspace: &Path, read_only: bool) -> Arc<ServerCore> {
        Arc::new(ServerCore {
            workspace: workspace.to_path_buf(),
            read_only,
            provider: Arc::new(StubProvider),
            initial_project: None,
            binding: std::sync::RwLock::new(None),
            meta_gate: std::sync::Mutex::new(()),
        })
    }

    /// 扫工作区内含 project.toml 的目录（测试辅助，不依赖 slug 拼写）
    fn scan_projects(ws: &Path) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = std::fs::read_dir(ws)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("project.toml").is_file())
            .collect();
        roots.sort();
        roots
    }

    /// 工作区模式全链路：未绑定 → create_project（自动切换）→ 项目工具
    /// 落到新书 → 再建一本切走 → 切回，两本书互不串目录。
    /// 工作区目录初始不存在：从零场景一条链走通。
    #[test]
    fn workspace_meta_tools_create_and_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        let core = test_core(&ws, false);

        // 未绑定：list / get_current 给指引；项目工具被拒并指路
        assert!(dispatch(&core, "list_projects", &serde_json::json!({}))
            .unwrap()
            .contains("暂无项目"));
        assert!(
            dispatch(&core, "get_current_project", &serde_json::json!({}))
                .unwrap()
                .contains("未绑定")
        );
        let err = dispatch(&core, "get_structure", &serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, CallFailure::MethodNotFound(ref m) if m.contains("未绑定项目")),
            "未绑定时项目工具应拒并指路：{err:?}"
        );

        // 从零创建第一本：工作区目录随之建立
        let out = dispatch(
            &core,
            "create_project",
            &serde_json::json!({ "name": "甲书" }),
        )
        .unwrap();
        assert!(out.contains("已创建并切换"));
        let first = scan_projects(&ws).pop().expect("新书应落在工作区内");
        assert!(first.join("project.toml").is_file());

        // 项目工具落在第一本
        dispatch(
            &core,
            "create_entity",
            &serde_json::json!({ "kind": "character", "name": "甲主角", "role": "lead" }),
        )
        .unwrap();
        let chars = scan_projects(&ws) // 不动 binding，只看盘上事实
            .into_iter()
            .map(|p| count_toml(&p.join("entities/characters")))
            .max()
            .unwrap();
        assert_eq!(chars, 1, "新建实体应落在当前绑定项目里");

        // 第二本：自动切换，新书里 ID 序号重新从 1 开始
        dispatch(
            &core,
            "create_project",
            &serde_json::json!({ "name": "乙书" }),
        )
        .unwrap();
        assert_eq!(scan_projects(&ws).len(), 2);
        dispatch(
            &core,
            "create_entity",
            &serde_json::json!({ "kind": "character", "name": "乙主角", "role": "lead" }),
        )
        .unwrap();
        for project in scan_projects(&ws) {
            // 每本书恰好一个角色：各自 c_0001，互不干扰
            assert_eq!(
                count_toml(&project.join("entities/characters")),
                1,
                "{}",
                project.display()
            );
        }

        // 列表标记当前；按目录名切回第一本，再按绝对路径切第二本
        let list = dispatch(&core, "list_projects", &serde_json::json!({})).unwrap();
        assert!(list.contains("← 当前"));
        let first_name = first.file_name().unwrap().to_string_lossy().into_owned();
        let out = dispatch(
            &core,
            "switch_project",
            &serde_json::json!({ "project": first_name }),
        )
        .unwrap();
        assert!(out.contains("甲书"));
        let second = scan_projects(&ws)
            .into_iter()
            .find(|p| p != &first)
            .unwrap();
        let out = dispatch(
            &core,
            "switch_project",
            &serde_json::json!({ "project": second.to_string_lossy() }),
        )
        .unwrap();
        assert!(out.contains("乙书"));

        // 不存在的项目 → 工具级错误（模型可见、可自行纠偏）
        let err = dispatch(
            &core,
            "switch_project",
            &serde_json::json!({ "project": "bu-cun-zai" }),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::ToolError(ref m) if m.contains("不存在")));
    }

    fn count_toml(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".toml"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// 只读模式：create_project 被拒（列表与直调双拦），
    /// list / switch 可用，项目写工具仍被拒
    #[test]
    fn read_only_blocks_create_but_allows_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
        story_studio_lib::create_project(&ws, "既有书").unwrap();
        let core = test_core(&ws, true);

        // 描述符层：create_project 不出现在只读 tools/list
        let names: Vec<String> = meta_tool_descriptors(true)
            .iter()
            .map(|t| t.name.as_ref().to_string())
            .collect();
        assert!(!names.contains(&"create_project".to_string()));
        assert!(names.contains(&"list_projects".to_string()));
        assert!(meta_tool_descriptors(false)
            .iter()
            .any(|t| t.name.as_ref() == "create_project"));

        // 调用层：绕过 tools/list 直调也拦住
        let err = dispatch(
            &core,
            "create_project",
            &serde_json::json!({ "name": "新书" }),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::MethodNotFound(_)));

        // list / switch 可用
        assert!(dispatch(&core, "list_projects", &serde_json::json!({})).is_ok());
        let book = scan_projects(&ws).pop().unwrap();
        let dir_name = book.file_name().unwrap().to_string_lossy().into_owned();
        dispatch(
            &core,
            "switch_project",
            &serde_json::json!({ "project": dir_name }),
        )
        .unwrap();

        // 切换后项目写工具仍被拒（只读针对的是服务模式，不随换书放宽）
        let err = dispatch(
            &core,
            "create_entity",
            &serde_json::json!({ "kind": "character", "name": "只读探针" }),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::MethodNotFound(_)));
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
    /// 写同一文件互相覆盖（多份"成功"回执只剩一份数据）。工具内部的项目
    /// 闸门（ProjectLock，读共享 / 写排他）串行后，N 路并发创建应得到 N
    /// 个互不相同的 ID。
    #[test]
    fn execute_gated_serializes_concurrent_creates() {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "gate-test").unwrap();
        let registry = Arc::new(story_studio_lib::create_tool_registry(
            tmp.path(),
            Arc::new(StubProvider),
        ));

        const N: usize = 6;
        let mut handles = Vec::new();
        for i in 0..N {
            let reg = registry.clone();
            handles.push(std::thread::spawn(move || {
                execute_gated(
                    &reg,
                    false,
                    "create_entity",
                    &serde_json::json!({ "kind": "character", "name": format!("并发{i}号"), "role": "probe" }),
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
            .filter(|e| e.file_name().to_string_lossy().starts_with("c_"))
            .collect();
        assert_eq!(files.len(), N, "每个实体应有自己的文件: {files:?}");

        // 只读模式下变更工具仍被拒（execute_gated 同时承担该复核）
        let err = execute_gated(
            &registry,
            true,
            "create_entity",
            &serde_json::json!({ "kind": "character", "name": "只读探针" }),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::MethodNotFound(_)));
    }
}
