//! 数据可靠性链路集成测试 — 编辑器保存 → 快照/校验/记账/取证 全链路

use story_project_format::ProjectPaths;
use story_studio_lib::mutations;
use story_studio_lib::{ai_ledger, history, journal, stats};

fn temp_project() -> (tempfile::TempDir, ProjectPaths) {
    let dir = tempfile::tempdir().unwrap();
    story_project_format::init_project(dir.path(), "reliability-test").unwrap();
    let paths = ProjectPaths::new(dir.path());
    (dir, paths)
}

fn scene_id(paths: &ProjectPaths) -> String {
    let id = mutations::create_chapter(paths, "第一章").unwrap();
    mutations::create_scene(paths, &id, "雨夜", None).unwrap()
}

#[test]
fn manual_save_chain_creates_journal_stats_and_history() {
    let (_dir, paths) = temp_project();
    let scene = scene_id(&paths);

    // 1. 编辑器路径：创建片段 → 手写覆盖
    let fid = mutations::create_fragment(&paths, &scene, "").unwrap();
    mutations::write_fragment_text(&paths, &fid, &"夜雨敲窗，他提灯走过长巷。".repeat(30)).unwrap();

    // 码字账本：手写净增
    let s = stats::summary(paths.root(), 7);
    assert_eq!(s.today.manual, 330); // 11 字 × 30（标点不计）

    // 取证日志：两次写入都记了行
    let log = std::fs::read_to_string(journal::journal_path(paths.root())).unwrap();
    assert!(log.contains("fragment_create"));
    assert!(log.contains("fragment_write"));
    assert!(log.contains("\tok\t"));

    // 2. 大改后再次保存 → 旧内容进版本历史
    mutations::write_fragment_text(&paths, &fid, "完全不同的新内容，情节推进了一大截。").unwrap();
    let versions = history::list_versions(&paths, &fid).unwrap();
    assert_eq!(versions.len(), 1, "大改前应留一版（≥200 字变化）");
    assert!(versions[0].words > 0);

    // 3. 恢复版本 → 内容回来，且恢复前留底
    let restored = history::restore_version(&paths, &fid, versions[0].timestamp).unwrap();
    assert!(restored.contains("夜雨敲窗"));
    assert_eq!(history::list_versions(&paths, &fid).unwrap().len(), 2);
}

#[test]
fn ai_write_chain_counts_ai_words() {
    let (_dir, paths) = temp_project();
    let scene = scene_id(&paths);

    // AI 续写落库路径（save_drafts 简化模拟：anchored 创建）
    let fid = mutations::create_fragment_anchored(
        &paths,
        &scene,
        "他睁开眼，发现自己躺在陌生的客栈里。",
        &[],
    )
    .unwrap();

    let s = stats::summary(paths.root(), 7);
    assert_eq!(s.today.ai, 16);
    assert_eq!(s.today.manual, 0);

    // Agent 工具覆写 → 记 AI 净增
    mutations::write_fragment_text_source(
        &paths,
        &fid,
        "他睁开眼，发现自己躺在陌生的客栈里。窗外传来更夫的梆子声。",
        stats::WriteSource::Ai,
    )
    .unwrap();
    let s = stats::summary(paths.root(), 7);
    assert_eq!(s.today.ai, 26);
}

#[test]
fn delete_fragment_leaves_history_snapshot() {
    let (_dir, paths) = temp_project();
    let scene = scene_id(&paths);
    let fid = mutations::create_fragment(&paths, &scene, "这段文字即将被删除，但会留底。").unwrap();
    mutations::delete_fragment(&paths, &fid).unwrap();

    assert!(!paths.fragments_dir().join(format!("{fid}.md")).exists());
    // 内容仍在版本历史里，可手工找回
    let versions = history::list_versions(&paths, &fid).unwrap();
    assert_eq!(versions.len(), 1);
    let content = history::read_version(&paths, &fid, versions[0].timestamp).unwrap();
    assert!(content.contains("即将被删除"));
}

#[test]
fn ai_ledger_records_calls() {
    let (_dir, paths) = temp_project();
    ai_ledger::record(paths.root(), "续写", "anthropic", 800, 2500, true, None);
    ai_ledger::record(paths.root(), "妙笔对话", "openai", 300, 1200, false, Some("超时"));

    let s = ai_ledger::summary(paths.root(), 10);
    assert_eq!(s.total_calls, 2);
    assert_eq!(s.total_tokens, 1100);
    assert_eq!(s.recent.len(), 2);
    assert_eq!(s.recent[0].scene, "妙笔对话");
}

#[test]
fn anchor_only_change_does_not_create_version() {
    let (_dir, paths) = temp_project();
    let scene = scene_id(&paths);
    let fid = mutations::create_fragment(&paths, &scene, &"他提笔蘸墨。".repeat(20)).unwrap();

    // 只加锚点（正文不变）→ 不产生版本
    mutations::create_entity(&paths, mutations::EntityKind::Character, "沈砚之").unwrap();
    mutations::set_fragment_anchor(&paths, &fid, "c_0001", "appears").unwrap();
    assert!(history::list_versions(&paths, &fid).unwrap().is_empty());
}
