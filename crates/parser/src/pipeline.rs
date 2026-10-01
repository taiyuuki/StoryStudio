//! 解析管线 (RFC-0006 §4)
//!
//! 四阶段管线的完整实现：
//!   Phase 1:   Load         — 读文件
//!   Phase 1.5: Entity Scan  — 构建 SymbolTable + SemanticGraph
//!   Phase 2:   Structure    — structure.toml → DocumentGraph
//!   Phase 3:   Fragment     — 解析 frontmatter + anchors
//!   Phase 4:   Entity Link   — Aho-Corasick 匹配 → mentions + LocationIndex

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use story_ir::{
    AnchorRole, BookNode, ChapterNode, DocNode, EntityAnchor, FragmentNode, LocationIndex,
    SceneNode, SemanticNode, StableId, StoryCalendar, StoryIR, Symbol,
};
use story_project_format::{
    EntityFile, FragmentFile, ProjectPaths, RelationDef, RelationsFile, StructureFile,
};

use crate::entity_linking::{link_in_text, EntityDictionary};
use crate::error::{ParseError, ParseErrorKind, SourceLocation};

// ============================================================================
// 公共入口
// ============================================================================

/// Parser 的输出：IR + Location Index + Threads + 警告
#[derive(Debug, Serialize, Deserialize)]
pub struct ParseOutput {
    pub ir: StoryIR,
    pub location_index: LocationIndex,
    pub threads: story_ir::ThreadCollection,
    pub warnings: Vec<ParseError>,
}

/// 文件级缓存（增量解析用，RFC-0006 §8.1）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParseCache {
    /// 文件路径 → 内容 hash
    pub file_hashes: std::collections::BTreeMap<PathBuf, u64>,
}

/// 入口：解析整个 Project 目录
pub fn parse_project(project_dir: &Path) -> Result<ParseOutput, Vec<ParseError>> {
    parse_project_with_cache(project_dir, &ParseCache::default())
}

/// 带缓存的解析入口
pub fn parse_project_with_cache(
    project_dir: &Path,
    _cache: &ParseCache,
) -> Result<ParseOutput, Vec<ParseError>> {
    let paths = ProjectPaths::new(project_dir);
    let mut errors = Vec::new();

    // Phase 1: Load — 读取文件
    let raw = load_files(&paths, &mut errors);

    // Phase 1.5: Entity Scan — 构建 SymbolTable + SemanticGraph
    let mut ir = build_semantic_graph(&raw, &mut errors);

    // Phase 2: Structure — structure.toml → DocumentGraph
    build_document_graph(&mut ir, &raw, &mut errors);

    // Phase 3: Fragment — 解析 Fragment 文件
    fill_fragments(&mut ir, &raw, &mut errors);

    // Phase 4: Entity Linking + Location Index
    let location_index = link_and_index(&ir, &mut errors);

    // Phase 5: Threads — 加载 threads.toml（可选）
    let threads = load_threads(&paths, &mut errors);

    // 分离 errors 与 warnings
    let (hard_errors, warnings): (Vec<_>, Vec<_>) =
        errors.into_iter().partition(|e| e.kind.is_error());

    if !hard_errors.is_empty() {
        return Err(hard_errors);
    }

    Ok(ParseOutput {
        ir,
        location_index,
        threads,
        warnings,
    })
}

// ============================================================================
// 原始文件集合
// ============================================================================

/// Phase 1 读入的原始文件
#[derive(Debug, Default)]
struct RawFiles {
    structure: Option<StructureFile>,
    entities: Vec<(PathBuf, EntityFile)>,
    fragments: Vec<(PathBuf, FragmentFile)>,
    /// 角色关系边（relations.toml）
    relations: Vec<RelationDef>,
    /// 所有磁盘上存在的 Fragment id（用于孤儿检测）
    fragment_ids_on_disk: BTreeSet<String>,
}

// ============================================================================
// Phase 1: Load
// ============================================================================

