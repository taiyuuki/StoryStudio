//! Streamable HTTP 传输 — 供桌面应用内嵌与 CLI 无头使用。
//!
//! stdio 服务由客户端拉起进程；HTTP 服务则由本方常驻监听，
//! 外部 Agent（ZCode / Claude 等）通过 URL + Bearer Token 接入。
//! 监听范围刻意锁定回环地址（rmcp 默认的 allowed_hosts 也是 loopback-only）。
//!
//! Token 持久化在 `~/.story-studio/mcp-token`：服务随项目切换重启、
//! 应用重启都不轮换，外部客户端配置一次长期有效；
//! 删除该文件或 CLI `--new-token` 可重置。

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::RwLock;

use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams, ProtocolVersion};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
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
    /// 会话持久化文件路径；None 时读取默认的 ~/.story-studio/mcp-sessions.json
    pub session_path: Option<PathBuf>,
}

impl Default for HttpServerOptions {
    fn default() -> Self {
        Self {
            bind: IpAddr::from([127, 0, 0, 1]),
            port: crate::DEFAULT_HTTP_PORT,
            token: None,
            session_path: None,
        }
    }
}

/// 会话缓存条目：状态 + 最近使用序号（LRU 逐出依据；用单调递增序号
/// 而非时间戳，保证并发下逐出结果确定）
#[derive(Debug, Clone)]
struct CachedSession {
    state: SessionState,
    last_used: u64,
}

/// 会话缓存上限。外部 Agent 客户端可能不断产生新的 session-id（重连、
/// 重启遗留、多客户端轮询），不设上限的话内存与持久化 JSON 双双无界
/// 增长；被逐出的 session 再次出现时走自动恢复兜底，与重启遗留的
/// 行为完全一致，逐出无感知。
const MAX_CACHED_SESSIONS: usize = 64;

/// 具备自动恢复与持久化能力的 SessionStore。
///
/// 外部 Agent 客户端（Antigravity、Claude Desktop、Cursor 等）在会话中
/// 可能经历服务重启、热重载或切书重启。标准 LocalSessionManager 为纯内存存储，
/// 服务重启后旧 session-id 丢失，客户端连独立 SSE 流会报 404 "Session not found" 并失效。
///
/// 本实现提供：
/// 1. 活跃 Session 持久化到 `~/.story-studio/mcp-sessions.json`，重启自动加载
///    （上限 [`MAX_CACHED_SESSIONS`]，LRU 逐出）；
/// 2. 面对未知历史 Session-Id，自动生成兼容的 SessionState（默认 2025-11-25 协议版本）
///    透明恢复会话，彻底避免 404 导致外部客户端崩溃。兜底状态**即用即弃**，
///    不写入缓存——旧实现每见一个未知 id 就永久插入一条，是无界增长的主因。
#[derive(Debug, Clone)]
pub struct AutoRecoveringSessionStore {
    cache: Arc<RwLock<HashMap<String, CachedSession>>>,
    persist_path: Option<PathBuf>,
    /// LRU 序号发号器
    seq: Arc<std::sync::atomic::AtomicU64>,
}

