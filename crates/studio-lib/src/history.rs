//! 片段版本历史 — 节流式本地快照
//!
//! 直接每次保存都存版本会淹没真正有价值的版本。因此对「即将被覆盖的
//! 当前内容」做节流快照：
//! - 与上一版正文完全一致（仅锚点/frontmatter 变化）→ 跳过
//! - 距上一版不足 5 分钟且正文字数变化不足 200 → 跳过
//! - 删除片段 / 恢复版本前 → 无条件强制快照当前内容
//! 每个片段保留最近 [`KEEP_VERSIONS`] 版，超出按时间淘汰。
//!
//! 快照是完整的片段文件内容（含 frontmatter），存于
//! `.novelstudio/history/f_XXXX/<毫秒时间戳>.md`，不入 git。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use story_project_format::ProjectPaths;
use story_ir::count_words;

use crate::error::StudioError;
use crate::journal;

/// 快照最小间隔（毫秒）
const MIN_INTERVAL_MS: u64 = 5 * 60 * 1000;
/// 触发快照的最小正文字符变化量
const MIN_BODY_DELTA_CHARS: usize = 200;
/// 每个片段保留的版本数上限
pub const KEEP_VERSIONS: usize = 20;

/// 版本摘要（列表用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentVersion {
    /// 快照时间（毫秒时间戳，同时是版本文件名）
    pub timestamp: u64,
    /// 快照正文字数（与全工作区口径一致，不含 frontmatter）
    pub words: u32,
    /// 快照文件字节数
    pub bytes: u64,
}

/// 片段历史目录：`.novelstudio/history/<fragment_id>/`
fn fragment_history_dir(paths: &ProjectPaths, fragment_id: &str) -> Result<PathBuf, StudioError> {
    // fragment_id 来自前端 / 工具参数，进入路径前校验形态，防目录穿越
    let valid = fragment_id.starts_with("f_")
        && fragment_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(StudioError::Config(format!(
            "无效的片段 ID: {fragment_id}"
        )));
    }
    Ok(paths
        .root()
        .join(".novelstudio")
        .join("history")
        .join(fragment_id))
}

/// 解析版本文件名为时间戳（`1695...md` / 同毫秒冲突时 `1695...-2.md`）
fn parse_version_ts(filename: &str) -> Option<u64> {
    let stem = filename.strip_suffix(".md")?;
    let base = stem.split('-').next()?;
    base.parse().ok()
}

/// 列出片段的所有版本，按时间倒序
pub fn list_versions(
    paths: &ProjectPaths,
    fragment_id: &str,
) -> Result<Vec<FragmentVersion>, StudioError> {
    let dir = fragment_history_dir(paths, fragment_id)?;
    let mut versions = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(ts) = parse_version_ts(&name) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        versions.push(FragmentVersion {
            timestamp: ts,
            words: count_words(crate::fragment::fragment_body(&content)),
            bytes: content.len() as u64,
        });
    }
    versions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(versions)
}

/// 读取指定版本的完整文件内容
pub fn read_version(
    paths: &ProjectPaths,
    fragment_id: &str,
    timestamp: u64,
) -> Result<String, StudioError> {
    let dir = fragment_history_dir(paths, fragment_id)?;
    // 先试精确文件名，再按 `<ts>-N` 前缀找同毫秒冲突版本
    let exact = dir.join(format!("{timestamp}.md"));
    if exact.exists() {
        return std::fs::read_to_string(&exact).map_err(StudioError::from);
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if parse_version_ts(&name) == Some(timestamp) {
                return std::fs::read_to_string(entry.path()).map_err(StudioError::from);
            }
        }
    }
    Err(StudioError::NotFound(format!(
        "片段 '{fragment_id}' 的版本 {timestamp} 不存在"
    )))
}

/// 恢复指定版本：先强制快照当前内容（恢复本身可再撤销），再覆写文件。
///
/// 返回恢复后的正文（调用方用于本地同步 UI）。
pub fn restore_version(
    paths: &ProjectPaths,
    fragment_id: &str,
    timestamp: u64,
) -> Result<String, StudioError> {
    let content = read_version(paths, fragment_id, timestamp)?;
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的片段 ID: {fragment_id}")))?;

    if frag_path.exists() {
        // 当前内容留底，防止"恢复错了想回去"
        force_snapshot(paths, fragment_id);
    }
    journal::write_verified(paths.root(), "version_restore", &frag_path, &content)?;
    Ok(crate::fragment::fragment_body(&content).to_string())
}