fn load_files(paths: &ProjectPaths, errors: &mut Vec<ParseError>) -> RawFiles {
    let mut raw = RawFiles::default();

    // structure.toml（必须存在）
    match load_structure(paths) {
        Ok(s) => raw.structure = Some(s),
        Err(e) => errors.push(e),
    }

    // entities/**/*.toml
    load_all_entities(paths, &mut raw, errors);

    // fragments/*.md
    load_all_fragments(paths, &mut raw, errors);

    // relations.toml（可选）
    load_relations(paths, &mut raw, errors);

    raw
}

/// 读取 relations.toml（可选文件，不存在则静默跳过）
fn load_relations(paths: &ProjectPaths, raw: &mut RawFiles, errors: &mut Vec<ParseError>) {
    let path = paths.relations_toml();
    if !path.exists() {
        return;
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(ParseError::new(
                ParseErrorKind::FileRead {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                SourceLocation::file(&path),
            ));
            return;
        }
    };

    match RelationsFile::from_toml_str(&content) {
        Ok(file) => raw.relations = file.relations,
        Err(e) => {
            errors.push(ParseError::new(
                ParseErrorKind::TomlParse {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                SourceLocation::file(&path),
            ));
        }
    }
}

/// `ParseError` 因携带 `SourceLocation` + 上下文字符串体积较大；
/// 解析路径非热路径，保留值类型以换取错误处理的便利性，暂不 Box。
#[allow(clippy::result_large_err)]
fn load_structure(paths: &ProjectPaths) -> Result<StructureFile, ParseError> {
    let path = paths.structure_toml();
    let content = std::fs::read_to_string(&path).map_err(|e| {
        ParseError::new(
            ParseErrorKind::FileRead {
                file: path.display().to_string(),
                reason: e.to_string(),
            },
            SourceLocation::file(&path),
        )
    })?;

    StructureFile::from_toml_str(&content).map_err(|e| {
        ParseError::new(
            ParseErrorKind::TomlParse {
                file: path.display().to_string(),
                reason: e.to_string(),
            },
            SourceLocation::file(&path),
        )
    })
}

fn load_all_entities(paths: &ProjectPaths, raw: &mut RawFiles, errors: &mut Vec<ParseError>) {
    let entity_dirs = [
        (paths.characters_dir(), "character"),
        (paths.locations_dir(), "location"),
        (paths.items_dir(), "item"),
        (paths.organizations_dir(), "organization"),
    ];

    for (dir, _type_name) in &entity_dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }

            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(ParseError::new(
                        ParseErrorKind::FileRead {
                            file: path.display().to_string(),
                            reason: e.to_string(),
                        },
                        SourceLocation::file(&path),
                    ));
                    continue;
                }
            };

            match EntityFile::from_toml_with_filename_check(&content, stem) {
                Ok(entity) => raw.entities.push((path.clone(), entity)),
                Err(e) => {
                    let kind = match &e {
                        story_project_format::EntityFileError::IdMismatch { expected, actual } => {
                            ParseErrorKind::IdMismatch {
                                expected: expected.clone(),
                                actual: actual.clone(),
                            }
                        }
                        story_project_format::EntityFileError::TomlParse(e) => {
                            ParseErrorKind::TomlParse {
                                file: path.display().to_string(),
                                reason: e.to_string(),
                            }
                        }
                        story_project_format::EntityFileError::TomlSerialize(_) => unreachable!(),
                    };
                    errors.push(ParseError::new(kind, SourceLocation::file(&path)));
                }
            }
        }
    }
}

fn load_all_fragments(paths: &ProjectPaths, raw: &mut RawFiles, errors: &mut Vec<ParseError>) {
    let frag_dir = paths.fragments_dir();
    if !frag_dir.exists() {
        return;
    }

    let entries = match std::fs::read_dir(&frag_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(ParseError::new(
                    ParseErrorKind::FileRead {
                        file: path.display().to_string(),
                        reason: e.to_string(),
                    },
                    SourceLocation::file(&path),
                ));
                continue;
            }
        };

        match FragmentFile::parse_with_filename_check(&content, &stem) {
            Ok(frag) => {
                raw.fragment_ids_on_disk.insert(stem);
                raw.fragments.push((path, frag));
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
                errors.push(ParseError::new(kind, SourceLocation::file(&path)));
            }
        }
    }
}

// ============================================================================
// Phase 5: Threads — 加载 threads.toml
// ============================================================================