impl AutoRecoveringSessionStore {
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        let seq = std::sync::atomic::AtomicU64::new(0);
        let mut initial = HashMap::new();
        if let Some(path) = &persist_path {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(map) = serde_json::from_str::<HashMap<String, SessionState>>(&content) {
                    // 旧版无上限时可能攒下超量会话：启动即截断
                    for (id, state) in map {
                        if initial.len() >= MAX_CACHED_SESSIONS {
                            break;
                        }
                        let last_used = seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        initial.insert(id, CachedSession { state, last_used });
                    }
                }
            }
        }
        Self {
            cache: Arc::new(RwLock::new(initial)),
            persist_path,
            seq: Arc::new(std::sync::atomic::AtomicU64::new(
                seq.load(std::sync::atomic::Ordering::Relaxed),
            )),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn persist_sync(&self, map: &HashMap<String, CachedSession>) {
        let Some(path) = &self.persist_path else {
            return;
        };
        // 磁盘格式保持 {session_id: SessionState}（与历史文件互通），LRU
        // 序号只在内存里
        let plain: HashMap<&String, &SessionState> =
            map.iter().map(|(k, v)| (k, &v.state)).collect();
        if let Ok(json) = serde_json::to_string(&plain) {
            let _ = std::fs::write(path, json);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

#[async_trait::async_trait]
impl SessionStore for AutoRecoveringSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        {
            let mut writer = self.cache.write().await;
            if let Some(entry) = writer.get_mut(session_id) {
                entry.last_used = self.next_seq();
                return Ok(Some(entry.state.clone()));
            }
        }

        // 未知或重启前遗留的 session_id：返回即用即弃的兼容握手状态，
        // 不写入缓存。真实状态随后由 store() 落位；纯探活 / 一次性 id
        // 不再累积成缓存条目。
        let mut init_params = InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("recovered-client", "1.0"),
        );
        init_params.protocol_version = ProtocolVersion::V_2025_11_25;
        Ok(Some(SessionState::new(init_params)))
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        let mut writer = self.cache.write().await;
        writer.insert(
            session_id.to_string(),
            CachedSession {
                state: state.clone(),
                last_used: self.next_seq(),
            },
        );
        // 超上限逐出最久未用（序号发号器保证无并列，逐出确定）
        while writer.len() > MAX_CACHED_SESSIONS {
            let Some(oldest) = writer
                .iter()
                .min_by_key(|(_, v)| v.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            writer.remove(&oldest);
        }
        self.persist_sync(&writer);
        Ok(())
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let mut writer = self.cache.write().await;
        writer.remove(session_id);
        self.persist_sync(&writer);
        Ok(())
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
    let session_path = match http.session_path.clone() {
        Some(p) => Some(p),
        None => persisted_sessions_path().ok(),
    };
    let session_store = std::sync::Arc::new(AutoRecoveringSessionStore::new(session_path));
    let session_store_for_thread = session_store.clone();

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
                let mut config = StreamableHttpServerConfig::default()
                    .with_cancellation_token(shutdown_for_config);
                config.session_store = Some(session_store_for_thread);
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

/// 持久化 Session 文件路径（~/.story-studio/mcp-sessions.json）
fn persisted_sessions_path() -> Result<PathBuf, StudioError> {
    let config = story_studio_lib::ai::global_config_path()?;
    Ok(config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mcp-sessions.json"))
}

#[cfg(test)]
mod session_store_tests {
    use super::*;

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn sample_state(client: &str) -> SessionState {
        let mut params = InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new(client, "1.0"),
        );
        params.protocol_version = ProtocolVersion::V_2025_11_25;
        SessionState::new(params)
    }

    /// 未知 session-id 只拿即用即弃的兜底状态，不写缓存——无界增长的回归
    #[test]
    fn unknown_session_ids_do_not_accumulate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sessions.json");
        let store = AutoRecoveringSessionStore::new(Some(path.clone()));

        for i in 0..50 {
            let sid = format!("probe-{i}");
            let state = block_on(store.load(&sid)).unwrap();
            assert!(state.is_some(), "未知 id 必须自动恢复（防 404）");
        }

        let cache = store.cache.try_read().unwrap();
        assert!(
            cache.is_empty(),
            "未知 id 不应写入缓存，实际 {} 条",
            cache.len()
        );
        drop(cache);
        assert!(!path.exists(), "纯探活不应产生持久化文件");
    }

    /// store 超上限按 LRU 逐出：最久未用的先走，且持久化文件同步收敛
    #[test]
    fn store_evicts_least_recently_used_beyond_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sessions.json");
        let store = AutoRecoveringSessionStore::new(Some(path.clone()));

        // 填满后再写 10 条，前 10 条应被逐出
        for i in 0..(MAX_CACHED_SESSIONS + 10) {
            let sid = format!("s-{i:03}");
            block_on(store.store(&sid, &sample_state(&sid))).unwrap();
            // 中途触碰 s-000，让它免于被逐出
            if i == MAX_CACHED_SESSIONS / 2 {
                block_on(store.load("s-000")).unwrap();
            }
        }

        let cache = store.cache.try_read().unwrap();
        assert_eq!(cache.len(), MAX_CACHED_SESSIONS, "缓存应收敛到上限");
        assert!(cache.contains_key("s-000"), "被触碰过的最老会话应存活");
        assert!(!cache.contains_key("s-001"), "未触碰的更老会话应被逐出");
        assert!(cache.contains_key(&format!("s-{:03}", MAX_CACHED_SESSIONS + 9)));
        drop(cache);

        let persisted: HashMap<String, SessionState> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted.len(), MAX_CACHED_SESSIONS);
        assert!(!persisted.contains_key("s-001"));
    }

    /// 旧版本攒下的超量持久化文件：重启加载时截断到上限
    #[test]
    fn oversized_persisted_file_truncates_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sessions.json");
        let mut legacy = HashMap::new();
        for i in 0..(MAX_CACHED_SESSIONS * 3) {
            let sid = format!("legacy-{i}");
            legacy.insert(sid.clone(), sample_state(&sid));
        }
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();

        let store = AutoRecoveringSessionStore::new(Some(path));
        assert_eq!(
            store.cache.try_read().unwrap().len(),
            MAX_CACHED_SESSIONS,
            "历史超量文件应截断到上限"
        );
    }
}