/// 覆写前的节流快照：内容变化满足阈值或距上一版超过间隔才落快照。
///
/// 在 [`crate::mutations::write_fragment_text_source`] 等写入路径中、
/// 覆盖磁盘文件**之前**调用——快照的是即将被覆盖的旧内容。
pub fn maybe_snapshot(paths: &ProjectPaths, fragment_id: &str) {
    let frag_path = match paths.file_for_id(fragment_id) {
        Some(p) if p.exists() => p,
        _ => return,
    };
    let Ok(content) = std::fs::read_to_string(&frag_path) else {
        return;
    };
    let body = crate::fragment::fragment_body(&content);
    if body.trim().is_empty() {
        // 空片段没有留底价值（首版基线由真正有内容时建立）
        return;
    }

    let dir = match fragment_history_dir(paths, fragment_id) {
        Ok(d) => d,
        Err(_) => return,
    };
    if let Some((last_ts, last_path)) = latest_snapshot(&dir) {
        if let Ok(last_content) = std::fs::read_to_string(&last_path) {
            let last_body = crate::fragment::fragment_body(&last_content);
            if last_body == body {
                return; // 正文没变（锚点/frontmatter 变化不产生新版本）
            }
            let elapsed = journal::now_ms().saturating_sub(last_ts);
            let delta = last_body.chars().count().abs_diff(body.chars().count());
            if elapsed < MIN_INTERVAL_MS && delta < MIN_BODY_DELTA_CHARS {
                return; // 变化太小且间隔太短，暂不留底
            }
        }
    }
    write_snapshot(paths, fragment_id, &content);
}

/// 无条件快照当前内容（删除片段 / 恢复版本前用）
pub fn force_snapshot(paths: &ProjectPaths, fragment_id: &str) {
    let frag_path = match paths.file_for_id(fragment_id) {
        Some(p) if p.exists() => p,
        _ => return,
    };
    let Ok(content) = std::fs::read_to_string(&frag_path) else {
        return;
    };
    if content.trim().is_empty() {
        return;
    }
    write_snapshot(paths, fragment_id, &content);
}

/// 写入一版快照并修剪到保留上限
fn write_snapshot(paths: &ProjectPaths, fragment_id: &str, content: &str) {
    let dir = match fragment_history_dir(paths, fragment_id) {
        Ok(d) => d,
        Err(_) => return,
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = next_snapshot_ts();
    let mut target = dir.join(format!("{ts}.md"));
    let mut n = 1u32;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{ts}-{n}.md"));
    }
    // 快照丢失不影响正文写入，失败仅记日志
    if journal::write_verified(paths.root(), "version_snapshot", &target, content).is_err() {
        let _ = std::fs::write(&target, content);
        journal::record(paths.root(), "version_snapshot", &target, content.len(), false, None);
    }
    prune(paths, &dir);
}

/// 分配单调递增的快照时间戳。
///
/// 连续两次快照可能落在同一毫秒内：文件名靠 `-2` 后缀区分，但解析回的
/// 时间戳会相同，导致「按时间戳恢复」无法区分这两版。这里保证进程内
/// 每次快照的毫秒时间戳严格递增（撞上一次就 +1），版本身份唯一。
fn next_snapshot_ts() -> u64 {
    static LAST: std::sync::OnceLock<std::sync::Mutex<u64>> = std::sync::OnceLock::new();
    let lock = LAST.get_or_init(|| std::sync::Mutex::new(0u64));
    let mut last = lock.lock().unwrap();
    let now = journal::now_ms();
    let ts = if now > *last { now } else { *last + 1 };
    *last = ts;
    ts
}

/// 目录中最新的快照（时间戳, 路径）
fn latest_snapshot(dir: &PathBuf) -> Option<(u64, PathBuf)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut latest: Option<(u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(ts) = parse_version_ts(&name.to_string_lossy()) {
            if latest.as_ref().map(|(t, _)| ts > *t).unwrap_or(true) {
                latest = Some((ts, entry.path()));
            }
        }
    }
    latest
}