fn load_threads(
    paths: &ProjectPaths,
    warnings: &mut Vec<ParseError>,
) -> story_ir::ThreadCollection {
    let path = paths.root().join("threads.toml");
    let mut collection = story_ir::ThreadCollection::new();

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return collection, // threads.toml 是可选的
    };

    let file = match story_project_format::ThreadsFile::from_toml_str(&content) {
        Ok(f) => f,
        Err(e) => {
            warnings.push(ParseError::new(
                ParseErrorKind::TomlParse {
                    file: path.display().to_string(),
                    reason: e.to_string(),
                },
                SourceLocation::file(&path),
            ));
            return collection;
        }
    };

    for def in &file.threads {
        let thread = story_ir::Thread {
            id: StableId::new_unchecked(&def.id),
            title: def.title.clone(),
            description: def.description.clone(),
            goal: def.goal.clone(),
            priority: story_ir::ThreadPriority::from_str(&def.priority),
            tags: def.tags.clone(),
            assertions: def.assertions.iter().map(StableId::new_unchecked).collect(),
            planted_at: def.planted_at.as_ref().map(StableId::new_unchecked),
            revealed_at: def.revealed_at.as_ref().map(StableId::new_unchecked),
            resolved_at: def.resolved_at.as_ref().map(StableId::new_unchecked),
            abandoned: def.abandoned,
            resolves: def.resolves.iter().map(StableId::new_unchecked).collect(),
            resolved_by: def
                .resolved_by
                .iter()
                .map(StableId::new_unchecked)
                .collect(),
            knowledge: def
                .knowledge
                .iter()
                .map(|k| story_ir::ThreadKnowledge {
                    observer: parse_observer(&k.observer),
                    state_timeline: k
                        .state_timeline
                        .iter()
                        .map(|e| story_ir::StateTimelineEntry {
                            scene: StableId::new_unchecked(&e.scene),
                            state: story_ir::KnowledgeState::from_str(&e.state),
                        })
                        .collect(),
                })
                .collect(),
            relationships: def
                .relationships
                .iter()
                .map(|r| story_ir::ThreadRelationship {
                    entities: r.entities.iter().map(StableId::new_unchecked).collect(),
                    relation: r.relation.clone(),
                    description: r.description.clone(),
                })
                .collect(),
        };
        collection.insert(thread);
    }

    collection
}

/// 解析 ObserverDef → Observer
fn parse_observer(def: &story_project_format::threads_file::ObserverDef) -> story_ir::Observer {
    use story_project_format::threads_file::ObserverDef;
    match def {
        ObserverDef::Builtin(s) => match s.as_str() {
            "reader" => story_ir::Observer::Reader,
            "author" => story_ir::Observer::Author,
            "omniscient" => story_ir::Observer::Omniscient,
            _ => story_ir::Observer::Reader,
        },
        ObserverDef::Character { character } => story_ir::Observer::Character {
            entity: StableId::new_unchecked(character),
        },
        ObserverDef::Organization { organization } => story_ir::Observer::Organization {
            entity: StableId::new_unchecked(organization),
        },
    }
}

// ============================================================================
// Phase 1.5: Entity Scan → SemanticGraph + SymbolTable
// ============================================================================

fn build_semantic_graph(raw: &RawFiles, errors: &mut Vec<ParseError>) -> StoryIR {
    let mut ir = StoryIR::new();
    let mut seen_ids = BTreeSet::new();
    let mut seen_symbols = BTreeSet::new();

    for (path, entity_file) in &raw.entities {
        let id_str = &entity_file.id;

        // 校验 id 无重复
        if !seen_ids.insert(id_str.clone()) {
            errors.push(ParseError::new(
                ParseErrorKind::DuplicateId { id: id_str.clone() },
                SourceLocation::file(path),
            ));
            continue;
        }

        // 构建 SemanticNode
        let stable_id = StableId::new_unchecked(id_str);
        let node = build_semantic_node(entity_file, stable_id.clone());

        // Symbol 无冲突
        if let Some(ref symbol) = entity_file.symbol {
            if !seen_symbols.insert(symbol.clone()) {
                errors.push(ParseError::new(
                    ParseErrorKind::DuplicateSymbol {
                        symbol: symbol.clone(),
                    },
                    SourceLocation::file(path),
                ));
            } else {
                ir.symbol_table.insert(Symbol::new(symbol), stable_id);
            }
        }

        ir.semantic.insert(node);
    }

    // 角色关系边 → SemanticGraph（relations.toml）
    let relations_file = RelationsFile {
        relations: raw.relations.clone(),
    };
    for edge in relations_file.to_semantic_edges() {
        ir.semantic.insert_edge(edge);
    }

    ir
}

