//! Cancel Token — Agent 主循环取消机制
//!
//! 提供 Agent 主循环的协作式取消：用户可在任意时刻触发取消，
//! Agent 在每轮循环顶部检查令牌状态并优雅退出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ============================================================================
// CancelToken
// ============================================================================

/// 取消令牌 — 线程安全、可克隆（共享同一状态）
///
/// Agent 持有 CancelToken 的克隆，在主循环每轮检查 `is_cancelled()`。
#[derive(Debug, Clone)]
pub struct CancelToken(Arc<AtomicBool>);

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    /// 创建未取消的令牌
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// 触发取消
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 是否已被取消
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// 重置为未取消状态（用于复用令牌，每次 chat 开始时调用）
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancel_sets_flag() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn clone_shares_state() {
        let token = CancelToken::new();
        let clone = token.clone();

        // 通过克隆取消，原件可见
        clone.cancel();
        assert!(token.is_cancelled());

        // 通过原件取消，克隆可见
        token.reset();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn default_is_not_cancelled() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled());
    }
}
