//! 码字统计 — 手写 / AI 双账本
//!
//! 所有正文写入都经过 mutations 层，在那里即可拿到
//! 新旧两份正文，按统一字数口径（[`story_ir::count_words`]，标点空白
//! 不计）计算净增：
//! - **手写**：编辑器防抖保存时的字数净增（>0 才记，删改不回冲）
//! - **AI**：AI 新建片段按全文字数记；AI 覆写已有片段按净增记
//! - **恢复版本**：不计（视为回到历史状态，不产生"新写的字"）
//!
//! 按本地日历日聚合；时区偏移由桌面端打开项目时同步（与 JS
//! `getTimezoneOffset()` 同号约定：东区为负）。保留最近 [`MAX_DAYS`] 天。
//! 存 `.novelstudio/stats.json`，不入 git。

use std::path::{Path, PathBuf};

use crate::journal;
use serde::{Deserialize, Serialize};

/// 保留天数上限（滚动淘汰最旧）
const MAX_DAYS: usize = 400;
/// 默认每日目标（字）
pub const DEFAULT_DAILY_GOAL: u32 = 2000;

/// 字的来源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteSource {
    /// 作者手写
    Manual,
    /// AI 生成
    Ai,
}

/// 单日计数
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DayCount {
    #[serde(default)]
    pub manual: u32,
    #[serde(default)]
    pub ai: u32,
}

impl DayCount {
    fn add(self, source: WriteSource, words: u32) -> Self {
        match source {
            WriteSource::Manual => Self {
                manual: self.manual.saturating_add(words),
                ai: self.ai,
            },
            WriteSource::Ai => Self {
                manual: self.manual,
                ai: self.ai.saturating_add(words),
            },
        }
    }

    /// 当日总字数
    pub fn total(self) -> u32 {
        self.manual.saturating_add(self.ai)
    }
}

/// 统计文件（`.novelstudio/stats.json`）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordStats {
    /// 本地时区偏移（分钟，与 JS getTimezoneOffset 同号：东区为负）
    #[serde(default)]
    pub tz_offset_min: i32,
    /// 每日目标（字）
    #[serde(default = "default_daily_goal")]
    pub daily_goal: u32,
    /// 按日聚合：`YYYY-MM-DD`（本地日历日）
    pub days: std::collections::BTreeMap<String, DayCount>,
}

fn default_daily_goal() -> u32 {
    DEFAULT_DAILY_GOAL
}

impl Default for WordStats {
    fn default() -> Self {
        Self {
            tz_offset_min: 0,
            daily_goal: default_daily_goal(),
            days: std::collections::BTreeMap::new(),
        }
    }
}

/// 统计概要（桌面端展示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordStatsSummary {
    pub daily_goal: u32,
    /// 今日（本地日）
    pub today: DayCount,
    /// 连续码字天数（今日无字数则从昨日起算）
    pub streak_days: u32,
    /// 最近 n 天（旧 → 新，空日补零），n 由调用方指定
    pub recent_days: Vec<RecentDay>,
    /// 全部保留期合计
    pub total: DayCount,
}

/// 单日条目（带日期键）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentDay {
    pub date: String,
    pub count: DayCount,
}

fn stats_path(root: &Path) -> PathBuf {
    root.join(".novelstudio").join("stats.json")
}

