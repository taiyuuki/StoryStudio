//! 文件监听（RFC-0019）——项目目录变更 → 前端实时同步的事件源
//!
//! 职责刻意收窄为「哑管道」（RFC-0019 §6.5）：递归监听项目根，防抖合并
//! 事件串（一次原子替换 = remove/create/modify 一串），过滤运行时噪音
//! （§6.2），把变更文件的相对路径经 Tauri 事件 `project-fs-changed` 推给
//! 前端。不分类、不解析语义、不读文件内容——分类与内容合并都在前端
//! （§8），数据读取走既有读命令（§9：watcher 不参与 ProjectLock）。
//!
//! 生命周期对偶 MCP 服务（commands.rs 的 mcp_slot 模式）：进程内至多
//! 一个 watcher，跟随当前打开的项目；root 变化由命令层先停旧再启新。
//!
//! 异常恢复（§6.4）：notify 底层报错或监听建立失败按 5s/15s/45s 退避
//! 重建，连续 3 次失败锁定 Stopped；每次状态迁移经 `project-watch-state`
//! 事件广播，供前端角标警示与手动重启。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer};
use serde::Serialize;
use tauri::Emitter;

/// 防抖窗口（RFC-0019 §7.2 L1：合并一次原子替换的事件串）
const DEBOUNCE: Duration = Duration::from_millis(400);
/// 底层报错后的重建退避（§6.4：最多 3 次，耗尽锁定 Stopped）
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(45),
];

/// 磁盘变更事件名（payload: [`FsChangedPayload`]）
pub const FS_CHANGED_EVENT: &str = "project-fs-changed";
/// 监听状态迁移事件名（payload: [`WatchStatusDto`]）
pub const WATCH_STATE_EVENT: &str = "project-watch-state";

// ============================================================================
// 事件出口 — 哑管道的两个 sink（Tauri 事件 / 测试桩）
// ============================================================================

/// 变更批次与状态迁移的出口。抽出 trait 是为了在单测里注入收集桩，
/// 不必构造 AppHandle。
pub(crate) trait FileEvents: Send + Sync + 'static {
    /// 防抖批次：过滤后的变更文件相对路径（已去重排序）
    fn changed(&self, files: Vec<String>);
    /// 状态迁移广播
    fn state(&self, dto: WatchStatusDto);
}

/// 生产出口：Tauri 全局事件
struct TauriEvents(tauri::AppHandle);

impl FileEvents for TauriEvents {
    fn changed(&self, files: Vec<String>) {
        let _ = self.0.emit(FS_CHANGED_EVENT, FsChangedPayload { files });
    }

