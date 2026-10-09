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

/// 写入回执的快照提示：最近一次快照若发生在数秒内（即本次写入前刚留底），
/// 返回提示行。
///
/// 版本快照功能一直在，但不在回执中露面时，用户和 Agent 都会以为
/// 整体改写/删除是"物理消失不可逆"而不敢用——这里把"已留底、可回滚"
/// 说到明面上。
pub fn fresh_snapshot_note(paths: &ProjectPaths, fragment_id: &str) -> Option<String> {
    let versions = list_versions(paths, fragment_id).ok()?;
    let latest = versions.first()?;
    if journal::now_ms().saturating_sub(latest.timestamp) > 10_000 {
        return None;
    }
    Some(format!(
        "\n📁 本次修改前已自动留存版本快照（该片段现共 {} 版，工作台片段菜单 → 版本历史 可差异比对与回滚）",
        versions.len()
    ))
}

// ============================================================================
// 版本差异（片段历史对话框用）
// ============================================================================

/// 差异行。`skipped` 表示被折叠的未变区段（`count` 为折叠行数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// same | added | removed | skipped
    pub kind: String,
    /// 旧文行号（1-based；added 为 None）
    pub old_ln: Option<u32>,
    /// 新文行号（1-based；removed 为 None）
    pub new_ln: Option<u32>,
    pub text: String,
    /// skipped 折叠的行数，其余 kind 恒为 1
    pub count: u32,
}

/// 比较两个版本的正文差异。
///
/// `old_ts` / `new_ts` 传 `None` 表示**当前磁盘内容**（与最新快照对比 /
/// 两版之间对比都不需要特殊分支）。比较的是剥除 frontmatter 后的正文。
pub fn diff_versions(
    paths: &ProjectPaths,
    fragment_id: &str,
    old_ts: Option<u64>,
    new_ts: Option<u64>,
) -> Result<Vec<DiffLine>, StudioError> {
    let old_body = version_body(paths, fragment_id, old_ts)?;
    let new_body = version_body(paths, fragment_id, new_ts)?;
    Ok(line_diff(&old_body, &new_body))
}

/// 取某版本正文：None → 当前文件；Some(ts) → 历史快照
fn version_body(
    paths: &ProjectPaths,
    fragment_id: &str,
    ts: Option<u64>,
) -> Result<String, StudioError> {
    let content = match ts {
        None => {
            let path = paths
                .file_for_id(fragment_id)
                .ok_or_else(|| StudioError::Config(format!("无效的片段 ID: {fragment_id}")))?;
            std::fs::read_to_string(&path).map_err(StudioError::from)?
        }
        Some(ts) => read_version(paths, fragment_id, ts)?,
    };
    Ok(crate::fragment::fragment_body(&content).to_string())
}

/// LCS 行级差异：added/removed/same 全量输出后折叠长段未变区。
///
/// 片段正文行数量级在几百行内，O(n·m) DP 足够；超限（异常超长正文）
/// 退化为"全部删除 + 全部新增"，仍能看清内容。
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = split_lines(old);
    let b: Vec<&str> = split_lines(new);

    // LCS 长度表（滚动数组回溯不便，直接存方向）
    let (n, m) = (a.len(), b.len());
    if n * m > 16_000_000 {
        return all_changed(&a, &b);
    }
    // dp[i][j] = a[i..] 与 b[j..] 的 LCS 长度
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * (m + 1) + j] = if a[i] == b[j] {
                dp[(i + 1) * (m + 1) + (j + 1)] + 1
            } else {
                std::cmp::max(dp[(i + 1) * (m + 1) + j], dp[i * (m + 1) + (j + 1)])
            };
        }
    }

    // 回溯产出操作序列（old/new 双指针同时推进，天然对齐行号）
    let mut ops: Vec<DiffLine> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut old_ln, mut new_ln) = (1u32, 1u32);
    while i < n && j < m {
        let idx = i * (m + 1) + j;
        if a[i] == b[j] {
            ops.push(DiffLine { kind: "same".into(), old_ln: Some(old_ln), new_ln: Some(new_ln), text: a[i].to_string(), count: 1 });
            i += 1; j += 1; old_ln += 1; new_ln += 1;
        } else if dp[(i + 1) * (m + 1) + j] >= dp[i * (m + 1) + (j + 1)] {
            ops.push(DiffLine { kind: "removed".into(), old_ln: Some(old_ln), new_ln: None, text: a[i].to_string(), count: 1 });
            i += 1; old_ln += 1;
        } else {
            ops.push(DiffLine { kind: "added".into(), old_ln: None, new_ln: Some(new_ln), text: b[j].to_string(), count: 1 });
            j += 1; new_ln += 1;
        }
    }
    while i < n {
        ops.push(DiffLine { kind: "removed".into(), old_ln: Some(old_ln), new_ln: None, text: a[i].to_string(), count: 1 });
        i += 1; old_ln += 1;
    }
    while j < m {
        ops.push(DiffLine { kind: "added".into(), old_ln: None, new_ln: Some(new_ln), text: b[j].to_string(), count: 1 });
        j += 1; new_ln += 1;
    }
    collapse_same(&ops)
}

