//! 编译编排 — parse → load KB → compile → 结构化结果

use std::path::Path;

use serde::{Deserialize, Serialize};

use story_compiler::{compile, CompileRequest, CompilerConfig, Diagnostics, RuleRegistry};
use story_ir::{LocationIndex, StoryIR};
use story_knowledge::KnowledgeBase;
use story_parser::{parse_project, ParseOutput};
use story_project_format::{
    AssertionsFile, EvidenceFile, ProjectPaths, ThreadsFile,
};

use crate::error::StudioError;

// ============================================================================
// CompileResult
// ============================================================================

/// 编译结果——包含解析产物 + 诊断 + 统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileResult {
    /// 解析后的 Story IR
    pub ir: StoryIR,
    /// 实体出场索引（派生缓存）
    pub location_index: LocationIndex,
    /// 编译诊断（Error / Warning）
    pub diagnostics: Diagnostics,
    /// 解析阶段的警告（非阻断）
    pub warnings: Vec<story_parser::ParseError>,
    /// 统计信息
    pub stats: CompileStats,
}

/// 编译统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompileStats {
    pub fragment_count: usize,
    pub entity_count: usize,
    pub edge_count: usize,
    pub assertion_count: usize,
    pub evidence_count: usize,
}

// ============================================================================
// API
// ============================================================================

/// 编译项目（带增量解析尝试）
///
/// 编排流程：
/// 1. 尝试增量解析（有缓存时），否则全量解析
/// 2. 加载 Knowledge Base（可选）
/// 3. 执行编译检查
/// 4. 返回结构化结果
pub fn compile_project(dir: &Path) -> Result<CompileResult, StudioError> {
    compile_impl(dir, false)
}

/// 强制全量编译（忽略增量缓存）
pub fn compile_project_full(dir: &Path) -> Result<CompileResult, StudioError> {
    compile_impl(dir, true)
}

// ============================================================================
// 内部实现
// ============================================================================

fn compile_impl(dir: &Path, force_full: bool) -> Result<CompileResult, StudioError> {
    let paths = ProjectPaths::new(dir);

    // 1. 解析
    let parse_output = if force_full {
        parse_full(dir)?
    } else {
        // 尝试增量（CLI v0.1 策略：每次重新解析）
        parse_full(dir)?
    };

    // 保存缓存（hash 快照，供未来增量用）
    save_parse_cache(&paths, &parse_output);

    // 2. 加载 KB
    let kb = load_knowledge_base(&paths);

    // 3. 编译（规则配置来自 config/compiler.toml，文件缺失/损坏退回默认）
    let compiler_config = load_compiler_config(&paths);
    let registry = RuleRegistry::new(compiler_config.clone());
    let request = CompileRequest::full_build();
    let mut diagnostics = compile(
        &registry,
        &parse_output.ir,
        Some(&parse_output.location_index),
        kb.as_ref(),
        Some(&parse_output.threads),
        &request,
    );

    // 3.5 unused_thread 卷级分级（见下方函数注释）；thread_checks 被禁用时一并跳过
    if !compiler_config.disabled.contains("thread_checks") {
        grade_unused_thread_diagnostics(&paths, &mut diagnostics);
    }

    // 4. 组装结果
    let stats = CompileStats {
        fragment_count: parse_output.ir.document.all_fragments().count(),
        entity_count: parse_output.ir.semantic.node_count(),
        edge_count: parse_output.ir.semantic.edge_count(),
        assertion_count: kb.as_ref().map(|k| k.assertion_count()).unwrap_or(0),
        evidence_count: kb.as_ref().map(|k| k.evidence_count()).unwrap_or(0),
    };

    Ok(CompileResult {
        ir: parse_output.ir,
        location_index: parse_output.location_index,
        diagnostics,
        warnings: parse_output.warnings,
        stats,
    })
}

/// 全量解析（错误归一化为 StudioError）
fn parse_full(dir: &Path) -> Result<ParseOutput, StudioError> {
    parse_project(dir).map_err(StudioError::from)
}

/// 从 config/compiler.toml 构建规则配置（RFC-0003 §5.6）。
///
/// `rules` 与 `severity_overrides` 合并（后者后写、优先级更高）；
/// 值为 "off" 时禁用整条规则，其余按 Severity 解析。非法值静默忽略——
/// 配置是作者手改的文件，写错不该阻断编译。文件不存在/解析失败 → 默认配置
/// （全部内置规则按默认级别）。
fn load_compiler_config(paths: &ProjectPaths) -> CompilerConfig {
    let path = paths.config_dir().join("compiler.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return CompilerConfig::default();
    };
    let Ok(file) = story_project_format::CompilerConfigFile::from_toml_str(&content) else {
        return CompilerConfig::default();
    };
    let mut config = CompilerConfig::default();
    for (rule_id, sev) in file.rules.into_iter().chain(file.severity_overrides) {
        if sev.eq_ignore_ascii_case("off") {
            config.disabled.insert(rule_id);
        } else if let Ok(s) = sev.parse() {
            config.severities.insert(rule_id, s);
        }
    }
    config
}

