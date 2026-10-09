//! 增量解析 (RFC-0006 §8)
//!
//! 文件级 dirty tracking：只重解析变化的文件，局部更新 IR。
//! structure.toml 变化触发 DocumentGraph 重建（单文件，成本可接受）。
//! Fragment / Entity 变化只重解析对应文件。
//!
//! 与 RFC-0001 §10.2 的两层 + Stale 策略对齐：
//! L0：变化的文件本身立即重解析
//! L1：直接引用（如 Entity alias 变化 → 受影响 Fragment 的 Entity Linking 重算）
//! L2+：间接依赖 → 标 Stale，不立即处理

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use story_ir::{DocNode, FragmentNode, LocationIndex, StableId, StoryIR};
use story_project_format::{EntityFile, FragmentFile, ProjectPaths};

use crate::entity_linking::EntityDictionary;
use crate::error::{ParseError, ParseErrorKind, SourceLocation};
use crate::pipeline::{parse_project, ParseCache, ParseOutput};

// ============================================================================
// 增量解析结果
// ============================================================================

/// 增量解析的结果
#[derive(Debug, Serialize, Deserialize)]
pub struct IncrementalResult {
    /// 更新后的完整 IR
    pub ir: StoryIR,
    /// 更新后的 Location Index
    pub location_index: LocationIndex,
    /// 本次重解析的文件数
    pub reparsed_files: usize,
    /// 跳过的文件数（未变化）
    pub skipped_files: usize,
    /// 警告
    pub warnings: Vec<ParseError>,
}

// ============================================================================
// Dirty 检测
// ============================================================================

/// 检测哪些文件自上次解析后发生了变化
///
/// 返回 (dirty_files, all_files)，all_files 包含当前磁盘上所有文件的最新 hash
pub fn detect_dirty(project_dir: &Path, old_cache: &ParseCache) -> (BTreeSet<PathBuf>, ParseCache) {
    let paths = ProjectPaths::new(project_dir);
    let mut dirty = BTreeSet::new();
    let mut new_cache = ParseCache::default();

    // 检查所有相关文件
    let files_to_check = collect_all_file_paths(&paths);

    for path in &files_to_check {
        let hash = hash_file(path);
        if let Some(hash) = hash {
            new_cache.file_hashes.insert(path.clone(), hash);

            let changed = match old_cache.file_hashes.get(path) {
                Some(old_hash) => *old_hash != hash,
                None => true, // 新文件
            };

            if changed {
                dirty.insert(path.clone());
            }
        }
    }

    (dirty, new_cache)
}

/// 收集项目中所有应被追踪的文件路径
fn collect_all_file_paths(paths: &ProjectPaths) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // structure.toml
    files.push(paths.structure_toml());

    // Fragment 文件
    if let Ok(entries) = std::fs::read_dir(paths.fragments_dir()) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("md") {
                files.push(p);
            }
        }
    }

    // Entity 文件（characters / locations / items / organizations）
    for sub in &["characters", "locations", "items", "organizations"] {
        let dir = paths.entities_dir().join(sub);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                    files.push(p);
                }
            }
        }
    }

    files
}

/// 计算文件的内容 hash（简单用文件大小 + mtime，避免读取大文件）
fn hash_file(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    // 用 mtime + size 作为快速 dirty 检测
    // 比 hash 内容快，且对"文件被编辑了"足够准确
    if let Ok(modified) = metadata.modified() {
        if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
            duration.as_nanos().hash(&mut hasher);
        }
    }
    metadata.len().hash(&mut hasher);
    path.to_str().hash(&mut hasher); // 路径也参与，避免不同文件碰撞

    Some(hasher.finish())
}

// ============================================================================
// 增量解析入口
// ============================================================================