/// 从 EntityFile 构建 SemanticNode
fn build_semantic_node(file: &EntityFile, id: StableId) -> SemanticNode {
    let symbol = file.symbol.as_ref().map(Symbol::new);
    let display_name = file.display_name.clone();
    let aliases = file.aliases.clone();

    // 优先根据 ID 前缀判断类型（权威信号，与 EntityKind::from_id 一致）
    // 没有专属 section 的实体（如 Organization）只能靠前缀区分。
    let prefix = id.as_str().split('_').next().unwrap_or("");
    match prefix {
        "o" => {
            let org = file.org.as_ref();
            return SemanticNode::Organization(story_ir::OrganizationNode {
                id,
                symbol,
                display_name,
                aliases,
                location: org.and_then(|o| o.location.as_ref().map(StableId::new_unchecked)),
                attributes: org_attributes(file),
            });
        }
        "l" => {
            let geo = file.geography.as_ref();
            return SemanticNode::Location(story_ir::LocationNode {
                id,
                symbol,
                display_name,
                aliases,
                parent: geo.and_then(|g| g.parent.as_ref().map(StableId::new_unchecked)),
                kind: geo.and_then(|g| g.kind.clone()),
                position: geo.and_then(|g| g.position.clone()),
            });
        }
        "i" => {
            let item = file.item.as_ref();
            return SemanticNode::Item(story_ir::ItemNode {
                id,
                symbol,
                display_name,
                aliases,
                kind: item.and_then(|i| i.kind.clone()),
                grade: item.and_then(|i| i.grade.clone()),
                unique: item.map(|i| i.unique).unwrap_or(false),
            });
        }
        _ => {}
    }

    // 默认：作为 Character
    SemanticNode::Character(story_ir::CharacterNode {
        id,
        symbol,
        display_name,
        aliases,
        attributes: character_attributes(file),
    })
}

/// 角色档案关键字段平铺进 attributes——正文角色卡与 AI 上下文靠它拿身份信息
fn character_attributes(file: &EntityFile) -> story_ir::EntityAttributes {
    let mut attrs = story_ir::EntityAttributes::default();
    if let Some(p) = &file.profile {
        let mut put = |k: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                attrs
                    .custom
                    .insert(k.to_string(), story_ir::AttributeValue::String(v.to_string()));
            }
        };
        put("职业", &p.occupation);
        put("性别", &p.gender);
        put("性格", &p.personality);
        put("背景", &p.background);
        put("种族", &p.race);
    }
    attrs
}

/// 组织关键信息平铺进 attributes
fn org_attributes(file: &EntityFile) -> story_ir::EntityAttributes {
    let mut attrs = story_ir::EntityAttributes::default();
    if let Some(o) = &file.org {
        for (k, v) in [("类型", &o.kind), ("理念", &o.ideology), ("简介", &o.summary)] {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                attrs
                    .custom
                    .insert(k.to_string(), story_ir::AttributeValue::String(v.to_string()));
            }
        }
    }
    attrs
}

// ============================================================================
// Phase 2: Structure → DocumentGraph
// ============================================================================

