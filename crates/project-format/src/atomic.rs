//! 原子写入 — 同目录写 `.tmp` → fsync → rename。
//!
//! 半截写入（`fs::write` 先截断后写，中途崩溃 / 断电）是文本项目最致命的
//! 损坏方式：磁盘上留下半个 TOML / 半章正文。rename 在同一文件系统上是
//! 原子的，换来的是更强的保证——**任一时刻磁盘上要么是完整旧文件，要么
//! 是完整新文件**。
//!
//! 顺序：临时文件写入并 `sync_all`，rename 之后同步父目录（unix）——
//! 只同步文件不同步目录的话，掉电后 rename 这个目录项本身可能丢失。
//!
//! 临时文件名带 pid + 进程内序号：桌面端与 MCP 外部 Agent 双进程写同一
//! 本书时（跨进程锁只闸到"写操作串行"，管不到 tmp 文件名），保证两个
//! 进程的暂存文件互不相踩。

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 目标同目录下的暂存文件名：`foo.toml.<pid>.<seq>.tmp`
fn staged_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map_or_else(|| std::ffi::OsString::from("file"), |n| n.to_os_string());
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".{}.{}.tmp", std::process::id(), seq));
    target.with_file_name(name)
}

/// 第一步：内容写入暂存文件并 fsync，返回暂存路径。原文件不受任何影响。
pub fn write_staged(target: impl AsRef<Path>, content: &str) -> std::io::Result<PathBuf> {
    let target = target.as_ref();
    let tmp = staged_path(target);
    let mut f = File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    Ok(tmp)
}

/// 第二步：暂存文件 rename 到位并同步父目录（unix）。失败时清理暂存文件。
pub fn commit(staged: &Path, target: impl AsRef<Path>) -> std::io::Result<()> {
    let target = target.as_ref();
    match fs::rename(staged, target) {
        Ok(()) => {
            sync_parent_dir(target);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(staged);
            Err(e)
        }
    }
}

/// 原子写入整份内容（= [`write_staged`] + [`commit`]）。
pub fn write(target: impl AsRef<Path>, content: &str) -> std::io::Result<()> {
    match write_staged(&target, content) {
        Ok(staged) => commit(&staged, target),
        Err(e) => Err(e),
    }
}

/// rename 落位后同步父目录，防掉电丢目录项。Windows 无法以 std 打开目录，跳过。
#[cfg(unix)]
fn sync_parent_dir(target: &Path) {
    if let Some(dir) = target.parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_target: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_creates_and_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("structure.toml");

        write(&target, "第一版").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "第一版");

        write(&target, "第二版").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "第二版");

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "成功后不应残留 .tmp: {leftovers:?}");
    }

    #[test]
    fn commit_failure_cleans_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        // 目标是个已存在的目录 → rename 必失败 → 暂存文件应被清理
        let target = dir.path().join("occupied");
        std::fs::create_dir(&target).unwrap();

        let staged = write_staged(&target, "内容").unwrap();
        assert!(commit(&staged, &target).is_err());
        assert!(!staged.exists(), "失败的暂存文件必须清理");
    }

    #[cfg(unix)]
    #[test]
    fn failed_stage_keeps_original_intact() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("scene.md");
        std::fs::write(&target, "完好原文").unwrap();

        // 父目录去写权限 → 暂存文件建不出来 → 原文件必须原样
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        let result = write(&target, "新内容");
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "完好原文");
    }
}
