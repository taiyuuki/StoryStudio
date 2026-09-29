//! Interactive Input — Agent 工具向用户提问的同步等待机制
//!
//! 解决一个问题：`Tool::execute` 是同步的 `Result<String, String>`，
//! 但某些工具（如 `ask_user`）需要暂停等待用户输入。这里复用
//! [`crate::session::SessionStore`] 已建立的「分离式 session map」模式
//! （参见 `cancel_tokens`）—— pending slot 存在独立 map 中，
//! 不依赖 `SessionEntry` 的 Mutex，避免与正在持锁运行的 `chat` 死锁。
//!
//! ## 工作流
//!
//! 1. `chat` 开始时，thread-local 记下 `session_id`（[`current_session_id`]）
//! 2. `ask_user` 工具在 `execute` 内调用 [`InteractiveStore::request`]，
//!    注册 [`PendingSlot`] 并 emit `AgentStep::AskUser`
//! 3. 工具线程在 [`PendingSlot::wait`] 阻塞等待（Condvar）
//! 4. 前端收到 `AskUser` step → 渲染问题与选项
//! 5. 用户回答 → 调命令 → [`InteractiveStore::respond`] 写入答案 + notify
//! 6. 工具线程被唤醒，返回答案作为 `ToolResult`
//!
//! 取消路径：当用户取消整个 chat 时，[`crate::session::SessionStore::cancel`]
//! 会调用 [`InteractiveStore::cancel_pending`]，向 slot 写入「已取消」并 notify，
//! 工具线程立即解锁，主循环随后检测 `CancelToken` 退出。

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

use crate::cancel::CancelToken;

/// 用户回答的等待结果
pub type InputOutcome = Result<String, String>;

/// 单个提问的等待槽 —— 答案 + Condvar
///
/// `answer` 初始为 `None`；被 [`InteractiveStore::respond`] 写入 `Ok`、
/// 被 [`InteractiveStore::cancel_pending`] 写入 `Err`、或等待超时后由
/// 等待方自行判定。Condvar 用于唤醒阻塞的等待方。
#[derive(Debug)]
pub struct PendingSlot {
    answer: Mutex<Option<InputOutcome>>,
    cvar: Condvar,
}

impl PendingSlot {
    fn new() -> Self {
        Self {
            answer: Mutex::new(None),
            cvar: Condvar::new(),
        }
    }

    /// 提交答案（Ok）或取消（Err），并唤醒等待方。
    fn resolve(&self, outcome: InputOutcome) {
        let mut guard = self.answer.lock().unwrap();
        // 仅首次提交生效，避免重复 respond 覆盖
        if guard.is_none() {
            *guard = Some(outcome);
            self.cvar.notify_all();
        }
    }

    /// 阻塞等待用户回答。
    ///
    /// - 响应到达 → 返回 `Ok(answer)` / `Err(cancel_msg)`
    /// - 超时 → 返回 `Err(timeout_msg)`（不解除 slot，由 store 清理）
    /// - `cancel_token` 被触发 → 仍需等待外部 `cancel_pending` 唤醒；
    ///   这里以 `cancel_token.is_cancelled()` 作为提前退出信号，
    ///   与 `cancel_pending` 配合使用（后者会写入 `Err` 并 notify）。
    pub fn wait(&self, timeout: Duration, cancel_token: &CancelToken) -> InputOutcome {
        let mut guard = self.answer.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(outcome) = guard.as_ref() {
                return outcome.clone();
            }
            // 已被取消 → 立即返回（cancel_pending 通常已写入答案，此为兜底）
            if cancel_token.is_cancelled() {
                return Err("用户取消了执行".to_string());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("用户未在限定时间内回答".to_string());
            }
            // wait_timeout 会原子释放锁并阻塞，返回时重新持锁
            let result = self.cvar.wait_timeout(guard, deadline - now).unwrap();
            guard = result.0;
        }
    }
}

/// 等待用户输入的全局存储 —— 按 session_id 索引。
///
/// 与 `SessionStore` 的 `cancel_tokens` 同构：`RwLock<HashMap<String, Arc<PendingSlot>>>`，
/// 完全不接触 `SessionEntry` 的 Mutex，因此即便 Agent 正在持锁运行 `chat`，
/// 外部 `respond` / `cancel_pending` 也能即时完成。
pub struct InteractiveStore {
    slots: RwLock<HashMap<String, Arc<PendingSlot>>>,
}

impl Default for InteractiveStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractiveStore {
    pub fn new() -> Self {
        Self {
            slots: RwLock::new(HashMap::new()),
        }
    }

    /// 为 session 注册一个新的等待槽，返回槽的 Arc（供等待方持有）。
    ///
    /// 若该 session 已有一个 pending 槽（罕见，串行场景下不会发生），
    /// 旧槽会被新槽覆盖（旧的等待方在 `take` 时拿到 `None` 自然退出）。
    pub fn register(&self, session_id: &str) -> Arc<PendingSlot> {
        let slot = Arc::new(PendingSlot::new());
        self.slots
            .write()
            .unwrap()
            .insert(session_id.to_string(), Arc::clone(&slot));
        slot
    }

    /// 提交用户答案。返回 `true` 表示 slot 存在并已写入；`false` 表示无对应槽。
    pub fn respond(&self, session_id: &str, answer: String) -> bool {
        let slot = {
            let slots = self.slots.read().unwrap();
            slots.get(session_id).cloned()
        };
        match slot {
            Some(s) => {
                s.resolve(Ok(answer));
                true
            }
            None => false,
        }
    }