    fn state(&self, dto: WatchStatusDto) {
        let _ = self.0.emit(WATCH_STATE_EVENT, dto);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FsChangedPayload {
    /// 相对项目的路径，'/' 分隔，已去重排序
    pub files: Vec<String>,
}

// ============================================================================
// 状态
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum WatchState {
    /// 正常监听中
    Running,
    /// 底层报错，按退避重建中（attempt 从 1 起；0 = 尚在首次启动）
    Retrying { attempt: u8, reason: String },
    /// 停止：用户主动停止为 None，重试耗尽 / panic 为 Some
    Stopped { reason: Option<String> },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatusDto {
    /// "running" | "retrying" | "stopped"
    pub state: String,
    pub root: String,
    pub attempt: u8,
    pub reason: Option<String>,
}

impl WatchStatusDto {
    /// 无监听实例时的空状态（watch_status 的未运行返回值）
    pub fn idle() -> Self {
        Self {
            state: "stopped".into(),
            root: String::new(),
            attempt: 0,
            reason: None,
        }
    }
}

// ============================================================================
// 监视线程
// ============================================================================

/// 监视线程的信箱：防抖事件与停止指令走同一 channel，退避等待期间也
/// 能被 Stop 即时打断
enum Msg {
    Events(DebounceEventResult),
    Stop,
}

/// 一条消息处理完后的主循环走向
enum MsgOutcome {
    /// 继续消费
    Continue,
    /// 已安排退避，需重建监听
    Rebuild,
    /// 终止监视线程
    Terminate,
}

/// 监视线程的共享上下文
struct Ctx {
    root: PathBuf,
    state: Arc<Mutex<WatchState>>,
    sink: Arc<dyn FileEvents>,
}

impl Ctx {
    fn lock(&self) -> MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 状态迁移：写共享状态并广播（§6.4）
    fn transition(&self, next: WatchState) {
        let dto = status_dto_of(&self.root, &next);
        *self.lock() = next;
        self.sink.state(dto);
    }
}

fn status_dto_of(root: &Path, state: &WatchState) -> WatchStatusDto {
    let (state_str, attempt, reason) = match state {
        WatchState::Running => ("running", 0u8, None),
        WatchState::Retrying { attempt, reason } => ("retrying", *attempt, Some(reason.clone())),
        WatchState::Stopped { reason } => ("stopped", 0, reason.clone()),
    };
    WatchStatusDto {
        state: state_str.into(),
        root: root.display().to_string(),
        attempt,
        reason,
    }
}

/// 进程内唯一监听实例的句柄；drop 不停线程，停止必须显式调 [`stop`](Self::stop)
/// （与 MCP HTTP 句柄同约定，由命令层管理生命周期）。
pub struct WatcherHandle {
    root: PathBuf,
    tx: Sender<Msg>,
    state: Arc<Mutex<WatchState>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatcherHandle {
    pub fn spawn(app: tauri::AppHandle, root: impl Into<PathBuf>) -> Self {
        Self::spawn_with_sink(root, Arc::new(TauriEvents(app)))
    }

    /// 测试出口：注入自定义 sink 而非 Tauri 事件
    pub(crate) fn spawn_with_sink(root: impl Into<PathBuf>, sink: Arc<dyn FileEvents>) -> Self {
        // 规范化根路径：事件回调里的绝对路径以其为前缀做相对化
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let (tx, rx) = mpsc::channel::<Msg>();
        let state = Arc::new(Mutex::new(WatchState::Retrying {
            attempt: 0,
            reason: "启动中".into(),
        }));
        let ctx = Ctx {
            root: root.clone(),
            state: state.clone(),
            sink,
        };
        // 防抖事件从 debouncer 内部线程转发进同一信箱，停止指令才能与
        // 事件在同一个 recv 上被消费
        let event_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("fs-watcher".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    supervise(&ctx, &rx, &event_tx);
                }));
                if let Err(panic) = result {
                    let detail = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "未知".into());
                    ctx.transition(WatchState::Stopped {
                        reason: Some(format!("监视线程 panic：{detail}")),
                    });
                }
            })
            .expect("fs-watcher 线程创建失败");
        Self {
            root,
            tx,
            state,
            thread: Some(thread),
        }
    }

    /// 监听的项目根（规范化后）
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn status(&self) -> WatchState {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn status_dto(&self) -> WatchStatusDto {
        status_dto_of(&self.root, &self.status())
    }

    /// 停止并回收监视线程（幂等）
    pub fn stop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 监视主循环：建立监听 → 消费事件；报错按退避重建，Stop 随时可打断。
/// `debouncer` 绑定后不被读用——它的存活就是监听本身，活到本轮迭代结束。
fn supervise(ctx: &Ctx, rx: &Receiver<Msg>, event_tx: &Sender<Msg>) {
    let mut failures: u8 = 0;
    'rebuild: loop {
        let _debouncer = match build_debouncer(ctx, event_tx) {
            Ok(d) => d,
            Err(reason) => {
                if !schedule_retry(ctx, rx, &mut failures, reason) {
                    return;
                }
                continue 'rebuild;
            }
        };
        ctx.transition(WatchState::Running);
        loop {
            match rx.recv() {
                Ok(msg) => match handle_msg(ctx, msg, &mut failures, rx) {
                    MsgOutcome::Continue => {}
                    MsgOutcome::Rebuild => continue 'rebuild,
                    MsgOutcome::Terminate => return,
                },
                Err(_) => {
                    // 发送端全部消亡（不应发生：句柄持有一份）——按停止处理
                    ctx.transition(WatchState::Stopped { reason: None });
                    return;
                }
            }
        }
    }
}

