//! Session Store — 跨调用保持 Agent 状态
//!
//! 解决 Agent 的跨调用记忆问题：每次命令调用不再新建 Agent，
//! 而是按 session_id 从 SessionStore 获取已有的 Agent 实例，
//! 使得对话历史、scratchpad 记忆、任务计划得以跨调用保持。
//!
//! SessionStore 是进程级的（内存存储），空闲会话超时后自动回收。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::agent::{Agent, AgentStep, AgentTrace};
use crate::cancel::CancelToken;
use crate::interactive::{set_current_chat, InteractiveStore};
use crate::provider::AiError;

// ============================================================================
// 类型别名
// ============================================================================

/// 步骤回调 — 每产生一个 AgentStep 即推送
pub type StepCallback = Arc<dyn Fn(AgentStep) + Send + Sync>;

/// 文本增量回调 — 流式输出时推送
pub type DeltaCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Agent 工厂 — 创建新 Agent 的闭包
pub type AgentFactory = Box<dyn FnOnce() -> Agent + Send>;

// ============================================================================
// SessionEntry
// ============================================================================

/// 单个会话条目
struct SessionEntry {
    /// Agent 实例（持有 messages/plan/scratchpad）
    agent: Agent,
    /// 最后活跃时间（用于 GC）
    last_active: Instant,
}

// ============================================================================
// SessionStore
// ============================================================================