// ============================================================================
// unused_thread 卷级分级
// ============================================================================

/// 用设计层信息（threads.toml 的 due_volume + chapters.toml 的卷完稿进度）
/// 重排 compiler 的 unused_thread 诊断。
///
/// compiler 只知道结构层（structure/fragments），不该读设计层的 chapters.toml；
/// 「该卷埋没埋」的判断放编排层做：
/// - due_volume 已到（该卷已封卷仍 Planned）→ Warning「本卷该埋未埋」；
/// - due_volume 未到 → 已排期，不报警；
/// - 未设 due_volume → 全书长线，降为 Hint（消除写作期长期挂着的黄色告警）。
///
/// 封卷判定：更大卷号里存在已动笔（writing/completed）的章节，其前的卷即封卷。
/// 不能只看本卷已有章节是否全部 completed——章节随写随录入，连载到一半时
/// 本卷已录章节全部完稿是常态，据此判「卷完稿」会在写作期频繁误报
/// （更大卷里只有规划章也不算：本卷之后仍可能补章）。
fn grade_unused_thread_diagnostics(paths: &ProjectPaths, diagnostics: &mut Diagnostics) {
    use story_compiler::{Diagnostic, DiagnosticSource, Severity, SourceLocation};

    if !diagnostics
        .0
        .iter()
        .any(|d| d.code.as_str() == "unused_thread")
    {
        return;
    }

    let threads = read_toml_if_exists::<ThreadsFile>(&paths.threads_toml()).unwrap_or_default();
    // 卷号语义依赖统一卷结构（outline.toml 为唯一真相源），走迁移读取
    let chapters = crate::mutations::load_chapters_file(&paths).unwrap_or_default();

    // 封卷卷号：已动笔的最大卷号 - 1（该卷之前的全部封卷）；
    // 只立了规划章的更大卷不算——作者可能回到前卷继续补章
    let done_volume = chapters
        .chapters
        .iter()
        .filter(|c| c.status != story_project_format::STATUS_PLANNED)
        .map(|c| c.volume_order.saturating_sub(1))
        .max()
        .unwrap_or(0);

    // 摘除 compiler 的 unused_thread，按台账重排
    diagnostics
        .0
        .retain(|d| d.code.as_str() != "unused_thread");

    for t in &threads.threads {
        if t.priority != "main" || t.abandoned || t.planted_at.is_some() {
            continue;
        }
        match t.due_volume {
            Some(dv) if done_volume >= dv => {
                diagnostics.0.push(Diagnostic::new(
                    DiagnosticSource::Compiler {
                        rule_id: "thread_checks".to_string(),
                    },
                    Severity::Warning,
                    "unused_thread",
                    format!(
                        "线索 '{}' 计划第 {dv} 卷埋设，该卷已完稿仍未埋（本卷该埋未埋）",
                        t.title
                    ),
                    SourceLocation::none(),
                ));
            }
            Some(_) => {} // 已排期未到期，不算问题
            None => {
                diagnostics.0.push(Diagnostic::new(
                    DiagnosticSource::Compiler {
                        rule_id: "thread_checks".to_string(),
                    },
                    Severity::Hint,
                    "unused_thread",
                    format!(
                        "线索 '{}' 仍为 Planned（全书长线暂缓；设 due_volume 可获得卷级提醒）",
                        t.title
                    ),
                    SourceLocation::none(),
                ));
            }
        }
    }
}

fn read_toml_if_exists<T>(path: &Path) -> Option<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    toml::from_str(&content).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutations::{
        create_chapter, create_scene, create_thread, load_chapters_file, save_chapters_file,
    };
    use story_project_format::init::init_project;
    use story_project_format::ChapterOutlineDef;

    /// config/compiler.toml 生效：severity_overrides 降级、rules 关闭规则
    #[test]
    fn compiler_toml_overrides_and_disables_rules() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();

        // 制造 orphan_reference 触发条件：场景 POV 引用不存在的角色
        // （片段锚点引用缺失实体会被解析层拦成硬错误，到不了编译规则）
        let structure = r#"[book]