/// 增量解析：检测变化文件，只重解析脏文件
///
/// v0.1 策略：
/// - 如果没有任何文件变化（dirty 为空），直接返回旧 IR
/// - 如果 structure.toml 变化，全量重建（structure 是所有关系的 SoT）
/// - 只有 Fragment / Entity 变化时，增量更新
///
/// 这是一个保守策略——structure.toml 变化触发全量，但它只有一个文件，
/// 变化频率低。Fragment / Entity 是高频变化，走增量。
pub fn parse_incremental(
    project_dir: &Path,
    old_output: &ParseOutput,
    old_cache: &ParseCache,
) -> Result<IncrementalResult, Vec<ParseError>> {
    let (dirty, new_cache) = detect_dirty(project_dir, old_cache);

    let dirty_count = dirty.len();
    let total_tracked = new_cache.file_hashes.len();

    // 没有变化：直接返回
    if dirty.is_empty() {
        return Ok(IncrementalResult {
            ir: old_output.ir.clone(),
            location_index: old_output.location_index.clone(),
            reparsed_files: 0,
            skipped_files: total_tracked,
            warnings: Vec::new(),
        });
    }

    // structure.toml 变化 → 全量重建
    let structure_changed = dirty
        .iter()
        .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("structure.toml"));

    if structure_changed {
        // 全量解析，但复用 cache
        let output = parse_project(project_dir)?;
        return Ok(IncrementalResult {
            ir: output.ir,
            location_index: output.location_index,
            reparsed_files: dirty_count,
            skipped_files: total_tracked.saturating_sub(dirty_count),
            warnings: output.warnings,
        });
    }

    // Fragment / Entity 增量更新
    let paths = ProjectPaths::new(project_dir);
    let mut ir = old_output.ir.clone();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    // 分类脏文件
    let mut dirty_fragments = Vec::new();
    let mut dirty_entities = Vec::new();

    for path in &dirty {
        let is_fragment = path.extension().and_then(|e| e.to_str()) == Some("md");
        let is_entity = path.extension().and_then(|e| e.to_str()) == Some("toml")
            && path.starts_with(paths.entities_dir());

        if is_fragment {
            dirty_fragments.push(path.clone());
        } else if is_entity {
            dirty_entities.push(path.clone());
        }
    }

    // 增量更新 Fragment
    for path in &dirty_fragments {
        reparse_fragment(&mut ir, path, &mut errors, &mut warnings);
    }

    // 增量更新 Entity
    let entity_changed = !dirty_entities.is_empty();
    for path in &dirty_entities {
        reparse_entity(&mut ir, path, &mut errors, &mut warnings);
    }

    // 如果 Fragment 或 Entity 变了，重算 Entity Linking + Location Index
    if !dirty_fragments.is_empty() || entity_changed {
        let location_index = rebuild_location_index(&ir, &mut warnings);
        if errors.is_empty() {
            return Ok(IncrementalResult {
                ir,
                location_index,
                reparsed_files: dirty_count,
                skipped_files: total_tracked.saturating_sub(dirty_count),
                warnings,
            });
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(IncrementalResult {
        ir,
        location_index: old_output.location_index.clone(),
        reparsed_files: dirty_count,
        skipped_files: total_tracked.saturating_sub(dirty_count),
        warnings,
    })
}

// ============================================================================
// 单文件重解析
// ============================================================================

/// 重解析单个 Fragment 文件，更新 IR 中对应的 FragmentNode
fn reparse_fragment(
    ir: &mut StoryIR,
    path: &Path,
    errors: &mut Vec<ParseError>,
    warnings: &mut Vec<ParseError>,
) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(ParseError::new(
                ParseErrorKind::FileRead {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                SourceLocation::file(path),
            ));
            return;
        }
    };

    match FragmentFile::parse_with_filename_check(&content, &stem) {
        Ok(frag_file) => {
            let frag_id = StableId::new_unchecked(&frag_file.frontmatter.id);

            // 解析 anchors（复用 pipeline 的逻辑）
            let anchors = frag_file
                .frontmatter
                .anchors
                .iter()
                .filter_map(|a| {
                    let entity = if ir.semantic.contains(&StableId::new_unchecked(&a.entity)) {
                        StableId::new_unchecked(&a.entity)
                    } else {
                        ir.symbol_table.resolve(&a.entity).cloned()?
                    };
                    let role = match a.role.as_str() {
                        "appears" => story_ir::AnchorRole::Appears,
                        "mentioned" => story_ir::AnchorRole::Mentioned,
                        "referenced" => story_ir::AnchorRole::Referenced,
                        _ => return None,
                    };
                    Some(story_ir::EntityAnchor { entity, role })
                })
                .collect();

            // 更新或插入 FragmentNode
            ir.document.insert(DocNode::Fragment(FragmentNode {
                id: frag_id,
                scene: ir
                    .document
                    .scene_of_fragment(&StableId::new_unchecked(&stem))
                    .cloned(),
                prev: None,
                next: None,
                text: frag_file.text,
                anchors,
            }));
        }
        Err(e) => {
            let kind = match &e {
                story_project_format::FragmentFileError::IdMismatch { expected, actual } => {
                    ParseErrorKind::IdMismatch {
                        expected: expected.clone(),
                        actual: actual.clone(),
                    }
                }
                story_project_format::FragmentFileError::YamlParse(e) => {
                    ParseErrorKind::YamlParse {
                        reason: e.to_string(),
                    }
                }
                story_project_format::FragmentFileError::NoFrontmatterDelim => {
                    ParseErrorKind::YamlParse {
                        reason: "缺少 frontmatter 分隔符 '---'".to_string(),
                    }
                }
                story_project_format::FragmentFileError::InvalidRole(role) => {
                    ParseErrorKind::InvalidAnchorRole { role: role.clone() }
                }
            };
            errors.push(ParseError::new(kind, SourceLocation::file(path)));
        }
    }

    let _ = warnings; // v0.1 不在增量中产出额外 warning
}