fn build_document_graph(ir: &mut StoryIR, raw: &RawFiles, errors: &mut Vec<ParseError>) {
    let structure = match &raw.structure {
        Some(s) => s,
        None => return, // structure.toml 缺失，错误已在 Phase 1 报
    };

    // Book
    let book_id = StableId::new_unchecked(&structure.book.id);
    ir.document.insert(DocNode::Book(BookNode {
        id: book_id.clone(),
        title: structure.book.title.clone(),
        symbol: structure.book.symbol.as_ref().map(Symbol::new),
    }));

    // Chapter 列表
    let chapters = &structure.book.chapters;
    let mut seen_chapter_ids = BTreeSet::new();

    for (i, ch_id_str) in chapters.iter().enumerate() {
        if !seen_chapter_ids.insert(ch_id_str.clone()) {
            errors.push(ParseError::new(
                ParseErrorKind::DuplicateId {
                    id: ch_id_str.clone(),
                },
                SourceLocation::default(),
            ));
            continue;
        }

        let ch_id = StableId::new_unchecked(ch_id_str);

        // 查找 Chapter 定义
        let ch_def = match structure.chapter.get(ch_id_str) {
            Some(d) => d,
            None => {
                errors.push(ParseError::new(
                    ParseErrorKind::DanglingReference {
                        reference: format!("book.chapters[{i}]"),
                        target_type: "Chapter".to_string(),
                        target_id: ch_id_str.clone(),
                    },
                    SourceLocation::default(),
                ));
                continue;
            }
        };

        // 链表前后
        let prev = if i > 0 {
            Some(StableId::new_unchecked(&chapters[i - 1]))
        } else {
            None
        };
        let next = if i < chapters.len() - 1 {
            Some(StableId::new_unchecked(&chapters[i + 1]))
        } else {
            None
        };

        ir.document.insert(DocNode::Chapter(ChapterNode {
            id: ch_id.clone(),
            title: ch_def.title.clone(),
            symbol: ch_def.symbol.as_ref().map(Symbol::new),
            prev,
            next,
            story_time_span: None, // v0.1 暂不解析 chapter time_span
        }));

        // 建立 Book → Chapter 的 contains 关系
        ir.document.set_children(
            book_id.clone(),
            chapters.iter().map(StableId::new_unchecked).collect(),
        );

        // Scene 列表
        let scenes = &ch_def.scenes;
        let mut seen_scene_ids = BTreeSet::new();

        for (j, sc_id_str) in scenes.iter().enumerate() {
            if !seen_scene_ids.insert(sc_id_str.clone()) {
                errors.push(ParseError::new(
                    ParseErrorKind::DuplicateId {
                        id: sc_id_str.clone(),
                    },
                    SourceLocation::default(),
                ));
                continue;
            }

            let sc_id = StableId::new_unchecked(sc_id_str);

            let sc_def = match structure.scene.get(sc_id_str) {
                Some(d) => d,
                None => {
                    errors.push(ParseError::new(
                        ParseErrorKind::DanglingReference {
                            reference: format!("chapter.{ch_id_str}.scenes[{j}]"),
                            target_type: "Scene".to_string(),
                            target_id: sc_id_str.clone(),
                        },
                        SourceLocation::default(),
                    ));
                    continue;
                }
            };

            let sc_prev = if j > 0 {
                Some(StableId::new_unchecked(&scenes[j - 1]))
            } else {
                None
            };
            let sc_next = if j < scenes.len() - 1 {
                Some(StableId::new_unchecked(&scenes[j + 1]))
            } else {
                None
            };

            // 空 Scene 检查
            if sc_def.fragments.is_empty() {
                errors.push(ParseError::new(
                    ParseErrorKind::EmptyScene {
                        scene_id: sc_id_str.clone(),
                    },
                    SourceLocation::default(),
                ));
            }

            ir.document.insert(DocNode::Scene(SceneNode {
                id: sc_id.clone(),
                chapter: ch_id.clone(),
                title: sc_def.title.clone(),
                symbol: None,
                prev: sc_prev,
                next: sc_next,
                pov: sc_def.pov.as_ref().map(StableId::new_unchecked),
                story_time: sc_def.story_time.as_ref().and_then(|t| {
                    t.year.map(|year| StoryCalendar {
                        year,
                        month: t.month,
                        day: t.day,
                    })
                }),
                story_time_anchor: sc_def
                    .story_time
                    .as_ref()
                    .and_then(|t| t.anchor.clone()),
                story_time_gap: sc_def
                    .story_time
                    .as_ref()
                    .and_then(|t| t.gap.clone()),
                mood: sc_def.mood.clone(),
                function: sc_def.function.clone(),
                summary: sc_def.summary.clone(),
            }));

            ir.document.set_children(
                sc_id.clone(),
                sc_def
                    .fragments
                    .iter()
                    .map(StableId::new_unchecked)
                    .collect(),
            );
        }

        // Chapter → Scene 的 contains
        ir.document.set_children(
            ch_id.clone(),
            scenes.iter().map(StableId::new_unchecked).collect(),
        );
    }
}

