//! AI 调用账本 — 每次模型调用的场景 / token / 耗时 / 结果
//!
//! 记账是 best-effort——账本读写失败绝不影响调用本身。
//! 保留最近 [`MAX_ENTRIES`] 条（环形淘汰最旧），存 `.novelstudio/ai-ledger.json`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::journal;

/// 保留条数上限
pub const MAX_ENTRIES: usize = 500;

/// 单次调用记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCallRecord {
    /// 调用时间（毫秒时间戳）
    pub ts: u64,
    /// 场景标签（续写 / 断言抽取 / 妙笔对话 / 逐章成书·章节名…）
    pub scene: String,
    /// Provider 标识（anthropic / openai …）
    pub provider: String,
    /// 模型名（旧账本无此字段，展示时回退 provider）
    #[serde(default)]
    pub model: String,
    /// token 消耗（不可得时为 0）
    pub tokens: u64,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 是否成功
    pub ok: bool,
    /// 失败原因
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 账本文件
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiLedger {
    #[serde(default)]
    pub entries: Vec<AiCallRecord>,
}

/// 账本概要（展示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiLedgerSummary {
    /// 保留期内的总调用数
    pub total_calls: u64,
    /// 保留期内的总 token
    pub total_tokens: u64,
    /// 保留期内的总耗时（毫秒）
    pub total_duration_ms: u64,
    /// 最近 n 条（新 → 旧）
    pub recent: Vec<AiCallRecord>,
}

fn ledger_path(root: &Path) -> std::path::PathBuf {
    root.join(".novelstudio").join("ai-ledger.json")
}

/// 追加一条调用记录（best-effort）
// 调用记账的字段各自独立，收成结构体会让全部调用点变啰嗦
#[allow(clippy::too_many_arguments)]
pub fn record(
    root: &Path,
    scene: &str,
    provider: &str,
    model: &str,
    tokens: u64,
    duration_ms: u64,
    ok: bool,
    error: Option<&str>,
) {
    let mut ledger = load(root);
    ledger.entries.push(AiCallRecord {
        ts: journal::now_ms(),
        scene: scene.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        tokens,
        duration_ms,
        ok,
        error: error.map(|e| e.replace(['\n', '\r'], " ")),
    });
    if ledger.entries.len() > MAX_ENTRIES {
        let drop = ledger.entries.len() - MAX_ENTRIES;
        ledger.entries.drain(0..drop);
    }
    let path = ledger_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&ledger) {
        let _ = journal::write_verified(root, "ai_ledger_store", &path, &json);
    }
}

/// 读取账本（缺失/损坏返回空账本）
pub fn load(root: &Path) -> AiLedger {
    std::fs::read_to_string(ledger_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 账本概要：合计 + 最近 `recent_limit` 条（新 → 旧）
pub fn summary(root: &Path, recent_limit: usize) -> AiLedgerSummary {
    let ledger = load(root);
    let total_calls = ledger.entries.len() as u64;
    let total_tokens = ledger.entries.iter().map(|e| e.tokens).sum();
    let total_duration_ms = ledger.entries.iter().map(|e| e.duration_ms).sum();
    let mut recent: Vec<AiCallRecord> = ledger
        .entries
        .iter()
        .rev()
        .take(recent_limit)
        .cloned()
        .collect();
    recent.truncate(recent_limit);
    AiLedgerSummary {
        total_calls,
        total_tokens,
        total_duration_ms,
        recent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    #[test]
    fn records_and_summarizes() {
        let (_dir, paths) = temp_root();
        record(
            &paths,
            "续写",
            "anthropic",
            "claude-sonnet-4-5",
            1200,
            3400,
            true,
            None,
        );
        record(
            &paths,
            "妙笔对话",
            "openai",
            "gpt-5.2",
            0,
            900,
            false,
            Some("401 未授权\n请检查 Key"),
        );

        let s = summary(&paths, 10);
        assert_eq!(s.total_calls, 2);
        assert_eq!(s.total_tokens, 1200);
        assert_eq!(s.recent.len(), 2);
        assert_eq!(s.recent[0].scene, "妙笔对话"); // 新 → 旧
        assert!(!s.recent[0].ok);
        assert!(
            !s.recent[0].error.as_ref().unwrap().contains('\n'),
            "错误信息压平为单行"
        );
    }

    #[test]
    fn caps_entries() {
        let (_dir, paths) = temp_root();
        for i in 0..(MAX_ENTRIES + 20) {
            record(
                &paths,
                &format!("场景{i}"),
                "openai",
                "gpt-5.2",
                1,
                1,
                true,
                None,
            );
        }
        assert_eq!(load(&paths).entries.len(), MAX_ENTRIES);
        let s = summary(&paths, 1);
        assert_eq!(s.recent[0].scene, format!("场景{}", MAX_ENTRIES + 19));
    }
}