/// 处理一条信箱消息
fn handle_msg(ctx: &Ctx, msg: Msg, failures: &mut u8, rx: &Receiver<Msg>) -> MsgOutcome {
    match msg {
        Msg::Events(Ok(events)) => {
            let files = collect_files(&ctx.root, &events);
            if !files.is_empty() {
                ctx.sink.changed(files);
            }
            MsgOutcome::Continue
        }
        Msg::Events(Err(errors)) => {
            let reason = errors
                .first()
                .map(|e| format!("监听底层报错：{e}"))
                .unwrap_or_else(|| "监听底层报错".into());
            if schedule_retry(ctx, rx, failures, reason) {
                MsgOutcome::Rebuild
            } else {
                MsgOutcome::Terminate
            }
        }
        Msg::Stop => {
            ctx.transition(WatchState::Stopped { reason: None });
            MsgOutcome::Terminate
        }
    }
}

/// 建立 debouncer 并递归监听项目根
fn build_debouncer(
    ctx: &Ctx,
    event_tx: &Sender<Msg>,
) -> Result<Debouncer<notify::RecommendedWatcher, notify_debouncer_full::RecommendedCache>, String>
{
    let mut debouncer = new_debouncer(DEBOUNCE, None, {
        let tx = event_tx.clone();
        move |res: DebounceEventResult| {
            let _ = tx.send(Msg::Events(res));
        }
    })
    .map_err(|e| format!("监听器创建失败：{e}"))?;
    debouncer
        .watch(&ctx.root, RecursiveMode::Recursive)
        .map_err(|e| format!("监听目录失败：{e}"))?;
    Ok(debouncer)
}

/// 记录一次失败并退避等待。返回 false = 终止（用户停止或重试耗尽）；
/// 返回 true = 退避已过，外层重建。
fn schedule_retry(ctx: &Ctx, rx: &Receiver<Msg>, failures: &mut u8, reason: String) -> bool {
    *failures += 1;
    if *failures as usize > RETRY_BACKOFF.len() {
        ctx.transition(WatchState::Stopped {
            reason: Some(format!("{reason}；重试 3 次仍失败，已停止")),
        });
        return false;
    }
    ctx.transition(WatchState::Retrying {
        attempt: *failures,
        reason,
    });
    let deadline = Instant::now() + RETRY_BACKOFF[(*failures - 1) as usize];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        match rx.recv_timeout(deadline - now) {
            // 退避期间迟到的旧事件直接丢弃（重建后以盘面为准）
            Ok(Msg::Events(_)) => {}
            Ok(Msg::Stop) | Err(_) => {
                ctx.transition(WatchState::Stopped { reason: None });
                return false;
            }
        }
    }
}

// ============================================================================
// 路径处理（纯函数，单测覆盖）
// ============================================================================

/// 事件路径 → 相对项目根的 '/' 分隔路径；不在项目内则 None
fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// RFC-0019 §6.2 噪音过滤：`.novelstudio/`、`.git/` 与 `*.tmp` 暂存文件
fn is_watched_rel(rel: &str) -> bool {
    if rel.is_empty() {
        return false; // 项目根自身的事件
    }
    match rel.split('/').next() {
        Some(".novelstudio") | Some(".git") => false,
        _ => !rel.ends_with(".tmp"),
    }
}

