//! ProjectLock — 项目级并发闸门（读共享 / 写排他），桌面端、内嵌 MCP
//! HTTP、外部 stdio CLI 三方在同一项目上的写操作都经此排队。
//!
//! 两级锁，获取顺序恒为**进程内 → 跨进程**，释放顺序相反（无死锁环，
//! 所有获取路径都在本模块内）：
//!
//! 1. **进程内** `RwLock<()>`——按规范化项目路径从进程级注册表取同一把
//!    （App 内嵌 HTTP 每会话一个 server 实例、Tauri 命令、妙笔 Agent
//!    工具调用，各自 new 出的句柄共享同一把锁）。
//! 2. **跨进程** std `File::lock` / `lock_shared`（1.89+；macOS 为 flock、
//!    Linux 为 OFD fcntl，均属 open file description——同进程独立打开的
//!    描述符也互斥）。锁文件是项目 `.novelstudio/tool-gate.lock`，只作
//!    锁载体永不写入；每次守卫独立打开、drop 自动释放、进程崩溃即解锁。
//!    锁文件不可用时退化为仅进程内闸门（best-effort）。
//!
//! 并发不加闸的两类事故（来自 MCP 时代的实战回归）：
//! - 流水线乱序：改名跑在建档之前；
//! - ID 分配竞态：并发 create_* 扫号互不可见，分到同一 ID、写同一文件
//!   互相覆盖，先写的数据静默丢失。

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

/// 跨进程锁文件路径。路径必须保持稳定：不同版本、不同进程都在这同一
/// 个文件上排队，改了路径等于没锁。
pub fn gate_file(root: &Path) -> PathBuf {
    root.join(".novelstudio").join("tool-gate.lock")
}

/// 进程级注册表：规范化路径 → 同一把进程内 RwLock。
/// Box::leak 每个路径只泄漏一次（一个 RwLock，几十字节），换来守卫可以
/// 携带 `'static` 生命周期、无需自引用借用。
fn process_cell(key: &Path) -> &'static RwLock<()> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, &'static RwLock<()>>>> = OnceLock::new();
    let reg = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = reg
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cell) = map.get(key) {
        return cell;
    }
    let cell: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
    map.insert(key.to_path_buf(), cell);
    cell
}

/// 项目锁句柄。`for_root` 同一路径（规范化后）的所有句柄共享同一把
/// 进程内锁；跨进程锁文件按同一路径解析。
#[derive(Debug, Clone)]
pub struct ProjectLock {
    /// 规范化项目根（注册表键 + 锁文件定位）
    key: PathBuf,
    /// 锁文件路径；`.novelstudio` 建不出来（只读介质等）时 None = 仅进程内
    gate: Option<PathBuf>,
}

impl ProjectLock {
    pub fn for_root(root: &Path) -> Self {
        // canonicalize 失败（项目尚未落盘等）时退回原路径——注册表键仍一致
        let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let gate = std::fs::create_dir_all(key.join(".novelstudio"))
            .ok()
            .map(|_| gate_file(&key));
        Self { key, gate }
    }

    /// 共享读守卫：读与读并存，与任何写守卫互斥。
    pub fn read(&self) -> ProjectReadGuard {
        let local = process_cell(&self.key)
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let flock = self.acquire_file(false);
        ProjectReadGuard {
            _flock: flock,
            _local: local,
        }
    }

    /// 排他写守卫：与一切读 / 写守卫互斥。变更操作应整体持锁
    /// （扫描→分配→落盘因此原子）。
    pub fn write(&self) -> ProjectWriteGuard {
        let local = process_cell(&self.key)
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let flock = self.acquire_file(true);
        ProjectWriteGuard {
            _flock: flock,
            _local: local,
        }
    }

    /// 跨进程文件锁。失败不致命：退化为仅进程内闸门（best-effort）。
    fn acquire_file(&self, exclusive: bool) -> Option<File> {
        let path = self.gate.as_ref()?;
        let f = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .ok()?;
        let res = if exclusive { f.lock() } else { f.lock_shared() };
        match res {
            Ok(()) => Some(f),
            Err(e) => {
                eprintln!("[project-lock] 跨进程锁获取失败（退化为仅进程内闸门）：{e}");
                None
            }
        }
    }
}

/// 守卫的字段声明顺序即 drop 顺序：先放跨进程文件锁，再放进程内锁
/// （与获取顺序严格相反）。
pub struct ProjectReadGuard {
    _flock: Option<File>,
    _local: std::sync::RwLockReadGuard<'static, ()>,
}

pub struct ProjectWriteGuard {
    _flock: Option<File>,
    _local: std::sync::RwLockWriteGuard<'static, ()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// 同一项目的两个句柄共享同一把进程内锁：一个持写，另一个的写必须等待
    #[test]
    fn same_root_instances_share_one_cell() {
        let dir = tempfile::tempdir().unwrap();
        let l1 = ProjectLock::for_root(dir.path());
        let l2 = ProjectLock::for_root(dir.path());
        // 路径带冗余分量也应归一到同一把锁（macOS 符号链接 / Linux 均如此）
        let redundant = dir.path().join(".");
        let l3 = ProjectLock::for_root(&redundant);
        assert_eq!(l3.key, l1.key, "冗余路径应规范化为同一注册表键");

        let gate = l1.write();
        let (tx, rx) = mpsc::channel();
        let t = std::thread::spawn(move || {
            let _g = l2.write();
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "写守卫持有期间，另一句柄的写应被阻塞"
        );
        drop(gate);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("释放后应放行");
        t.join().unwrap();
        drop(l3);
    }

    /// 不同项目互不影响：A 的写守卫不挡 B
    #[test]
    fn different_roots_are_independent() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _ga = ProjectLock::for_root(a.path()).write();
        let _gb = ProjectLock::for_root(b.path()).write(); // 不应死锁
    }

    /// 读共享、写排他（进程内语义）
    #[test]
    fn read_shares_and_write_excludes() {
        let dir = tempfile::tempdir().unwrap();
        let l = ProjectLock::for_root(dir.path());

        let r1 = l.read();
        let r2 = l.read(); // 读共享：两把并存
        drop((r1, r2));

        let w = l.write();
        assert!(
            process_cell(&l.key).try_read().is_err(),
            "写持有期间读应被拒绝"
        );
        drop(w);
        assert!(process_cell(&l.key).try_read().is_ok(), "释放后应恢复");
    }

    /// 跨进程锁语义：锁属 open file description——同进程两次独立打开也
    /// 互斥；读共享；释放后可重取。（桌面端 / 外部 CLI 双进程竞态的基础，
    /// 用同进程双描述符模拟。）
    #[test]
    fn file_lock_excludes_and_shares() {
        let dir = tempfile::tempdir().unwrap();
        let path = gate_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let open = || {
            File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap()
        };

        let w1 = open();
        w1.lock().unwrap();
        let w2 = open();
        assert!(w2.try_lock().is_err(), "独立打开的描述符应互斥");
        drop(w1);
        w2.lock().unwrap(); // 释放后可获取

        w2.unlock().unwrap();
        let r1 = open();
        r1.lock_shared().unwrap();
        let r2 = open();
        r2.lock_shared().unwrap(); // 读共享：两把并存
        let w3 = open();
        assert!(w3.try_lock().is_err(), "读持有期间写应被拒");
    }
}