fn split_lines(s: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = s.split('\n').collect();
    // 末尾换行产生的空尾行不参与比较
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn all_changed(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let mut ops: Vec<DiffLine> = a
        .iter()
        .enumerate()
        .map(|(i, t)| DiffLine { kind: "removed".into(), old_ln: Some(i as u32 + 1), new_ln: None, text: t.to_string(), count: 1 })
        .collect();
    ops.extend(b.iter().enumerate().map(|(j, t)| DiffLine {
        kind: "added".into(),
        old_ln: None,
        new_ln: Some(j as u32 + 1),
        text: t.to_string(),
        count: 1,
    }));
    ops
}

/// 连续 8 行以上的未变区折叠为上下各留 3 行 + 一个 skipped 标记
fn collapse_same(ops: &[DiffLine]) -> Vec<DiffLine> {
    const CONTEXT: usize = 3;
    const MIN_SKIP: usize = 8;

    let keep: Vec<bool> = {
        let mut keep = vec![false; ops.len()];
        // 先标记变更行，再向两侧扩上下文
        for (i, op) in ops.iter().enumerate() {
            if op.kind != "same" {
                for k in i.saturating_sub(CONTEXT)..(i + CONTEXT + 1).min(ops.len()) {
                    keep[k] = true;
                }
            }
        }
        keep
    };

    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        if keep[i] {
            out.push(ops[i].clone());
            i += 1;
            continue;
        }
        // 未保留的 same 连续段折叠（跨过保留行的空档不合并）
        let start = i;
        let mut skipped = 0usize;
        while i < ops.len() && !keep[i] && ops[i].kind == "same" {
            skipped += ops[i].count as usize;
            i += 1;
        }
        if skipped >= MIN_SKIP {
            out.push(DiffLine {
                kind: "skipped".into(),
                old_ln: ops[start].old_ln,
                new_ln: ops[start].new_ln,
                text: String::new(),
                count: skipped as u32,
            });
        } else {
            out.extend_from_slice(&ops[start..i]);
        }
    }
    out
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

    // —— 版本差异 ——

    #[test]
    fn line_diff_marks_add_remove_and_same() {
        let old = "第一行\n第二行\n第三行";
        let new = "第一行\n第二行改\n第三行\n第四行";
        let ops = line_diff(old, new);

        let removed: Vec<&str> = ops.iter().filter(|o| o.kind == "removed").map(|o| o.text.as_str()).collect();
        let added: Vec<&str> = ops.iter().filter(|o| o.kind == "added").map(|o| o.text.as_str()).collect();
        assert_eq!(removed, vec!["第二行"]);
        assert_eq!(added, vec!["第二行改", "第四行"]);
        // 行号对齐：removed 带旧行号，added 带新行号
        let rm = ops.iter().find(|o| o.kind == "removed").unwrap();
        assert_eq!(rm.old_ln, Some(2));
        let add4 = ops.iter().find(|o| o.text == "第四行").unwrap();
        assert_eq!(add4.new_ln, Some(4));
        assert_eq!(add4.old_ln, None);
    }

    #[test]
    fn line_diff_collapses_long_same_runs() {
        let old: Vec<String> = (0..30).map(|i| format!("行{i}")).collect();
        let mut new = old.clone();
        new[29] = "结尾改了".to_string();
        let ops = line_diff(&old.join("\n"), &new.join("\n"));

        let skipped: Vec<&DiffLine> = ops.iter().filter(|o| o.kind == "skipped").collect();
        assert_eq!(skipped.len(), 1, "长未变段折叠为一个标记: {ops:?}");
        // 变更行只在末尾：前 26 行折叠，变更前保留 3 行上下文
        let same = ops.iter().filter(|o| o.kind == "same").count();
        assert_eq!(same, 3, "变更前留 3 行上下文");
    }

    #[test]
    fn diff_versions_between_snapshots_and_current() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0006", "旧文第一段\n旧文第二段");
        force_snapshot(&paths, "f_0006");
        write_fragment(&paths, "f_0006", "旧文第一段\n新文第二段");
        let versions = list_versions(&paths, "f_0006").unwrap();
        let ts = versions[0].timestamp;

        // 快照 → 当前
        let ops = diff_versions(&paths, "f_0006", Some(ts), None).unwrap();
        assert!(ops.iter().any(|o| o.kind == "removed" && o.text == "旧文第二段"));
        assert!(ops.iter().any(|o| o.kind == "added" && o.text == "新文第二段"));

        // 自身对比无差异
        let ops = diff_versions(&paths, "f_0006", None, None).unwrap();
        assert!(ops.iter().all(|o| o.kind == "same" || o.kind == "skipped"));
    }

    #[test]
    fn fresh_snapshot_note_only_after_recent_snapshot() {
        let (_dir, paths) = temp_paths();
        write_fragment(&paths, "f_0007", "正文");
        assert!(fresh_snapshot_note(&paths, "f_0007").is_none());

        force_snapshot(&paths, "f_0007");
        let note = fresh_snapshot_note(&paths, "f_0007").unwrap();
        assert!(note.contains("版本快照"), "{note}");
        assert!(note.contains("1 版"), "{note}");
    }
}