/// 会话存储 — 进程级，按 session_id 保持 Agent 状态
///
/// 线程安全（RwLock）
/// 空闲超过 `idle_ttl` 的会话会被 GC 回收。
///
/// **取消机制设计**：CancelToken 独立存储在 `cancel_tokens` map 中，
/// 不依赖 SessionEntry 的 Mutex。这样在 Agent 运行（chat 持锁）期间，
/// cancel 命令仍能即时获取 token 并触发取消，避免死锁。
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Arc<Mutex<SessionEntry>>>>,
    /// 独立的取消令牌存储 —— 与 sessions 分离，避免 cancel 与 chat 争锁死锁。
    /// get_or_create 时同步写入；cancel 时直接读取，完全不碰 SessionEntry 的锁。
    cancel_tokens: RwLock<HashMap<String, CancelToken>>,
    /// 向用户提问的等待槽存储 —— 同样独立于 sessions，让 ask_user 工具
    /// 阻塞等待时，外部的 respond / cancel_pending 命令无需争 SessionEntry 锁。
    pending_inputs: InteractiveStore,
    /// 空闲会话回收时间（默认 30 分钟）
    idle_ttl: Duration,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    /// 创建空的 SessionStore，默认 30 分钟空闲回收
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            cancel_tokens: RwLock::new(HashMap::new()),
            pending_inputs: InteractiveStore::new(),
            idle_ttl: Duration::from_secs(30 * 60),
        }
    }

    /// 设置空闲回收时间
    pub fn with_idle_ttl(mut self, ttl: Duration) -> Self {
        self.idle_ttl = ttl;
        self
    }

    /// 获取或创建会话
    ///
    /// 若 session_id 已存在则返回已有 Agent 的锁守卫；
    /// 否则用 factory 创建新 Agent 并存入。
    pub fn get_or_create(&self, session_id: &str, factory: AgentFactory) -> SessionHandle {
        // 读锁快速路径
        {
            let sessions = self.sessions.read().unwrap();
            if let Some(entry) = sessions.get(session_id) {
                return SessionHandle {
                    entry: Arc::clone(entry),
                };
            }
        }

        // 写锁创建
        let mut sessions = self.sessions.write().unwrap();
        // 双重检查（可能其他线程刚创建）
        if let Some(entry) = sessions.get(session_id) {
            return SessionHandle {
                entry: Arc::clone(entry),
            };
        }

        let agent = factory();
        // 同步把 cancel_token 存入独立 map，供 cancel 路径无锁访问
        let token = agent.cancel_token();
        let entry = Arc::new(Mutex::new(SessionEntry {
            agent,
            last_active: Instant::now(),
        }));
        sessions.insert(session_id.to_string(), Arc::clone(&entry));
        self.cancel_tokens
            .write()
            .unwrap()
            .insert(session_id.to_string(), token);
        SessionHandle { entry }
    }

    /// 删除指定会话
    pub fn drop_session(&self, session_id: &str) {
        self.sessions.write().unwrap().remove(session_id);
        self.cancel_tokens.write().unwrap().remove(session_id);
        // 同步取消并清理 pending 槽，避免工具线程卡死
        self.pending_inputs.cancel_pending(session_id);
        self.pending_inputs.take(session_id);
    }

    /// 列出所有活跃会话 ID
    pub fn list_session_ids(&self) -> Vec<String> {
        self.sessions.read().unwrap().keys().cloned().collect()
    }

    /// 获取指定会话的取消令牌（从独立 map 读取，不碰 SessionEntry 锁，避免死锁）
    pub fn cancel_token(&self, session_id: &str) -> Option<CancelToken> {
        let tokens = self.cancel_tokens.read().unwrap();
        tokens.get(session_id).cloned()
    }

    /// 取消指定会话的当前运行
    ///
    /// 从独立的 cancel_tokens map 读取令牌，完全不接触 SessionEntry 的 Mutex，
    /// 因此即使 Agent 正在运行（chat 持有 SessionEntry 锁），cancel 也能即时返回。
    /// 同时唤醒可能阻塞在 ask_user 工具上的等待线程，避免取消后卡死。
    pub fn cancel(&self, session_id: &str) -> bool {
        let cancelled = if let Some(token) = self.cancel_token(session_id) {
            token.cancel();
            true
        } else {
            false
        };
        // 解除 ask_user 等交互工具的阻塞
        self.pending_inputs.cancel_pending(session_id);
        cancelled
    }

    /// 会话数量
    pub fn len(&self) -> usize {
        self.sessions.read().unwrap().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.sessions.read().unwrap().is_empty()
    }

    /// 清理超时空闲会话
    pub fn gc(&self) {
        let now = Instant::now();
        let mut sessions = self.sessions.write().unwrap();
        let mut to_remove: Vec<String> = Vec::new();
        sessions.retain(|id, entry| {
            let entry_guard = entry.lock().unwrap();
            let keep = now.duration_since(entry_guard.last_active) < self.idle_ttl;
            if !keep {
                to_remove.push(id.clone());
            }
            keep
        });
        // 同步清理 cancel_tokens 与 pending_inputs（避免内存泄漏）
        if !to_remove.is_empty() {
            let mut tokens = self.cancel_tokens.write().unwrap();
            for id in &to_remove {
                tokens.remove(id);
                self.pending_inputs.cancel_pending(id);
                self.pending_inputs.take(id);
            }
        }
    }

    /// 执行会话的 chat（便捷方法）
    ///
    /// 获取会话锁 → 更新活跃时间 → 调用 agent.chat → 返回 trace。
    /// 同时把 `session_id` 写入 thread-local，让 `ask_user` 等工具能定位到
    /// 所属 session 的 pending slot（chat 与工具 execute 在同一线程同步执行）。
    pub fn chat(
        &self,
        session_id: &str,
        factory: AgentFactory,
        user_input: &str,
        on_step: Option<StepCallback>,
        on_delta: Option<DeltaCallback>,
    ) -> Result<AgentTrace, AiError> {
        let handle = self.get_or_create(session_id, factory);
        let mut entry = handle.entry.lock().unwrap();
        entry.last_active = Instant::now();

        // 组装 chat 上下文，让同步执行的工具（ask_user）能取到
        // session_id、cancel_token 与 step 回调。
        let ctx = crate::interactive::ChatContext {
            session_id: session_id.to_string(),
            cancel_token: entry.agent.cancel_token(),
            on_step: on_step.clone(),
        };
        set_current_chat(Some(ctx));

        let result = entry.agent.chat(user_input, on_step, on_delta);

        set_current_chat(None);
        // chat 结束后清理 pending 槽（残留通常是异常路径）
        self.pending_inputs.take(session_id);
        result
    }

    // ========================================================================
    // 交互式输入（ask_user 等）
    // ========================================================================

    /// 注册一个提问并阻塞等待用户回答。
    ///
    /// 由工具 `execute` 在 chat 线程同步调用：
    /// 1. 注册 pending slot
    /// 2. 通过 `on_step` 回调 emit `AgentStep::AskUser`（前端据此渲染 UI）
    /// 3. 阻塞等待回答 / 取消 / 超时
    /// 4. 返回答案（`Ok`）或取消/超时信息（`Err`），作为 `ToolResult` 回传 LLM
    ///
    /// 超时默认 5 分钟。等待期间外部 `respond` / `cancel` 均从独立的
    /// `pending_inputs` map 访问，不争 SessionEntry 锁。
    pub fn request_input(
        &self,
        session_id: &str,
        prompt_id: &str,
        question: &str,
        options: &[String],
        cancel_token: &CancelToken,
        on_step: &StepCallback,
    ) -> Result<String, String> {
        let slot = self.pending_inputs.register(session_id);

        // 先通知前端展示问题
        on_step(AgentStep::AskUser {
            prompt_id: prompt_id.to_string(),
            question: question.to_string(),
            options: options.to_vec(),
        });

        let outcome = slot.wait(Duration::from_secs(5 * 60), cancel_token);

        // 清理槽（无论成功与否）
        self.pending_inputs.take(session_id);

        outcome
    }

    /// 提交用户对当前提问的回答（由 `agent_respond` 调用）。
    ///
    /// 返回 `true` 表示存在 pending 槽并已写入；`false` 表示无对应槽
    /// （可能已超时/已取消/已回答）。
    pub fn respond(&self, session_id: &str, answer: String) -> bool {
        self.pending_inputs.respond(session_id, answer)
    }
}

