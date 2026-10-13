//! Git 版本档案 — 项目目录的快照 / 历史 / 恢复（调用系统 git CLI）
//!
//! v1 范围（按设计收窄）：
//! - 检测（可用性 / 是否已建仓库 / 是否嵌在别的仓库里 / 有无未存档变更）
//! - init（补 .gitignore，不动已有的）
//! - 存档（git add -A + commit，附全书字数；无变更自动跳过）
//! - 历史（git log）
//! - 恢复（先自动备份当前状态，再把工作区切回目标——历史全程保留，可再恢复回来）
//!
//! 不做：远程推送、分支管理。项目目录是纯文本 + TOML，天然适合 git；
//! AI 批量改稿前后的快照让「这一轮 AI 到底改了什么」可审可回滚。
//!
//! 依赖系统安装了 git；未安装时状态返回 available=false，各操作报
//! 中文错误，前端优雅降级。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::StudioError;

// ============================================================================
// DTO
// ============================================================================

/// 仓库状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusDto {
    /// 系统是否装有 git
    pub available: bool,
    /// 项目目录本身是否已是 git 仓库
    pub initialized: bool,
    /// 是否嵌在别的 git 仓库里（不接管，仅提示）
    pub nested: bool,
    /// 有无未存档变更
    pub dirty: bool,
}

/// 一条存档记录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitDto {
    /// 短哈希
    pub id: String,
    /// Unix 时间戳（秒）
    pub timestamp: i64,
    pub message: String,
}

/// 存档 / 恢复操作结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitResult {
    /// 是否真的产生了新提交（无变更时 false）
    pub committed: bool,
    /// 新提交的短哈希
    pub id: Option<String>,
    /// 给用户看的一句话说明
    pub detail: String,
}

// ── diff（改动对比） ──────────────────────────────────────

/// 单文件改动统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffFileDto {
    /// 仓库相对路径（如 fragments/f_0001.md）
    pub path: String,
    /// A=新增 / M=修改 / D=删除
    pub status: String,
    /// 新增行数
    pub added: u64,
    /// 删除行数
    pub deleted: u64,
}

/// diff 中的一行
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffLineDto {
    /// ctx | add | del
    pub kind: String,
    pub text: String,
}

/// 一个 hunk（含位置头的连续改动块）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffHunkDto {
    /// @@ -1,3 +1,4 @@ 形式的位置头
    pub header: String,
    pub lines: Vec<GitDiffLineDto>,
}

/// 单文件 diff 明细
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffDetailDto {
    pub path: String,
    pub hunks: Vec<GitDiffHunkDto>,
    pub binary: bool,
    /// 行数超上限被截断
    pub truncated: bool,
}

// ============================================================================
// 内部：git 调用
// ============================================================================

fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output, StudioError> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| StudioError::Config(format!("无法启动 git（可能未安装）: {e}")))
}