/// 重解析单个 Entity 文件，更新 IR 中对应的 SemanticNode
fn reparse_entity(
    ir: &mut StoryIR,
    path: &Path,
    errors: &mut Vec<ParseError>,
    _warnings: &mut Vec<ParseError>,
) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(ParseError::new(
                ParseErrorKind::FileRead {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                SourceLocation::file(path),
            ));
            return;
        }
    };

    match EntityFile::from_toml_with_filename_check(&content, &stem) {
        Ok(entity_file) => {
            let stable_id = StableId::new_unchecked(&entity_file.id);
            let symbol = entity_file.symbol.as_ref().map(story_ir::Symbol::new);
            let display_name = entity_file.display_name.clone();
            let aliases = entity_file.aliases.clone();

            // 移除旧的 SymbolTable 条目（如果有）
            // v0.1 简化：直接重建 SymbolTable 中的该条目
            if let Some(ref sym) = symbol {
                ir.symbol_table.insert(sym.clone(), stable_id.clone());
            }

            // 优先根据 ID 前缀判断类型（权威信号，与 build_semantic_node 一致）
            let prefix = stable_id.as_str().split('_').next().unwrap_or("");
            let node = match prefix {
                "o" => {
                    let org = entity_file.org.as_ref();
                    story_ir::SemanticNode::Organization(story_ir::OrganizationNode {
                        id: stable_id,
                        symbol,
                        display_name,
                        aliases,
                        location: org.and_then(|o| {
                            o.location.as_ref().map(story_ir::StableId::new_unchecked)
                        }),
                        attributes: Default::default(),
                    })
                }
                "l" => {
                    let geo = entity_file.geography.as_ref();
                    story_ir::SemanticNode::Location(story_ir::LocationNode {
                        id: stable_id,
                        symbol,
                        display_name,
                        aliases,
                        parent: geo.and_then(|g| g.parent.as_ref().map(StableId::new_unchecked)),
                        kind: geo.and_then(|g| g.kind.clone()),
                        position: geo.and_then(|g| g.position.clone()),
                    })
                }
                "i" => {
                    let item = entity_file.item.as_ref();
                    story_ir::SemanticNode::Item(story_ir::ItemNode {
                        id: stable_id,
                        symbol,
                        display_name,
                        aliases,
                        kind: item.and_then(|i| i.kind.clone()),
                        grade: item.and_then(|i| i.grade.clone()),
                        unique: item.map(|i| i.unique).unwrap_or(false),
                    })
                }
                _ => story_ir::SemanticNode::Character(story_ir::CharacterNode {
                    id: stable_id,
                    symbol,
                    display_name,
                    aliases,
                    name_concealed: entity_file.is_name_concealed(),
                    attributes: Default::default(),
                }),
            };

            ir.semantic.insert(node);
        }
        Err(e) => {
            let kind = match &e {
                story_project_format::EntityFileError::IdMismatch { expected, actual } => {
                    ParseErrorKind::IdMismatch {
                        expected: expected.clone(),
                        actual: actual.clone(),
                    }
                }
                story_project_format::EntityFileError::TomlParse(e) => ParseErrorKind::TomlParse {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                story_project_format::EntityFileError::TomlSerialize(_) => unreachable!(),
            };
            errors.push(ParseError::new(kind, SourceLocation::file(path)));
        }
    }
}