// ============================================================================
// SessionHandle
// ============================================================================

/// 会话句柄 — 持有对 SessionEntry 的 Arc<Mutex>
///
/// 通过 lock() 获取 Agent 的可变引用。
pub struct SessionHandle {
    entry: Arc<Mutex<SessionEntry>>,
}

impl SessionHandle {
    /// 锁定并执行操作
    pub fn with_agent<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Agent) -> R,
    {
        let mut entry = self.entry.lock().unwrap();
        entry.last_active = Instant::now();
        f(&mut entry.agent)
    }

    /// 获取取消令牌
    pub fn cancel_token(&self) -> CancelToken {
        let entry = self.entry.lock().unwrap();
        entry.agent.cancel_token()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentConfig, ToolRegistry};
    use crate::provider::{
        AiError, AiProvider, GenerateRequest, GenerateResponse, StopReason, TokenUsage,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    struct MockProvider {
        text: String,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new(text: &str) -> Self {
            Self {
                text: text.to_string(),
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl AiProvider for MockProvider {
        fn id(&self) -> &str {
            "mock-session"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(GenerateResponse {
                text: self.text.clone(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    fn make_factory(text: &str) -> AgentFactory {
        let provider = Arc::new(MockProvider::new(text)) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        Box::new(move || {
            Agent::new(
                "system",
                AgentConfig::default(),
                provider.clone(),
                tools.clone(),
            )
        })
    }

    #[test]
    fn store_creates_and_reuses_session() {
        let store = SessionStore::new();

        // 第一次获取：创建
        let h1 = store.get_or_create("sess_1", make_factory("hi"));
        assert_eq!(store.len(), 1);

        // 第二次获取：复用（同一个 Arc）
        let h2 = store.get_or_create("sess_1", make_factory("hi"));
        // Arc 指针相等 = 复用
        assert!(Arc::ptr_eq(&h1.entry, &h2.entry));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn store_drops_session() {
        let store = SessionStore::new();
        let _h = store.get_or_create("sess_1", make_factory("hi"));
        assert_eq!(store.len(), 1);

        store.drop_session("sess_1");
        assert!(store.is_empty());
    }

    #[test]
    fn store_chat_persists_memory() {
        let store = SessionStore::new();

        // 第一次 chat
        let trace1 = store
            .chat(
                "sess_1",
                make_factory("第一句回复"),
                "用户第一句",
                None,
                None,
            )
            .unwrap();
        assert!(!trace1.steps.is_empty());

        // 第二次 chat（同 session_id）—— 应复用同一个 Agent
        let trace2 = store
            .chat(
                "sess_1",
                make_factory("不会被使用"),
                "用户第二句",
                None,
                None,
            )
            .unwrap();

        // 两次都应成功，且 Agent 保持了历史
        assert!(trace2.steps.len() >= 2);
    }

    #[test]
    fn store_chat_second_turn_sees_first_turn_history() {
        // 关键测试：第二次 chat 时，发送给 LLM 的消息应包含第一次的 user + assistant
        let store = SessionStore::new();

        // 用一个能记录消息数量的 provider
        let provider = Arc::new(MockProvider::new("AI 回复")) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let factory = Box::new(move || {
            Agent::new(
                "sys",
                AgentConfig::default(),
                provider.clone(),
                tools.clone(),
            )
        }) as AgentFactory;

        // 第一次：用户说"你好"，AI 回复
        store
            .chat("sess_mem", factory, "你好，我是小明", None, None)
            .unwrap();

        // 第二次：用户说"我叫什么？"
        // 此时 Agent 的 messages 应有 [user("你好"), assistant("AI 回复"), user("我叫什么")] = 3 条
        let handle = store.get_or_create("sess_mem", {
            // factory 不会被调用（session 已存在）
            Box::new(|| panic!("factory should not be called for existing session"))
        });

        handle.with_agent(|agent| {
            let msgs = agent.messages();
            assert!(
                msgs.len() >= 2,
                "第二次调用前应已有历史消息，实际: {} 条",
                msgs.len()
            );
            // 第一条应该是第一次的用户消息
            assert!(
                msgs[0].content.contains("小明"),
                "第一条消息应包含第一次的用户输入，实际: {}",
                msgs[0].content
            );
        });
    }

    #[test]
    fn store_list_sessions() {
        let store = SessionStore::new();
        store.get_or_create("sess_a", make_factory("a"));
        store.get_or_create("sess_b", make_factory("b"));
        store.get_or_create("sess_c", make_factory("c"));

        let mut ids = store.list_session_ids();
        ids.sort();
        assert_eq!(ids, vec!["sess_a", "sess_b", "sess_c"]);
    }

    #[test]
    fn store_cancel() {
        let store = SessionStore::new();
        let h = store.get_or_create("sess_1", make_factory("hi"));

        // 通过 store 取消
        assert!(store.cancel("sess_1"));
        // 不存在的会话
        assert!(!store.cancel("sess_nonexistent"));

        // 验证 Agent 确实被取消
        h.with_agent(|agent| {
            assert!(agent.cancel_token().is_cancelled());
        });
    }

    #[test]
    fn store_handle_with_agent() {
        let store = SessionStore::new();
        let h = store.get_or_create("sess_1", make_factory("hi"));

        h.with_agent(|agent| {
            agent.chat("hello", None, None).unwrap();
            assert!(!agent.messages().is_empty());
        });

        // 第二次操作应看到之前的消息
        h.with_agent(|agent| {
            assert!(!agent.messages().is_empty());
        });
    }

    // —— 死锁修复验证 ——

    /// 慢速 provider：generate 时阻塞指定时长，模拟 LLM 长时间调用
    #[derive(Debug)]
    struct SlowProvider {
        delay: Duration,
    }

    impl AiProvider for SlowProvider {
        fn id(&self) -> &str {
            "slow"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            thread::sleep(self.delay);
            Ok(GenerateResponse {
                text: "慢回复".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    fn make_slow_factory(delay: Duration) -> AgentFactory {
        let provider = Arc::new(SlowProvider { delay }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        Box::new(move || {
            Agent::new(
                "system",
                AgentConfig::default(),
                provider.clone(),
                tools.clone(),
            )
        })
    }

    /// 关键测试：cancel 在 chat 持锁期间必须能即时返回（不死锁）
    ///
    /// 修复前：cancel 会在 entry.lock() 处阻塞，直到 chat 结束（慢 provider 要等很久）
    /// 修复后：cancel 从独立的 cancel_tokens map 读取，立即返回
    #[test]
    fn cancel_returns_immediately_while_chat_running() {
        let store = Arc::new(SessionStore::new());
        let store_clone = Arc::clone(&store);

        // chat 在另一个线程运行，provider 阻塞 2 秒
        let chat_handle = thread::spawn(move || {
            store_clone.chat(
                "sess_deadlock",
                make_slow_factory(Duration::from_secs(2)),
                "hello",
                None,
                None,
            )
        });

        // 等一下让 chat 开始并持锁
        thread::sleep(Duration::from_millis(200));

        // 此时 chat 正持有 SessionEntry 锁（在 SlowProvider::generate 中 sleep）
        // cancel 必须能立即返回，而不是等到 chat 结束
        let cancel_start = Instant::now();
        let cancelled = store.cancel("sess_deadlock");
        let cancel_elapsed = cancel_start.elapsed();

        assert!(cancelled, "cancel 应返回 true");
        // cancel 应在远小于 2 秒内完成（死锁时会卡到 chat 结束）
        assert!(
            cancel_elapsed < Duration::from_millis(500),
            "cancel 应即时返回，实际耗时 {:?}（可能死锁）",
            cancel_elapsed
        );

        // 等 chat 结束
        let trace = chat_handle.join().unwrap().unwrap();
        // 因 cancel 在 generate 返回前已设置（200ms 设置 vs 2s 返回），
        // generate 返回后的取消检查会触发，Agent 进入 Cancelled 状态。
        assert!(
            trace
                .steps
                .iter()
                .any(|s| matches!(s, AgentStep::Error { .. })),
            "trace 应包含取消错误步骤（generate 返回后应检测到取消）"
        );
    }

    /// 验证 cancel_tokens 在 drop_session 后被清理
    #[test]
    fn drop_session_cleans_cancel_token() {
        let store = SessionStore::new();
        store.get_or_create("sess_clean", make_factory("hi"));
        assert!(store.cancel_token("sess_clean").is_some());

        store.drop_session("sess_clean");
        assert!(store.cancel_token("sess_clean").is_none());
        assert!(!store.cancel("sess_clean"));
    }

    /// 验证 cancel_tokens 在 gc 后被清理
    #[test]
    fn gc_cleans_cancel_tokens() {
        let store = SessionStore::new().with_idle_ttl(Duration::from_millis(1));
        store.get_or_create("sess_old", make_factory("hi"));
        assert!(store.cancel_token("sess_old").is_some());

        // 等待超时
        thread::sleep(Duration::from_millis(10));
        store.gc();

        assert!(store.cancel_token("sess_old").is_none());
    }
}