fn run_git_ok(dir: &Path, args: &[&str]) -> Result<String, StudioError> {
    let out = run_git(dir, args)?;
    if !out.status.success() {
        return Err(StudioError::Config(format!(
            "git {} 失败: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// git 是否可用（系统装了 git）
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 项目根自身的仓库根（不在仓库里 → None）
fn repo_toplevel(dir: &Path) -> Option<std::path::PathBuf> {
    let out = run_git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::canonicalize(top).ok()
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// 本地身份兜底：全局没配 user.name/email 时写仓库级配置，
/// 否则 commit 会因 "tell me who you are" 失败
fn ensure_identity(dir: &Path) -> Result<(), StudioError> {
    let email = run_git(dir, &["config", "user.email"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if !email.is_empty() {
        return Ok(());
    }
    run_git_ok(dir, &["config", "user.name", "StoryStudio"])?;
    run_git_ok(dir, &["config", "user.email", "storystudio@local"])?;
    Ok(())
}

/// 补写标准 .gitignore（已有则不动——尊重用户自定义）
fn ensure_gitignore(dir: &Path) {
    let path = dir.join(".gitignore");
    if path.exists() {
        return;
    }
    let content = "# Story Studio Project\n\n\
        # 工具私有目录（派生缓存）\n\
        /.novelstudio/\n\n\
        # 运行期派生数据（频繁变动 / 体积大，不进版本库）\n\
        cache\nchats\nentity_id_watermark\nfragment_id_watermark\nhistory\nlogs\nstats.json\nai-ledger.json\nworkflow\n\n\
        # 临时文件\n\
        *.tmp\n*.bak\n";
    let _ = std::fs::write(path, content);
}

/// 全书正文字数（与前端展示同口径：story_ir::count_words）
fn total_words(dir: &Path) -> u64 {
    let fragments = dir.join("fragments");
    let Ok(entries) = std::fs::read_dir(fragments) else {
        return 0;
    };
    let mut total = 0u64;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // 剥 frontmatter
        let body = content
            .strip_prefix("---")
            .and_then(|rest| rest.find("---").map(|end| &content[3 + end + 3..]));
        total += story_ir::count_words(body.unwrap_or(&content).trim()) as u64;
    }
    total
}

// ============================================================================
// API
// ============================================================================

/// 仓库状态检测
pub fn git_status(dir: &Path) -> GitStatusDto {
    let mut st = GitStatusDto {
        available: git_available(),
        initialized: false,
        nested: false,
        dirty: false,
    };
    if !st.available {
        return st;
    }
    let Some(top) = repo_toplevel(dir) else {
        return st;
    };
    if same_path(&top, dir) {
        st.initialized = true;
        st.dirty = run_git(dir, &["status", "--porcelain"])
            .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
            .unwrap_or(false);
    } else {
        st.nested = true;
    }
    st
}

/// 初始化仓库（已初始化时是幂等空操作；嵌套仓库报错拒绝接管）
pub fn git_init(dir: &Path) -> Result<GitStatusDto, StudioError> {
    let st = git_status(dir);
    if !st.available {
        return Err(StudioError::Config(
            "系统未安装 git，无法启用版本档案".into(),
        ));
    }
    if st.nested {
        return Err(StudioError::Config(
            "项目嵌在别的 git 仓库里，为避免影响原仓库不再接管".into(),
        ));
    }
    if !st.initialized {
        run_git_ok(dir, &["init"])?;
        ensure_gitignore(dir);
        ensure_identity(dir)?;
    }
    Ok(git_status(dir))
}

/// 存档当前状态（git add -A + commit）。无变更时返回 committed=false。
pub fn git_commit(dir: &Path, message: &str) -> Result<GitCommitResult, StudioError> {
    run_git_ok(dir, &["add", "-A"])?;
    // 变更检测用 status --porcelain：对还没有任何提交的空仓库同样成立
    // （git diff --cached --quiet 在无 HEAD 时本身就会报错）
    let dirty = run_git(dir, &["status", "--porcelain"])
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false);
    if !dirty {
        return Ok(GitCommitResult {
            committed: false,
            id: None,
            detail: "没有可存档的变更".into(),
        });
    }

    ensure_identity(dir)?;
    let full = format!("{message}（全书 {} 字）", total_words(dir));
    let out = run_git(dir, &["commit", "-m", &full])?;
    if !out.status.success() {
        return Err(StudioError::Config(format!(
            "存档失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let id = run_git_ok(dir, &["rev-parse", "--short", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let detail = format!("已存档 {id}");
    Ok(GitCommitResult {
        committed: true,
        id: if id.is_empty() { None } else { Some(id) },
        detail,
    })
}

/// 存档历史（新 → 旧，最多 limit 条）
pub fn git_log(dir: &Path, limit: u32) -> Result<Vec<GitCommitDto>, StudioError> {
    // 空仓库（还没有任何提交）→ 空列表，不是错误
    let has_head = run_git(dir, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_head {
        return Ok(Vec::new());
    }
    let out = run_git_ok(
        dir,
        &[
            "log",
            "-n",
            &limit.to_string(),
            "--pretty=format:%h%x1f%at%x1f%s",
        ],
    )?;
    let mut commits = Vec::new();
    for line in out.lines() {
        let mut parts = line.split('\u{1f}');
        let (Some(id), Some(ts), Some(msg)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        commits.push(GitCommitDto {
            id: id.trim().to_string(),
            timestamp: ts.trim().parse().unwrap_or(0),
            message: msg.trim().to_string(),
        });
    }
    Ok(commits)
}

/// 恢复到指定存档。
///
/// 安全设计：先把当前状态提交为「恢复前自动备份」（无变更则跳过），
/// 再用 `read-tree -u --reset` 把工作区与索引切回目标状态（目标之后
/// 新增的文件会一并移除），最后提交一个「恢复」提交——**历史全程保留**，
/// 恢复错了还能再恢复回来。
pub fn git_restore(dir: &Path, target: &str) -> Result<GitCommitResult, StudioError> {
    // 校验目标是真实存在的提交
    run_git_ok(dir, &["cat-file", "-e", &format!("{target}^{{commit}}")])
        .map_err(|_| StudioError::NotFound(format!("存档 '{target}' 不存在")))?;

    git_commit(dir, "恢复前自动备份")?;
    run_git_ok(dir, &["read-tree", "-u", "--reset", target])?;
    git_commit(dir, &format!("恢复到存档 {target}"))
}

// ============================================================================
// diff（改动对比）
// ============================================================================

/// 单文件明细的行数上限（防止整个 diff 对话框被巨量改动淹没）
const DIFF_LINE_CAP: usize = 2000;

/// 校验提交存在 + 路径安全（仓库内相对路径，拒绝 .. 与绝对路径）
fn ensure_commit(dir: &Path, id: &str) -> Result<(), StudioError> {
    run_git_ok(dir, &["cat-file", "-e", &format!("{id}^{{commit}}")])
        .map(|_| ())
        .map_err(|_| StudioError::NotFound(format!("存档 '{id}' 不存在")))
}

fn ensure_safe_path(path: &str) -> Result<(), StudioError> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('-')
        || std::path::Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if bad {
        return Err(StudioError::Config(format!("无效的文件路径: {path}")));
    }
    Ok(())
}

/// 两个存档（或 存档→工作区，`to = None`）之间的文件级改动统计。
/// `--no-renames`：重命名表现为 删除+新增，避免解析 rename 特有的路径格式。
pub fn git_diff_summary(
    dir: &Path,
    from: &str,
    to: Option<&str>,
) -> Result<Vec<GitDiffFileDto>, StudioError> {
    ensure_commit(dir, from)?;
    if let Some(to) = to {
        ensure_commit(dir, to)?;
    }

    // 状态（A/M/D）与行数（numstat）分两趟，按路径合并
    let mut args: Vec<&str> = vec!["diff", "--no-renames", "--name-status", from];
    if let Some(to) = to {
        args.push(to);
    }
    let status_out = run_git_ok(dir, &args)?;

    let mut args: Vec<&str> = vec!["diff", "--no-renames", "--numstat", from];
    if let Some(to) = to {
        args.push(to);
    }
    let numstat_out = run_git_ok(dir, &args)?;

    let mut nums: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    for line in numstat_out.lines() {
        let mut parts = line.split('\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<u64>(), deleted.parse::<u64>()) else {
            continue; // 二进制文件是 "-"
        };
        nums.insert(path.to_string(), (added, deleted));
    }

    let mut files = Vec::new();
    for line in status_out.lines() {
        let mut parts = line.split('\t');
        let (Some(status), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        // rename 被禁用后不出现双路径形态；保险起见取最后一个字段
        let path = parts.next_back().unwrap_or(path);
        let (added, deleted) = nums.get(path).copied().unwrap_or((0, 0));
        files.push(GitDiffFileDto {
            path: path.to_string(),
            status: status.chars().take(1).collect(),
            added,
            deleted,
        });
    }
    Ok(files)
}

/// 单文件 diff 明细（统一 diff 解析为 hunk + 行）。
pub fn git_diff_file(
    dir: &Path,
    from: &str,
    to: Option<&str>,
    path: &str,
) -> Result<GitDiffDetailDto, StudioError> {
    ensure_commit(dir, from)?;
    if let Some(to) = to {
        ensure_commit(dir, to)?;
    }
    ensure_safe_path(path)?;

    let mut args: Vec<&str> = vec!["diff", "--no-renames", "-U3", from];
    if let Some(to) = to {
        args.push(to);
    }
    args.push("--");
    args.push(path);
    let out = run_git_ok(dir, &args)?;

    let mut detail = GitDiffDetailDto {
        path: path.to_string(),
        hunks: Vec::new(),
        binary: false,
        truncated: false,
    };
    let mut line_count = 0usize;
    let mut current: Option<GitDiffHunkDto> = None;

    for line in out.lines() {
        if detail.binary || detail.truncated {
            break;
        }
        if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("\\ No newline")
        {
            continue;
        }
        if line.starts_with("Binary files") {
            detail.binary = true;
            continue;
        }
        if let Some(header) = line.strip_prefix("@@") {
            let header = format!("@{header}");
            if let Some(hunk) = current.take() {
                detail.hunks.push(hunk);
            }
            current = Some(GitDiffHunkDto {
                header,
                lines: Vec::new(),
            });
            continue;
        }
        let (kind, text) = if let Some(t) = line.strip_prefix('+') {
            ("add", t)
        } else if let Some(t) = line.strip_prefix('-') {
            ("del", t)
        } else if let Some(t) = line.strip_prefix(' ') {
            ("ctx", t)
        } else {
            continue;
        };
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        hunk.lines.push(GitDiffLineDto {
            kind: kind.to_string(),
            text: text.to_string(),
        });
        line_count += 1;
        if line_count >= DIFF_LINE_CAP {
            detail.truncated = true;
        }
    }
    if let Some(hunk) = current.take() {
        detail.hunks.push(hunk);
    }
    Ok(detail)
}

// ============================================================================
// 测试（需要系统装有 git，未装则跳过）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;

    fn git_ready() -> bool {
        git_available()
    }

    #[test]
    fn init_commit_log_restore_roundtrip() {
        if !git_ready() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "git-test").unwrap();
        let dir = tmp.path();

        // 未初始化 → init
        assert!(!git_status(dir).initialized);
        let st = git_init(dir).unwrap();
        assert!(st.initialized && !st.nested);

        // 写入正文 → 存档成功，信息带字数
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n林风推开门。\n",
        )
        .unwrap();
        let r = git_commit(dir, "第一次存档").unwrap();
        assert!(r.committed, "{:?}", r);
        let first = r.id.unwrap();

        // 无变更 → 跳过
        let r = git_commit(dir, "空存档").unwrap();
        assert!(!r.committed);
        assert!(r.detail.contains("没有"));

        // 修改正文 → 恢复到第一次 → 内容回到旧版，历史保留
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n完全不同的内容。\n",
        )
        .unwrap();
        let r = git_restore(dir, &first).unwrap();
        assert!(r.committed, "{:?}", r);
        let content = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("林风推开门"), "应恢复旧内容: {content}");

        let log = git_log(dir, 50).unwrap();
        assert!(log.len() >= 3, "备份 + 原始 + 恢复提交都应在: {log:?}");
        assert!(log.iter().any(|c| c.message.contains("恢复到存档")));
        assert!(log.iter().any(|c| c.message.contains("恢复前自动备份")));
        assert!(log[0].message.contains("全书"));
    }

    #[test]
    fn diff_summary_and_detail_between_commits() {
        if !git_ready() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "diff-test").unwrap();
        let dir = tmp.path();
        git_init(dir).unwrap();

        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n第一版内容。\n",
        )
        .unwrap();
        let a = git_commit(dir, "存档A").unwrap().id.unwrap();

        // 修改 f_0001 + 新增 f_0002
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n第二版内容，改了。\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("fragments/f_0002.md"),
            "---\nid: f_0002\n---\n全新段落。\n",
        )
        .unwrap();
        let b = git_commit(dir, "存档B").unwrap().id.unwrap();

        // 统计：f_0001 修改（有增有删）、f_0002 新增
        let files = git_diff_summary(dir, &a, Some(&b)).unwrap();
        assert_eq!(files.len(), 2, "{files:?}");
        let f1 = files
            .iter()
            .find(|f| f.path.ends_with("f_0001.md"))
            .unwrap();
        assert_eq!(f1.status, "M");
        assert!(f1.added > 0 && f1.deleted > 0, "{f1:?}");
        let f2 = files
            .iter()
            .find(|f| f.path.ends_with("f_0002.md"))
            .unwrap();
        assert_eq!(f2.status, "A");

        // 明细：hunk 含 del（第一版）与 add（第二版）
        let detail = git_diff_file(dir, &a, Some(&b), "fragments/f_0001.md").unwrap();
        assert!(!detail.binary);
        assert!(!detail.hunks.is_empty(), "{detail:?}");
        let all_lines: Vec<_> = detail.hunks.iter().flat_map(|h| &h.lines).collect();
        assert!(
            all_lines
                .iter()
                .any(|l| l.kind == "add" && l.text.contains("第二版")),
            "{detail:?}"
        );
        assert!(
            all_lines
                .iter()
                .any(|l| l.kind == "del" && l.text.contains("第一版")),
            "{detail:?}"
        );

        // 不存在的提交 / 危险路径 → 报错
        assert!(git_diff_summary(dir, "deadbeef", Some(&b)).is_err());
        assert!(git_diff_file(dir, &a, Some(&b), "../outside.txt").is_err());
    }

    #[test]
    fn diff_against_working_tree() {
        if !git_ready() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "diff-wt").unwrap();
        let dir = tmp.path();
        git_init(dir).unwrap();

        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n已存档内容。\n",
        )
        .unwrap();
        git_commit(dir, "存档").unwrap();

        // 工作区再改（未存档）→ to=None 对比工作区可见
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n未存档的新内容。\n",
        )
        .unwrap();
        let head = run_git_ok(dir, &["rev-parse", "--short", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let files = git_diff_summary(dir, &head, None).unwrap();
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].status, "M");
    }

    #[test]
    fn empty_repo_log_and_commit_are_graceful() {
        if !git_ready() {
            return;
        }
        // 空目录仓库（没有任何提交）：log 返回空列表、commit 静默跳过，都不报错
        let tmp = tempfile::tempdir().unwrap();
        run_git_ok(tmp.path(), &["init"]).unwrap();
        assert!(git_log(tmp.path(), 10).unwrap().is_empty());

        let r = git_commit(tmp.path(), "空仓库存档").unwrap();
        assert!(!r.committed);
        assert!(r.detail.contains("没有"));
    }

    #[test]
    fn nested_repo_detected() {
        if !git_ready() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        // 外层仓库 + 内层项目
        run_git_ok(tmp.path(), &["init"]).unwrap();
        let proj = tmp.path().join("novel");
        std::fs::create_dir_all(&proj).unwrap();
        init_project(&proj, "nested").unwrap();

        let st = git_status(&proj);
        assert!(st.nested && !st.initialized);
        // 嵌套仓库拒绝接管
        assert!(git_init(&proj).is_err());
    }
}