/// 只保留最近 [`KEEP_VERSIONS`] 版
fn prune(paths: &ProjectPaths, dir: &PathBuf) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut snaps: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            parse_version_ts(&e.file_name().to_string_lossy()).map(|ts| (ts, e.path()))
        })
        .collect();
    if snaps.len() <= KEEP_VERSIONS {
        return;
    }
    snaps.sort_by(|a, b| b.0.cmp(&a.0)); // 新 → 旧
    for (_, path) in snaps.iter().skip(KEEP_VERSIONS) {
        let bytes = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);
        if std::fs::remove_file(path).is_ok() {
            journal::record(paths.root(), "version_prune", path, bytes, true, None);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, ProjectPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(dir.path());
        std::fs::create_dir_all(paths.fragments_dir()).unwrap();
        (dir, paths)
    }

    fn write_fragment(paths: &ProjectPaths, id: &str, body: &str) {
        let content = format!("---\nid: {id}\n---\n\n{body}");
        journal::write_verified(paths.root(), "fragment_write", &paths.fragments_dir().join(format!("{id}.md")), &content).unwrap();
    }

    #[test]
    fn snapshot_requires_body_delta_or_interval() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0001", &"雨".repeat(400));

        // 首次：无上一版 → 快照
        maybe_snapshot(&paths, "f_0001");
        assert_eq!(list_versions(&paths, "f_0001").unwrap().len(), 1);

        // 正文未变（只动 frontmatter）→ 不新增
        let content = std::fs::read_to_string(paths.fragments_dir().join("f_0001.md")).unwrap();
        let anchored = content.replace("---\nid: f_0001\n", "---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n");
        journal::write_verified(paths.root(), "fragment_write", &paths.fragments_dir().join("f_0001.md"), &anchored).unwrap();
        maybe_snapshot(&paths, "f_0001");
        assert_eq!(list_versions(&paths, "f_0001").unwrap().len(), 1, "锚点变化不应产生版本");

        // 5 分钟内但正文变化 ≥200 字 → 快照
        write_fragment(&paths, "f_0001", &"雨夜。".repeat(250));
        maybe_snapshot(&paths, "f_0001");
        assert_eq!(list_versions(&paths, "f_0001").unwrap().len(), 2);

        // 5 分钟内且只改了几个字 → 不快照
        write_fragment(&paths, "f_0001", &format!("{}改", "雨夜。".repeat(250)));
        maybe_snapshot(&paths, "f_0001");
        assert_eq!(list_versions(&paths, "f_0001").unwrap().len(), 2);
    }

    #[test]
    fn force_snapshot_always_writes() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0002", "正文内容");
        force_snapshot(&paths, "f_0002");
        force_snapshot(&paths, "f_0002");
        assert_eq!(list_versions(&paths, "f_0002").unwrap().len(), 2);
    }

    #[test]
    fn restore_roundtrip_and_undoable() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0003", "旧版本内容");
        force_snapshot(&paths, "f_0003");
        write_fragment(&paths, "f_0003", "新版本内容");

        let versions = list_versions(&paths, "f_0003").unwrap();
        assert_eq!(versions.len(), 1);
        let restored = restore_version(&paths, "f_0003", versions[0].timestamp).unwrap();
        assert_eq!(restored, "旧版本内容");

        // 恢复前强制留底了"新版本内容"，可再恢复回去
        let versions = list_versions(&paths, "f_0003").unwrap();
        assert_eq!(versions.len(), 2);
        let back = restore_version(&paths, "f_0003", versions[0].timestamp).unwrap();
        assert_eq!(back, "新版本内容");
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let (_dir, paths) = temp_paths();
        assert!(list_versions(&paths, "../evil").is_err());
        assert!(read_version(&paths, "c_0001", 0).is_err()); // 非 f_ 前缀
    }

    #[test]
    fn words_count_excludes_frontmatter() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0004", "林风推开门");
        force_snapshot(&paths, "f_0004");
        let v = &list_versions(&paths, "f_0004").unwrap()[0];
        assert_eq!(v.words, 5, "frontmatter 的 id/anchors 不计入字数");
    }

    #[test]
    fn prune_keeps_limit() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0005", "内容");
        for i in 0..(KEEP_VERSIONS + 5) {
            write_fragment(&paths, "f_0005", &format!("内容{i}"));
            force_snapshot(&paths, "f_0005");
        }
        assert_eq!(
            list_versions(&paths, "f_0005").unwrap().len(),
            KEEP_VERSIONS
        );
    }
}
