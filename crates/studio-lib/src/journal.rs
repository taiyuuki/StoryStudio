//! 写入取证日志 — 正文落盘的"黑匣子"
//!
//! 写入（片段正文、版本快照）后回读比对，并把成败记入日志，丢稿时可以
//! 对账排查。设计取舍：
//! - 成功也逐条落日志（本应用写入已被编辑器防抖到 ~800ms/次，一行追加
//!   的开销可忽略，无需攒批与后台线程）
//! - 失败与回读校验不一致立即记录，且以 `verify_fail` 区分"写入报错"
//!   与"写入声称成功但内容对不上"两种事故
//! - 日志超 1MB 轮转为 `.1`（只保留一代）
//! - 取证日志自身的任何 IO 失败都不影响正文写入路径（best-effort）
//!
//! 存放于 `.novelstudio/logs/write-journal.log`，不入 git。

use std::path::{Path, PathBuf};

use crate::error::StudioError;

/// 日志轮转阈值（字节）
const ROTATE_BYTES: u64 = 1024 * 1024;

/// 当前毫秒时间戳（UNIX epoch 起）
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 取证日志路径：`.novelstudio/logs/write-journal.log`
pub fn journal_path(root: &Path) -> PathBuf {
    root.join(".novelstudio")
        .join("logs")
        .join("write-journal.log")
}

/// 追加一条取证记录（best-effort，绝不向上传播错误）
///
/// `note` 记录补充信息（错误消息 / 回读差异摘要等）。
pub fn record(root: &Path, op: &str, target: &Path, bytes: usize, ok: bool, note: Option<&str>) {
    let path = journal_path(root);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= ROTATE_BYTES {
            // 轮转：旧日志 → .1（覆盖上一代）
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    // 路径记相对根（跨机器迁移后仍可读）；分隔符统一为 /，Windows 下日志格式保持一致
    let rel = target.strip_prefix(root).unwrap_or(target);
    let shown = rel.to_string_lossy().replace('\\', "/");
    let outcome = if ok { "ok" } else { "fail" };
    let note = note
        .map(|n| n.replace(['\n', '\r'], " "))
        .unwrap_or_default();
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        now_ms(),
        op,
        shown,
        bytes,
        outcome,
        note
    );
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 原子写入并回读校验：写 `.tmp` → fsync → 回读比对 → rename 到位。
///
/// 这是片段正文的唯一写出口——杜绝两种事故：
/// - "写入返回成功但磁盘上不是这份内容"（回读对不上）
/// - 半截写入（先截断后写，中途崩溃/断电）——rename 之前原文件分毫未动，
///   即使回读校验失败，磁盘上留的也是完整的旧版本
pub fn write_verified(
    root: &Path,
    op: &str,
    target: &Path,
    content: &str,
) -> Result<(), StudioError> {
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let bytes = content.len();
    // 回读校验对暂存文件做：通过后才 rename，原文件只在验证通过后被替换
    let staged = match crate::atomic::write_staged(target, content) {
        Ok(tmp) => tmp,
        Err(e) => {
            record(root, op, target, bytes, false, Some(&e.to_string()));
            return Err(StudioError::Io(format!(
                "写入 {} 失败: {e}",
                target.display()
            )));
        }
    };
    match std::fs::read(&staged) {
        Ok(readback) if readback == content.as_bytes() => {
            if let Err(e) = crate::atomic::commit(&staged, target) {
                record(root, op, target, bytes, false, Some(&e.to_string()));
                return Err(StudioError::Io(format!(
                    "写入 {} 失败: {e}",
                    target.display()
                )));
            }
            record(root, op, target, bytes, true, None);
            Ok(())
        }
        other => {
            let note = match other {
                Ok(b) => format!("回读 {} 字节，与写入的 {} 字节不一致", b.len(), bytes),
                Err(e) => format!("回读失败: {e}"),
            };
            let _ = std::fs::remove_file(&staged);
            // 回读不一致是最高级别的事故信号，单独标记（此时原文件未被触碰）
            record(
                root,
                op,
                target,
                bytes,
                false,
                Some(&format!("verify_fail: {note}")),
            );
            Err(StudioError::Io(format!(
                "这次保存似乎没有写进磁盘（{}）。请再保存一次；若反复失败，详情已记录在项目 .novelstudio/logs/ 中",
                note
            )))
        }
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
    fn verified_write_roundtrips_and_journals() {
        let (_dir, root) = temp_root();
        let paths = story_project_format::ProjectPaths::new(&root);
        let target = paths.fragments_dir().join("f_0001.md");
        write_verified(
            &root,
            "fragment_write",
            &target,
            "---\nid: f_0001\n---\n\n正文",
        )
        .unwrap();

        let log = std::fs::read_to_string(journal_path(&root)).unwrap();
        assert!(log.contains("fragment_write"));
        assert!(log.contains("ok"));
        assert!(log.contains("f_0001.md"));
    }

    #[test]
    fn record_never_panics_on_unwritable_path() {
        let (dir, root) = temp_root();
        // 日志目录被同名文件占用 → create_dir_all 失败 → 静默跳过
        std::fs::write(root.join(".novelstudio"), "not a dir").unwrap();
        record(&root, "op", Path::new("/tmp/x.md"), 10, true, None);
        assert!(dir.path().exists());
    }

    #[test]
    fn record_strips_root_prefix() {
        let (_dir, root) = temp_root();
        let paths = story_project_format::ProjectPaths::new(&root);
        let target = paths.fragments_dir().join("f_0002.md");
        record(&root, "op", &target, 3, false, Some("line1\nline2"));
        let log = std::fs::read_to_string(journal_path(&root)).unwrap();
        assert!(log.contains("fragments/f_0002.md"));
        let line = log.trim_end_matches('\n');
        assert!(!line.contains('\n'), "note 中的换行必须被压平: {log}");
    }
}
