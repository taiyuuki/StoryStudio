//! Story Studio MCP Server — 把妙笔 Agent 的项目工具集暴露为 MCP 服务
//!
//! 外部 Agent 客户端（Claude Desktop / ZCode / Cursor 等）通过 MCP
//! 直接调用 studio-lib 的 80+ 个项目操作工具，用客户端自己的模型操作
//! 小说项目。工具实现、增量解析缓存、AI 档案配置全部与桌面端共用
//! 同一条代码路径。
//!
//! 传输：stdio（stdout 是协议通道，所有日志必须走 stderr）
//! 或 Streamable HTTP（桌面应用内嵌 / 无头常驻，URL + Bearer Token 接入）。
//! 入口：CLI 子命令 `story-studio mcp -d <项目目录> [--read-only]
//! [--transport stdio|http] [--port N]`。

mod server;

pub mod http;

use std::path::PathBuf;

use story_studio_lib::StudioError;

pub use http::{
    load_or_create_token, load_or_create_token_at, regenerate_token, start_server_http,
    HttpServerOptions, McpHttpHandle,
};
pub use server::{build_tool_descriptors, is_exposed, HIDDEN_TOOLS};

/// HTTP 传输的默认端口
pub const DEFAULT_HTTP_PORT: u16 = 27520;

/// MCP 服务器启动选项
#[derive(Debug, Clone)]
pub struct McpServerOptions {
    /// Story Studio 项目根目录（须含 project.toml）
    pub project_dir: PathBuf,
    /// 只读模式：只暴露查询类工具；即便客户端绕过列表直接调用写工具，
    /// 调用层也会拒绝执行
    pub read_only: bool,
}

/// 启动 MCP 服务器（阻塞直到 stdio 关闭）
///
/// 自带 tokio 多线程运行时，CLI 进程无需预先存在异步上下文。
pub fn run_server(options: McpServerOptions) -> Result<(), StudioError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| StudioError::Config(format!("创建 tokio 运行时失败：{e}")))?;
    runtime.block_on(async {
        let service = server::StoryMcpServer::new(options)?;
        let running = rmcp::serve_server(service, rmcp::transport::stdio())
            .await
            .map_err(|e| StudioError::Config(format!("MCP 服务器启动失败：{e}")))?;
        running
            .waiting()
            .await
            .map_err(|e| StudioError::Config(format!("MCP 服务器异常退出：{e}")))?;
        Ok(())
    })
}
