//! Streamable HTTP 传输 — 供桌面应用内嵌与 CLI 无头使用。
//!
//! stdio 服务由客户端拉起进程；HTTP 服务则由本方常驻监听，
//! 外部 Agent（ZCode / Claude 等）通过 URL + Bearer Token 接入。
//! 监听范围刻意锁定回环地址（rmcp 默认的 allowed_hosts 也是 loopback-only）。
//!
//! Token 持久化在 `~/.story-studio/mcp-token`：服务随项目切换重启、
//! 应用重启都不轮换，外部客户端配置一次长期有效；
//! 删除该文件或 CLI `--new-token` 可重置。

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use story_studio_lib::StudioError;
use tokio_util::sync::CancellationToken;

use crate::server::StoryMcpServer;
use crate::McpServerOptions;

use axum::response::IntoResponse as _;

/// HTTP 监听选项
#[derive(Debug, Clone)]
pub struct HttpServerOptions {
    /// 监听地址（默认 127.0.0.1，只暴露本机回环）
    pub bind: IpAddr,
    /// 监听端口；0 表示由系统分配空闲端口
    pub port: u16,
    /// 显式指定 Token（测试 / 特殊场景）；None 时读取或生成持久化 Token
    pub token: Option<String>,
}

impl Default for HttpServerOptions {
    fn default() -> Self {
        Self {
            bind: IpAddr::from([127, 0, 0, 1]),
            port: crate::DEFAULT_HTTP_PORT,
            token: None,
        }
    }
}

/// 运行中的 HTTP MCP 服务句柄
pub struct McpHttpHandle {
    port: u16,
    token: String,
    read_only: bool,
    project_dir: PathBuf,
    tool_count: usize,
    shutdown: CancellationToken,
    join: Option<std::thread::JoinHandle<()>>,
}

impl McpHttpHandle {
    /// 实际监听端口（传入 0 时为系统分配值）
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 接入 URL
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// 本次启动随机生成的 Bearer Token
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn project_dir(&self) -> &PathBuf {
        &self.project_dir
    }

    pub fn tool_count(&self) -> usize {
        self.tool_count
    }

    /// 停止服务并等待线程退出
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    /// 阻塞直到服务线程自行退出（不主动停止；供 CLI 前台等待用）
    pub fn wait(mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    fn shutdown_now(&mut self) {
        self.shutdown.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for McpHttpHandle {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

/// 启动 HTTP MCP 服务（专用线程 + 独立 tokio 运行时，绑定成功即返回）
///
/// 启动时先完整构建一次服务（校验项目目录 / provider / 工具注册表），
/// 配置错误立刻失败，而不是等到第一个请求。
pub fn start_server_http(
    options: McpServerOptions,
    http: HttpServerOptions,
) -> Result<McpHttpHandle, StudioError> {
    // 预构建：快速失败 + 拿工具数
    let tool_count = StoryMcpServer::new(options.clone())?.tool_count();
    let token = match http.token.clone() {
        Some(t) => t,
        None => load_or_create_token()?,
    };

    let (tx, rx) = mpsc::channel::<Result<u16, String>>();
    // 同一枚取消令牌三路共享：会话终止（config）、axum 优雅停机、外部句柄
    let shutdown = CancellationToken::new();
    let shutdown_for_config = shutdown.clone();
    let shutdown_for_graceful = shutdown.clone();
    let token_for_mw = token.clone();
    let options_for_thread = options.clone();

    let join = std::thread::Builder::new()
        .name("story-mcp-http".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(format!("创建 tokio 运行时失败：{e}")));
                    return;
                }
            };
            runtime.block_on(async move {
                // 每个会话独立构建 StoryMcpServer（各自持有解析缓存）
                let options_for_factory = options_for_thread;
                let factory = move || {
                    StoryMcpServer::new(options_for_factory.clone())
                        .map_err(|e| std::io::Error::other(e.to_string()))
                };
                // 默认配置：legacy session 模式 + 仅回环 Host 校验，
                // 与本服务 ≤2025-11-25 的协议版本表一致
                let config = StreamableHttpServerConfig::default()
                    .with_cancellation_token(shutdown_for_config);
                let service = StreamableHttpService::new(
                    factory,
                    std::sync::Arc::new(LocalSessionManager::default()),
                    config,
                );
                let app = axum::Router::new()
                    .route("/mcp", axum::routing::any_service(service))
                    .layer(axum::middleware::from_fn_with_state(
                        token_for_mw,
                        auth_bearer,
                    ));
                let listener = match tokio::net::TcpListener::bind((http.bind, http.port)).await {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = tx.send(Err(format!("监听 {}:{} 失败：{e}", http.bind, http.port)));
                        return;
                    }
                };
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(http.port);
                if tx.send(Ok(port)).is_err() {
                    return; // 启动方已放弃
                }
                let server = axum::serve(listener, app)
                    .with_graceful_shutdown(async move { shutdown_for_graceful.cancelled().await });
                if let Err(e) = server.await {
                    tracing::error!("MCP HTTP 服务异常退出：{e}");
                }
            });
        })
        .map_err(|e| StudioError::Config(format!("创建服务线程失败：{e}")))?;

    match rx.recv() {
        Ok(Ok(port)) => Ok(McpHttpHandle {
            port,
            token,
            read_only: options.read_only,
            project_dir: options.project_dir,
            tool_count,
            shutdown,
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(StudioError::Config(e))
        }
        Err(_) => Err(StudioError::Config("服务线程在绑定前退出".into())),
    }
}

/// Bearer Token 校验中间件
async fn auth_bearer(
    axum::extract::State(token): axum::extract::State<String>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let expected = format!("Bearer {token}");
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case(&expected));
    if !ok {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

/// 生成 32 位十六进制随机 Token
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("系统熵源不可用");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_valid_token(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// 持久化 Token 文件路径（~/.story-studio/mcp-token）
fn persisted_token_path() -> Result<PathBuf, StudioError> {
    let config = story_studio_lib::ai::global_config_path()?;
    Ok(config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mcp-token"))
}

/// 在指定路径读取或生成持久化 Token（公开以供测试注入路径）
pub fn load_or_create_token_at(path: &Path) -> Result<String, StudioError> {
    if let Ok(tok) = std::fs::read_to_string(path) {
        let tok = tok.trim();
        if is_valid_token(tok) {
            return Ok(tok.to_string());
        }
    }
    let token = random_token();
    std::fs::write(path, &token).map_err(|e| {
        StudioError::Config(format!("写入 Token 文件失败（{}）：{e}", path.display()))
    })?;
    // 秘密文件收紧权限（unix）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// 读取或生成持久化 Token（~/.story-studio/mcp-token）
///
/// Token 跨启动稳定：服务随项目切换重启、应用重启都不轮换，
/// 外部客户端配置一次长期有效。删除该文件或 [`regenerate_token`] 重置。
pub fn load_or_create_token() -> Result<String, StudioError> {
    load_or_create_token_at(&persisted_token_path()?)
}

/// 重置持久化 Token（生成新值并落盘），下次启动生效
pub fn regenerate_token() -> Result<String, StudioError> {
    let path = persisted_token_path()?;
    let token = random_token();
    std::fs::write(&path, &token).map_err(|e| {
        StudioError::Config(format!("写入 Token 文件失败（{}）：{e}", path.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}
