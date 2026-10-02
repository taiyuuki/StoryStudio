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

    // 3. 编译
    let registry = RuleRegistry::new(CompilerConfig::default());
    let request = CompileRequest::full_build();
    let mut diagnostics = compile(
        &registry,
        &parse_output.ir,
        Some(&parse_output.location_index),
        kb.as_ref(),
        Some(&parse_output.threads),
        &request,
    );

    // 3.5 unused_thread 卷级分级（见下方函数注释）
    grade_unused_thread_diagnostics(&paths, &mut diagnostics);

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

// ============================================================================
// unused_thread 卷级分级
// ============================================================================

/// 用设计层信息（threads.toml 的 due_volume + chapters.toml 的卷完稿进度）
/// 重排 compiler 的 unused_thread 诊断。
///
/// compiler 只知道结构层（structure/fragments），不该读设计层的 chapters.toml；
/// 「该卷埋没埋」的判断放编排层做：
/// - due_volume 已到（已完稿章节的最大卷号 ≥ due_volume）仍 Planned → Warning
///   「本卷该埋未埋」；
/// - due_volume 未到 → 已排期，不报警；
/// - 未设 due_volume → 全书长线，降为 Hint（消除写作期长期挂着的黄色告警）。
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

    // 已完稿章节的最大卷号（writing 的卷不算到期——还在写）
    let done_volume = chapters
        .chapters
        .iter()
        .filter(|c| c.status == "completed")
        .map(|c| c.volume_order)
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
        create_thread(&paths, "到期未埋", "", "main", None, Some(1)).unwrap();
        create_thread(&paths, "已排期", "", "main", None, Some(2)).unwrap();
        create_thread(&paths, "全书长线", "", "main", None, None).unwrap();
        let ch = create_chapter(&paths, "第一章").unwrap();
        let sc = create_scene(&paths, &ch, "场景", None).unwrap();
        create_thread(&paths, "已埋设", "", "main", Some(&sc), None).unwrap();

        // 第一卷已完稿
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters
            .chapters
            .push(completed_chapter(Some(ch), "第一章", 1));
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

        create_thread(&paths, "写作中卷的线索", "", "main", None, Some(1)).unwrap();
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
