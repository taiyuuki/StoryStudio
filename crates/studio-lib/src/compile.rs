//! 编译编排 — parse → load KB → compile → 结构化结果

use std::path::Path;

use serde::{Deserialize, Serialize};

use story_compiler::{compile, CompileRequest, CompilerConfig, Diagnostics, RuleRegistry};
use story_ir::{LocationIndex, StoryIR};
use story_knowledge::KnowledgeBase;
use story_parser::{parse_project, ParseOutput};
use story_project_format::{AssertionsFile, EvidenceFile, ProjectPaths};

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
    let diagnostics = compile(
        &registry,
        &parse_output.ir,
        Some(&parse_output.location_index),
        kb.as_ref(),
        Some(&parse_output.threads),
        &request,
    );

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