id = "b_root"
title = "t"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
pov = "c_missing"
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        let write_config = |toml: &str| {
            std::fs::create_dir_all(dir.join("config")).unwrap();
            std::fs::write(dir.join("config/compiler.toml"), toml).unwrap();
        };

        // 默认：orphan_pov_target 为 error
        let result = compile_project(dir).unwrap();
        let diag = result
            .diagnostics
            .iter()
            .find(|d| d.code.as_str() == "orphan_pov_target")
            .expect("默认应报 orphan_pov_target");
        assert_eq!(diag.severity.as_str(), "error");

        // severity_overrides 降级为 warning
        write_config("[severity_overrides]\norphan_reference = \"warning\"\n");
        let result = compile_project(dir).unwrap();
        let diag = result
            .diagnostics
            .iter()
            .find(|d| d.code.as_str() == "orphan_pov_target")
            .expect("降级后仍应报，只是级别变化");
        assert_eq!(diag.severity.as_str(), "warning");

        // rules 关闭规则 → 诊断消失
        write_config("[rules]\norphan_reference = \"off\"\n");
        let result = compile_project(dir).unwrap();
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.code.as_str() == "orphan_pov_target"),
            "规则关闭后不应再报"
        );
    }

    /// thread_checks 关闭时，编排层的 unused_thread 卷级分级也一并跳过
    #[test]
    fn compiler_toml_off_skips_unused_thread_grading() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 未埋设的 main 线索：默认会有 unused_thread Hint
        create_thread(&paths, "长线", "", "main", None, None, None).unwrap();
        let result = compile_project(dir).unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.as_str() == "unused_thread"),
            "默认应有 unused_thread"
        );

        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config/compiler.toml"),
            "[rules]\nthread_checks = \"off\"\n",
        )
        .unwrap();
        let result = compile_project(dir).unwrap();
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.code.as_str() == "unused_thread"),
            "thread_checks 关闭后 unused_thread 分级也应静默"
        );
    }

    fn completed_chapter(chapter_id: Option<String>, title: &str, volume_order: u32) -> ChapterOutlineDef {
        ChapterOutlineDef {
            chapter_id,
            title: title.into(),
            volume_order,
            summary: None,
            goal: None,
            beats: vec![],
            characters: vec![],
            threads: vec![],
            scenes: vec![],
            hook: None,
            notes: None,
            status: "completed".into(),
        }
    }

    #[test]
    fn unused_thread_graded_by_volume() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 到期未埋（due 1）、已排期未到期（due 2）、全书长线（无 due）、已埋设
        create_thread(&paths, "到期未埋", "", "main", None, Some(1), None).unwrap();
        create_thread(&paths, "已排期", "", "main", None, Some(2), None).unwrap();
        create_thread(&paths, "全书长线", "", "main", None, None, None).unwrap();
        let ch = create_chapter(&paths, "第一章").unwrap();
        let sc = create_scene(&paths, &ch, "场景", None).unwrap();
        create_thread(&paths, "已埋设", "", "main", Some(&sc), None, None).unwrap();

        // 作者已动笔第二卷 → 第一卷封卷
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters
            .chapters
            .push(completed_chapter(Some(ch), "第一章", 1));
        chapters.chapters.push(ChapterOutlineDef {
            chapter_id: None,
            title: "第二卷首章".into(),
            volume_order: 2,
            summary: None,
            goal: None,
            beats: vec![],
            characters: vec![],
            threads: vec![],
            scenes: vec![],
            hook: None,
            notes: None,
            status: "writing".into(),
        });
        save_chapters_file(&paths, &chapters).unwrap();

        let result = compile_project(dir).unwrap();
        let unused: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code.as_str() == "unused_thread")
            .collect();
        assert!(!unused.is_empty(), "应有 unused_thread 诊断");

        let overdue = unused
            .iter()
            .find(|d| d.message.contains("到期未埋"))
            .expect("到期线索应有诊断");
        assert_eq!(overdue.severity.as_str(), "warning");
        assert!(overdue.message.contains("本卷该埋未埋"), "{}", overdue.message);

        let longline = unused
            .iter()
            .find(|d| d.message.contains("全书长线"))
            .expect("全书长线线索应有 Hint");
        assert_eq!(longline.severity.as_str(), "hint");

        assert!(
            unused.iter().all(|d| !d.message.contains("已排期")),
            "未到期的排期线索不应报警"
        );
        assert!(
            unused.iter().all(|d| !d.message.contains("已埋设")),
            "已埋设线索不应报警"
        );
    }

    #[test]
    fn writing_volume_not_counted_as_done() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        create_thread(&paths, "写作中卷的线索", "", "main", None, Some(1), None).unwrap();
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters.push(ChapterOutlineDef {
            chapter_id: None,
            title: "第一章".into(),
            volume_order: 1,
            summary: None,
            goal: None,
            beats: vec![],
            characters: vec![],
            threads: vec![],
            scenes: vec![],
            hook: None,
            notes: None,
            status: "writing".into(),
        });
        save_chapters_file(&paths, &chapters).unwrap();

        let result = compile_project(dir).unwrap();
        let unused: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.code.as_str() == "unused_thread")
            .collect();
        // 卷还在写作中（无完稿章）：未到期，完全静默——写作期不制造告警疲劳
        assert!(
            unused
                .iter()
                .all(|d| !d.message.contains("写作中卷的线索")),
            "写作中的卷不应触发 unused_thread: {:?}",
            unused.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn completed_chapters_in_writing_volume_not_counted_as_done() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 报告中的误报场景：连载开篇，第一卷已录入章节全部完稿、
        // 后续卷尚未动笔——第一卷仍在写作中，不该报「本卷该埋未埋」
        create_thread(&paths, "连载线索", "", "main", None, Some(1), None).unwrap();
        let mut chapters = load_chapters_file(&paths).unwrap();
        for i in 1..=3 {
            chapters
                .chapters
                .push(completed_chapter(None, &format!("第{i}章"), 1));
        }
        save_chapters_file(&paths, &chapters).unwrap();

        let result = compile_project(dir).unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| !d.message.contains("连载线索")),
            "未封卷不应触发 unused_thread: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn planned_only_later_volume_not_counted_as_done() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "t").unwrap();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 更大卷里只有规划章：作者可能回到前卷补章，前卷不算封卷
        create_thread(&paths, "排期线索", "", "main", None, Some(1), None).unwrap();
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters
            .chapters
            .push(completed_chapter(None, "第一章", 1));
        chapters.chapters.push(ChapterOutlineDef {
            chapter_id: None,
            title: "第二卷规划章".into(),
            volume_order: 2,
            summary: None,
            goal: None,
            beats: vec![],
            characters: vec![],
            threads: vec![],
            scenes: vec![],
            hook: None,
            notes: None,
            status: "planned".into(),
        });
        save_chapters_file(&paths, &chapters).unwrap();

        let result = compile_project(dir).unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| !d.message.contains("排期线索")),
            "只有规划章的后续卷不应封前卷: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }
}