    /// 取消某个 session 的等待（用户取消 chat 时调用）。
    pub fn cancel_pending(&self, session_id: &str) {
        let slot = {
            let slots = self.slots.read().unwrap();
            slots.get(session_id).cloned()
        };
        if let Some(s) = slot {
            s.resolve(Err("用户取消了执行".to_string()));
        }
    }

    /// 移除并丢弃 session 的等待槽（清理用，drop_session / gc 调用）。
    pub fn take(&self, session_id: &str) {
        self.slots.write().unwrap().remove(session_id);
    }

    /// 是否存在未完成的等待槽
    #[cfg(test)]
    pub fn has_pending(&self, session_id: &str) -> bool {
        self.slots.read().unwrap().contains_key(session_id)
    }
}

// ============================================================================
// thread-local chat context
// ============================================================================

/// 当前 chat 的上下文快照 —— 由 `SessionStore::chat` 在入口设置、出口清空，
/// 让同步执行的工具（如 `ask_user`）能取到 session_id、cancel_token 与 step 回调。
///
/// `on_step` 用 `Option<Arc<dyn Fn...>>` 而非裸 `StepCallback`：thread-local
/// 需要 `'static`，而 `StepCallback = Arc<dyn Fn(...) + Send + Sync>` 本身满足。
#[derive(Clone)]
pub struct ChatContext {
    pub session_id: String,
    pub cancel_token: CancelToken,
    pub on_step: Option<std::sync::Arc<dyn Fn(crate::agent::AgentStep) + Send + Sync>>,
}

thread_local! {
    static CURRENT_CHAT: std::cell::RefCell<Option<ChatContext>> = const { std::cell::RefCell::new(None) };
}

/// 设置当前线程的 chat 上下文（`chat` 入口处调用，结束时清空）
pub fn set_current_chat(ctx: Option<ChatContext>) {
    CURRENT_CHAT.with(|cell| *cell.borrow_mut() = ctx);
}

/// 获取当前线程绑定的 session_id。
///
/// 在 `chat` 执行期间（含工具 `execute`）返回 `Some(session_id)`，
/// 让 `ask_user` 等工具能定位到所属 session。
pub fn current_session_id() -> Option<String> {
    CURRENT_CHAT.with(|cell| cell.borrow().as_ref().map(|c| c.session_id.clone()))
}

/// 获取当前 chat 上下文的完整克隆（工具需要 cancel_token / on_step 时使用）。
pub fn current_chat() -> Option<ChatContext> {
    CURRENT_CHAT.with(|cell| cell.borrow().clone())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn slot_resolves_with_answer() {
        let store = InteractiveStore::new();
        let slot = store.register("sess_a");
        assert!(store.has_pending("sess_a"));

        // 等待方线程
        let slot_clone = Arc::clone(&slot);
        let token = CancelToken::new();
        let token_clone = token.clone();
        let handle = thread::spawn(move || slot_clone.wait(Duration::from_secs(2), &token_clone));

        thread::sleep(Duration::from_millis(50));
        assert!(store.respond("sess_a", "是".to_string()));

        let outcome = handle.join().unwrap();
        assert_eq!(outcome, Ok("是".to_string()));
    }

    #[test]
    fn slot_times_out() {
        let store = InteractiveStore::new();
        let slot = store.register("sess_t");
        let token = CancelToken::new();

        let start = std::time::Instant::now();
        let outcome = slot.wait(Duration::from_millis(100), &token);
        let elapsed = start.elapsed();

        assert!(matches!(outcome, Err(ref m) if m.contains("限定时间内")));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn cancel_pending_wakes_waiter() {
        let store = InteractiveStore::new();
        let slot = store.register("sess_c");
        let token = CancelToken::new();

        let store_clone = Arc::new(store);
        let store_for_thread = Arc::clone(&store_clone);
        let slot_clone = Arc::clone(&slot);
        let token_clone = token.clone();
        let handle = thread::spawn(move || slot_clone.wait(Duration::from_secs(5), &token_clone));

        thread::sleep(Duration::from_millis(50));
        store_for_thread.cancel_pending("sess_c");

        let outcome = handle.join().unwrap();
        assert!(matches!(outcome, Err(ref m) if m.contains("取消")));
    }

    #[test]
    fn respond_to_missing_returns_false() {
        let store = InteractiveStore::new();
        assert!(!store.respond("nonexistent", "x".to_string()));
    }

    #[test]
    fn first_resolve_wins() {
        // 重复 respond 不覆盖首次答案
        let store = InteractiveStore::new();
        let slot = store.register("sess_d");
        let token = CancelToken::new();

        store.respond("sess_d", "首次".to_string());
        store.respond("sess_d", "二次".to_string()); // 应被忽略

        let outcome = slot.wait(Duration::from_millis(10), &token);
        assert_eq!(outcome, Ok("首次".to_string()));
    }

    #[test]
    fn take_removes_slot() {
        let store = InteractiveStore::new();
        store.register("sess_x");
        assert!(store.has_pending("sess_x"));
        store.take("sess_x");
        assert!(!store.has_pending("sess_x"));
    }

    #[test]
    fn current_session_id_thread_local() {
        assert!(current_session_id().is_none());
        let ctx = ChatContext {
            session_id: "sess_tl".to_string(),
            cancel_token: CancelToken::new(),
            on_step: None,
        };
        set_current_chat(Some(ctx));
        assert_eq!(current_session_id().as_deref(), Some("sess_tl"));

        // 子线程不继承
        let handle = thread::spawn(current_session_id);
        assert!(handle.join().unwrap().is_none());

        set_current_chat(None);
        assert!(current_session_id().is_none());
    }
}
