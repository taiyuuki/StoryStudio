//! ServerHandler 实现 — ToolRegistry 到 MCP 的适配层

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
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
enum CallFailure {
    /// 工具执行失败 — 以 isError 结果返回，让调用方模型看到并自行纠偏
    ToolError(String),
    /// 协议层参数错误 — 以 JSON-RPC error 返回
    InvalidParams(String),
}

pub(crate) struct StoryMcpServer {
    registry: Arc<ToolRegistry>,
    tools: Vec<Tool>,
    instructions: String,
    read_only: bool,
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
        Ok(Self {
            registry,
            tools,
            instructions,
            read_only: options.read_only,
        })
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

        // 工具全部是同步阻塞实现（LLM 工具含网络调用），放到阻塞线程池，
        // 不占用 tokio worker
        let outcome = tokio::task::spawn_blocking(move || -> Result<String, CallFailure> {
            if HIDDEN_TOOLS.contains(&name.as_str()) {
                return Err(CallFailure::InvalidParams(format!(
                    "工具 {name} 在 MCP 服务中不可用（依赖桌面端交互）"
                )));
            }
            // 只读模式在调用层复核：即使客户端绕过 tools/list 直接调用
            if read_only && !READ_ONLY_TOOLS.contains(&name.as_str()) {
                return Err(CallFailure::InvalidParams(format!(
                    "服务器以 --read-only 模式启动，{name} 会修改项目，不可用"
                )));
            }
            match registry.execute(&name, &arguments) {
                Some(Ok(text)) => Ok(clamp_tool_result(&text)),
                Some(Err(e)) => Err(CallFailure::ToolError(e)),
                None => Err(CallFailure::InvalidParams(format!("未知工具：{name}"))),
            }
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