// ============================================================================
// Phase 3: Fragment — 填充正文与 anchors
// ============================================================================

fn fill_fragments(ir: &mut StoryIR, raw: &RawFiles, errors: &mut Vec<ParseError>) {
    let structure = match &raw.structure {
        Some(s) => s,
        None => return,
    };

    // 收集 structure.toml 中引用的所有 Fragment id
    let mut referenced_fragments: BTreeSet<String> = BTreeSet::new();
    for scene_def in structure.scene.values() {
        for frag_id in &scene_def.fragments {
            referenced_fragments.insert(frag_id.clone());
        }
    }

    // 处理磁盘上的每个 Fragment 文件
    let mut seen_frag_ids = BTreeSet::new();
    for (path, frag_file) in &raw.fragments {
        let frag_id_str = &frag_file.frontmatter.id;

        if !seen_frag_ids.insert(frag_id_str.clone()) {
            errors.push(ParseError::new(
                ParseErrorKind::DuplicateId {
                    id: frag_id_str.clone(),
                },
                SourceLocation::file(path),
            ));
            continue;
        }

        // 查找此 Fragment 所属的 Scene（从 structure.toml 反向查找）
        let scene_id_str = structure
            .scene
            .iter()
            .find(|(_, def)| def.fragments.contains(frag_id_str))
            .map(|(sc_id, _)| sc_id.clone());

        let scene_id = scene_id_str.map(|s| StableId::new_unchecked(&s));

        // 构建 FragmentNode
        let mut anchors = Vec::new();
        for anchor_entry in &frag_file.frontmatter.anchors {
            let entity_id = resolve_entity_id(&anchor_entry.entity, ir);
            let role = match anchor_entry.role.as_str() {
                "appears" => AnchorRole::Appears,
                "mentioned" => AnchorRole::Mentioned,
                "referenced" => AnchorRole::Referenced,
                _ => {
                    errors.push(ParseError::new(
                        ParseErrorKind::InvalidAnchorRole {
                            role: anchor_entry.role.clone(),
                        },
                        SourceLocation::file(path),
                    ));
                    continue;
                }
            };

            match &entity_id {
                Some(id) => anchors.push(EntityAnchor {
                    entity: id.clone(),
                    role,
                }),
                None => {
                    errors.push(ParseError::new(
                        ParseErrorKind::DanglingReference {
                            reference: format!("fragment.{frag_id_str}.anchors"),
                            target_type: "Entity".to_string(),
                            target_id: anchor_entry.entity.clone(),
                        },
                        SourceLocation::file(path),
                    ));
                }
            }
        }

        let frag_id = StableId::new_unchecked(frag_id_str);
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: frag_id,
            scene: scene_id,
            prev: None, // 链表由 Parser 在下面构建
            next: None,
            text: frag_file.text.clone(),
            anchors,
        }));
    }

    // 构建 Fragment 的 prev/next 链表（从 Scene.fragments 数组）
    build_fragment_links(ir, structure);

    // 检查孤儿 Fragment（磁盘存在但未被 structure 引用）
    for (path, frag_file) in &raw.fragments {
        if !referenced_fragments.contains(&frag_file.frontmatter.id) {
            errors.push(ParseError::new(
                ParseErrorKind::OrphanFragment {
                    fragment_id: frag_file.frontmatter.id.clone(),
                },
                SourceLocation::file(path),
            ));
        }
    }
}

/// 解析 entity 引用（StableId 或 Symbol）
fn resolve_entity_id(reference: &str, ir: &StoryIR) -> Option<StableId> {
    // 先尝试作为 StableId 直接查找
    if let Ok(id) = StableId::new(reference) {
        if ir.semantic.contains(&id) {
            return Some(id);
        }
    }
    // 再尝试作为 Symbol 查找
    ir.symbol_table.resolve(reference).cloned()
}