/// 重建 Location Index（增量场景下全量重算，性能可接受）
fn rebuild_location_index(ir: &StoryIR, _warnings: &mut Vec<ParseError>) -> LocationIndex {
    let mut location_index = LocationIndex::new();

    // 显式 anchors
    for (_frag_id, frag) in ir.document.all_fragments() {
        for anchor in &frag.anchors {
            location_index.add(anchor.entity.clone(), frag.id.clone(), anchor.role);
        }
    }

    // 隐式 mentions（Entity Linking）
    let dict = EntityDictionary::from_semantic_graph(&ir.semantic);
    if let Some(automaton) = dict.build_automaton() {
        for (_frag_id, frag) in ir.document.all_fragments() {
            let (mentions, _ambiguities) =
                crate::entity_linking::link_in_text(&frag.text, &dict, &automaton);
            for mention in &mentions {
                location_index.add(
                    mention.entity.clone(),
                    frag.id.clone(),
                    story_ir::AnchorRole::Mentioned,
                );
            }
        }
    }

    location_index
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_test_project(dir: &Path) {
        let paths = ProjectPaths::new(dir);
        std::fs::create_dir_all(paths.fragments_dir()).unwrap();
        std::fs::create_dir_all(paths.characters_dir()).unwrap();

        std::fs::write(
            paths.structure_toml(),
            r#"
[book]
id = "b_001"
title = "测试"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
fragments = ["f_0001"]
"#,
        )
        .unwrap();

        std::fs::write(
            paths.characters_dir().join("c_001.toml"),
            "id = \"c_001\"\ndisplay_name = \"林风\"\naliases = [\"林少侠\"]\n[profile]\nrace = \"human\"",
        ).unwrap();

        std::fs::write(
            paths.fragments_dir().join("f_0001.md"),
            "---\nid: f_0001\nanchors:\n  - entity: c_001\n    role: appears\n---\n\n林风推开门。\n",
        ).unwrap();
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "story-incremental-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_change_returns_immediately() {
        let dir = temp_dir();
        create_test_project(&dir);

        // 全量解析
        let output = parse_project(&dir).unwrap();
        let (_, cache) = detect_dirty(&dir, &ParseCache::default());

        // 增量（无变化）
        let result = parse_incremental(&dir, &output, &cache).unwrap();
        assert_eq!(result.reparsed_files, 0);
        assert!(result.skipped_files > 0);
    }

    #[test]
    fn fragment_change_detected() {
        let dir = temp_dir();
        create_test_project(&dir);

        let output = parse_project(&dir).unwrap();
        let (_, cache) = detect_dirty(&dir, &ParseCache::default());

        // 修改 Fragment
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(dir.join("fragments/f_0001.md"))
            .unwrap();
        writeln!(file, "---\nid: f_0001\n---\n\n林风走了进来。\n").unwrap();
        drop(file);

        let result = parse_incremental(&dir, &output, &cache).unwrap();
        assert!(result.reparsed_files > 0);

        // 验证 IR 已更新
        let frag = result
            .ir
            .document
            .get_fragment(&StableId::new_unchecked("f_0001"))
            .unwrap();
        assert!(frag.text.contains("林风走了进来"));
    }

    #[test]
    fn structure_change_triggers_full_rebuild() {
        let dir = temp_dir();
        create_test_project(&dir);

        let output = parse_project(&dir).unwrap();
        let (_, cache) = detect_dirty(&dir, &ParseCache::default());

        // 修改 structure.toml
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(
            dir.join("structure.toml"),
            r#"
[book]
id = "b_001"
title = "修改后的书名"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
fragments = ["f_0001"]
"#,
        )
        .unwrap();

        let result = parse_incremental(&dir, &output, &cache).unwrap();
        assert!(result.reparsed_files > 0);

        // 验证全量重建后的书名
        let book = result
            .ir
            .document
            .get_book(&StableId::new_unchecked("b_001"))
            .unwrap();
        assert_eq!(book.title, "修改后的书名");
    }

    #[test]
    fn entity_change_updates_alias() {
        let dir = temp_dir();
        create_test_project(&dir);

        let output = parse_project(&dir).unwrap();
        let (_, cache) = detect_dirty(&dir, &ParseCache::default());

        // 修改 Entity 的 alias
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(
            dir.join("entities/characters/c_001.toml"),
            "id = \"c_001\"\ndisplay_name = \"林风\"\naliases = [\"风公子\"]\n[profile]\nrace = \"human\"",
        ).unwrap();

        let result = parse_incremental(&dir, &output, &cache).unwrap();
        let char = result
            .ir
            .semantic
            .get_character(&StableId::new_unchecked("c_001"))
            .unwrap();
        assert_eq!(char.aliases, vec!["风公子"]);
    }
}
