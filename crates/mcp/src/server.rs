//! ServerHandler 实现 — ToolRegistry 到 MCP 的适配层

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use story_ai_runtime::{clamp_tool_result, ToolDef, ToolRegistry};
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
        // 工具执行时，启动阶段无需 API Key。用动态包装器承载：外部
        // Agent 会话生命周期远长于一次档案切换，静态 provider 会把旧
        // 档案冻结在会话里，切档后仍报旧档案的错。
        let provider = Arc::from(story_studio_lib::create_dynamic_provider_for_purpose(
            purpose::AGENT,
            "anthropic",
            None,
        )?);
        let registry = Arc::new(create_tool_registry(&dir, provider));
        let tools = build_tool_descriptors(&registry, options.read_only);
        let instructions = build_instructions(&dir, options.read_only);
        Ok(Self {
            registry,
            tools,
            instructions,
            read_only: options.read_only,
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

        // 工具全部是同步阻塞实现（LLM 工具含网络调用），放到阻塞线程池，
        // 不占用 tokio worker。并发闸门（读共享 / 写排他）在工具内部
        // （ProjectLock），与桌面端、外部 CLI 三方共闸
        let outcome = tokio::task::spawn_blocking(move || {
            execute_gated(&registry, read_only, &name, &arguments)
        })
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
        });
        assert!(!versions.contains(&ProtocolVersion::V_2026_07_28));
        assert!(versions.contains(&ProtocolVersion::V_2025_11_25));
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
                    "create_character",
                    &serde_json::json!({ "name": format!("并发{i}号"), "role": "probe" }),
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
            "create_character",
            &serde_json::json!({ "name": "只读探针" }),
        )
        .unwrap_err();
        assert!(matches!(err, CallFailure::MethodNotFound(_)));
    }
}