/// 读取统计文件（缺失/损坏时返回默认值——账本不能阻塞写作）
pub fn load(root: &Path) -> WordStats {
    std::fs::read_to_string(stats_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 写回统计文件
pub fn store(root: &Path, stats: &WordStats) {
    let path = stats_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(stats) {
        let _ = journal::write_verified(root, "stats_store", &path, &json);
    }
}

/// 当前本地日键（`YYYY-MM-DD`）
fn today_key(tz_offset_min: i32) -> String {
    day_key_for_ms(journal::now_ms(), tz_offset_min)
}

/// 指定时刻的本地日键（可注入时间，便于测试日界）
fn day_key_for_ms(ms: u64, tz_offset_min: i32) -> String {
    // JS getTimezoneOffset 约定：本地时间 = UTC − offset（东区为负）
    let shifted = ms as i64 - tz_offset_min as i64 * 60_000;
    let (y, m, d) = civil_from_days(shifted.div_euclid(86_400_000));
    format!("{y:04}-{m:02}-{d:02}")
}

/// 从本地日键推算 n 天前的键
fn days_before(key: &str, n: u32) -> Option<String> {
    let (y, m, d) = split_key(key)?;
    let days = days_from_civil(y, m, d) - n as i64;
    let (y, m, d) = civil_from_days(days);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn split_key(key: &str) -> Option<(i64, u32, u32)> {
    let mut it = key.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    Some((y, m, d))
}

/// Howard Hinnant 的 civil_from_days：epoch 天数 → 公历年月日
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// civil_from_days 的逆运算：公历年月日 → epoch 天数
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// 记一笔字数增量（>0 才有意义；由 mutations 层计算净增后调用）
pub fn record_delta(root: &Path, source: WriteSource, words: u32) {
    if words == 0 {
        return;
    }
    let mut stats = load(root);
    let key = today_key(stats.tz_offset_min);
    let day = stats.days.get(&key).copied().unwrap_or_default();
    stats.days.insert(key, day.add(source, words));
    prune(&mut stats);
    store(root, &stats);
}

/// 按新旧正文净增记账（写入路径统一入口）
///
/// `old_body` 为 None 表示新建片段（全文字数计入）。
pub fn record_change(root: &Path, old_body: Option<&str>, new_body: &str, source: WriteSource) {
    let old_words = old_body.map(story_ir::count_words).unwrap_or(0) as i64;
    let new_words = story_ir::count_words(new_body) as i64;
    let delta = (new_words - old_words).max(0) as u32;
    record_delta(root, source, delta);
}

/// 淘汰超出保留期的最旧日期
fn prune(stats: &mut WordStats) {
    while stats.days.len() > MAX_DAYS {
        let oldest = stats.days.keys().next().cloned();
        match oldest {
            Some(k) => {
                stats.days.remove(&k);
            }
            None => break,
        }
    }
}

/// 统计概要
pub fn summary(root: &Path, recent: usize) -> WordStatsSummary {
    let stats = load(root);
    let today = today_key(stats.tz_offset_min);
    let today_count = stats.days.get(&today).copied().unwrap_or_default();

    // 近 n 天（旧 → 新），空日补零
    let mut recent_days = Vec::with_capacity(recent);
    for i in (0..recent).rev() {
        let key = days_before(&today, i as u32).unwrap_or_else(|| today.clone());
        recent_days.push(RecentDay {
            date: key.clone(),
            count: stats.days.get(&key).copied().unwrap_or_default(),
        });
    }

    // 连续码字天数：从今日往回数；今日没写则从昨日起算（保持已有序列）
    let streak_start = if today_count.total() > 0 { 0 } else { 1 };
    let mut streak_days = 0u32;
    let mut i = streak_start;
    while let Some(key) = days_before(&today, i) {
        if stats.days.get(&key).map(|c| c.total() > 0).unwrap_or(false) {
            streak_days += 1;
            i += 1;
        } else {
            break;
        }
    }

    let total = stats
        .days
        .values()
        .fold(DayCount::default(), |acc, c| DayCount {
            manual: acc.manual.saturating_add(c.manual),
            ai: acc.ai.saturating_add(c.ai),
        });

    WordStatsSummary {
        daily_goal: stats.daily_goal,
        today: today_count,
        streak_days,
        recent_days,
        total,
    }
}

/// 设置每日目标
pub fn set_daily_goal(root: &Path, goal: u32) {
    let mut stats = load(root);
    stats.daily_goal = goal;
    store(root, &stats);
}

/// 同步本地时区偏移（桌面端打开项目时调用一次）
pub fn sync_tz(root: &Path, tz_offset_min: i32) {
    let mut stats = load(root);
    if stats.tz_offset_min != tz_offset_min {
        stats.tz_offset_min = tz_offset_min;
        store(root, &stats);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    #[test]
    fn civil_date_roundtrip() {
        // 与已知 epoch 日期互推
        for (days, y, m, d) in [
            (0i64, 1970, 1, 1),
            (19_723, 2024, 1, 1),
            (20_666, 2026, 8, 1),
            (20_715, 2026, 9, 19),
            (20_725, 2026, 9, 29),
        ] {
            assert_eq!(civil_from_days(days), (y, m, d));
            assert_eq!(days_from_civil(y, m, d), days);
        }
        // 跨闰年 2 月末
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(
            civil_from_days(days_from_civil(2023, 12, 31) + 1),
            (2024, 1, 1)
        );
    }

    #[test]
    fn tz_offset_shifts_day_boundary() {
        // 2026-09-29 01:00 UTC：UTC 还是 29 日，北京（UTC+8）也是 29 日
        let ms = 1_790_643_600_000u64; // 2026-09-29T01:00:00Z
        assert_eq!(day_key_for_ms(ms, 0), "2026-09-29");
        assert_eq!(day_key_for_ms(ms, -480), "2026-09-29");
        // 2026-09-29 20:00 UTC：UTC 29 日，北京 30 日凌晨
        let ms = 1_790_712_000_000u64; // 2026-09-29T20:00:00Z
        assert_eq!(day_key_for_ms(ms, 0), "2026-09-29");
        assert_eq!(day_key_for_ms(ms, -480), "2026-09-30");
    }

    #[test]
    fn record_change_counts_net_increase_only() {
        let (_dir, paths) = temp_root();
        record_change(
            &paths,
            Some("十个字十个字"),
            "十个字十个字十个字",
            WriteSource::Manual,
        );
        let s = summary(&paths, 7);
        assert_eq!(s.today.manual, 3);

        // 字数减少不回冲
        record_change(
            &paths,
            Some("十个字十个字十个字"),
            "两字",
            WriteSource::Manual,
        );
        let s = summary(&paths, 7);
        assert_eq!(s.today.manual, 3);

        // 新建片段全额计入（"十二个字十二个字" = 8 字）
        record_change(&paths, None, "十二个字十二个字", WriteSource::Ai);
        let s = summary(&paths, 7);
        assert_eq!(s.today.ai, 8);
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let (_dir, paths) = temp_root();
        let mut stats = load(&paths);
        let today = today_key(0);
        let yesterday = days_before(&today, 1).unwrap();
        let two_days_ago = days_before(&today, 2).unwrap();
        stats
            .days
            .insert(yesterday, DayCount { manual: 100, ai: 0 });
        stats
            .days
            .insert(two_days_ago, DayCount { manual: 50, ai: 0 });
        store(&paths, &stats);

        // 今日未写：streak 从昨日起算 = 2
        let s = summary(&paths, 7);
        assert_eq!(s.streak_days, 2);

        // 今日写了：streak = 3
        stats.days.insert(today, DayCount { manual: 10, ai: 0 });
        store(&paths, &stats);
        let s = summary(&paths, 7);
        assert_eq!(s.streak_days, 3);
    }

    #[test]
    fn goal_persists() {
        let (_dir, paths) = temp_root();
        assert_eq!(load(&paths).daily_goal, DEFAULT_DAILY_GOAL);
        set_daily_goal(&paths, 5000);
        assert_eq!(load(&paths).daily_goal, 5000);
    }
}