/// 构建 Fragment 链表（从 Scene.fragments 数组顺序）
fn build_fragment_links(ir: &mut StoryIR, structure: &StructureFile) {
    for (sc_id_str, scene_def) in &structure.scene {
        let fragments = &scene_def.fragments;
        let sc_id = StableId::new_unchecked(sc_id_str);

        for (i, frag_id_str) in fragments.iter().enumerate() {
            let frag_id = StableId::new_unchecked(frag_id_str);
            let prev = if i > 0 {
                Some(StableId::new_unchecked(&fragments[i - 1]))
            } else {
                None
            };
            let next = if i < fragments.len() - 1 {
                Some(StableId::new_unchecked(&fragments[i + 1]))
            } else {
                None
            };

            if let Some(DocNode::Fragment(frag)) = ir.document.get_mut(&frag_id) {
                frag.prev = prev;
                frag.next = next;
                frag.scene = Some(sc_id.clone());
            }
        }
    }
}

// ============================================================================
// Phase 4: Entity Linking + Location Index
// ============================================================================

fn link_and_index(ir: &StoryIR, errors: &mut Vec<ParseError>) -> LocationIndex {
    let mut location_index = LocationIndex::new();

    // 1. 显式 anchors → Location Index
    for (_frag_id, frag) in ir.document.all_fragments() {
        for anchor in &frag.anchors {
            location_index.add(anchor.entity.clone(), frag.id.clone(), anchor.role);
        }
    }

    // 2. 隐式 mentions（Entity Linking）
    let dict = EntityDictionary::from_semantic_graph(&ir.semantic);
    if let Some(automaton) = dict.build_automaton() {
        for (_frag_id, frag) in ir.document.all_fragments() {
            let (mentions, ambiguities) = link_in_text(&frag.text, &dict, &automaton);

            for mention in &mentions {
                location_index.add(
                    mention.entity.clone(),
                    frag.id.clone(),
                    AnchorRole::Mentioned,
                );
            }

            // 歧义 → Warning
            for amb_text in ambiguities {
                let entities = dict
                    .lookup(&amb_text)
                    .iter()
                    .map(|id| id.to_string())
                    .collect();
                errors.push(ParseError::new(
                    ParseErrorKind::AmbiguousMatch {
                        text: amb_text,
                        entities,
                    },
                    SourceLocation::fragment(frag.id.as_str()),
                ));
            }
        }
    }

    location_index
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建完整的测试项目
    fn create_test_project(dir: &Path) {
        let paths = ProjectPaths::new(dir);

        // 创建目录
        std::fs::create_dir_all(paths.fragments_dir()).unwrap();
        std::fs::create_dir_all(paths.characters_dir()).unwrap();

        // structure.toml
        let structure = r#"
[book]
id = "b_0001"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
pov = "c_001"
fragments = ["f_0001", "f_0002"]
"#;
        std::fs::write(paths.structure_toml(), structure).unwrap();

        // Character
        let char_toml = r#"
id = "c_001"
display_name = "林风"
aliases = ["林少侠"]

[profile]
race = "human"
"#;
        std::fs::write(paths.characters_dir().join("c_001.toml"), char_toml).unwrap();

        // Fragment 1
        let frag1 = "---\nid: f_0001\nanchors:\n  - entity: c_001\n    role: appears\n---\n\n林风推开门。\n";
        std::fs::write(paths.fragments_dir().join("f_0001.md"), frag1).unwrap();

        // Fragment 2
        let frag2 = "---\nid: f_0002\n---\n\n林少侠走了过来。\n";
        std::fs::write(paths.fragments_dir().join("f_0002.md"), frag2).unwrap();
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "story-parser-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_full_project() {
        let dir = temp_dir();
        create_test_project(&dir);

        let output = parse_project(&dir).unwrap();

        // Book
        assert_eq!(
            output
                .ir
                .document
                .get_book(&StableId::new_unchecked("b_0001"))
                .unwrap()
                .title,
            "测试小说"
        );

        // Character
        let char_node = output
            .ir
            .semantic
            .get_character(&StableId::new_unchecked("c_001"))
            .unwrap();
        assert_eq!(char_node.display_name, "林风");
        assert_eq!(char_node.aliases, vec!["林少侠"]);
        // [profile] 关键字段平铺进 attributes——正文角色卡靠它拿身份信息
        assert_eq!(
            char_node.attributes.custom.get("种族"),
            Some(&story_ir::AttributeValue::String("human".to_string()))
        );

        // SymbolTable：此测试的 Character 没有 symbol，所以表为空
        assert!(output.ir.symbol_table.is_empty());

        // Fragment 1 有显式 anchor
        let frag1 = output
            .ir
            .document
            .get_fragment(&StableId::new_unchecked("f_0001"))
            .unwrap();
        assert_eq!(frag1.anchors.len(), 1);
        assert_eq!(frag1.anchors[0].entity, StableId::new_unchecked("c_001"));
        assert_eq!(frag1.anchors[0].role, AnchorRole::Appears);

        // Fragment 链表
        assert_eq!(frag1.prev, None);
        assert_eq!(frag1.next, Some(StableId::new_unchecked("f_0002")));
        assert_eq!(frag1.scene, Some(StableId::new_unchecked("sc_01_01")));

        let frag2 = output
            .ir
            .document
            .get_fragment(&StableId::new_unchecked("f_0002"))
            .unwrap();
        assert_eq!(frag2.prev, Some(StableId::new_unchecked("f_0001")));
        assert_eq!(frag2.next, None);

        // Location Index：显式 anchor + 隐式 mention
        // f_0001: anchor(c_001, appears) + mention(林风)
        // f_0002: mention(林少侠)
        let appearances = output
            .location_index
            .appearances_of(&StableId::new_unchecked("c_001"));
        assert!(appearances.len() >= 2); // 至少 f_0001(anchor) + f_0002(mention)

        // 无错误
        let hard_errors: Vec<_> = output
            .warnings
            .iter()
            .filter(|w| w.kind.is_error())
            .collect();
        assert!(hard_errors.is_empty());
    }

    #[test]
    fn parse_detects_orphan_fragment() {
        let dir = temp_dir();
        create_test_project(&dir);

        // 添加一个未被 structure 引用的孤儿 Fragment
        let orphan = "---\nid: f_9999\n---\n\n孤儿片段\n";
        std::fs::write(dir.join("fragments/f_9999.md"), orphan).unwrap();

        let output = parse_project(&dir).unwrap(); // 孤儿是 Warning，不阻断

        let orphan_warning = output.warnings.iter()
            .find(|w| matches!(&w.kind, ParseErrorKind::OrphanFragment { fragment_id } if fragment_id == "f_9999"));
        assert!(orphan_warning.is_some());
    }

    #[test]
    fn parse_detects_dangling_reference() {
        let dir = temp_dir();
        create_test_project(&dir);

        // structure 引用不存在的 Scene
        let bad_structure = r#"
[book]
id = "b_0001"
title = "测试"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_nonexistent"]
"#;
        std::fs::write(dir.join("structure.toml"), bad_structure).unwrap();

        let result = parse_project(&dir);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| matches!(
            &e.kind,
            ParseErrorKind::DanglingReference {
                target_type,
                ..
            } if target_type == "Scene"
        )));
    }

    #[test]
    fn parse_detects_id_mismatch_not_duplicate() {
        // 文件名=id 的约束（RFC-0003 §5.3）使得同一目录下无法有两个同 id 的文件。
        // 如果 c_002.toml 内含 id="c_001"，文件名校验会先拦截（IdMismatch），
        // 而非到达 DuplicateId 检测。这是正确行为——文件名校验是第一道防线。
        let dir = temp_dir();
        create_test_project(&dir);

        // c_002.toml 的 id 写成 c_001 → 文件名校验失败
        std::fs::write(
            dir.join("entities/characters/c_002.toml"),
            "id = \"c_001\"\ndisplay_name = \"另一个\"\n[profile]\nrace = \"human\"\n",
        )
        .unwrap();

        let result = parse_project(&dir);
        assert!(result.is_err());

        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| matches!(
            &e.kind,
            ParseErrorKind::IdMismatch { expected, actual } if expected == "c_002" && actual == "c_001"
        )));
    }
}