/// 加载 Knowledge Base（无文件时返回 None）
pub(crate) fn load_knowledge_base(paths: &ProjectPaths) -> Option<KnowledgeBase> {
    let assertions_path = paths.knowledge_dir().join("assertions.toml");
    let evidence_path = paths.knowledge_dir().join("evidence.toml");

    let has_assertions = assertions_path.exists();
    let has_evidence = evidence_path.exists();

    if !has_assertions && !has_evidence {
        return None;
    }

    let mut kb = KnowledgeBase::new();

    if has_assertions {
        if let Ok(content) = std::fs::read_to_string(&assertions_path) {
            if let Ok(file) = AssertionsFile::from_toml_str(&content) {
                if let Ok(assertions) = file.to_assertions() {
                    for a in assertions {
                        kb.insert_assertion(a);
                    }
                }
            }
        }
    }

    if has_evidence {
        if let Ok(content) = std::fs::read_to_string(&evidence_path) {
            if let Ok(file) = EvidenceFile::from_toml_str(&content) {
                for e in file.to_evidence() {
                    kb.insert_evidence(e);
                }
            }
        }
    }

    Some(kb)
}

/// 保存解析缓存（文件 hash 快照）
fn save_parse_cache(paths: &ProjectPaths, _output: &ParseOutput) {
    let cache_path = paths.cache_dir().join("parse-cache.bin");

    // 确保 .novelstudio/cache/ 存在
    let _ = std::fs::create_dir_all(paths.cache_dir());

    // v0.1：只存文件 hash，不存 IR 快照
    let (_, cache) = story_parser::detect_dirty(paths.root(), &story_parser::ParseCache::default());

    let snapshot = CacheSnapshot {
        hashes: cache
            .file_hashes
            .iter()
            .map(|(path, hash)| (path.display().to_string(), *hash))
            .collect(),
    };

    if let Ok(json) = serde_json::to_string(&snapshot) {
        let _ = std::fs::write(&cache_path, json);
    }
}

/// 缓存文件的序列化格式
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CacheSnapshot {
    hashes: Vec<(String, u64)>,
}