/// 批次内路径合并：相对化 + 过滤 + 去重排序（BTreeSet）
fn collect_files(root: &Path, events: &[DebouncedEvent]) -> Vec<String> {
    let mut files = BTreeSet::new();
    for ev in events {
        for p in &ev.paths {
            if let Some(rel) = rel_path(root, p) {
                if is_watched_rel(&rel) {
                    files.insert(rel);
                }
            }
        }
    }
    files.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// §6.2 排除规则
    #[test]
    fn filters_runtime_noise() {
        assert!(is_watched_rel("structure.toml"));
        assert!(is_watched_rel("fragments/f_0001.md"));
        assert!(is_watched_rel("entities/characters/c_0001.toml"));
        assert!(is_watched_rel("worldview/concepts.toml"));
        assert!(!is_watched_rel(".novelstudio/stats.json"));
        assert!(!is_watched_rel(".novelstudio/history/f_0001/1.md"));
        assert!(!is_watched_rel(".git/HEAD"));
        assert!(!is_watched_rel(".git/objects/ab/cdef"));
        assert!(!is_watched_rel("fragments/f_0001.md.tmp"));
        assert!(!is_watched_rel(".tmp"));
        assert!(!is_watched_rel(""));
    }

    /// 批次合并：相对化 + 过滤 + 去重
    #[test]
    fn collects_relative_deduped_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ev = |p: &std::path::PathBuf| DebouncedEvent {
            event: notify::Event::new(notify::EventKind::Other).add_path(p.clone()),
            time: Instant::now(),
        };
        let files = collect_files(
            &root,
            &[
                ev(&root.join("fragments/f_0001.md")),
                ev(&root.join("fragments/f_0001.md")), // 去重
                ev(&root.join(".novelstudio/stats.json")), // 排除
                ev(&root.join("entities/characters/c_0001.toml")),
            ],
        );
        assert_eq!(
            files,
            vec![
                "entities/characters/c_0001.toml".to_string(),
                "fragments/f_0001.md".to_string(),
            ]
        );
    }

    struct CollectSink(StdMutex<Vec<String>>, StdMutex<Vec<WatchStatusDto>>);

    impl FileEvents for CollectSink {
        fn changed(&self, files: Vec<String>) {
            self.0.lock().unwrap().push(files.join(","));
        }
        fn state(&self, dto: WatchStatusDto) {
            self.1.lock().unwrap().push(dto);
        }
    }

    fn wait_for<T>(timeout: Duration, f: impl Fn() -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = f() {
                return Some(v);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 端到端冒烟：真实文件写入 → 400ms 防抖 → 收到相对路径事件；
    /// 噪音目录写入不产生事件。
    #[test]
    fn end_to_end_emits_relative_paths_and_ignores_noise() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".novelstudio")).unwrap();
        let sink = Arc::new(CollectSink(StdMutex::new(vec![]), StdMutex::new(vec![])));
        let mut handle = WatcherHandle::spawn_with_sink(dir.path(), sink.clone());

        let running = wait_for(Duration::from_secs(5), || {
            matches!(handle.status(), WatchState::Running).then_some(())
        });
        assert!(
            running.is_some(),
            "watcher 应进入 Running：{:?}",
            handle.status()
        );

        std::fs::write(dir.path().join("structure.toml"), "chapters = []\n").unwrap();
        std::fs::write(dir.path().join(".novelstudio/stats.json"), "{}").unwrap();

        let got = wait_for(Duration::from_secs(5), || {
            sink.0
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.as_str() == "structure.toml")
                .cloned()
        });
        assert_eq!(got.as_deref(), Some("structure.toml"));
        // 防抖窗口过后噪音不应出现（再等一个窗口确认）
        std::thread::sleep(DEBOUNCE + Duration::from_millis(300));
        let all = sink.0.lock().unwrap().join("|");
        assert!(!all.contains("stats.json"), "噪音文件不应产生事件：{all}");

        handle.stop();
        assert_eq!(handle.status(), WatchState::Stopped { reason: None });
    }

    /// stop 幂等且能回收线程
    #[test]
    fn stop_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(CollectSink(StdMutex::new(vec![]), StdMutex::new(vec![])));
        let mut handle = WatcherHandle::spawn_with_sink(dir.path(), sink);
        handle.stop();
        handle.stop(); // 第二次不应 panic / 阻塞
        assert!(handle.thread.is_none());
    }
}
