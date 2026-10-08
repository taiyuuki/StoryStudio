//! 共享变更层 — 所有项目文件写入的唯一入口
//!
//! 设计目标：消除 Agent tools 和 commands 之间的平行实现，
//! 所有结构性变更（创建/删除/重命名）都经过本模块。
//!
//! 级联删除策略：
//! - 硬级联（拥有关系）：删 chapter → 连带删其下 scene + fragment 文件；
//!   删 scene → 连带删其下 fragment 文件。
//! - 半级联（关系边引用）：删 entity → 连带清理 relations.toml 中以该实体为
//!   from 或 to 的关系边，避免悬空关系。
//! - 软级联（其他引用）：断言、POV、锚点等引用不会清理，
//!   由 compiler 诊断（orphan_assertion / orphan_reference）暴露。

use std::path::Path;

use serde::Serialize;
use story_project_format::knowledge_file::{AssertionDef, EvidenceDef, EvidenceFile};
use story_project_format::structure_toml::StoryTimeDef;
use story_project_format::{
    is_valid_status, AnchoringFile, AssertionsFile, ChapterDef, ChapterOutlineDef, ChaptersFile,
    EntityFile, FragmentFile, OutlineFile, ProjectPaths, RelationDef, RelationsFile, SceneDef,
    StructureFile, ThreadsFile, UnresolvedRefDef,
};

use crate::error::StudioError;

// ============================================================================
// 辅助宏/函数：加载或创建默认
// ============================================================================

/// 从 TOML 文件加载，文件不存在或为空则返回 Default。
trait LoadOrDefault: Sized {
    fn load_or_default(path: &Path) -> Result<Self, StudioError>;
}

impl LoadOrDefault for StructureFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for AssertionsFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for EvidenceFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for ThreadsFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for OutlineFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for RelationsFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl LoadOrDefault for AnchoringFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

fn load_toml<T>(path: &Path) -> Result<T, StudioError>
where
    T: for<'de> serde::Deserialize<'de> + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }
    toml::from_str(&content)
        .map_err(|e| StudioError::Config(format!("解析 {} 失败: {e}", path.display())))
}

/// 写入 TOML 文件，先序列化再写入。
trait SaveToml {
    fn save(&self, path: &Path) -> Result<(), StudioError>;
}

impl SaveToml for StructureFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 structure.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for AssertionsFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 assertions.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for EvidenceFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 evidence.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for ThreadsFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 threads.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for OutlineFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 outline.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl LoadOrDefault for ChaptersFile {
    fn load_or_default(path: &Path) -> Result<Self, StudioError> {
        load_toml(path)
    }
}

impl SaveToml for ChaptersFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 chapters.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for RelationsFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 relations.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

impl SaveToml for AnchoringFile {
    fn save(&self, path: &Path) -> Result<(), StudioError> {
        let s = self
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 anchoring.toml 失败: {e}")))?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

/// 读取并解析 structure.toml（必须存在）
fn load_structure(paths: &ProjectPaths) -> Result<StructureFile, StudioError> {
    let path = paths.structure_toml();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StudioError::Io(format!("读取 structure.toml 失败: {e}")))?;
    StructureFile::from_toml_str(&content)
        .map_err(|e| StudioError::Config(format!("解析 structure.toml 失败: {e}")))
}

/// 读取 chapters.toml，文件不存在或为空时返回 default。
/// 卷的唯一真相源在 outline.toml：读取时顺带完成遗留卷副本的合并迁移。
pub fn load_chapters_file(paths: &ProjectPaths) -> Result<ChaptersFile, StudioError> {
    let (file, _) = story_project_format::load_chapters_unified(paths);
    Ok(file)
}

/// 将 ChaptersFile 写回 chapters.toml。
pub fn save_chapters_file(paths: &ProjectPaths, file: &ChaptersFile) -> Result<(), StudioError> {
    file.save(&paths.chapters_toml())
}

/// 章节收尾提示（写正文回执用）：scene/fragment 所属章节的全部场景
/// 都已有正文片段、且章节脉络状态未到 completed 时，返回提示行。
///
/// 场景"有正文"以 fragments 列表非空为准（与 parse.empty_scene 同口径）。
pub fn chapter_completion_hint(
    paths: &ProjectPaths,
    scene_or_fragment_id: &str,
) -> Option<String> {
    let scene_id = if scene_or_fragment_id.starts_with("f_") {
        scene_of_fragment_id(paths, scene_or_fragment_id)?
    } else {
        scene_or_fragment_id.to_string()
    };
    let structure = load_structure(paths).ok()?;
    // 定位场景所属章节
    let (ch_id, chapter) = structure
        .chapter
        .iter()
        .find(|(_, ch)| ch.scenes.iter().any(|s| s == &scene_id))?;
    if chapter.scenes.is_empty() {
        return None;
    }
    let all_written = chapter.scenes.iter().all(|s| {
        structure
            .scene
            .get(s)
            .map(|sc| !sc.fragments.is_empty())
            .unwrap_or(false)
    });
    if !all_written {
        return None;
    }
    let chapters = ChaptersFile::load_or_default(&paths.chapters_toml()).ok()?;
    let index = chapters
        .chapters
        .iter()
        .position(|c| c.chapter_id.as_deref() == Some(ch_id))?;
    if chapters.chapters[index].status == story_project_format::chapters_file::STATUS_COMPLETED {
        return None;
    }
    Some(format!(
        "\n💡 本章 {}「{}」的所有场景均已有正文，可用 update_chapter_outline(index={}, status=\"completed\") 标记本章完成",
        ch_id,
        chapters.chapters[index].title,
        index + 1
    ))
}

// ---------------------------------------------------------------------------
// 章节脉络变更（逐章生文流水线用；index 为 1-based，与
// tools/chapters.rs 中 get_chapter_outline 展示的编号一致）
// ---------------------------------------------------------------------------

fn chapter_outline_at(
    file: &mut ChaptersFile,
    index: usize,
) -> Result<&mut ChapterOutlineDef, StudioError> {
    let slot = index
        .checked_sub(1)
        .filter(|i| *i < file.chapters.len())
        .ok_or_else(|| {
            StudioError::NotFound(format!(
                "章节脉络 index={index} 不存在（共 {} 章）",
                file.chapters.len()
            ))
        })?;
    Ok(&mut file.chapters[slot])
}

/// 推进单章脉络状态（planned→writing→completed，单向）。
///
/// 返回推进后的状态。
pub fn set_chapter_outline_status(
    paths: &ProjectPaths,
    index: usize,
    status: &str,
) -> Result<String, StudioError> {
    if !is_valid_status(status) {
        return Err(StudioError::Config(format!(
            "status 必须是 planned/writing/completed，收到 '{status}'"
        )));
    }
    let order = |s: &str| -> u8 {
        match s {
            "planned" => 0,
            "writing" => 1,
            _ => 2,
        }
    };

    let mut file = load_chapters_file(paths)?;
    let item = chapter_outline_at(&mut file, index)?;
    if order(status) < order(&item.status) {
        return Err(StudioError::Config(format!(
            "状态只能单向推进，当前为 {}，不能回退到 {}",
            item.status, status
        )));
    }
    item.status = status.to_string();
    save_chapters_file(paths, &file)?;
    Ok(status.to_string())
}

/// 回填章节脉络的 chapter_id（对齐 structure.toml）。
///
/// 空字符串清除回填。
pub fn backfill_chapter_outline_id(
    paths: &ProjectPaths,
    index: usize,
    chapter_id: &str,
) -> Result<(), StudioError> {
    let mut file = load_chapters_file(paths)?;
    let item = chapter_outline_at(&mut file, index)?;
    item.chapter_id = if chapter_id.is_empty() {
        None
    } else {
        Some(chapter_id.to_string())
    };
    save_chapters_file(paths, &file)?;
    Ok(())
}

/// 整体替换单章脉络的节拍（beats）。
pub fn update_chapter_outline_beats(
    paths: &ProjectPaths,
    index: usize,
    beats: Vec<String>,
) -> Result<(), StudioError> {
    let mut file = load_chapters_file(paths)?;
    let item = chapter_outline_at(&mut file, index)?;
    item.beats = beats;
    save_chapters_file(paths, &file)?;
    Ok(())
}

/// 追加单章脉络已落实的场景 ID（scenes）。
pub fn append_chapter_outline_scene(
    paths: &ProjectPaths,
    index: usize,
    scene_id: &str,
) -> Result<(), StudioError> {
    let mut file = load_chapters_file(paths)?;
    let item = chapter_outline_at(&mut file, index)?;
    if !item.scenes.iter().any(|s| s == scene_id) {
        item.scenes.push(scene_id.to_string());
    }
    save_chapters_file(paths, &file)?;
    Ok(())
}

// ============================================================================
// ID 生成（集中化，保持各前缀的格式约定）
// ============================================================================

/// 扫描目录中的 `prefix_XXXX.ext` 文件，返回最大十六进制编号
fn find_max_id_hex(dir: &Path, prefix: &str, ext: &str) -> u32 {
    let mut max_num = 0u32;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(num_part) = name.strip_prefix(prefix).and_then(|s| s.strip_suffix(ext)) {
                if let Ok(num) = u32::from_str_radix(num_part, 16) {
                    max_num = max_num.max(num);
                }
            }
        }
    }
    max_num
}

/// 生成下一个 entity ID（角色 c_/地点 l_/道具 i_/组织 o_，4位十六进制）。
///
/// 除目录扫描外，同时把知识层与历史引用（assertions/threads/relations 及
/// 高水位文件）纳入：已删除实体的 ID 若被复用，原先指向它的断言/锚点会
/// 静默改绑到语义完全不同的新实体上——既不报错也无提示。ID 只增不减。
fn next_entity_id(paths: &ProjectPaths, dir: &Path, prefix: &str) -> String {
    let scan_prefix = format!("{prefix}_");
    let mut max = find_max_id_hex(dir, &scan_prefix, ".toml");

    // 知识层引用里的同前缀 ID（悬空引用也是"已分配"）
    for file in [
        paths.knowledge_dir().join("assertions.toml"),
        paths.threads_toml(),
        paths.relations_toml(),
    ] {
        if let Ok(content) = std::fs::read_to_string(&file) {
            max = max.max(max_ref_of_prefix(&content, &scan_prefix));
        }
    }

    // 持久高水位（每前缀一行）
    let watermark_path = paths.root().join(".novelstudio").join("entity_id_watermark");
    if let Ok(content) = std::fs::read_to_string(&watermark_path) {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix(&scan_prefix) {
                if let Ok(n) = u32::from_str_radix(value.trim(), 16) {
                    max = max.max(n);
                }
            }
        }
    }

    let next = max + 1;
    let _ = std::fs::create_dir_all(paths.root().join(".novelstudio"));
    // 读改写（同进程串行安全；跨进程由文件锁外的事务性保证近似）
    let mut lines: Vec<String> = std::fs::read_to_string(&watermark_path)
        .map(|c| c.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let entry = format!("{scan_prefix}{next:04X}");
    match lines.iter_mut().find(|l| l.starts_with(&scan_prefix)) {
        Some(slot) => *slot = entry.clone(),
        None => lines.push(entry.clone()),
    }
    let _ = std::fs::write(&watermark_path, lines.join("\n"));

    entry
}

/// 从文本中提取指定前缀（如 l_）十六进制 ID 的最大序号
fn max_ref_of_prefix(content: &str, scan_prefix: &str) -> u32 {
    let prefix_str = scan_prefix.to_string();
    let mut max = 0u32;
    let bytes = content.as_bytes();
    let prefix_bytes = prefix_str.as_bytes();
    let mut i = 0usize;
    while i + prefix_bytes.len() <= bytes.len() {
        if &bytes[i..i + prefix_bytes.len()] == prefix_bytes {
            let start = i + prefix_bytes.len();
            let mut j = start;
            while j < bytes.len() && (bytes[j] as char).is_ascii_hexdigit() {
                j += 1;
            }
            if (3..=6).contains(&(j - start)) {
                if let Ok(n) = u32::from_str_radix(&content[start..j], 16) {
                    max = max.max(n);
                }
            }
            i = j.max(i + prefix_bytes.len());
        } else {
            i += 1;
        }
    }
    max
}

/// 生成下一个 fragment ID（f_，4位十六进制）。
///
/// 除片段目录外，同时把知识层引用（assertions/evidence 中的 f_XXXX）
/// 纳入高水位：已被引用过的 ID 即使文件删除也不回收——否则新片段复用
/// 该 ID 时，外部引用会静默指向全新内容，编译器无从报警。
pub(crate) fn next_fragment_id(paths: &ProjectPaths) -> String {
    let mut max = find_max_id_hex(&paths.fragments_dir(), "f_", ".md");
    for file in ["assertions.toml", "evidence.toml"] {
        if let Ok(content) = std::fs::read_to_string(paths.knowledge_dir().join(file)) {
            max = max.max(max_fragment_ref(&content));
        }
    }

    // 持久高水位：无引用的片段删除后 ID 也不回收，杜绝外部笔记/历史
    // 记录静默指向新内容（目录与知识层引用都看不到已删除的 ID）
    let watermark_path = paths
        .root()
        .join(".novelstudio")
        .join("fragment_id_watermark");
    if let Ok(w) = std::fs::read_to_string(&watermark_path) {
        if let Ok(n) = u32::from_str_radix(w.trim(), 16) {
            max = max.max(n);
        }
    }

    let next = max + 1;
    let _ = std::fs::create_dir_all(paths.root().join(".novelstudio"));
    let _ = std::fs::write(&watermark_path, format!("{next:04X}"));

    format!("f_{next:04X}")
}

/// 从文本中提取 f_XXXX（十六进制）引用的最大序号。
fn max_fragment_ref(content: &str) -> u32 {
    let bytes = content.as_bytes();
    let mut max = 0u32;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'f' && bytes[i + 1] == b'_' {
            let start = i + 2;
            let mut j = start;
            while j < bytes.len() && (bytes[j] as char).is_ascii_hexdigit() {
                j += 1;
            }
            // 4~6 位十六进制视为 ID（过短/过长的是普通文本）
            if (4..=6).contains(&(j - start)) {
                if let Ok(n) = u32::from_str_radix(&content[start..j], 16) {
                    max = max.max(n);
                }
            }
            i = j.max(i + 2);
        } else {
            i += 1;
        }
    }
    max
}

// ============================================================================
// Entity 变更
// ============================================================================

/// 实体类型（用于路由到正确的子目录）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Character,
    Location,
    Item,
    Organization,
}

impl EntityKind {
    pub fn id_prefix(&self) -> &'static str {
        match self {
            EntityKind::Character => "c",
            EntityKind::Location => "l",
            EntityKind::Item => "i",
            EntityKind::Organization => "o",
        }
    }

    pub fn dir(&self, paths: &ProjectPaths) -> std::path::PathBuf {
        match self {
            EntityKind::Character => paths.characters_dir(),
            EntityKind::Location => paths.locations_dir(),
            EntityKind::Item => paths.items_dir(),
            EntityKind::Organization => paths.organizations_dir(),
        }
    }

    /// 实体类型中文名（提示与错误文案用）
    pub fn type_name_zh(&self) -> &'static str {
        match self {
            EntityKind::Character => "角色",
            EntityKind::Location => "地点",
            EntityKind::Item => "道具",
            EntityKind::Organization => "组织",
        }
    }

    pub fn from_type_str(s: &str) -> Result<Self, StudioError> {
        match s {
            "characters" | "character" => Ok(EntityKind::Character),
            "locations" | "location" => Ok(EntityKind::Location),
            "items" | "item" => Ok(EntityKind::Item),
            "organizations" | "organization" => Ok(EntityKind::Organization),
            other => Err(StudioError::Config(format!("未知实体类型: {other}"))),
        }
    }

    pub fn type_str(&self) -> &'static str {
        match self {
            EntityKind::Character => "characters",
            EntityKind::Location => "locations",
            EntityKind::Item => "items",
            EntityKind::Organization => "organizations",
        }
    }

    /// 根据实体 ID 的前缀推断类型
    pub fn from_id(id: &str) -> Result<Self, StudioError> {
        let prefix = id.split('_').next().unwrap_or("");
        match prefix {
            "c" => Ok(EntityKind::Character),
            "l" => Ok(EntityKind::Location),
            "i" => Ok(EntityKind::Item),
            "o" => Ok(EntityKind::Organization),
            _ => Err(StudioError::Config(format!(
                "无法从 ID '{id}' 推断实体类型"
            ))),
        }
    }
}

/// 创建实体
///
/// 生成新 ID，写入 entities/<subdir>/<id>.toml。
/// 返回新实体的 ID。
pub fn create_entity(
    paths: &ProjectPaths,
    kind: EntityKind,
    name: &str,
) -> Result<String, StudioError> {
    let dir = kind.dir(paths);
    std::fs::create_dir_all(&dir)?;

    let prefix = kind.id_prefix();
    let new_id = next_entity_id(paths, &dir, prefix);

    let entity = match kind {
        EntityKind::Character => EntityFile::new_character(&new_id, name),
        EntityKind::Location => EntityFile::new_location(&new_id, name),
        EntityKind::Item => EntityFile::new_item(&new_id, name),
        EntityKind::Organization => EntityFile::new_organization(&new_id, name),
    };

    let toml_content = entity
        .to_toml_string()
        .map_err(|e| StudioError::Config(format!("序列化实体失败: {e}")))?;
    let file_path = dir.join(format!("{new_id}.toml"));
    std::fs::write(&file_path, toml_content)?;

    Ok(new_id)
}

/// 角色档案的可选字段集（create 时一步建档用，仅 Character）。
///
/// 供 resolve_unresolved_ref 的 create 分支在建档时直接带入，
/// 免去"建档 → 再 update_entity 补档案"的两步。
#[derive(Debug, Clone, Default)]
pub struct CharacterProfileInput {
    pub gender: Option<String>,
    pub occupation: Option<String>,
    pub appearance: Option<String>,
    pub background: Option<String>,
    pub personality: Option<String>,
    pub habits: Option<String>,
}

impl CharacterProfileInput {
    pub fn is_empty(&self) -> bool {
        self.gender.is_none()
            && self.occupation.is_none()
            && self.appearance.is_none()
            && self.background.is_none()
            && self.personality.is_none()
            && self.habits.is_none()
    }
}

/// 将档案字段合并进已存在的角色实体（create_entity 只建空档案）。
///
/// 只覆盖提供的字段，与 update_entity 的合并语义一致。
fn apply_character_profile(
    paths: &ProjectPaths,
    entity_id: &str,
    profile: &CharacterProfileInput,
) -> Result<(), StudioError> {
    let path = paths.characters_dir().join(format!("{entity_id}.toml"));
    let content = std::fs::read_to_string(&path)?;
    let mut entity =
        EntityFile::from_toml_str(&content).map_err(|e| StudioError::Config(format!("解析实体失败: {e}")))?;
    {
        let p = entity.profile.get_or_insert_with(Default::default);
        if let Some(v) = &profile.gender {
            p.gender = Some(v.clone());
        }
        if let Some(v) = &profile.occupation {
            p.occupation = Some(v.clone());
        }
        if let Some(v) = &profile.appearance {
            p.appearance = Some(v.clone());
        }
        if let Some(v) = &profile.background {
            p.background = Some(v.clone());
        }
        if let Some(v) = &profile.personality {
            p.personality = Some(v.clone());
        }
        if let Some(v) = &profile.habits {
            p.habits = Some(v.clone());
        }
    }
    let toml_content = entity
        .to_toml_string()
        .map_err(|e| StudioError::Config(format!("序列化实体失败: {e}")))?;
    std::fs::write(&path, toml_content)?;
    Ok(())
}

/// 删除实体
///
/// 删除实体自己的 .toml 文件，并清理 relations.toml 中以该实体为端点
/// （from 或 to）的关系边，避免悬空关系。
/// 断言、POV、锚点等其他引用不会清理——由 compiler 诊断暴露悬空引用。
pub fn delete_entity(paths: &ProjectPaths, entity_id: &str) -> Result<(), StudioError> {
    let kind = EntityKind::from_id(entity_id)?;
    let dir = kind.dir(paths);
    let file_path = dir.join(format!("{entity_id}.toml"));

    if !file_path.exists() {
        return Err(StudioError::NotFound(format!("实体 '{entity_id}' 不存在")));
    }

    // 先清理 relations.toml 中引用该实体的关系边（半级联）。
    cleanup_relations_for_entity(paths, entity_id)?;

    std::fs::remove_file(&file_path)?;
    Ok(())
}

/// 清理 relations.toml 中以 `entity_id` 为端点（from 或 to）的所有关系边。
///
/// 若没有任何边命中则不写入文件。返回被移除的边数。
fn cleanup_relations_for_entity(
    paths: &ProjectPaths,
    entity_id: &str,
) -> Result<usize, StudioError> {
    let relations_path = paths.relations_toml();
    let mut file: RelationsFile = RelationsFile::load_or_default(&relations_path)?;

    let before = file.relations.len();
    file.relations
        .retain(|r| r.from != entity_id && r.to != entity_id);
    let removed = before - file.relations.len();

    // 只有发生变更时才回写
    if removed > 0 {
        file.save(&relations_path)?;
    }

    Ok(removed)
}

// ============================================================================
// 实体合并
// ============================================================================

/// merge_entities 的重定向统计（回执展示用）
#[derive(Debug, Default, Clone, Serialize)]
pub struct MergeReport {
    /// 重定向的片段锚点数
    pub anchors: usize,
    /// 重定向的关系端点数（去重/删自环前）
    pub relation_edges: usize,
    /// 合并后删除的自环边数
    pub self_loops_removed: usize,
    /// 重定向的断言 subject 数
    pub assertions: usize,
    /// 重定向的场景 POV 数
    pub povs: usize,
    /// 章节脉络 characters 列表中替换的章数
    pub chapter_character_lists: usize,
    /// 线索观察者/关系成员重定向数
    pub thread_refs: usize,
    /// outline 弧线角色重定向数
    pub outline_refs: usize,
    /// 未锚定指代台账 target 重定向数
    pub unresolved_targets: usize,
    /// 吸收进目标档案的别名数
    pub aliases_absorbed: usize,
}

impl MergeReport {
    /// 生成给调用方的回执摘要
    pub fn summary(&self, from_id: &str, into_id: &str) -> String {
        let mut parts = vec![format!(
            "实体 {from_id} 已合并到 {into_id}（原档案名与别名已转入目标档案的别名）"
        )];
        if self.anchors > 0 {
            parts.push(format!("锚点 {} 处", self.anchors));
        }
        if self.relation_edges > 0 {
            let mut s = format!("关系边 {} 条", self.relation_edges);
            if self.self_loops_removed > 0 {
                s.push_str(&format!("（合并产生自环，删除 {} 条）", self.self_loops_removed));
            }
            parts.push(s);
        }
        if self.assertions > 0 {
            parts.push(format!("断言 {} 条", self.assertions));
        }
        if self.povs > 0 {
            parts.push(format!("场景 POV {} 处", self.povs));
        }
        if self.chapter_character_lists > 0 {
            parts.push(format!("章节脉络出场表 {} 章", self.chapter_character_lists));
        }
        if self.thread_refs > 0 {
            parts.push(format!("线索观察者/关系成员 {} 处", self.thread_refs));
        }
        if self.outline_refs > 0 {
            parts.push(format!("角色弧线 {} 条", self.outline_refs));
        }
        if self.unresolved_targets > 0 {
            parts.push(format!("未锚定指代归拢目标 {} 条", self.unresolved_targets));
        }
        if self.aliases_absorbed > 0 {
            parts.push(format!("吸收别名 {} 个", self.aliases_absorbed));
        }
        parts.join("，")
    }
}

/// 锚点 role 的去重优先级：appears > mentioned > referenced
fn anchor_role_rank(role: &str) -> u8 {
    match role {
        "appears" => 3,
        "mentioned" => 2,
        _ => 1,
    }
}

/// 合并实体：把 `from_id` 的一切引用重定向到 `into_id`，然后删除 from。
///
/// 典型场景：匿名占位实体（如「九一真凶」）在真名揭晓后并入正式角色档案。
/// 约束：两个实体必须同类型；from 的档案名与别名转入 into 的别名
/// （from 的其他档案字段不合并，由 compiler 的空段提示人工核对）。
///
/// 重定向范围：片段锚点 / relations / 断言 subject / 场景 POV /
/// 章节脉络 characters / 线索观察者与关系成员 / outline 弧线角色 /
/// 未锚定指代台账 target。
pub fn merge_entities(
    paths: &ProjectPaths,
    from_id: &str,
    into_id: &str,
) -> Result<MergeReport, StudioError> {
    if from_id == into_id {
        return Err(StudioError::Config("合并的两个实体不能相同".to_string()));
    }
    let from_kind = EntityKind::from_id(from_id)?;
    let into_kind = EntityKind::from_id(into_id)?;
    if from_kind != into_kind {
        return Err(StudioError::Config(format!(
            "跨类型实体不能合并（{from_id} 是 {from_kind:?}，{into_id} 是 {into_kind:?}）"
        )));
    }
    let from_path = paths.file_for_id(from_id).unwrap_or_else(|| {
        Path::new("/nonexistent").to_path_buf()
    });
    let into_path = paths
        .file_for_id(into_id)
        .ok_or_else(|| StudioError::Config(format!("无效的实体 ID: {into_id}")))?;
    if !from_path.exists() {
        return Err(StudioError::NotFound(format!("实体 '{from_id}' 不存在")));
    }
    if !into_path.exists() {
        return Err(StudioError::NotFound(format!("实体 '{into_id}' 不存在")));
    }

    let mut report = MergeReport::default();

    // 1. 名称吸收：from 的 display_name + aliases 并入 into 的 aliases
    let from_content = std::fs::read_to_string(&from_path)?;
    let into_content = std::fs::read_to_string(&into_path)?;
    let from_entity = EntityFile::from_toml_str(&from_content)
        .map_err(|e| StudioError::Config(format!("解析 {from_id} 失败: {e}")))?;
    let mut into_entity = EntityFile::from_toml_str(&into_content)
        .map_err(|e| StudioError::Config(format!("解析 {into_id} 失败: {e}")))?;
    for name in std::iter::once(&from_entity.display_name).chain(from_entity.aliases.iter()) {
        if name != &into_entity.display_name
            && !name.is_empty()
            && !into_entity.aliases.iter().any(|a| a == name)
        {
            into_entity.aliases.push(name.clone());
            report.aliases_absorbed += 1;
        }
    }
    if report.aliases_absorbed > 0 {
        let toml_content = into_entity
            .to_toml_string()
            .map_err(|e| StudioError::Config(format!("序列化 {into_id} 失败: {e}")))?;
        std::fs::write(&into_path, toml_content)?;
    }

    // 2. 片段锚点重定向（同实体去重，保留高优先级 role）
    if let Ok(entries) = std::fs::read_dir(paths.fragments_dir()) {
        for entry in entries.filter_map(|e| e.ok()) {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(".md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(mut fragment) = FragmentFile::parse(&content) else {
                continue;
            };
            let mut changed = false;
            for anchor in &mut fragment.frontmatter.anchors {
                if anchor.entity == from_id {
                    anchor.entity = into_id.to_string();
                    report.anchors += 1;
                    changed = true;
                }
            }
            if changed {
                // 同实体去重：保留 role 优先级高的条目
                let mut merged: Vec<story_project_format::fragment_file::AnchorEntry> =
                    Vec::new();
                for anchor in fragment.frontmatter.anchors {
                    match merged.iter_mut().find(|a| a.entity == anchor.entity) {
                        Some(existing) => {
                            if anchor_role_rank(&anchor.role) > anchor_role_rank(&existing.role) {
                                existing.role = anchor.role;
                            }
                        }
                        None => merged.push(anchor),
                    }
                }
                fragment.frontmatter.anchors = merged;
                crate::journal::write_verified(
                    paths.root(),
                    "entity_merge",
                    &entry.path(),
                    &fragment.to_markdown(),
                )?;
            }
        }
    }

    // 3. relations：端点重定向 + 删自环 + 去重
    let relations_path = paths.relations_toml();
    let mut relations = RelationsFile::load_or_default(&relations_path)?;
    for r in &mut relations.relations {
        let mut hit = false;
        if r.from == from_id {
            r.from = into_id.to_string();
            hit = true;
        }
        if r.to == from_id {
            r.to = into_id.to_string();
            hit = true;
        }
        if hit {
            report.relation_edges += 1;
        }
    }
    let before_dedup = relations.relations.len();
    relations.relations.retain(|r| r.from != r.to);
    report.self_loops_removed = before_dedup - relations.relations.len();
    // 按 (from, to, relation) 去重（保序，保留首个）
    let mut seen = std::collections::BTreeSet::new();
    relations.relations.retain(|r| {
        seen.insert((r.from.clone(), r.to.clone(), r.relation.clone()))
    });
    if report.relation_edges > 0 || report.self_loops_removed > 0 {
        relations.save(&relations_path)?;
    }

    // 4. 断言 subject / object（object 为实体引用的谓词，如 holds/member_of/located_at）
    let assertions_path = paths.assertions_toml();
    let mut assertions = AssertionsFile::load_or_default(&assertions_path)?;
    let mut assertion_changed = false;
    for a in &mut assertions.assertions {
        let mut hit = false;
        if a.subject == from_id {
            a.subject = into_id.to_string();
            hit = true;
        }
        if matches!(&a.object, toml::Value::String(s) if s == from_id) {
            a.object = toml::Value::String(into_id.to_string());
            hit = true;
        }
        if hit {
            report.assertions += 1;
            assertion_changed = true;
        }
    }
    if assertion_changed {
        assertions.save(&assertions_path)?;
    }

    // 5. 场景 POV
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;
    let mut structure_changed = false;
    for scene in structure.scene.values_mut() {
        if scene.pov.as_deref() == Some(from_id) {
            scene.pov = Some(into_id.to_string());
            report.povs += 1;
            structure_changed = true;
        }
    }
    if structure_changed {
        structure.save(&structure_path)?;
    }

    // 6. 章节脉络 characters
    let mut chapters = load_chapters_file(paths)?;
    let mut chapters_changed = false;
    for ch in chapters.chapters.iter_mut() {
        if ch.characters.iter().any(|c| c == from_id) {
            ch.characters.retain(|c| c != from_id);
            if !ch.characters.iter().any(|c| c == into_id) {
                ch.characters.push(into_id.to_string());
            }
            report.chapter_character_lists += 1;
            chapters_changed = true;
        }
    }
    if chapters_changed {
        save_chapters_file(paths, &chapters)?;
    }

    // 7. 线索观察者 + 关系成员
    let threads_path = paths.threads_toml();
    let mut threads = ThreadsFile::load_or_default(&threads_path)?;
    let mut threads_changed = false;
    for t in threads.threads.iter_mut() {
        for k in &mut t.knowledge {
            let hit = match &mut k.observer {
                story_project_format::threads_file::ObserverDef::Character { character }
                    if character == from_id =>
                {
                    *character = into_id.to_string();
                    true
                }
                story_project_format::threads_file::ObserverDef::Organization { organization }
                    if organization == from_id =>
                {
                    *organization = into_id.to_string();
                    true
                }
                _ => false,
            };
            if hit {
                report.thread_refs += 1;
                threads_changed = true;
            }
        }
        for rel in &mut t.relationships {
            for e in &mut rel.entities {
                if *e == from_id {
                    *e = into_id.to_string();
                    report.thread_refs += 1;
                    threads_changed = true;
                }
            }
            rel.entities.dedup();
        }
    }
    if threads_changed {
        threads.save(&threads_path)?;
    }

    // 8. outline 弧线角色
    let outline_path = paths.outline_toml();
    let mut outline = OutlineFile::load_or_default(&outline_path)?;
    let mut outline_changed = false;
    for arc in outline.arcs.iter_mut() {
        if arc.character == from_id {
            arc.character = into_id.to_string();
            report.outline_refs += 1;
            outline_changed = true;
        }
    }
    if outline_changed {
        outline.save(&outline_path)?;
    }

    // 9. 未锚定指代台账 target
    let anchoring_path = paths.anchoring_toml();
    let mut anchoring = AnchoringFile::load_or_default(&anchoring_path)?;
    let mut anchoring_changed = false;
    for r in &mut anchoring.refs {
        if r.target.as_deref() == Some(from_id) {
            r.target = Some(into_id.to_string());
            report.unresolved_targets += 1;
            anchoring_changed = true;
        }
    }
    if anchoring_changed {
        anchoring.save(&anchoring_path)?;
    }

    // 10. 删除 from 实体文件
    std::fs::remove_file(&from_path)?;

    Ok(report)
}

// ============================================================================
// Structure 变更（Chapter / Scene / Fragment）
// ============================================================================

/// 创建章节
///
/// 生成 ch_XX ID，追加到 book.chapters 数组并写入 chapter 定义。
/// 若 chapters.toml 中存在同名的待写脉络（chapter_id 为空），自动回填
/// chapter_id，避免脉络与结构两套 ID 脱钩。
/// 返回新章节 ID。
pub fn create_chapter(paths: &ProjectPaths, title: &str) -> Result<String, StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let max_num = structure
        .book
        .chapters
        .iter()
        .filter_map(|ch| ch.strip_prefix("ch_").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);

    let new_id = format!("ch_{:02}", max_num + 1);
    structure.book.chapters.push(new_id.clone());
    structure.chapter.insert(
        new_id.clone(),
        ChapterDef {
            title: title.to_string(),
            symbol: None,
            scenes: Vec::new(),
        },
    );

    structure.save(&structure_path)?;

    // 按标题自动关联章节脉络（同名且未回填 chapter_id 的待写章）
    link_chapter_outline_by_title(paths, &new_id, title);

    Ok(new_id)
}

/// 标题归一化：去空白、去全半角标点、去"第X章"前缀，用于模糊匹配。
fn normalize_chapter_title(title: &str) -> String {
    let stripped = title.trim();
    // 去"第X章"前缀（X 为数字或中文数字）
    let body = if let Some(rest) = stripped.strip_prefix('第') {
        let chars: Vec<char> = rest.chars().collect();
        let digits: &[char] = &[
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '〇', '零', '一', '二', '三', '四',
            '五', '六', '七', '八', '九', '十', '百', '千',
        ];
        let mut i = 0;
        while i < chars.len() && digits.contains(&chars[i]) {
            i += 1;
        }
        if i < chars.len() && (chars[i] == '章' || chars[i] == '回') {
            chars[i + 1..].iter().collect::<String>()
        } else {
            stripped.to_string()
        }
    } else {
        stripped.to_string()
    };
    body.chars()
        .filter(|c| !c.is_whitespace() && !" 　·.。:：，,、-—《》\"'「」『』".contains(*c))
        .collect()
}

/// 在 chapters.toml 中找标题匹配且未回填 chapter_id 的脉络，回填之。
fn link_chapter_outline_by_title(paths: &ProjectPaths, chapter_id: &str, title: &str) {
    let Ok(mut file) = load_chapters_file(paths) else {
        return;
    };
    if file.chapters.is_empty() {
        return;
    }
    let normalized = normalize_chapter_title(title);
    if normalized.is_empty() {
        return;
    }
    let matched = file.chapters.iter().position(|c| {
        c.chapter_id.is_none() && normalize_chapter_title(&c.title) == normalized
    });
    if let Some(index) = matched {
        file.chapters[index].chapter_id = Some(chapter_id.to_string());
        let _ = save_chapters_file(paths, &file);
    }
}

/// 重命名章节
pub fn rename_chapter(
    paths: &ProjectPaths,
    chapter_id: &str,
    new_title: &str,
) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let chapter = structure
        .chapter
        .get_mut(chapter_id)
        .ok_or_else(|| StudioError::NotFound(format!("章节 '{chapter_id}' 不存在")))?;
    chapter.title = new_title.to_string();

    structure.save(&structure_path)?;
    Ok(())
}

/// 删除章节（硬级联）
///
/// 从 book.chapters 移除 + 删除 chapter 条目。
/// 级联删除其下所有 scene 及 scene 下的 fragment 文件。
pub fn delete_chapter(paths: &ProjectPaths, chapter_id: &str) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let chapter = structure
        .chapter
        .remove(chapter_id)
        .ok_or_else(|| StudioError::NotFound(format!("章节 '{chapter_id}' 不存在")))?;

    structure.book.chapters.retain(|ch| ch != chapter_id);

    // 级联删除 scene 和 fragment 文件
    for scene_id in &chapter.scenes {
        cascade_delete_scene_files(paths, &mut structure, scene_id);
    }

    structure.save(&structure_path)?;
    Ok(())
}

/// 创建场景
///
/// 生成 sc_XX_YY ID（章节号内递增），追加到 chapter.scenes 并写入 scene 定义。
/// 返回新场景 ID。
pub fn create_scene(
    paths: &ProjectPaths,
    chapter_id: &str,
    title: &str,
    pov: Option<&str>,
) -> Result<String, StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let ch_num = chapter_id.strip_prefix("ch_").unwrap_or("01");

    let chapter = structure
        .chapter
        .get_mut(chapter_id)
        .ok_or_else(|| StudioError::NotFound(format!("章节 '{chapter_id}' 不存在")))?;

    let max_num = chapter
        .scenes
        .iter()
        .filter_map(|s| {
            s.strip_prefix(&format!("sc_{ch_num}_"))
                .and_then(|n| n.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0);

    let new_id = format!("sc_{ch_num}_{:02}", max_num + 1);
    chapter.scenes.push(new_id.clone());

    let scene_def = SceneDef {
        title: Some(title.to_string()),
        pov: pov.map(|p| p.to_string()),
        ..SceneDef::default()
    };
    structure.scene.insert(new_id.clone(), scene_def);

    structure.save(&structure_path)?;
    Ok(new_id)
}

/// 重命名场景
pub fn rename_scene(
    paths: &ProjectPaths,
    scene_id: &str,
    new_title: &str,
) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;
    scene.title = Some(new_title.to_string());

    structure.save(&structure_path)?;
    Ok(())
}

/// 更新章节属性（标题、symbol）
pub fn update_chapter(
    paths: &ProjectPaths,
    chapter_id: &str,
    title: Option<&str>,
    symbol: Option<&str>,
) -> Result<Vec<&'static str>, StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let chapter = structure
        .chapter
        .get_mut(chapter_id)
        .ok_or_else(|| StudioError::NotFound(format!("章节 '{chapter_id}' 不存在")))?;

    let mut changes: Vec<&'static str> = Vec::new();
    if let Some(t) = title {
        chapter.title = t.to_string();
        changes.push("title");
    }
    if let Some(s) = symbol {
        chapter.symbol = if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
        changes.push("symbol");
    }

    if changes.is_empty() {
        return Ok(changes);
    }
    structure.save(&structure_path)?;
    Ok(changes)
}

/// 场景可更新字段的集合（patch 式更新，None = 不修改该字段）
pub struct SceneUpdate<'a> {
    pub title: Option<&'a str>,
    pub pov: Option<Option<&'a str>>,
    pub mood: Option<Option<&'a str>>,
    pub function: Option<Option<&'a str>>,
    pub summary: Option<Option<&'a str>>,
    pub story_time: Option<Option<StoryTimeDef>>,
}

/// 更新场景属性（标题、pov、mood、function、story_time）
///
/// 每个字段使用 `Option<Option<T>>` 语义：
/// - 外层 None = 不修改
/// - `Some(None)` = 清空该字段
/// - `Some(Some(v))` = 设为新值
pub fn update_scene(
    paths: &ProjectPaths,
    scene_id: &str,
    update: &SceneUpdate,
) -> Result<Vec<&'static str>, StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;

    let mut changes: Vec<&'static str> = Vec::new();
    if let Some(t) = update.title {
        scene.title = Some(t.to_string());
        changes.push("title");
    }
    if let Some(pov) = update.pov {
        scene.pov = pov.map(|s| s.to_string());
        changes.push("pov");
    }
    if let Some(mood) = update.mood {
        scene.mood = mood.map(|s| s.to_string());
        changes.push("mood");
    }
    if let Some(func) = update.function {
        scene.function = func.map(|s| s.to_string());
        changes.push("function");
    }
    if let Some(summary) = update.summary {
        scene.summary = summary.map(|s| s.to_string());
        changes.push("summary");
    }
    if let Some(st) = &update.story_time {
        scene.story_time = st.clone();
        changes.push("story_time");
    }

    if changes.is_empty() {
        return Ok(changes);
    }
    structure.save(&structure_path)?;
    Ok(changes)
}

/// 删除场景（硬级联）
///
/// 从所属 chapter.scenes 移除 + 删除 scene 条目。
/// 级联删除其下所有 fragment 文件。
pub fn delete_scene(paths: &ProjectPaths, scene_id: &str) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    // 确认 scene 存在
    if !structure.scene.contains_key(scene_id) {
        return Err(StudioError::NotFound(format!("场景 '{scene_id}' 不存在")));
    }

    // 从所属 chapter.scenes 移除
    for chapter in structure.chapter.values_mut() {
        chapter.scenes.retain(|s| s != scene_id);
    }

    cascade_delete_scene_files(paths, &mut structure, scene_id);

    structure.save(&structure_path)?;
    Ok(())
}

/// 内部：删除 scene 条目 + 其下所有 fragment 文件（不改 structure 的 chapters/scenes 数组）
fn cascade_delete_scene_files(paths: &ProjectPaths, structure: &mut StructureFile, scene_id: &str) {
    if let Some(scene) = structure.scene.remove(scene_id) {
        let frag_dir = paths.fragments_dir();
        for frag_id in &scene.fragments {
            let frag_path = frag_dir.join(format!("{frag_id}.md"));
            std::fs::remove_file(&frag_path).ok();
        }
    }
}

// ============================================================================
// Fragment 变更
// ============================================================================

/// 创建新片段
///
/// 生成 f_XXXX ID，写入 fragments/<id>.md（含 frontmatter），
/// 并追加到 scene.fragments 数组。
/// 返回新片段 ID。
pub fn create_fragment(
    paths: &ProjectPaths,
    scene_id: &str,
    text: &str,
) -> Result<String, StudioError> {
    let fragments_dir = paths.fragments_dir();
    std::fs::create_dir_all(&fragments_dir)?;

    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;

    let new_id = next_fragment_id(paths);
    let frag_path = fragments_dir.join(format!("{new_id}.md"));

    let file_content = render_fragment_file(&new_id, text);
    crate::journal::write_verified(paths.root(), "fragment_create", &frag_path, &file_content)?;
    crate::stats::record_change(paths.root(), None, text, crate::stats::WriteSource::Manual);

    if !scene.fragments.contains(&new_id) {
        scene.fragments.push(new_id.clone());
        structure.save(&structure_path)?;
    }

    Ok(new_id)
}

/// 覆盖片段正文（保留 frontmatter）
///
/// 如果 fragment_id 不存在则创建新文件（但不更新 structure.toml，
/// 因为新创建的片段应该通过 create_fragment 关联到 scene）。
///
/// 记为手写来源（编辑器保存路径）；Agent 工具走
/// [`write_fragment_text_source`] 并声明 AI 来源。
pub fn write_fragment_text(
    paths: &ProjectPaths,
    fragment_id: &str,
    text: &str,
) -> Result<(), StudioError> {
    write_fragment_text_source(paths, fragment_id, text, crate::stats::WriteSource::Manual)
}

/// 覆盖片段正文（带字数来源）— 节流版本快照 + 回读校验落盘 + 码字记账
pub fn write_fragment_text_source(
    paths: &ProjectPaths,
    fragment_id: &str,
    text: &str,
    source: crate::stats::WriteSource,
) -> Result<(), StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {fragment_id}")))?;

    let (old_content, old_body) = if frag_path.exists() {
        let old = std::fs::read_to_string(&frag_path)?;
        let body = crate::fragment::fragment_body(&old).to_string();
        // 覆盖前对即将被替换的内容做节流快照（可从版本历史恢复）
        crate::history::maybe_snapshot(paths, fragment_id);
        (Some(old), Some(body))
    } else {
        (None, None)
    };

    let file_content = match &old_content {
        // 保留已有 frontmatter
        Some(old) => preserve_frontmatter(fragment_id, old, text),
        None => render_fragment_file(fragment_id, text),
    };

    crate::journal::write_verified(paths.root(), "fragment_write", &frag_path, &file_content)?;
    crate::stats::record_change(paths.root(), old_body.as_deref(), text, source);
    Ok(())
}

/// 渲染 frontmatter（id + anchors）+ 正文。
///
/// anchors 为空时与 [`render_fragment_file`] 输出一致。
fn render_fragment_file_anchored(
    fragment_id: &str,
    text: &str,
    anchors: &[story_ir::EntityAnchor],
) -> String {
    if anchors.is_empty() {
        return render_fragment_file(fragment_id, text);
    }
    let mut content = format!("---\nid: {fragment_id}\nanchors:\n");
    for anchor in anchors {
        let role = match anchor.role {
            story_ir::AnchorRole::Appears => "appears",
            story_ir::AnchorRole::Mentioned => "mentioned",
            story_ir::AnchorRole::Referenced => "referenced",
        };
        content.push_str(&format!(
            "  - entity: {}\n    role: {role}\n",
            anchor.entity
        ));
    }
    content.push_str("---\n\n");
    // 剥离模型复读的 YAML 块（anchored 渲染曾遗漏此步，导致流水线片段
    // 出现双层 frontmatter，第二层被解析器当正文）
    content.push_str(crate::fragment::strip_leading_frontmatter(text));
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

/// 创建新片段（带 anchors frontmatter）。
///
/// 逐章生文流水线用：AI 输出的 EntityAnchor 随正文一起落盘。
pub fn create_fragment_anchored(
    paths: &ProjectPaths,
    scene_id: &str,
    text: &str,
    anchors: &[story_ir::EntityAnchor],
) -> Result<String, StudioError> {
    create_fragment_anchored_source(
        paths,
        scene_id,
        text,
        anchors,
        crate::stats::WriteSource::Ai,
    )
}

/// 创建新片段（带 anchors frontmatter + 字数来源）。
///
/// 逐章生文流水线 / AI 续写走 AI 来源；编辑器插入空片段走手写来源。
pub fn create_fragment_anchored_source(
    paths: &ProjectPaths,
    scene_id: &str,
    text: &str,
    anchors: &[story_ir::EntityAnchor],
    source: crate::stats::WriteSource,
) -> Result<String, StudioError> {
    let fragments_dir = paths.fragments_dir();
    std::fs::create_dir_all(&fragments_dir)?;

    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;

    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;

    let new_id = next_fragment_id(paths);
    let frag_path = fragments_dir.join(format!("{new_id}.md"));

    let file_content = render_fragment_file_anchored(&new_id, text, anchors);
    crate::journal::write_verified(paths.root(), "fragment_create", &frag_path, &file_content)?;
    crate::stats::record_change(paths.root(), None, text, source);

    if !scene.fragments.contains(&new_id) {
        scene.fragments.push(new_id.clone());
        structure.save(&structure_path)?;
    }

    Ok(new_id)
}

/// 覆盖片段正文并整体替换 anchors frontmatter。
///
/// 重写场景用：frontmatter 重新渲染（id + anchors），正文整体替换。
/// 记为 AI 来源（流水线重写 / Agent 整体覆写）。
pub fn write_fragment_text_anchored(
    paths: &ProjectPaths,
    fragment_id: &str,
    text: &str,
    anchors: &[story_ir::EntityAnchor],
) -> Result<(), StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {fragment_id}")))?;

    let old_body = if frag_path.exists() {
        let old = std::fs::read_to_string(&frag_path)?;
        let body = crate::fragment::fragment_body(&old).to_string();
        crate::history::maybe_snapshot(paths, fragment_id);
        Some(body)
    } else {
        None
    };

    let file_content = render_fragment_file_anchored(fragment_id, text, anchors);
    crate::journal::write_verified(paths.root(), "fragment_write", &frag_path, &file_content)?;
    crate::stats::record_change(
        paths.root(),
        old_body.as_deref(),
        text,
        crate::stats::WriteSource::Ai,
    );
    Ok(())
}

/// 向已有片段追加内容（保留 frontmatter）。
///
/// Agent 工具用 → AI 来源。
pub fn append_fragment_text(
    paths: &ProjectPaths,
    fragment_id: &str,
    text: &str,
) -> Result<(), StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {fragment_id}")))?;

    if !frag_path.exists() {
        return Err(StudioError::NotFound(format!(
            "片段 '{fragment_id}' 不存在"
        )));
    }

    let old = std::fs::read_to_string(&frag_path)?;
    crate::history::maybe_snapshot(paths, fragment_id);
    // 追加的文本同样要剥掉模型复读的 YAML 块
    let text = crate::fragment::strip_leading_frontmatter(text);
    let new_content = if let Some(rest) = old.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let frontmatter_end = 3 + end + 3;
            let frontmatter = &old[..frontmatter_end];
            let body = old[frontmatter_end..].trim_end();
            format!("{frontmatter}\n\n{body}\n\n{text}\n")
        } else {
            format!("{old}\n\n{text}\n")
        }
    } else {
        format!("{old}\n\n{text}\n")
    };

    crate::journal::write_verified(paths.root(), "fragment_write", &frag_path, &new_content)?;
    crate::stats::record_change(
        paths.root(),
        Some(crate::fragment::fragment_body(&old)),
        crate::fragment::fragment_body(&new_content),
        crate::stats::WriteSource::Ai,
    );
    Ok(())
}

/// 为片段添加显式锚点（写入 frontmatter anchors 列表）。
///
/// role：appears（出场）/ mentioned（提及）/ referenced（指代）。
/// 已有相同实体的锚点时更新 role，不重复添加。
pub fn set_fragment_anchor(
    paths: &ProjectPaths,
    fragment_id: &str,
    entity_id: &str,
    role: &str,
) -> Result<(), StudioError> {
    set_fragment_anchors(paths, fragment_id, &[(entity_id.to_string(), role.to_string())])
        .map(|_| ())
}

/// 批量为片段设置显式锚点：一次读盘、合并全部、一次写盘。
///
/// anchors 为 (entity_id, role) 对；同一实体重复出现时后者覆盖 role。
/// 任一实体 ID / role 非法即整体报错，不部分落盘。返回写入的锚点总数。
pub fn set_fragment_anchors(
    paths: &ProjectPaths,
    fragment_id: &str,
    anchors: &[(String, String)],
) -> Result<usize, StudioError> {
    if anchors.is_empty() {
        return Err(StudioError::Config("锚点列表不能为空".to_string()));
    }

    let (frontmatter, body, mut merged) = read_fragment_with_anchors(paths, fragment_id)?;
    let _ = frontmatter;
    for (entity_id, role) in anchors {
        let role = match role.as_str() {
            "appears" => story_ir::AnchorRole::Appears,
            "mentioned" => story_ir::AnchorRole::Mentioned,
            "referenced" => story_ir::AnchorRole::Referenced,
            other => {
                return Err(StudioError::Config(format!(
                    "锚点角色 '{other}' 无效，应为 appears / mentioned / referenced"
                )))
            }
        };
        let entity = story_ir::StableId::new(entity_id)
            .map_err(|_| StudioError::Config(format!("无效的实体 ID: {entity_id}")))?;
        match merged.iter_mut().find(|a| a.entity == entity) {
            Some(a) => a.role = role,
            None => merged.push(story_ir::EntityAnchor { entity, role }),
        }
    }
    let count = merged.len();
    let content = render_fragment_file_anchored(fragment_id, &body, &merged);
    crate::journal::write_verified(
        paths.root(),
        "anchor_update",
        &paths.fragments_dir().join(format!("{fragment_id}.md")),
        &content,
    )?;
    Ok(count)
}

/// 整体替换片段的显式锚点列表（理解锚定用：LLM 判定结果为权威集）。
///
/// anchors 为 (entity_id, role) 对；role 非法值按 appears 处理。
/// 返回写入的锚点数。
pub fn replace_fragment_anchors(
    paths: &ProjectPaths,
    fragment_id: &str,
    anchors: &[(String, String)],
) -> Result<usize, StudioError> {
    let (_frontmatter, body, _old) = read_fragment_with_anchors(paths, fragment_id)?;
    let mut parsed = Vec::with_capacity(anchors.len());
    for (entity_id, role) in anchors {
        let entity = story_ir::StableId::new(entity_id)
            .map_err(|_| StudioError::Config(format!("无效的实体 ID: {entity_id}")))?;
        let r = match role.as_str() {
            "mentioned" => story_ir::AnchorRole::Mentioned,
            "referenced" => story_ir::AnchorRole::Referenced,
            _ => story_ir::AnchorRole::Appears,
        };
        if !parsed.iter().any(|a: &story_ir::EntityAnchor| a.entity == entity) {
            parsed.push(story_ir::EntityAnchor { entity, role: r });
        }
    }
    let count = parsed.len();
    let content = render_fragment_file_anchored(fragment_id, &body, &parsed);
    crate::journal::write_verified(
        paths.root(),
        "anchor_update",
        &paths.fragments_dir().join(format!("{fragment_id}.md")),
        &content,
    )?;
    Ok(count)
}

/// 移除片段上指定实体的显式锚点。
pub fn remove_fragment_anchor(
    paths: &ProjectPaths,
    fragment_id: &str,
    entity_id: &str,
) -> Result<(), StudioError> {
    let (_frontmatter, body, mut anchors) = read_fragment_with_anchors(paths, fragment_id)?;
    let entity = story_ir::StableId::new(entity_id)
        .map_err(|_| StudioError::Config(format!("无效的实体 ID: {entity_id}")))?;
    let before = anchors.len();
    anchors.retain(|a| a.entity != entity);
    if anchors.len() == before {
        return Err(StudioError::NotFound(format!(
            "片段 '{fragment_id}' 上没有实体 '{entity_id}' 的锚点"
        )));
    }
    let content = render_fragment_file_anchored(fragment_id, &body, &anchors);
    crate::journal::write_verified(
        paths.root(),
        "anchor_update",
        &paths.fragments_dir().join(format!("{fragment_id}.md")),
        &content,
    )?;
    Ok(())
}

// ============================================================================
// 未锚定指代处置（anchoring.toml 台账）
// ============================================================================

/// 同步某片段的未锚定指代台账。
///
/// - 本次上报的指代：新 quote 插入 pending；已有记录仅刷新 hint（保留处置状态）；
/// - 台账中该片段的 quote 已不在正文里的记录删除（改稿自动清理）；
/// - 台账清空时删除 anchoring.toml，不留空文件。
///
/// 返回该片段当前仍为 pending 的 (quote, hint) 列表——回执只展示这些，
/// 已处置（ignored/resolved）的不再重复提示。
pub fn sync_unresolved_refs(
    paths: &ProjectPaths,
    fragment_id: &str,
    body: &str,
    unresolved: &[(String, String)],
) -> Result<Vec<(String, String)>, StudioError> {
    let mut file = AnchoringFile::load_or_default(&paths.anchoring_toml())?;
    let mut changed = false;

    // 清理：正文已不含该 quote 的记录
    let before = file.refs.len();
    file.refs
        .retain(|r| !(r.fragment_id == fragment_id && !body.contains(&r.quote)));
    if file.refs.len() != before {
        changed = true;
    }

    // upsert：本次 LLM 上报的未锚定指代
    for (quote, hint) in unresolved {
        if quote.is_empty() || !body.contains(quote.as_str()) {
            continue;
        }
        match file
            .refs
            .iter_mut()
            .find(|r| r.fragment_id == fragment_id && r.quote == *quote)
        {
            Some(r) => {
                if r.hint != *hint {
                    r.hint = hint.clone();
                    changed = true;
                }
            }
            None => {
                file.refs.push(UnresolvedRefDef {
                    fragment_id: fragment_id.to_string(),
                    quote: quote.clone(),
                    hint: hint.clone(),
                    status: "pending".to_string(),
                    target: None,
                });
                changed = true;
            }
        }
    }

    if changed {
        if file.refs.is_empty() {
            let _ = std::fs::remove_file(paths.anchoring_toml());
        } else {
            file.save(&paths.anchoring_toml())?;
        }
    }

    Ok(file
        .refs
        .iter()
        .filter(|r| r.fragment_id == fragment_id && r.is_pending())
        .map(|r| (r.quote.clone(), r.hint.clone()))
        .collect())
}

/// 读取全部未锚定指代记录（按片段 + quote 排序）。
pub fn load_unresolved_refs(paths: &ProjectPaths) -> Result<Vec<UnresolvedRefDef>, StudioError> {
    let file = AnchoringFile::load_or_default(&paths.anchoring_toml())?;
    let mut refs = file.refs;
    refs.sort_by(|a, b| {
        a.fragment_id
            .cmp(&b.fragment_id)
            .then_with(|| a.quote.cmp(&b.quote))
    });
    Ok(refs)
}

/// 对未锚定指代做处置。
///
/// action：
/// - `ignore`  — 忽略，后续锚定不再提示；
/// - `merge`   — 归拢到已有实体（target_entity_id 必填），并为片段补锚点；
/// - `create`  — 新建档案（name 必填；匿名指代可用占位名如「九一真凶」，
///   真名揭晓后用 merge_entities 合并），并为片段补锚点。角色可经
///   profile 一步带入性别/职业/外貌/背景/性格/习性，免去再 update_entity。
///
/// 返回给调用方的回执摘要。
pub fn resolve_unresolved_ref(
    paths: &ProjectPaths,
    fragment_id: &str,
    quote: &str,
    action: &str,
    target_entity_id: Option<&str>,
    name: Option<&str>,
    kind: Option<&str>,
    profile: Option<&CharacterProfileInput>,
) -> Result<String, StudioError> {
    resolve_unresolved_refs(
        paths,
        &[(fragment_id.to_string(), quote.to_string())],
        action,
        target_entity_id,
        name,
        kind,
        profile,
    )
}

/// 批量处置未锚定指代：一次台账读写，action / target / name / kind 对每条生效。
///
/// refs 为 (fragment_id, quote) 列表。典型场景：同一实体的多个代称
/// （「摆的人」「写的人」…）一次归拢——create 只新建**一个**档案，
/// 全部指代与涉及的片段（每片段一次写盘）都归到它。
///
/// 单条时回执与「未找到即 Err」的行为和逐条调用完全一致；
/// 多条时未命中的条目记入回执继续处理其余，不整体失败。
pub fn resolve_unresolved_refs(
    paths: &ProjectPaths,
    refs: &[(String, String)],
    action: &str,
    target_entity_id: Option<&str>,
    name: Option<&str>,
    kind: Option<&str>,
    profile: Option<&CharacterProfileInput>,
) -> Result<String, StudioError> {
    if refs.is_empty() {
        return Err(StudioError::Config("refs 不能为空".to_string()));
    }
    let single = refs.len() == 1;
    let anchoring_path = paths.anchoring_toml();
    let mut file = AnchoringFile::load_or_default(&anchoring_path)?;

    // 定位每条记录：单条未找到直接 Err（兼容逐条行为），多条收集后继续
    let mut found: Vec<usize> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (fragment_id, quote) in refs {
        match file.index_of(fragment_id, quote) {
            Some(i) => found.push(i),
            None => missing.push(format!("{fragment_id}「{quote}」")),
        }
    }
    if single && !missing.is_empty() {
        let (fragment_id, quote) = &refs[0];
        return Err(StudioError::NotFound(format!(
            "未找到指代记录：片段 {fragment_id} 的 \"{quote}\""
        )));
    }

    // 目标实体：merge 校验传入 ID；create 新建一个（全部指代共享）
    let mut profile_applied = false;
    let target: String = match action {
        "ignore" => String::new(),
        "merge" => {
            let target = target_entity_id.ok_or_else(|| {
                StudioError::Config("merge 处置需要 target_entity_id".to_string())
            })?;
            validate_entity_exists(paths, target)?;
            target.to_string()
        }
        "create" => {
            let name = name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| StudioError::Config("create 处置需要 name".to_string()))?;
            let kind = EntityKind::from_type_str(kind.unwrap_or("characters"))?;
            let id = create_entity(paths, kind, name)?;
            // 一步建档：档案字段直接带入（仅角色）
            if kind == EntityKind::Character {
                if let Some(p) = profile.filter(|p| !p.is_empty()) {
                    apply_character_profile(paths, &id, p)?;
                    profile_applied = true;
                }
            }
            id
        }
        other => {
            return Err(StudioError::Config(format!(
                "无效的处置动作 '{other}'，应为 ignore / merge / create"
            )))
        }
    };

    // 涉及的片段各补一次锚点（同片段多条指代不重复写盘）
    if !found.is_empty() && action != "ignore" {
        let mut anchored: Vec<String> = Vec::new();
        for i in &found {
            let fragment_id = file.refs[*i].fragment_id.clone();
            if anchored.contains(&fragment_id) {
                continue;
            }
            anchored.push(fragment_id.clone());
            set_fragment_anchor(paths, &fragment_id, &target, "referenced")?;
        }
    }

    for i in &found {
        if action == "ignore" {
            file.refs[*i].status = "ignored".into();
            file.refs[*i].target = None;
        } else {
            file.refs[*i].status = "resolved".into();
            file.refs[*i].target = Some(target.clone());
        }
    }

    file.save(&anchoring_path)?;

    if single {
        let (_, quote) = &refs[0];
        return Ok(match action {
            "ignore" => format!("已忽略 \"{quote}\"（后续锚定不再提示）"),
            "merge" => format!("\"{quote}\" 已归拢到 {target}"),
            _ => format!(
                "已为 \"{quote}\" 建档 {target}「{}」并补锚点{}",
                name.unwrap_or(""),
                if profile_applied { "，档案字段已写入" } else { "" }
            ),
        });
    }

    let mut out = match action {
        "ignore" => format!("已批量忽略 {} 条指代", found.len()),
        "merge" => format!("已批量归拢 {} 条指代到 {target}", found.len()),
        _ => format!(
            "已建档 {target}「{}」，{} 条指代全部归拢并补锚点{}",
            name.unwrap_or(""),
            found.len(),
            if profile_applied { "，档案字段已写入" } else { "" }
        ),
    };
    if !missing.is_empty() {
        out.push_str(&format!(
            "；未找到 {} 条（片段与 quote 需与台账完全一致）：{}",
            missing.len(),
            missing.join("、")
        ));
    }
    Ok(out)
}

/// 校验实体文件存在（按 ID 前缀路由）。
fn validate_entity_exists(paths: &ProjectPaths, entity_id: &str) -> Result<(), StudioError> {
    let path = paths.file_for_id(entity_id).ok_or_else(|| {
        StudioError::Config(format!("无效的实体 ID: {entity_id}"))
    })?;
    if !path.exists() {
        return Err(StudioError::NotFound(format!("实体 '{entity_id}' 不存在")));
    }
    Ok(())
}

/// 读取片段文件，拆出（frontmatter 原文, 正文, 锚点列表）。
fn read_fragment_with_anchors(
    paths: &ProjectPaths,
    fragment_id: &str,
) -> Result<(String, String, Vec<story_ir::EntityAnchor>), StudioError> {
    let path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的片段 ID: {fragment_id}")))?;
    if !path.exists() {
        return Err(StudioError::NotFound(format!("片段 '{fragment_id}' 不存在")));
    }
    let content = std::fs::read_to_string(&path)?;

    let frontmatter;
    let body_raw;
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let frontmatter_end = 3 + end + 3;
            frontmatter = content[..frontmatter_end].to_string();
            body_raw = content[frontmatter_end..].to_string();
        } else {
            return Err(StudioError::Config(format!(
                "片段 '{fragment_id}' 的 frontmatter 缺少闭合 ---"
            )));
        }
    } else {
        frontmatter = String::new();
        body_raw = content;
    }

    // 解析 anchors：- entity: c_XXXX / role: appears
    let mut anchors = Vec::new();
    if let Some(rest) = frontmatter.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let mut current_entity: Option<story_ir::StableId> = None;
            for line in rest[..end].lines() {
                let t = line.trim_start_matches([' ', '-']);
                if let Some(id) = t.strip_prefix("entity:") {
                    if let Ok(sid) = story_ir::StableId::new(id.trim()) {
                        current_entity = Some(sid);
                    }
                } else if let Some(r) = t.strip_prefix("role:") {
                    let role = match r.trim() {
                        "mentioned" => story_ir::AnchorRole::Mentioned,
                        "referenced" => story_ir::AnchorRole::Referenced,
                        _ => story_ir::AnchorRole::Appears,
                    };
                    if let Some(entity) = current_entity.take() {
                        anchors.push(story_ir::EntityAnchor { entity, role });
                    }
                }
            }
        }
    }

    let body = body_raw.trim_start_matches(['\r', '\n']).to_string();
    Ok((frontmatter, body, anchors))
}

/// 删除片段
///
/// 删除 .md 文件 + 从所属 scene.fragments 移除。
pub fn delete_fragment(paths: &ProjectPaths, fragment_id: &str) -> Result<(), StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {fragment_id}")))?;

    if !frag_path.exists() {
        return Err(StudioError::NotFound(format!(
            "片段 '{fragment_id}' 不存在"
        )));
    }

    // 删除前把即将消失的内容强制留底到版本历史（目录随项目走，可手工找回）
    crate::history::force_snapshot(paths, fragment_id);

    std::fs::remove_file(&frag_path)?;
    crate::journal::record(paths.root(), "fragment_delete", &frag_path, 0, true, None);

    // 从 scene.fragments 移除引用
    let structure_path = paths.structure_toml();
    if structure_path.exists() {
        let mut structure = load_structure(paths)?;
        let mut changed = false;
        for scene in structure.scene.values_mut() {
            let before = scene.fragments.len();
            scene.fragments.retain(|f| f != fragment_id);
            if scene.fragments.len() != before {
                changed = true;
            }
        }
        if changed {
            structure.save(&structure_path)?;
        }
    }

    Ok(())
}

/// 在指定片段之后插入新片段到 scene.fragments（用于 split）
///
/// 新片段的 .md 文件应已创建；本函数只更新 structure.toml 的顺序数组。
pub fn insert_fragment_after(
    paths: &ProjectPaths,
    scene_id: &str,
    after_id: &str,
    new_id: &str,
) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;
    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;

    // 真实"移到其后"语义：new_id 已在列表中（create_fragment 会把新片段推到
    // 场景末尾）时先移除再插入，否则会被防重逻辑留在原位（末尾）——
    // 拆分场景时造成叙事顺序错乱。移除可能使 after_id 索引前移，需重新定位。
    scene.fragments.retain(|f| f != new_id);
    let pos = scene
        .fragments
        .iter()
        .position(|f| f == after_id)
        .ok_or_else(|| {
            StudioError::NotFound(format!("片段 '{after_id}' 不在场景 '{scene_id}' 中"))
        })?;

    scene.fragments.insert(pos + 1, new_id.to_string());
    structure.save(&structure_path)?;
    Ok(())
}

/// 移动片段到同场景内的新位置
///
/// new_index 是"移除该片段后的目标位置"（0-based）。
/// 例如场景有 [A, B, C]，把 B 移到末尾：移除后是 [A, C]（长度 2），目标位置 2（追加）。
pub fn move_fragment(
    paths: &ProjectPaths,
    scene_id: &str,
    fragment_id: &str,
    new_index: usize,
) -> Result<(), StudioError> {
    let structure_path = paths.structure_toml();
    let mut structure = load_structure(paths)?;
    let scene = structure
        .scene
        .get_mut(scene_id)
        .ok_or_else(|| StudioError::NotFound(format!("场景 '{scene_id}' 不存在")))?;

    let pos = scene
        .fragments
        .iter()
        .position(|f| f == fragment_id)
        .ok_or_else(|| {
            StudioError::NotFound(format!("片段 '{fragment_id}' 不在场景 '{scene_id}' 中"))
        })?;

    // 移除后校验 new_index 范围
    scene.fragments.remove(pos);
    if new_index > scene.fragments.len() {
        return Err(StudioError::Config(format!(
            "目标位置 {new_index} 超出范围（移除后有效范围 0-{}）",
            scene.fragments.len()
        )));
    }
    scene.fragments.insert(new_index, fragment_id.to_string());
    structure.save(&structure_path)?;
    Ok(())
}

/// 合并两个片段：把 secondary 的正文与 anchors 合并到 primary，然后删除 secondary
///
/// anchors 会去重（按 entity+role 组合）。primary 的 frontmatter 保留，
/// secondary 的 anchors 追加（去重）。正文拼接为 primary 正文 + 空行 + secondary 正文。
pub fn merge_fragments(
    paths: &ProjectPaths,
    primary_id: &str,
    secondary_id: &str,
) -> Result<(), StudioError> {
    if primary_id == secondary_id {
        return Err(StudioError::Config("不能合并片段到自身".to_string()));
    }

    let primary_path = paths
        .file_for_id(primary_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {primary_id}")))?;
    let secondary_path = paths
        .file_for_id(secondary_id)
        .ok_or_else(|| StudioError::Config(format!("无效的 fragment ID: {secondary_id}")))?;

    if !primary_path.exists() {
        return Err(StudioError::NotFound(format!("片段 '{primary_id}' 不存在")));
    }
    if !secondary_path.exists() {
        return Err(StudioError::NotFound(format!(
            "片段 '{secondary_id}' 不存在"
        )));
    }

    // 精确解析两个片段（保留 anchors）
    let primary_content = std::fs::read_to_string(&primary_path)?;
    let secondary_content = std::fs::read_to_string(&secondary_path)?;
    let mut primary = FragmentFile::parse(&primary_content)
        .map_err(|e| StudioError::Config(format!("解析 {primary_id} 失败: {e}")))?;
    let secondary = FragmentFile::parse(&secondary_content)
        .map_err(|e| StudioError::Config(format!("解析 {secondary_id} 失败: {e}")))?;

    // 合并正文
    let merged_text = if primary.text.is_empty() {
        secondary.text.clone()
    } else if secondary.text.is_empty() {
        primary.text.clone()
    } else {
        format!("{}\n\n{}", primary.text.trim_end(), secondary.text)
    };
    primary.text = merged_text;

    // 合并 anchors（去重：按 entity + role）
    for anchor in &secondary.frontmatter.anchors {
        let exists = primary
            .frontmatter
            .anchors
            .iter()
            .any(|a| a.entity == anchor.entity && a.role == anchor.role);
        if !exists {
            primary.frontmatter.anchors.push(anchor.clone());
        }
    }

    // 写回 primary
    std::fs::write(&primary_path, primary.to_markdown())?;

    // 删除 secondary（文件 + structure 引用）
    std::fs::remove_file(&secondary_path)?;
    let structure_path = paths.structure_toml();
    if structure_path.exists() {
        let mut structure = load_structure(paths)?;
        let mut changed = false;
        for scene in structure.scene.values_mut() {
            let before = scene.fragments.len();
            scene.fragments.retain(|f| f != secondary_id);
            if scene.fragments.len() != before {
                changed = true;
            }
        }
        if changed {
            structure.save(&structure_path)?;
        }
    }

    Ok(())
}

/// 渲染 fragment 文件内容（frontmatter + 正文）
fn render_fragment_file(fragment_id: &str, text: &str) -> String {
    let mut content = format!("---\nid: {fragment_id}\n---\n\n");
    // 剥离模型复读的 YAML 块，避免叠成双层 frontmatter
    content.push_str(crate::fragment::strip_leading_frontmatter(text));
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

/// 保留旧 frontmatter，替换正文
fn preserve_frontmatter(fragment_id: &str, old: &str, new_text: &str) -> String {
    if let Some(rest) = old.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let frontmatter_end = 3 + end + 3;
            let frontmatter = &old[..frontmatter_end];
            let mut result = format!(
                "{frontmatter}\n\n{}",
                crate::fragment::strip_leading_frontmatter(new_text)
            );
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result
        } else {
            render_fragment_file(fragment_id, new_text)
        }
    } else {
        render_fragment_file(fragment_id, new_text)
    }
}

// ============================================================================
// Assertion 变更
// ============================================================================

/// add_assertion 的结果
#[derive(Debug, Clone)]
pub struct AddAssertionOutcome {
    pub id: String,
    /// 被新断言自动取代（status → superseded）的旧断言 ID
    pub superseded: Vec<String>,
    /// 与已有 active 断言完全相同，直接复用未新建
    pub deduplicated: bool,
    /// 同主体同谓词下仍处 active 的其他旧值（仅非自动覆盖谓词，
    /// 如 occupation/holds——可多值并存，但同一身份/职位的演进
    /// 应由调用方决定是否废弃旧值）
    pub coexisting: Vec<String>,
}

/// toml object 值转可读字符串（String 去引号）
fn object_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// 时变谓词：新断言落地时，同主体同谓词的旧 active 断言自动置为 superseded。
/// （located_at/status 描述"当前状态"，alive/age 随剧情推进会被覆盖）
const AUTO_SUPERSEDE_PREDICATES: [&str; 4] = ["located_at", "status", "alive", "age"];

fn is_active_status(s: &str) -> bool {
    s.is_empty() || s == "active"
}

/// 添加断言
///
/// 生成 a_XXXX ID（十进制 4 位），写入 knowledge/assertions.toml。
/// 根据 predicate 自动转换 object 的 TOML 类型（alive→布尔，age→整数，其他→字符串）。
///
/// 数据一致性约束（保持断言与编译器/证据登记表同步）：
/// - `evidence` 传片段 ID 时会自动在 knowledge/evidence.toml 建立对应证据条目
///   （ev_XXXX，loc 指向该片段），断言引用证据 ID 而非片段 ID——编译器按证据
///   登记表校验，直接写片段 ID 会误报"证据不存在"。
/// - 时变谓词（located_at/status/alive/age）自动将同主体同谓词的旧 active
///   断言置为 superseded，避免"人在坟地又在老宅"式的状态堆积。
/// - 与已有 active 断言完全相同（主体+谓词+值）时不重复创建。
pub fn add_assertion(
    paths: &ProjectPaths,
    subject: &str,
    predicate: &str,
    object: &str,
    evidence: Option<&str>,
) -> Result<AddAssertionOutcome, StudioError> {
    add_assertions(
        paths,
        &[(
            subject.to_string(),
            predicate.to_string(),
            object.to_string(),
            evidence.map(str::to_string),
        )],
    )
    .map(|mut v| v.remove(0))
}

/// 批量添加断言：一次读写 assertions.toml（证据登记逐条进行）。
///
/// items 为 (subject, predicate, object, evidence_fragment) 列表，
/// 返回与输入顺序对应的每条结果。任一条失败（如证据片段不存在）则
/// 整体报错，断言不部分落盘。
pub fn add_assertions(
    paths: &ProjectPaths,
    items: &[(String, String, String, Option<String>)],
) -> Result<Vec<AddAssertionOutcome>, StudioError> {
    if items.is_empty() {
        return Err(StudioError::Config("assertions 不能为空".to_string()));
    }
    let assertions_path = paths.knowledge_dir().join("assertions.toml");
    std::fs::create_dir_all(paths.knowledge_dir())?;

    let mut file: AssertionsFile = AssertionsFile::load_or_default(&assertions_path)?;
    let mut outcomes = Vec::with_capacity(items.len());
    for (subject, predicate, object, evidence) in items {
        outcomes.push(apply_assertion(
            paths,
            &mut file,
            subject,
            predicate,
            object,
            evidence.as_deref(),
        )?);
    }
    file.save(&assertions_path)?;
    Ok(outcomes)
}

/// 在已加载的断言文件上应用一条断言（查重 / 时变自动失效 / 并存收集 /
/// 证据登记 / 追加），不落盘——单条与批量共用。
fn apply_assertion(
    paths: &ProjectPaths,
    file: &mut AssertionsFile,
    subject: &str,
    predicate: &str,
    object: &str,
    evidence: Option<&str>,
) -> Result<AddAssertionOutcome, StudioError> {
    let object_value = coerce_object_value(predicate, object);

    // 查重：完全相同的 active 断言直接复用（不登记证据，避免孤儿条目）
    if let Some(existing) = file.assertions.iter().find(|a| {
        a.subject == subject
            && a.predicate == predicate
            && is_active_status(&a.status)
            && a.object == object_value
    }) {
        return Ok(AddAssertionOutcome {
            id: existing.id.clone(),
            superseded: Vec::new(),
            deduplicated: true,
            coexisting: Vec::new(),
        });
    }

    // 时变谓词：旧 active 断言自动失效；
    // 可多值谓词（occupation/holds 等）收集并存旧值供调用方判断
    let mut superseded = Vec::new();
    let mut coexisting = Vec::new();
    if AUTO_SUPERSEDE_PREDICATES.contains(&predicate) {
        for a in file.assertions.iter_mut() {
            if a.subject == subject && a.predicate == predicate && is_active_status(&a.status) {
                a.status = "superseded".to_string();
                superseded.push(a.id.clone());
            }
        }
    } else {
        for a in file.assertions.iter() {
            if a.subject == subject
                && a.predicate == predicate
                && is_active_status(&a.status)
                && a.object != object_value
            {
                coexisting.push(object_value_to_string(&a.object));
            }
        }
    }

    // 证据：片段 ID → evidence.toml 登记条目（ev_XXXX）
    let evidence_list: Vec<String> = match evidence {
        Some(fragment_id) => vec![register_evidence(paths, fragment_id)?],
        None => Vec::new(),
    };

    let max_num = file
        .assertions
        .iter()
        .filter_map(|a| a.id.strip_prefix("a_").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);

    let new_id = format!("a_{:04}", max_num + 1);

    file.assertions.push(AssertionDef {
        id: new_id.clone(),
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: object_value,
        status: "active".to_string(),
        extracted_by: "manual".to_string(),
        evidence: evidence_list,
        valid: None,
    });

    Ok(AddAssertionOutcome {
        id: new_id,
        superseded,
        deduplicated: false,
        coexisting,
    })
}

/// 为片段建立（或复用）evidence.toml 登记条目，返回证据 ID（ev_XXXX）。
///
/// 同一片段已有条目时直接复用；片段文件不存在时报错，避免产生指向
/// 不存在片段的证据。
fn register_evidence(paths: &ProjectPaths, fragment_id: &str) -> Result<String, StudioError> {
    let frag_path = paths.file_for_id(fragment_id).ok_or_else(|| {
        StudioError::Config(format!("无效的片段 ID: {fragment_id}"))
    })?;
    if !frag_path.exists() {
        return Err(StudioError::NotFound(format!(
            "证据片段 '{fragment_id}' 不存在，请先写正文再挂证据"
        )));
    }

    let evidence_path = paths.knowledge_dir().join("evidence.toml");
    let mut file = EvidenceFile::load_or_default(&evidence_path)?;

    if let Some(existing) = file.evidence.iter().find(|e| e.loc == fragment_id) {
        return Ok(existing.id.clone());
    }

    let max_num = file
        .evidence
        .iter()
        .filter_map(|e| e.id.strip_prefix("ev_").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    let id = format!("ev_{:04}", max_num + 1);

    file.evidence.push(EvidenceDef {
        id: id.clone(),
        loc: fragment_id.to_string(),
        quote: String::new(),
        source: "manual".to_string(),
    });
    file.save(&evidence_path)?;
    Ok(id)
}

// ============================================================================
// 数据修复（一次性维护）
// ============================================================================

/// repair_project 的结果报告
#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    /// 清除正文残留 YAML 块的片段
    pub fragments_cleaned: Vec<String>,
    /// 证据字段从片段 ID 重建为证据条目的断言数
    pub assertions_fixed: usize,
    /// 证据指向的片段文件不存在、跳过的引用数
    pub evidence_skipped: usize,
}

/// 一次性数据修复（历史脏数据兜底）：
///
/// 1. 片段正文开头残留的模型复读 YAML 块（曾因写入路径未剥离而叠加的
///    双层 frontmatter，第二层会被解析器当作正文显示）
/// 2. 断言 evidence 字段直接存片段 ID（f_XXXX）→ 重建为 evidence.toml
///    登记条目（ev_XXXX），消除编译器"证据不存在"误报
pub fn repair_project(dir: &Path) -> Result<RepairReport, StudioError> {
    let paths = ProjectPaths::new(dir);
    let mut report = RepairReport::default();

    // —— 1. 清洗片段正文残留的 YAML 块 ——
    let fragments_dir = paths.fragments_dir();
    if fragments_dir.exists() {
        let mut names: Vec<String> = std::fs::read_dir(&fragments_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md"))
            .collect();
        names.sort();

        for name in &names {
            let path = fragments_dir.join(name);
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let fragment_id = name.trim_end_matches(".md");

            // 全文扫描清理 id/anchors 残块（开头与中部一视同仁）。
            // 头部合法 frontmatter 保留；若其缺失 id: 字段则整体重渲染规范化。
            let (frontmatter, body) = if let Some(rest) = content.strip_prefix("---") {
                if let Some(end) = rest.find("---") {
                    let frontmatter_end = 3 + end + 3;
                    (&content[..frontmatter_end], &content[frontmatter_end..])
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let (clean_body, body_changed) =
                crate::fragment::strip_residue_blocks(body);
            let head_has_id = frontmatter.lines().any(|l| {
                l.trim().starts_with("id:") && l["id:".len()..].trim_start().starts_with("f_")
            });

            let cleaned = if body_changed || !head_has_id {
                let normalized_body =
                    crate::fragment::strip_all_leading_frontmatter(&clean_body);
                render_fragment_file(fragment_id, normalized_body)
            } else {
                continue;
            };

            if cleaned != content {
                std::fs::write(&path, &cleaned)?;
                report.fragments_cleaned.push(fragment_id.to_string());
            }
        }
    }

    // —— 2. 断言证据：片段 ID → 证据登记条目 ——
    let assertions_path = paths.knowledge_dir().join("assertions.toml");
    if assertions_path.exists() {
        let mut file: AssertionsFile = AssertionsFile::load_or_default(&assertions_path)?;
        let mut fixed_ids: Vec<String> = Vec::new();

        for assertion in file.assertions.iter_mut() {
            let mut new_evidence = Vec::with_capacity(assertion.evidence.len());
            let mut changed = false;
            for ev in &assertion.evidence {
                // 旧格式：evidence 里存的是片段 ID（f_XXXX）
                if ev.starts_with("f_") {
                    match register_evidence(&paths, ev) {
                        Ok(ev_id) => {
                            new_evidence.push(ev_id);
                            changed = true;
                        }
                        Err(_) => {
                            // 片段文件不存在：保留原值，由编译器告警暴露
                            report.evidence_skipped += 1;
                            new_evidence.push(ev.clone());
                        }
                    }
                } else {
                    new_evidence.push(ev.clone());
                }
            }
            if changed {
                assertion.evidence = new_evidence;
                fixed_ids.push(assertion.id.clone());
            }
        }

        if !fixed_ids.is_empty() {
            file.save(&assertions_path)?;
            report.assertions_fixed = fixed_ids.len();
        }
    }

    Ok(report)
}

/// 根据 predicate 把字符串形式的 object 转换为合适类型的 toml::Value。
///
/// - alive：解析为布尔。true/"true"/"存活"/"活着"/"alive" → true；false/"false"/"已故"/"死亡"/"dead" → false。
///   （修复 AI 误传 predicate 字面量 "alive" 作为值的 bug）
/// - age：解析为整数，解析失败则退回字符串。
/// - 其他（holds/located_at/member_of/identity 等）：保持字符串。
fn coerce_object_value(predicate: &str, object: &str) -> toml::Value {
    match predicate {
        "alive" => {
            let lower = object.trim().to_lowercase();
            // 明确的 false 语义
            let is_false = matches!(
                lower.as_str(),
                "false" | "已故" | "死亡" | "死了" | "dead" | "0" | "no"
            );
            toml::Value::Boolean(!is_false)
        }
        "age" => match object.trim().parse::<i64>() {
            Ok(n) => toml::Value::Integer(n),
            Err(_) => toml::Value::String(object.to_string()),
        },
        _ => toml::Value::String(object.to_string()),
    }
}

/// 移除断言上的一个证据引用。
///
/// `fragment_id` 为片段 ID：会先在 evidence.toml 中解析出该片段对应的
/// 证据 ID（ev_XXXX），再从断言上移除这些证据引用。
pub fn remove_evidence(
    paths: &ProjectPaths,
    assertion_id: &str,
    fragment_id: &str,
) -> Result<(), StudioError> {
    let assertions_path = paths.knowledge_dir().join("assertions.toml");

    // 片段 ID → 该片段的所有证据 ID（兼容直接传 ev_ ID 的调用方）
    let evidence_file = EvidenceFile::load_or_default(&paths.knowledge_dir().join("evidence.toml"))?;
    let ev_ids: Vec<String> = if fragment_id.starts_with("ev_") {
        vec![fragment_id.to_string()]
    } else {
        evidence_file
            .evidence
            .iter()
            .filter(|e| e.loc == fragment_id)
            .map(|e| e.id.clone())
            .collect()
    };

    let mut file: AssertionsFile = AssertionsFile::load_or_default(&assertions_path)?;

    let assertion = file
        .assertions
        .iter_mut()
        .find(|a| a.id == assertion_id)
        .ok_or_else(|| StudioError::NotFound(format!("断言 '{assertion_id}' 不存在")))?;

    let before = assertion.evidence.len();
    assertion.evidence.retain(|e| !ev_ids.contains(e));
    if assertion.evidence.len() == before {
        return Err(StudioError::NotFound(format!(
            "证据 '{fragment_id}' 未在断言 '{assertion_id}' 上找到"
        )));
    }

    file.save(&assertions_path)?;
    Ok(())
}

/// 删除单条断言（按 id）
///
/// QuickFix 使用：删除孤儿断言 / 非法有效期断言。
pub fn delete_assertion(paths: &ProjectPaths, assertion_id: &str) -> Result<(), StudioError> {
    let assertions_path = paths.knowledge_dir().join("assertions.toml");

    let mut file: AssertionsFile = AssertionsFile::load_or_default(&assertions_path)?;

    let before = file.assertions.len();
    file.assertions.retain(|a| a.id != assertion_id);
    if file.assertions.len() == before {
        return Err(StudioError::NotFound(format!(
            "断言 '{assertion_id}' 不存在"
        )));
    }

    file.save(&assertions_path)?;
    Ok(())
}

/// 修改断言状态（active / superseded / contradicted）
///
/// QuickFix 使用：将矛盾断言标记为 contradicted（软解决，不删数据）。
/// status 必须是合法的 AssertionStatus 字符串。
pub fn set_assertion_status(
    paths: &ProjectPaths,
    assertion_id: &str,
    status: &str,
) -> Result<(), StudioError> {
    // 校验 status 合法性
    let normalized = match status {
        "active" | "superseded" | "contradicted" => status,
        _ => {
            return Err(StudioError::Config(format!(
                "非法的断言状态 '{status}'，应为 active / superseded / contradicted"
            )))
        }
    };

    let assertions_path = paths.knowledge_dir().join("assertions.toml");

    let mut file: AssertionsFile = AssertionsFile::load_or_default(&assertions_path)?;

    let assertion = file
        .assertions
        .iter_mut()
        .find(|a| a.id == assertion_id)
        .ok_or_else(|| StudioError::NotFound(format!("断言 '{assertion_id}' 不存在")))?;

    assertion.status = normalized.to_string();
    file.save(&assertions_path)?;
    Ok(())
}

// ============================================================================
// Relation 变更
// ============================================================================

/// 添加角色关系边
///
/// 生成 se_XXXX ID（十进制 4 位），写入 relations.toml。
/// 返回新关系 ID。
pub fn add_relation(
    paths: &ProjectPaths,
    from: &str,
    to: &str,
    relation: &str,
) -> Result<String, StudioError> {
    add_relations(
        paths,
        &[(from.to_string(), to.to_string(), relation.to_string())],
    )
    .map(|mut ids| ids.remove(0))
}

/// 批量添加角色关系：一次读写 relations.toml。
///
/// relations 为 (from, to, relation) 三元组列表，返回与输入顺序对应的新关系 ID。
pub fn add_relations(
    paths: &ProjectPaths,
    relations: &[(String, String, String)],
) -> Result<Vec<String>, StudioError> {
    if relations.is_empty() {
        return Err(StudioError::Config("relations 不能为空".to_string()));
    }
    let relations_path = paths.relations_toml();

    let mut file: RelationsFile = RelationsFile::load_or_default(&relations_path)?;

    // se_ 前缀 + 4 位十进制自增
    let mut max_num = file
        .relations
        .iter()
        .filter_map(|r| r.id.strip_prefix("se_").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);

    let mut ids = Vec::with_capacity(relations.len());
    for (from, to, relation) in relations {
        max_num += 1;
        let new_id = format!("se_{max_num:04}");
        file.relations.push(RelationDef {
            id: new_id.clone(),
            from: from.clone(),
            to: to.clone(),
            relation: relation.clone(),
            valid: None,
            evidence: vec![],
        });
        ids.push(new_id);
    }

    file.save(&relations_path)?;
    Ok(ids)
}

/// 移除角色关系边
pub fn remove_relation(paths: &ProjectPaths, relation_id: &str) -> Result<(), StudioError> {
    let relations_path = paths.relations_toml();

    let mut file: RelationsFile = RelationsFile::load_or_default(&relations_path)?;

    let before = file.relations.len();
    file.relations.retain(|r| r.id != relation_id);
    if file.relations.len() == before {
        return Err(StudioError::NotFound(format!(
            "关系 '{relation_id}' 不存在"
        )));
    }

    file.save(&relations_path)?;
    Ok(())
}

// ============================================================================
// Thread 变更
// ============================================================================

/// 创建叙事线索
///
/// 生成 t_XXX ID（十进制 3 位），写入 threads.toml。
/// 返回新线索 ID。
pub fn create_thread(
    paths: &ProjectPaths,
    title: &str,
    description: &str,
    priority: &str,
    planted_at: Option<&str>,
    due_volume: Option<u32>,
    scope: Option<&str>,
) -> Result<String, StudioError> {
    let threads_path = paths.root().join("threads.toml");
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;

    let max_num = file
        .threads
        .iter()
        .filter_map(|t| t.id.strip_prefix("t_").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);

    let new_id = format!("t_{:03}", max_num + 1);

    file.threads
        .push(story_project_format::threads_file::ThreadDef {
            id: new_id.clone(),
            title: title.to_string(),
            description: Some(description.to_string()),
            priority: if priority.is_empty() {
                "minor".to_string()
            } else {
                priority.to_string()
            },
            // 仅暗线落盘 scope 字段，明线（reader）保持缺省
            scope: (scope.map(str::trim) == Some("pov")).then(|| "pov".to_string()),
            due_volume: due_volume.filter(|v| *v > 0),
            planted_at: planted_at.map(|s| s.to_string()),
            revealed_at: None,
            resolved_at: None,
            abandoned: false,
            goal: None,
            tags: vec![],
            assertions: vec![],
            resolves: vec![],
            resolved_by: vec![],
            knowledge: vec![],
            relationships: vec![],
        });

    file.save(&threads_path)?;

    // 埋设点为场景或片段时，自动初始化可见性时间线：
    // 在场角色（POV + 锚点实体）known，读者 suspected。
    // 避免新线索时间线空白、依赖手工逐个补 set_thread_knowledge。
    if let Some(anchor) = planted_at {
        let resolved = if anchor.starts_with("sc_") {
            Some((anchor.to_string(), None))
        } else if anchor.starts_with("f_") {
            // 片段 → 所属场景；时间线锚定到场景（与 advance_thread 一致）
            scene_of_fragment_id(paths, anchor).map(|sc| (sc, Some(anchor.to_string())))
        } else {
            None
        };
        if let Some((scene_id, frag_filter)) = resolved {
            for observer in scene_observers(paths, &scene_id, frag_filter.as_deref()) {
                let _ = set_thread_knowledge(paths, &new_id, &observer, "known", &scene_id);
            }
            let _ = set_thread_knowledge(paths, &new_id, "reader", "suspected", &scene_id);
        }
    }

    Ok(new_id)
}

/// 找到片段所属的场景 ID（structure.toml 的 scene.fragments 反查）。
fn scene_of_fragment_id(paths: &ProjectPaths, fragment_id: &str) -> Option<String> {
    let structure = load_structure(paths).ok()?;
    structure
        .scene
        .iter()
        .find(|(_, sc)| sc.fragments.iter().any(|f| f == fragment_id))
        .map(|(sc_id, _)| sc_id.clone())
}

/// 收集场景的观察者：POV 角色 + 场景片段 frontmatter 中锚定的实体。
/// `only_fragment` 为 Some 时只取该片段的锚点（埋设点为片段时更精确）。
/// 解析失败时尽力返回（只含 POV 或为空），不阻断建线索。
fn scene_observers(
    paths: &ProjectPaths,
    scene_id: &str,
    only_fragment: Option<&str>,
) -> Vec<String> {
    let mut observers: Vec<String> = Vec::new();

    let structure = load_structure(paths).ok();
    if let Some(structure) = structure.as_ref() {
        if let Some(scene) = structure.scene.get(scene_id) {
            if let Some(pov) = &scene.pov {
                observers.push(pov.clone());
            }
            let fragment_ids: Vec<&String> = match only_fragment {
                Some(fid) => scene.fragments.iter().filter(|f| f.as_str() == fid).collect(),
                None => scene.fragments.iter().collect(),
            };
            for fid in fragment_ids {
                let Ok(content) =
                    std::fs::read_to_string(paths.fragments_dir().join(format!("{fid}.md")))
                else {
                    continue;
                };
                // frontmatter 内的 "  - entity: c_XXXX" 行
                if let Some(rest) = content.strip_prefix("---") {
                    if let Some(end) = rest.find("---") {
                        for line in rest[..end].lines() {
                            let trimmed = line.trim_start_matches([' ', '-']);
                            if let Some(id) = trimmed.strip_prefix("entity:") {
                                let id = id.trim().to_string();
                                if !observers.contains(&id) {
                                    observers.push(id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    observers
}

/// 修改叙事线索的核心字段。
///
/// 只更新提供的字段（None 跳过）。tags 以逗号分隔传入，整体覆盖。
/// 返回实际修改的字段名列表。
#[allow(clippy::too_many_arguments)] // 逐字段 Option 更新，与 Tauri/Agent 两侧直通
pub fn update_thread(
    paths: &ProjectPaths,
    thread_id: &str,
    title: Option<&str>,
    description: Option<&str>,
    goal: Option<&str>,
    priority: Option<&str>,
    tags: Option<&[String]>,
    due_volume: Option<Option<u32>>,
    scope: Option<&str>,
) -> Result<Vec<&'static str>, StudioError> {
    let threads_path = paths.threads_toml();
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;

    let thread = file
        .threads
        .iter_mut()
        .find(|t| t.id == thread_id)
        .ok_or_else(|| StudioError::NotFound(format!("线索 '{thread_id}' 不存在")))?;

    let mut changed: Vec<&'static str> = Vec::new();

    if let Some(v) = title {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            return Err(StudioError::Config("线索标题不能为空".into()));
        }
        thread.title = trimmed.to_string();
        changed.push("title");
    }
    if let Some(v) = description {
        // 空串 = 清除描述
        thread.description = (!v.trim().is_empty()).then(|| v.trim().to_string());
        changed.push("description");
    }
    if let Some(v) = goal {
        // 空串 = 清除目标
        thread.goal = (!v.trim().is_empty()).then(|| v.trim().to_string());
        changed.push("goal");
    }
    if let Some(v) = priority {
        // 校验 priority 合法值
        if !matches!(v, "main" | "minor") {
            return Err(StudioError::Config(format!(
                "非法的优先级 '{v}'，应为 main / minor"
            )));
        }
        thread.priority = v.to_string();
        changed.push("priority");
    }
    if let Some(tags) = tags {
        thread.tags = tags.to_vec();
        changed.push("tags");
    }
    if let Some(v) = due_volume {
        // None=全书长线（清除），Some(n)=计划第 n 卷埋设（0 视为清除）
        thread.due_volume = v.filter(|n| *n > 0);
        changed.push("due_volume");
    }
    if let Some(v) = scope {
        // reader/空串 = 恢复明线（清除字段），pov = 暗线
        if !matches!(v.trim(), "pov" | "reader" | "") {
            return Err(StudioError::Config(format!(
                "非法的线索范围 '{}'，应为 pov / reader",
                v.trim()
            )));
        }
        thread.scope = (v.trim() == "pov").then(|| "pov".to_string());
        changed.push("scope");
    }

    file.save(&threads_path)?;
    Ok(changed)
}

/// 推进叙事线索的状态。
///
/// status 映射到对应的叙事锚点：
/// - `planted`  → planted_at = scene_id
/// - `revealed` → revealed_at = scene_id
/// - `resolved` → resolved_at = scene_id
/// - `abandoned`→ abandoned = true（忽略 scene_id）
///
/// 前向推进保留已有锚点（不回退），符合叙事历史记录语义。
/// planted/revealed/resolved 需要传 scene_id 作为叙事锚点。
pub fn advance_thread(
    paths: &ProjectPaths,
    thread_id: &str,
    status: &str,
    scene_id: Option<&str>,
) -> Result<&'static str, StudioError> {
    advance_threads(
        paths,
        &[(thread_id.to_string(), status.to_string(), scene_id.map(str::to_string))],
    )
    .map(|mut v| v.remove(0))
}

/// 批量推进线索状态：一次读写 threads.toml（单条与批量共用）。
///
/// items 为 (thread_id, status, scene_id) 列表；任一条校验失败
/// （状态非法 / 缺锚点 / 回退 / 线索不存在）则整体报错，不部分落盘。
/// revealed/resolved 触发的 reader 联动知情在同一文件上统一补记。
pub fn advance_threads(
    paths: &ProjectPaths,
    items: &[(String, String, Option<String>)],
) -> Result<Vec<&'static str>, StudioError> {
    if items.is_empty() {
        return Err(StudioError::Config("advances 不能为空".to_string()));
    }
    let threads_path = paths.threads_toml();
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;

    let mut results = Vec::with_capacity(items.len());
    // (thread_id, scene) — revealed/resolved 需要联动 reader 知情的条目
    let mut reader_notes: Vec<(String, String)> = Vec::new();
    for (thread_id, status, scene_id) in items {
        let new_status = apply_thread_advance(&mut file, thread_id, status, scene_id.as_deref())?;
        if matches!(new_status, "revealed" | "resolved") {
            if let Some(anchor) = scene_id {
                let scene = if anchor.starts_with("sc_") {
                    Some(anchor.clone())
                } else if anchor.starts_with("f_") {
                    scene_of_fragment_id(paths, anchor)
                } else {
                    None
                };
                if let Some(scene) = scene {
                    reader_notes.push((thread_id.clone(), scene));
                }
            }
        }
        results.push(new_status);
    }

    // 揭示/收束时联动 reader（默认观察者）的知晓状态：自动置为 known。
    // 线索既已对读者揭开，逐章手工补 set_thread_knowledge(reader) 属于
    // 纯机械操作；角色层知情涉及逐人判断，保留手工维护。
    for (thread_id, scene) in &reader_notes {
        let _ = apply_thread_knowledge(&mut file, thread_id, "reader", "known", scene);
    }

    file.save(&threads_path)?;
    Ok(results)
}

/// 在已加载的线索文件上推进单条线索（校验 + 变更），不落盘。
fn apply_thread_advance(
    file: &mut ThreadsFile,
    thread_id: &str,
    status: &str,
    scene_id: Option<&str>,
) -> Result<&'static str, StudioError> {
    let normalized = match status {
        "planted" | "revealed" | "resolved" | "abandoned" => status,
        _ => {
            return Err(StudioError::Config(format!(
                "非法的线索状态 '{status}'，应为 planted / revealed / resolved / abandoned"
            )))
        }
    };

    // 非 abandoned 状态需要 scene_id 锚点
    if normalized != "abandoned" && scene_id.map(|s| s.is_empty()).unwrap_or(true) {
        return Err(StudioError::Config(format!(
            "推进到 '{normalized}' 状态需要提供 scene_id 锚点"
        )));
    }

    let thread = file
        .threads
        .iter_mut()
        .find(|t| t.id == thread_id)
        .ok_or_else(|| StudioError::NotFound(format!("线索 '{thread_id}' 不存在")))?;

    // 单向推进（与 set_chapter_outline_status 同语义）：回退直接报错，
    // 避免静默改写历史锚点（如 revealed 后重设 planted_at）
    let current = if thread.resolved_at.is_some() {
        "resolved"
    } else if thread.revealed_at.is_some() {
        "revealed"
    } else {
        "planted"
    };
    let rank = |st: &str| match st {
        "planted" => 0u8,
        "revealed" => 1,
        _ => 2,
    };
    if normalized != "abandoned" && rank(normalized) < rank(current) {
        return Err(StudioError::Config(format!(
            "线索状态只能单向推进，当前为 {current}，不能回退到 {normalized}"
        )));
    }

    let new_status: &'static str = match normalized {
        "planted" => {
            thread.planted_at = Some(scene_id.unwrap().to_string());
            "planted"
        }
        "revealed" => {
            thread.revealed_at = Some(scene_id.unwrap().to_string());
            "revealed"
        }
        "resolved" => {
            thread.resolved_at = Some(scene_id.unwrap().to_string());
            "resolved"
        }
        "abandoned" => {
            thread.abandoned = true;
            "abandoned"
        }
        // unreachable：上面已校验
        _ => unreachable!(),
    };

    Ok(new_status)
}

/// 删除叙事线索（按 id）。
///
/// 软删除：只删 thread 自身，悬空引用（如 outline.storyline.thread）由 compiler 诊断暴露。
pub fn delete_thread(paths: &ProjectPaths, thread_id: &str) -> Result<(), StudioError> {
    let threads_path = paths.threads_toml();
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;

    let before = file.threads.len();
    file.threads.retain(|t| t.id != thread_id);
    if file.threads.len() == before {
        return Err(StudioError::NotFound(format!("线索 '{thread_id}' 不存在")));
    }

    file.save(&threads_path)?;
    Ok(())
}

/// 比较两个 ObserverDef 是否相同（ObserverDef 未派生 PartialEq，手写比较）。
fn observer_matches(
    a: &story_project_format::threads_file::ObserverDef,
    b: &story_project_format::threads_file::ObserverDef,
) -> bool {
    use story_project_format::threads_file::ObserverDef;
    matches!(
        (a, b),
        (ObserverDef::Builtin(sa), ObserverDef::Builtin(sb)) if sa == sb
    ) || matches!(
        (a, b),
        (ObserverDef::Character { character: ca }, ObserverDef::Character { character: cb }) if ca == cb
    ) || matches!(
        (a, b),
        (ObserverDef::Organization { organization: oa }, ObserverDef::Organization { organization: ob }) if oa == ob
    )
}

/// 将原始 observer 字符串解析为 ObserverDef。
///
/// - `"reader"` / `"author"` / `"omniscient"` → Builtin
/// - `"c_XXXX"` → Character
/// - `"o_XXXX"` → Organization
fn parse_observer(s: &str) -> story_project_format::threads_file::ObserverDef {
    use story_project_format::threads_file::ObserverDef;
    if s.starts_with("c_") {
        ObserverDef::Character {
            character: s.to_string(),
        }
    } else if s.starts_with("o_") {
        ObserverDef::Organization {
            organization: s.to_string(),
        }
    } else {
        ObserverDef::Builtin(s.to_string())
    }
}

/// 设置线索的可见性记录。
///
/// 在指定场景为指定观察者记录知晓程度的变化（追加到时间线）。
/// 如果同观察者同场景已有记录，则覆盖（更新而非重复）。
///
/// - `observer`：原始字符串（`"reader"` / `"c_001"` / `"o_001"` 等）
/// - `state`：`unknown` / `suspected` / `known`
/// - `scene_id`：叙事锚点场景 ID
pub fn set_thread_knowledge(
    paths: &ProjectPaths,
    thread_id: &str,
    observer: &str,
    state: &str,
    scene_id: &str,
) -> Result<(), StudioError> {
    set_thread_knowledge_batch(
        paths,
        &[(
            thread_id.to_string(),
            observer.to_string(),
            state.to_string(),
            scene_id.to_string(),
        )],
    )
}

/// 批量记录线索可见性：一次读写 threads.toml（单条与批量共用）。
///
/// items 为 (thread_id, observer, state, scene_id) 列表；任一条失败
/// （state 非法 / 线索不存在）则整体报错，不部分落盘。
pub fn set_thread_knowledge_batch(
    paths: &ProjectPaths,
    items: &[(String, String, String, String)],
) -> Result<(), StudioError> {
    if items.is_empty() {
        return Err(StudioError::Config("records 不能为空".to_string()));
    }
    let threads_path = paths.threads_toml();
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;
    for (thread_id, observer, state, scene_id) in items {
        apply_thread_knowledge(&mut file, thread_id, observer, state, scene_id)?;
    }
    file.save(&threads_path)?;
    Ok(())
}

/// 删除线索的可见性记录。
///
/// - `observer`：原始字符串（`"reader"` / `"c_001"` / `"o_001"` 等）
/// - `scene_id`：`Some` 时只删该场景的时间线记录（删空后移除整条），
///   `None` 时删除该观察者的整条记录
///
/// 返回是否实际删除了内容（线索/观察者不存在或时间线无该场景时为 false）。
pub fn remove_thread_knowledge(
    paths: &ProjectPaths,
    thread_id: &str,
    observer: &str,
    scene_id: Option<&str>,
) -> Result<bool, StudioError> {
    let threads_path = paths.threads_toml();
    let mut file: ThreadsFile = ThreadsFile::load_or_default(&threads_path)?;

    let thread = file
        .threads
        .iter_mut()
        .find(|t| t.id == thread_id)
        .ok_or_else(|| StudioError::NotFound(format!("线索 '{thread_id}' 不存在")))?;

    let target = parse_observer(observer);
    let idx = thread
        .knowledge
        .iter()
        .position(|k| observer_matches(&k.observer, &target));
    let Some(idx) = idx else {
        return Ok(false);
    };

    let removed = match scene_id {
        None => {
            thread.knowledge.remove(idx);
            true
        }
        Some(scene) => {
            let timeline = &mut thread.knowledge[idx].state_timeline;
            let before = timeline.len();
            timeline.retain(|e| e.scene != scene);
            let removed = timeline.len() < before;
            if timeline.is_empty() {
                thread.knowledge.remove(idx);
            }
            removed
        }
    };

    if removed {
        file.save(&threads_path)?;
    }
    Ok(removed)
}

/// 在已加载的线索文件上记录一条可见性（校验 + 变更），不落盘。
fn apply_thread_knowledge(
    file: &mut ThreadsFile,
    thread_id: &str,
    observer: &str,
    state: &str,
    scene_id: &str,
) -> Result<(), StudioError> {
    use story_project_format::threads_file::{KnowledgeDef, StateTimelineEntry};

    // 校验 state 合法值
    if !matches!(state, "unknown" | "suspected" | "known") {
        return Err(StudioError::Config(format!(
            "非法的知晓程度 '{state}'，应为 unknown / suspected / known"
        )));
    }

    let target_observer = parse_observer(observer);

    let thread = file
        .threads
        .iter_mut()
        .find(|t| t.id == thread_id)
        .ok_or_else(|| StudioError::NotFound(format!("线索 '{thread_id}' 不存在")))?;

    // 查找已有的 knowledge 条目
    let entry_idx = thread
        .knowledge
        .iter()
        .position(|k| observer_matches(&k.observer, &target_observer));

    match entry_idx {
        Some(idx) => {
            // 已有条目：在同 scene 的记录上覆盖，否则追加
            let timeline = &mut thread.knowledge[idx].state_timeline;
            if let Some(existing) = timeline.iter_mut().find(|e| e.scene == scene_id) {
                existing.state = state.to_string();
            } else {
                timeline.push(StateTimelineEntry {
                    scene: scene_id.to_string(),
                    state: state.to_string(),
                });
            }
            // 按 scene 排序，保证时间线有序
            timeline.sort_by(|a, b| a.scene.cmp(&b.scene));
        }
        None => {
            // 新观察者：创建条目
            thread.knowledge.push(KnowledgeDef {
                observer: target_observer,
                state_timeline: vec![StateTimelineEntry {
                    scene: scene_id.to_string(),
                    state: state.to_string(),
                }],
            });
        }
    }

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;

    /// 创建临时项目用于测试
    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    #[test]
    fn create_and_delete_character() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 创建
        let id = create_entity(&paths, EntityKind::Character, "林风").unwrap();
        assert!(id.starts_with("c_"));
        let file_path = paths.characters_dir().join(format!("{id}.toml"));
        assert!(file_path.exists(), "角色文件应已创建");

        // 验证内容
        let content = std::fs::read_to_string(&file_path).unwrap();
        let entity = EntityFile::from_toml_str(&content).unwrap();
        assert_eq!(entity.display_name, "林风");
        assert!(entity.profile.is_some());

        // 删除
        delete_entity(&paths, &id).unwrap();
        assert!(!file_path.exists(), "角色文件应已删除");
    }

    #[test]
    fn delete_entity_cleans_relations_pointing_to_it() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 三个角色：A 删除，B/C 作为对端保留
        let a = create_entity(&paths, EntityKind::Character, "甲").unwrap();
        let b = create_entity(&paths, EntityKind::Character, "乙").unwrap();
        let c = create_entity(&paths, EntityKind::Character, "丙").unwrap();

        // A → B（A 作为 from）
        let r1 = add_relation(&paths, &a, &b, "mentor_of").unwrap();
        // C → A（A 作为 to）
        let r2 = add_relation(&paths, &c, &a, "rival_of").unwrap();
        // B → C（与 A 无关，应保留）
        let r3 = add_relation(&paths, &b, &c, "knows").unwrap();

        // 删除 A：以 A 为端点的两条边应被级联清理，B↔C 应保留
        delete_entity(&paths, &a).unwrap();

        let file: RelationsFile = RelationsFile::load_or_default(&paths.relations_toml()).unwrap();
        let ids: Vec<&str> = file.relations.iter().map(|r| r.id.as_str()).collect();
        assert!(
            !ids.contains(&r1.as_str()),
            "以被删实体为 from 的关系应被清理"
        );
        assert!(
            !ids.contains(&r2.as_str()),
            "以被删实体为 to 的关系应被清理"
        );
        assert!(ids.contains(&r3.as_str()), "与被删实体无关的关系应保留");
    }

    #[test]
    fn merge_entities_redirects_all_references() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 占位实体（凶手）与正式角色
        let placeholder = create_entity(&paths, EntityKind::Character, "九一真凶").unwrap();
        let real = create_entity(&paths, EntityKind::Character, "康明远").unwrap();
        let other = create_entity(&paths, EntityKind::Character, "沈屹").unwrap();

        // 片段锚点（placeholder appears + real mentioned → 合并后同实体去重保留 appears）
        let ch = create_chapter(&paths, "第一章").unwrap();
        let sc = create_scene(&paths, &ch, "场景", Some(&real)).unwrap();
        let fid = create_fragment(&paths, &sc, "摆的人露出了马脚。康明远下令移交。").unwrap();
        set_fragment_anchor(&paths, &fid, &placeholder, "appears").unwrap();
        set_fragment_anchor(&paths, &fid, &real, "mentioned").unwrap();

        // 关系：other→placeholder、placeholder→real（合并后成自环应删除）
        add_relation(&paths, &other, &placeholder, "enemy_of").unwrap();
        add_relation(&paths, &placeholder, &real, "knows").unwrap();

        // 场景 POV 指向 placeholder
        let sc2 = create_scene(&paths, &ch, "场景二", Some(&placeholder)).unwrap();

        // 断言 subject（a_001）与 object 为实体引用的断言（a_002：other 持有 placeholder）
        let mut assertions = AssertionsFile::load_or_default(&paths.assertions_toml()).unwrap();
        assertions.assertions.push(AssertionDef {
            id: "a_001".into(),
            subject: placeholder.clone(),
            predicate: "is".into(),
            object: toml::Value::String("真凶".into()),
            status: String::new(),
            extracted_by: String::new(),
            evidence: vec![],
            valid: None,
        });
        assertions.assertions.push(AssertionDef {
            id: "a_002".into(),
            subject: other.clone(),
            predicate: "holds".into(),
            object: toml::Value::String(placeholder.clone()),
            status: String::new(),
            extracted_by: String::new(),
            evidence: vec![],
            valid: None,
        });
        assertions.save(&paths.assertions_toml()).unwrap();

        // 线索：观察者为 placeholder
        let t_id = create_thread(&paths, "真凶线", "描述", "main", Some(&sc2), None, None).unwrap();
        let mut threads = ThreadsFile::load_or_default(&paths.threads_toml()).unwrap();
        threads.threads[0].knowledge.push(
            story_project_format::threads_file::KnowledgeDef {
                observer: story_project_format::threads_file::ObserverDef::Character {
                    character: placeholder.clone(),
                },
                state_timeline: vec![],
            },
        );
        threads.save(&paths.threads_toml()).unwrap();

        // 章节脉络 characters
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters.push(ChapterOutlineDef {
            chapter_id: Some(ch.clone()),
            title: "第一章".into(),
            volume_order: 1,
            summary: None,
            goal: None,
            beats: vec![],
            characters: vec![placeholder.clone(), other.clone()],
            threads: vec![],
            scenes: vec![],
            hook: None,
            notes: None,
            status: "planned".into(),
        });
        save_chapters_file(&paths, &chapters).unwrap();

        // outline 弧线
        let mut outline = OutlineFile::load_or_default(&paths.outline_toml()).unwrap();
        outline.arcs.push(story_project_format::outline_file::ArcDef {
            character: placeholder.clone(),
            name: "真凶弧线".into(),
            stages: None,
            summary: None,
        });
        outline.save(&paths.outline_toml()).unwrap();

        // 未锚定指代台账 target
        let mut anchoring = AnchoringFile::load_or_default(&paths.anchoring_toml()).unwrap();
        anchoring.refs.push(UnresolvedRefDef {
            fragment_id: fid.clone(),
            quote: "摆的人".into(),
            hint: "疑似凶手".into(),
            status: "resolved".into(),
            target: Some(placeholder.clone()),
        });
        anchoring.save(&paths.anchoring_toml()).unwrap();

        // 执行合并
        let report = merge_entities(&paths, &placeholder, &real).unwrap();
        assert_eq!(report.anchors, 1);
        assert_eq!(report.relation_edges, 2);
        assert_eq!(report.self_loops_removed, 1);
        assert_eq!(report.assertions, 2);
        assert_eq!(report.povs, 1);
        assert_eq!(report.chapter_character_lists, 1);
        assert!(report.thread_refs >= 1);
        assert_eq!(report.outline_refs, 1);
        assert_eq!(report.unresolved_targets, 1);
        assert_eq!(report.aliases_absorbed, 1);

        // from 文件已删除；into 别名吸收了占位名
        assert!(!paths.characters_dir().join(format!("{placeholder}.toml")).exists());
        let real_content =
            std::fs::read_to_string(paths.characters_dir().join(format!("{real}.toml"))).unwrap();
        let real_entity = EntityFile::from_toml_str(&real_content).unwrap();
        assert!(real_entity.aliases.contains(&"九一真凶".to_string()));

        // 片段锚点：同实体去重且保留高优先级 role
        let frag =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{fid}.md"))).unwrap();
        assert!(frag.contains(&format!("entity: {real}")), "{frag}");
        assert!(frag.contains("role: appears"), "{frag}");
        assert!(!frag.contains(&placeholder), "{frag}");

        // 关系：other→real 保留；placeholder→real 合并后自环删除
        let relations = RelationsFile::load_or_default(&paths.relations_toml()).unwrap();
        assert_eq!(relations.relations.len(), 1);
        assert_eq!(relations.relations[0].from, other);
        assert_eq!(relations.relations[0].to, real);

        // POV / 断言 / 线索观察者 / 章节脉络 / outline / 台账全部改指
        let structure = load_structure(&paths).unwrap();
        assert_eq!(structure.scene[&sc2].pov.as_deref(), Some(real.as_str()));

        let assertions = AssertionsFile::load_or_default(&paths.assertions_toml()).unwrap();
        assert_eq!(assertions.assertions[0].subject, real);
        let holds = assertions
            .assertions
            .iter()
            .find(|a| a.id == "a_002")
            .unwrap();
        assert_eq!(holds.object, toml::Value::String(real.clone()));

        let threads = ThreadsFile::load_or_default(&paths.threads_toml()).unwrap();
        let t = threads.threads.iter().find(|t| t.id == t_id).unwrap();
        assert!(
            threads.threads.iter().all(|t| {
                t.knowledge.iter().all(|k| {
                    !matches!(
                        &k.observer,
                        story_project_format::threads_file::ObserverDef::Character { character }
                            if character == &placeholder
                    )
                })
            }),
            "observer 不应再引用被合并实体"
        );
        assert!(t.knowledge.len() >= 2);

        let chapters = load_chapters_file(&paths).unwrap();
        assert_eq!(chapters.chapters[0].characters, vec![other, real.clone()]);

        let outline = OutlineFile::load_or_default(&paths.outline_toml()).unwrap();
        assert_eq!(outline.arcs[0].character, real);

        let anchoring = AnchoringFile::load_or_default(&paths.anchoring_toml()).unwrap();
        assert_eq!(anchoring.refs[0].target.as_deref(), Some(real.as_str()));
    }

    #[test]
    fn merge_entities_rejects_same_or_cross_kind() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let a = create_entity(&paths, EntityKind::Character, "甲").unwrap();
        let b = create_entity(&paths, EntityKind::Character, "乙").unwrap();
        let loc = create_entity(&paths, EntityKind::Location, "某地").unwrap();

        assert!(merge_entities(&paths, &a, &a).is_err(), "相同实体不能合并");
        assert!(merge_entities(&paths, &a, &loc).is_err(), "跨类型不能合并");
        assert!(merge_entities(&paths, "c_9999", &b).is_err(), "不存在的实体应报错");
    }

    #[test]
    fn resolve_create_with_profile_fields() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "第一章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let body = "摆的人沉默地放下了茶盏。";
        let fid = create_fragment(&paths, &sc_id, body).unwrap();
        sync_unresolved_refs(
            &paths,
            &fid,
            body,
            &[("摆的人".to_string(), "疑似新角色".to_string())],
        )
        .unwrap();

        let summary = resolve_unresolved_ref(
            &paths,
            &fid,
            "摆的人",
            "create",
            None,
            Some("九一真凶"),
            None,
            Some(&CharacterProfileInput {
                gender: Some("男".into()),
                occupation: Some("当铺朝奉".into()),
                background: Some("十年前欠下一笔命债".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        assert!(summary.contains("档案字段已写入"), "{summary}");

        // 新档案一步带上了档案字段（初始项目无角色，目录里只有这一个文件）
        let mut files: Vec<_> = std::fs::read_dir(paths.characters_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .collect();
        assert_eq!(files.len(), 1, "应只建了一个角色");
        let content = std::fs::read_to_string(files.remove(0).path()).unwrap();
        let entity = EntityFile::from_toml_str(&content).unwrap();
        assert_eq!(entity.display_name, "九一真凶");
        let profile = entity.profile.unwrap();
        assert_eq!(profile.gender.as_deref(), Some("男"));
        assert_eq!(profile.occupation.as_deref(), Some("当铺朝奉"));
        assert_eq!(profile.background.as_deref(), Some("十年前欠下一笔命债"));
        assert!(profile.appearance.is_none(), "未提供的字段保持为空");
    }

    #[test]
    fn chapter_completion_hint_fires_when_all_scenes_written() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "第一章").unwrap();
        let sc1 = create_scene(&paths, &ch_id, "场景一", None).unwrap();
        let sc2 = create_scene(&paths, &ch_id, "场景二", None).unwrap();
        let f1 = create_fragment(&paths, &sc1, "第一场正文。").unwrap();

        // 场景二还没正文 → 不提示
        assert!(chapter_completion_hint(&paths, &f1).is_none());

        // 全部场景有正文、但章节脉络没有对应条目（无从标记）→ 不提示
        let f2 = create_fragment(&paths, &sc2, "第二场正文。").unwrap();
        assert!(chapter_completion_hint(&paths, &sc1).is_none());

        // 补上脉络条目 → 提示（含 index 与工具名）
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters.push(ChapterOutlineDef {
            chapter_id: Some(ch_id.clone()),
            title: "第一章".into(),
            volume_order: 1,
            status: "planned".into(),
            ..Default::default()
        });
        save_chapters_file(&paths, &chapters).unwrap();
        let hint =
            chapter_completion_hint(&paths, &f2).expect("全部场景有正文且未完成时应提示");
        assert!(hint.contains("update_chapter_outline"), "{hint}");
        assert!(hint.contains("index=1"), "{hint}");

        // 已完成 → 不再提示
        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters[0].status = "completed".into();
        save_chapters_file(&paths, &chapters).unwrap();
        assert!(chapter_completion_hint(&paths, &sc1).is_none());
    }

    #[test]
    fn create_and_delete_location() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_entity(&paths, EntityKind::Location, "青江渡口").unwrap();
        assert!(id.starts_with("l_"));

        let content =
            std::fs::read_to_string(paths.locations_dir().join(format!("{id}.toml"))).unwrap();
        let entity = EntityFile::from_toml_str(&content).unwrap();
        assert_eq!(entity.display_name, "青江渡口");
        assert!(entity.geography.is_some());

        delete_entity(&paths, &id).unwrap();
    }

    #[test]
    fn create_and_delete_item() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_entity(&paths, EntityKind::Item, "寒霜剑").unwrap();
        assert!(id.starts_with("i_"));

        let content =
            std::fs::read_to_string(paths.items_dir().join(format!("{id}.toml"))).unwrap();
        let entity = EntityFile::from_toml_str(&content).unwrap();
        assert_eq!(entity.display_name, "寒霜剑");
        assert!(entity.item.is_some());

        delete_entity(&paths, &id).unwrap();
    }

    #[test]
    fn create_and_delete_organization() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_entity(&paths, EntityKind::Organization, "天机阁").unwrap();
        assert!(id.starts_with("o_"));

        let content =
            std::fs::read_to_string(paths.organizations_dir().join(format!("{id}.toml"))).unwrap();
        let entity = EntityFile::from_toml_str(&content).unwrap();
        assert_eq!(entity.display_name, "天机阁");
        assert!(entity.org.is_some());

        delete_entity(&paths, &id).unwrap();
    }

    #[test]
    fn delete_nonexistent_entity_fails() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let result = delete_entity(&paths, "c_9999");
        assert!(result.is_err());
    }

    #[test]
    fn create_chapter_and_verify_structure() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_chapter(&paths, "第一章 雨夜").unwrap();
        assert_eq!(id, "ch_01");

        let structure = load_structure(&paths).unwrap();
        assert!(structure.book.chapters.contains(&id));
        assert_eq!(structure.chapter[&id].title, "第一章 雨夜");
        assert!(structure.chapter[&id].scenes.is_empty());
    }

    #[test]
    fn rename_chapter_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_chapter(&paths, "旧标题").unwrap();
        rename_chapter(&paths, &id, "新标题").unwrap();

        let structure = load_structure(&paths).unwrap();
        assert_eq!(structure.chapter[&id].title, "新标题");
    }

    #[test]
    fn delete_chapter_cascades() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "片段正文").unwrap();

        // 确认文件存在
        assert!(paths.fragments_dir().join(format!("{frag_id}.md")).exists());

        // 删除章节应级联删除场景和片段
        delete_chapter(&paths, &ch_id).unwrap();

        let structure = load_structure(&paths).unwrap();
        assert!(!structure.book.chapters.contains(&ch_id));
        assert!(!structure.chapter.contains_key(&ch_id));
        assert!(!structure.scene.contains_key(&sc_id));
        assert!(
            !paths.fragments_dir().join(format!("{frag_id}.md")).exists(),
            "片段文件应被级联删除"
        );
    }

    #[test]
    fn create_scene_with_pov() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "章").unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "主角").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", Some(&char_id)).unwrap();

        let structure = load_structure(&paths).unwrap();
        let scene = &structure.scene[&sc_id];
        assert_eq!(scene.title.as_deref(), Some("场景"));
        assert_eq!(scene.pov.as_deref(), Some(char_id.as_str()));
    }

    #[test]
    fn delete_scene_cascades_fragments() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "内容").unwrap();

        assert!(paths.fragments_dir().join(format!("{frag_id}.md")).exists());

        delete_scene(&paths, &sc_id).unwrap();

        let structure = load_structure(&paths).unwrap();
        assert!(!structure.scene.contains_key(&sc_id));
        assert!(!structure.chapter[&ch_id].scenes.contains(&sc_id));
        assert!(
            !paths.fragments_dir().join(format!("{frag_id}.md")).exists(),
            "片段文件应被级联删除"
        );
    }

    #[test]
    fn create_and_delete_fragment() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "正文内容").unwrap();

        // 验证文件内容
        let content =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{frag_id}.md"))).unwrap();
        assert!(content.contains("---\nid:"));
        assert!(content.contains("正文内容"));

        // 验证 structure 中关联
        let structure = load_structure(&paths).unwrap();
        assert!(structure.scene[&sc_id].fragments.contains(&frag_id));

        // 删除片段
        delete_fragment(&paths, &frag_id).unwrap();
        assert!(!paths.fragments_dir().join(format!("{frag_id}.md")).exists());

        // 验证 structure 中已移除
        let structure = load_structure(&paths).unwrap();
        assert!(!structure.scene[&sc_id].fragments.contains(&frag_id));
    }

    #[test]
    fn write_fragment_preserves_frontmatter() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "原始内容").unwrap();

        // 手动添加 frontmatter 扩展字段
        let frag_path = paths.fragments_dir().join(format!("{frag_id}.md"));
        let enhanced = format!(
            "---\nid: {frag_id}\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n原始内容\n"
        );
        std::fs::write(&frag_path, &enhanced).unwrap();

        // 覆盖正文
        write_fragment_text(&paths, &frag_id, "新正文").unwrap();

        let content = std::fs::read_to_string(&frag_path).unwrap();
        assert!(content.contains("anchors:"), "frontmatter 应被保留");
        assert!(content.contains("新正文"), "正文应已更新");
        assert!(!content.contains("原始内容"), "旧正文应已被替换");
    }

    #[test]
    fn append_fragment() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "第一段").unwrap();

        append_fragment_text(&paths, &frag_id, "第二段").unwrap();

        let content =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{frag_id}.md"))).unwrap();
        assert!(content.contains("第一段"));
        assert!(content.contains("第二段"));
    }

    #[test]
    fn add_and_remove_evidence() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 先创建角色和片段（作为断言的 subject 和 evidence）
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "正文").unwrap();

        // 添加断言（带证据）——片段 ID 自动登记为 evidence.toml 条目
        let outcome = add_assertion(&paths, &char_id, "alive", "true", Some(&frag_id)).unwrap();
        let a_id = outcome.id;
        let ev_id = {
            let file: AssertionsFile = AssertionsFile::load_or_default(
                &paths.knowledge_dir().join("assertions.toml"),
            )
            .unwrap();
            file.assertions
                .iter()
                .find(|a| a.id == a_id)
                .unwrap()
                .evidence[0]
                .clone()
        };
        assert!(ev_id.starts_with("ev_"));

        // 验证：断言引用证据 ID（ev_XXXX），证据条目指向片段
        let path = paths.knowledge_dir().join("assertions.toml");
        let file: AssertionsFile = AssertionsFile::load_or_default(&path).unwrap();
        let a = file.assertions.iter().find(|a| a.id == a_id).unwrap();
        assert!(a.evidence.contains(&ev_id));
        assert!(!a.evidence.contains(&frag_id));
        let ev_file = EvidenceFile::load_or_default(&paths.knowledge_dir().join("evidence.toml"))
            .unwrap();
        assert!(ev_file.evidence.iter().any(|e| e.id == ev_id && e.loc == frag_id));

        // 移除证据
        remove_evidence(&paths, &a_id, &frag_id).unwrap();

        let file: AssertionsFile = AssertionsFile::load_or_default(&path).unwrap();
        let a = file.assertions.iter().find(|a| a.id == a_id).unwrap();
        assert!(!a.evidence.contains(&ev_id));
    }

    #[test]
    fn create_thread_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_thread(&paths, "伏笔", "测试描述", "main", None, None, None).unwrap();
        assert_eq!(id, "t_001");

        let path = paths.root().join("threads.toml");
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.title, "伏笔");
        assert_eq!(t.priority, "main");
    }

    #[test]
    fn update_thread_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_thread(&paths, "原标题", "原描述", "minor", None, None, None).unwrap();

        // 修改部分字段
        let changed = update_thread(
            &paths,
            &id,
            Some("新标题"),
            None,
            Some("击败反派"),
            Some("main"),
            Some(&["foreshadowing".into(), "identity".into()]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(changed, vec!["title", "goal", "priority", "tags"]);

        let path = paths.threads_toml();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.title, "新标题");
        assert_eq!(t.description.as_deref(), Some("原描述")); // 未修改
        assert_eq!(t.goal.as_deref(), Some("击败反派"));
        assert_eq!(t.priority, "main");
        assert_eq!(t.tags, vec!["foreshadowing", "identity"]);

        // 非法 priority 被拒绝
        let err =
            update_thread(&paths, &id, None, None, None, Some("bogus"), None, None, None).unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 修改 description；goal 传空串 = 清除
        let changed = update_thread(
            &paths,
            &id,
            None,
            Some("  新描述  "),
            Some(""),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(changed, vec!["description", "goal"]);
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.description.as_deref(), Some("新描述"));
        assert_eq!(t.goal, None);

        // description 传空串 = 清除；title 空串被拒绝
        update_thread(&paths, &id, None, Some(""), None, None, None, None, None).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(
            file.threads.iter().find(|t| t.id == id).unwrap().description,
            None
        );
        let err = update_thread(&paths, &id, Some("  "), None, None, None, None, None, None).unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 不存在的线索
        let err =
            update_thread(&paths, "t_999", Some("x"), None, None, None, None, None, None).unwrap_err();
        assert!(matches!(err, StudioError::NotFound(_)));

        // due_volume 设置与清除
        update_thread(&paths, &id, None, None, None, None, None, Some(Some(2)), None).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(
            file.threads.iter().find(|t| t.id == id).unwrap().due_volume,
            Some(2)
        );
        update_thread(&paths, &id, None, None, None, None, None, Some(None), None).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(
            file.threads.iter().find(|t| t.id == id).unwrap().due_volume,
            None
        );

        // scope：pov = 暗线；reader/空串 = 恢复明线（字段清除）
        update_thread(&paths, &id, None, None, None, None, None, None, Some("pov")).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(
            file.threads.iter().find(|t| t.id == id).unwrap().scope.as_deref(),
            Some("pov")
        );
        update_thread(&paths, &id, None, None, None, None, None, None, Some("reader")).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(
            file.threads.iter().find(|t| t.id == id).unwrap().scope,
            None
        );
        // 非法值被拒绝
        let err = update_thread(&paths, &id, None, None, None, None, None, None, Some("bogus"))
            .unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));
    }

    #[test]
    fn advance_thread_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 创建时 planted_at = None（planned 状态）
        let id = create_thread(&paths, "伏笔", "测试", "main", None, None, None).unwrap();

        // planted → planted_at 被设置，保留之前的 None 不影响
        let s = advance_thread(&paths, &id, "planted", Some("sc_01_02")).unwrap();
        assert_eq!(s, "planted");

        // revealed → revealed_at 被设置，planted_at 保留
        let s = advance_thread(&paths, &id, "revealed", Some("sc_03_01")).unwrap();
        assert_eq!(s, "revealed");

        // resolved → resolved_at 被设置
        let s = advance_thread(&paths, &id, "resolved", Some("sc_05_03")).unwrap();
        assert_eq!(s, "resolved");

        let path = paths.threads_toml();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.planted_at.as_deref(), Some("sc_01_02"));
        assert_eq!(t.revealed_at.as_deref(), Some("sc_03_01"));
        assert_eq!(t.resolved_at.as_deref(), Some("sc_05_03"));

        // abandoned 独立标记（忽略 scene_id）
        let s = advance_thread(&paths, &id, "abandoned", None).unwrap();
        assert_eq!(s, "abandoned");
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert!(t.abandoned);
    }

    #[test]
    fn advance_thread_validates_status() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_thread(&paths, "伏笔", "测试", "minor", None, None, None).unwrap();

        // 非法 status
        let err = advance_thread(&paths, &id, "bogus", Some("sc_01")).unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 非 abandoned 缺少 scene_id
        let err = advance_thread(&paths, &id, "planted", None).unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 空 scene_id 也算缺失
        let err = advance_thread(&paths, &id, "resolved", Some("")).unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 不存在的线索
        let err = advance_thread(&paths, "t_999", "planted", Some("sc_01")).unwrap_err();
        assert!(matches!(err, StudioError::NotFound(_)));
    }

    #[test]
    fn advance_threads_and_knowledge_batch() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let t1 = create_thread(&paths, "真凶线", "描述", "main", None, None, None).unwrap();
        let t2 = create_thread(&paths, "旧案线", "描述", "minor", None, None, None).unwrap();

        // 批量推进：多条线索一次落盘
        let results = advance_threads(
            &paths,
            &[
                (t1.clone(), "revealed".into(), Some("sc_02_01".into())),
                (t2.clone(), "planted".into(), Some("sc_02_02".into())),
            ],
        )
        .unwrap();
        assert_eq!(results, vec!["revealed", "planted"]);

        let file: ThreadsFile = ThreadsFile::load_or_default(&paths.threads_toml()).unwrap();
        let t = file.threads.iter().find(|t| t.id == t1).unwrap();
        assert_eq!(t.revealed_at.as_deref(), Some("sc_02_01"));
        // revealed 联动 reader 知情
        let reader = t
            .knowledge
            .iter()
            .find(|k| {
                matches!(
                    &k.observer,
                    story_project_format::threads_file::ObserverDef::Builtin(s) if s == "reader"
                )
            })
            .expect("revealed 应联动 reader 知情");
        assert!(
            reader
                .state_timeline
                .iter()
                .any(|e| e.scene == "sc_02_01" && e.state == "known")
        );

        // 批量可见性：多条记录一次落盘
        set_thread_knowledge_batch(
            &paths,
            &[
                (t1.clone(), "c_0001".into(), "suspected".into(), "sc_02_01".into()),
                (t1.clone(), "c_0002".into(), "known".into(), "sc_02_03".into()),
            ],
        )
        .unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&paths.threads_toml()).unwrap();
        let t = file.threads.iter().find(|t| t.id == t1).unwrap();
        assert_eq!(t.knowledge.len(), 3, "reader + c_0001 + c_0002");

        // 批量中任一条失败则整体不落盘
        let err = advance_threads(
            &paths,
            &[
                (t2.clone(), "revealed".into(), Some("sc_09_01".into())),
                ("t_999".into(), "planted".into(), Some("sc_09_02".into())),
            ],
        );
        assert!(err.is_err());
        let file: ThreadsFile = ThreadsFile::load_or_default(&paths.threads_toml()).unwrap();
        let t2d = file.threads.iter().find(|t| t.id == t2).unwrap();
        assert!(t2d.revealed_at.is_none(), "批量失败时不应部分落盘");
    }

    #[test]
    fn delete_thread_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id1 = create_thread(&paths, "线索1", "描述", "main", None, None, None).unwrap();
        let id2 = create_thread(&paths, "线索2", "描述", "minor", None, None, None).unwrap();

        let path = paths.threads_toml();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(file.threads.len(), 2);

        // 删除 id1
        delete_thread(&paths, &id1).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert_eq!(file.threads.len(), 1);
        assert!(file.threads.iter().all(|t| t.id != id1));

        // 删除不存在的
        let err = delete_thread(&paths, "t_999").unwrap_err();
        assert!(matches!(err, StudioError::NotFound(_)));

        // 删除剩余的，文件保留但为空列表
        delete_thread(&paths, &id2).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        assert!(file.threads.is_empty());
    }

    #[test]
    fn set_thread_knowledge_works() {
        use story_project_format::threads_file::{ObserverDef, ThreadsFile};

        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = create_thread(&paths, "皇子身份", "秘密", "main", None, None, None).unwrap();
        let path = paths.threads_toml();

        // 1. 新观察者：reader 在 sc_01 开始怀疑
        set_thread_knowledge(&paths, &id, "reader", "suspected", "sc_01_02").unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge.len(), 1);
        assert!(matches!(&t.knowledge[0].observer, ObserverDef::Builtin(s) if s == "reader"));
        assert_eq!(t.knowledge[0].state_timeline.len(), 1);
        assert_eq!(t.knowledge[0].state_timeline[0].state, "suspected");

        // 2. 同观察者追加：reader 在 sc_05 变为 known
        set_thread_knowledge(&paths, &id, "reader", "known", "sc_05_03").unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge.len(), 1); // 仍只有 1 个观察者
        assert_eq!(t.knowledge[0].state_timeline.len(), 2); // 时间线有 2 条

        // 3. 覆盖同场景：reader 在 sc_01_02 改为 known
        set_thread_knowledge(&paths, &id, "reader", "known", "sc_01_02").unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge[0].state_timeline.len(), 2); // 不增加，覆盖
        let sc01 = t.knowledge[0]
            .state_timeline
            .iter()
            .find(|e| e.scene == "sc_01_02")
            .unwrap();
        assert_eq!(sc01.state, "known"); // 已更新

        // 4. 新观察者：角色 c_001 从一开始就知道
        set_thread_knowledge(&paths, &id, "c_001", "known", "sc_01_02").unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge.len(), 2); // 2 个观察者
        assert!(
            matches!(&t.knowledge[1].observer, ObserverDef::Character { character } if character == "c_001")
        );

        // 5. 非法 state 被拒绝
        let err = set_thread_knowledge(&paths, &id, "reader", "bogus", "sc_01").unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // 6. 不存在的线索
        let err = set_thread_knowledge(&paths, "t_999", "reader", "known", "sc_01").unwrap_err();
        assert!(matches!(err, StudioError::NotFound(_)));

        // 7. 删除单点：reader 在 sc_01_02 的记录（时间线还剩 sc_05_03）
        let removed = remove_thread_knowledge(&paths, &id, "reader", Some("sc_01_02")).unwrap();
        assert!(removed);
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge.len(), 2);
        assert_eq!(t.knowledge[0].state_timeline.len(), 1);
        assert_eq!(t.knowledge[0].state_timeline[0].scene, "sc_05_03");

        // 8. 删空时间线后移除整条：reader 只剩一条，删掉后整条消失
        remove_thread_knowledge(&paths, &id, "reader", Some("sc_05_03")).unwrap();
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert_eq!(t.knowledge.len(), 1); // 只剩 c_001

        // 9. 删除整条：不传 scene_id
        let removed = remove_thread_knowledge(&paths, &id, "c_001", None).unwrap();
        assert!(removed);
        let file: ThreadsFile = ThreadsFile::load_or_default(&path).unwrap();
        let t = file.threads.iter().find(|t| t.id == id).unwrap();
        assert!(t.knowledge.is_empty());

        // 10. 观察者/场景不存在 → false，不报错
        assert!(!remove_thread_knowledge(&paths, &id, "reader", None).unwrap());
        assert!(!remove_thread_knowledge(&paths, &id, "c_001", Some("sc_01")).unwrap());

        // 11. 不存在的线索
        let err = remove_thread_knowledge(&paths, "t_999", "reader", None).unwrap_err();
        assert!(matches!(err, StudioError::NotFound(_)));
    }

    #[test]
    fn entity_kind_from_id() {
        assert_eq!(
            EntityKind::from_id("c_0001").unwrap(),
            EntityKind::Character
        );
        assert_eq!(EntityKind::from_id("l_0001").unwrap(), EntityKind::Location);
        assert_eq!(EntityKind::from_id("i_0001").unwrap(), EntityKind::Item);
        assert_eq!(
            EntityKind::from_id("o_0001").unwrap(),
            EntityKind::Organization
        );
        assert!(EntityKind::from_id("x_0001").is_err());
    }

    #[test]
    fn entity_kind_roundtrip() {
        for kind in [
            EntityKind::Character,
            EntityKind::Location,
            EntityKind::Item,
            EntityKind::Organization,
        ] {
            assert_eq!(EntityKind::from_type_str(kind.type_str()).unwrap(), kind);
        }
    }

    #[test]
    fn sequential_entity_ids() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id1 = create_entity(&paths, EntityKind::Character, "A").unwrap();
        let id2 = create_entity(&paths, EntityKind::Character, "B").unwrap();

        assert_ne!(id1, id2, "连续创建的 ID 应不同");

        // 解析十六进制编号验证递增
        let n1 = u32::from_str_radix(&id1[2..], 16).unwrap();
        let n2 = u32::from_str_radix(&id2[2..], 16).unwrap();
        assert_eq!(n2, n1 + 1);
    }

    #[test]
    fn delete_assertion_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id1 = add_assertion(&paths, "c_0001", "alive", "true", None).unwrap().id;
        let id2 = add_assertion(&paths, "c_0001", "age", "25", None).unwrap().id;
        assert!(id1.starts_with("a_"));
        assert!(id2.starts_with("a_"));

        // 删除第一条
        delete_assertion(&paths, &id1).unwrap();

        // 验证只剩 id2
        let assertions_path = paths.knowledge_dir().join("assertions.toml");
        let file = AssertionsFile::load_or_default(&assertions_path).unwrap();
        assert_eq!(file.assertions.len(), 1);
        assert_eq!(file.assertions[0].id, id2);
    }

    #[test]
    fn delete_nonexistent_assertion_fails() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let result = delete_assertion(&paths, "a_9999");
        assert!(result.is_err());
    }

    #[test]
    fn set_assertion_status_works() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = add_assertion(&paths, "c_0001", "alive", "true", None).unwrap().id;

        set_assertion_status(&paths, &id, "contradicted").unwrap();

        // 验证状态已更新
        let assertions_path = paths.knowledge_dir().join("assertions.toml");
        let file = AssertionsFile::load_or_default(&assertions_path).unwrap();
        let assertion = file.assertions.iter().find(|a| a.id == id).unwrap();
        assert_eq!(assertion.status, "contradicted");
    }

    #[test]
    fn set_assertion_status_rejects_invalid_status() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = add_assertion(&paths, "c_0001", "alive", "true", None).unwrap().id;

        let result = set_assertion_status(&paths, &id, "deleted");
        assert!(result.is_err());
    }

    // —— add_assertion 数据一致性（自动取代 / 查重 / 证据登记）——

    #[test]
    fn add_assertion_supersedes_stale_state() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let first = add_assertion(&paths, "c_0001", "located_at", "l_0001", None).unwrap();
        let second = add_assertion(&paths, "c_0001", "located_at", "l_0002", None).unwrap();

        assert!(!second.deduplicated);
        assert_eq!(second.superseded, vec![first.id.clone()]);

        let file =
            AssertionsFile::load_or_default(&paths.knowledge_dir().join("assertions.toml"))
                .unwrap();
        let old = file.assertions.iter().find(|a| a.id == first.id).unwrap();
        assert_eq!(old.status, "superseded");
        let new = file.assertions.iter().find(|a| a.id == second.id).unwrap();
        assert_eq!(new.status, "active");
        assert_eq!(new.object, toml::Value::String("l_0002".into()));
    }

    #[test]
    fn add_assertion_does_not_supersede_accumulating_predicates() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // holds 可多值共存：不应被取代
        let first = add_assertion(&paths, "c_0001", "holds", "i_0001", None).unwrap();
        let second = add_assertion(&paths, "c_0001", "holds", "i_0002", None).unwrap();

        assert!(second.superseded.is_empty());
        let file =
            AssertionsFile::load_or_default(&paths.knowledge_dir().join("assertions.toml"))
                .unwrap();
        let old = file.assertions.iter().find(|a| a.id == first.id).unwrap();
        assert_eq!(old.status, "active");
    }

    #[test]
    fn add_assertion_deduplicates_identical() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let first = add_assertion(&paths, "c_0001", "located_at", "l_0001", None).unwrap();
        let again = add_assertion(&paths, "c_0001", "located_at", "l_0001", None).unwrap();

        assert!(again.deduplicated);
        assert_eq!(again.id, first.id);

        let file =
            AssertionsFile::load_or_default(&paths.knowledge_dir().join("assertions.toml"))
                .unwrap();
        assert_eq!(
            file.assertions
                .iter()
                .filter(|a| a.subject == "c_0001" && a.predicate == "located_at")
                .count(),
            1
        );
    }

    #[test]
    fn add_assertion_rejects_missing_evidence_fragment() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let result = add_assertion(&paths, "c_0001", "alive", "true", Some("f_9999"));
        assert!(result.is_err());
    }

    // —— 写入路径剥离模型复读 frontmatter / repair_project ——

    #[test]
    fn create_fragment_strips_echoed_frontmatter() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();

        // 模型复读的 YAML 块（前带空行）
        let text = "\n---\nanchors:\n  - 沈砚之\n---\n\n他没有应声。";
        let fid = create_fragment(&paths, &sc_id, text).unwrap();

        let content =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{fid}.md"))).unwrap();
        assert_eq!(content.matches("---").count(), 2, "只应有一层 frontmatter: {content}");
        assert!(!content.contains("沈砚之"), "复读块不应残留: {content}");
        assert!(content.contains("他没有应声。"));
    }

    #[test]
    fn repair_project_cleans_dirty_fragments_and_evidence() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();
        let frag_id = create_fragment(&paths, &sc_id, "干净正文。").unwrap();

        // —— 构造脏片段：正文前叠残留 YAML 块 ——
        let frag_path = paths.fragments_dir().join(format!("{frag_id}.md"));
        let dirty = format!(
            "---\nid: {frag_id}\n---\n\n---\nanchors:\n  - 沈砚之\n---\n\n干净正文。\n"
        );
        std::fs::write(&frag_path, dirty).unwrap();

        // —— 构造旧格式断言：evidence 直接存片段 ID ——
        let assertions_path = paths.knowledge_dir().join("assertions.toml");
        std::fs::create_dir_all(paths.knowledge_dir()).unwrap();
        std::fs::write(
            &assertions_path,
            format!(
                "[[assertion]]\nid = \"a_0001\"\nsubject = \"{char_id}\"\npredicate = \"alive\"\nobject = true\nstatus = \"active\"\nextracted_by = \"manual\"\nevidence = [\"{frag_id}\"]\n"
            ),
        )
        .unwrap();

        // —— 修复 ——
        let report = repair_project(tmp.path()).unwrap();
        assert_eq!(report.fragments_cleaned, vec![frag_id.clone()]);
        assert_eq!(report.assertions_fixed, 1);
        assert_eq!(report.evidence_skipped, 0);

        // 片段已清洗
        let content = std::fs::read_to_string(&frag_path).unwrap();
        assert_eq!(content.matches("---").count(), 2, "清洗后只应有一层 frontmatter: {content}");
        assert!(content.contains("干净正文。"));

        // 断言证据已重建为 ev_ ID
        let file = AssertionsFile::load_or_default(&assertions_path).unwrap();
        let ev = &file.assertions[0].evidence;
        assert_eq!(ev.len(), 1);
        assert!(ev[0].starts_with("ev_"), "应为证据 ID: {ev:?}");

        // 幂等：再跑一遍无变化
        let report2 = repair_project(tmp.path()).unwrap();
        assert!(report2.fragments_cleaned.is_empty());
        assert_eq!(report2.assertions_fixed, 0);
    }

    // —— create_thread 自动初始化可见性 ——

    #[test]
    fn create_thread_initializes_knowledge_timeline() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", Some(&char_id)).unwrap();
        let fid = create_fragment(&paths, &sc_id, "正文。").unwrap();

        // 场景 POV 即 char_id
        let t_id = create_thread(&paths, "伏笔", "描述", "main", Some(&sc_id), None, None).unwrap();

        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread = file.threads.iter().find(|t| t.id == t_id).unwrap();
        assert!(
            !thread.knowledge.is_empty(),
            "应自动初始化可见性时间线: {:?}",
            thread.knowledge
        );

        // POV 角色 known + reader suspected
        let has_pov = thread.knowledge.iter().any(|k| {
            matches!(
                &k.observer,
                story_project_format::threads_file::ObserverDef::Character { character }
                    if character == &char_id
            ) && k.state_timeline.iter().any(|e| e.state == "known")
        });
        assert!(has_pov, "POV 角色应初始化为 known: {:?}", thread.knowledge);
        let has_reader = thread.knowledge.iter().any(|k| {
            matches!(&k.observer, story_project_format::threads_file::ObserverDef::Builtin(b) if b == "reader")
        });
        assert!(has_reader, "reader 应初始化: {:?}", thread.knowledge);

        // 无 planted_at 时不应初始化
        let t2 = create_thread(&paths, "伏笔2", "描述", "minor", None, None, None).unwrap();
        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread2 = file.threads.iter().find(|t| t.id == t2).unwrap();
        assert!(thread2.knowledge.is_empty());
        let _ = fid;
    }


    // —— 本轮：repair 全文扫描 / ID 引用高水位 / 片段埋设点 ——

    #[test]
    fn repair_project_cleans_mid_body_residue() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let fid = create_fragment(&paths, &sc_id, "第一段。\n\n第二段。").unwrap();

        // 正文中部注入残块（显示名与实体 ID 两种形态）
        let frag_path = paths.fragments_dir().join(format!("{fid}.md"));
        let dirty = format!(
            "---\nid: {fid}\n---\n\n第一段。\n\n---\nanchors:\n  - 沈砚之\n---\n\n中段。\n\n---\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n第二段。\n"
        );
        std::fs::write(&frag_path, dirty).unwrap();

        let report = repair_project(tmp.path()).unwrap();
        assert_eq!(report.fragments_cleaned, vec![fid.clone()]);

        let content = std::fs::read_to_string(&frag_path).unwrap();
        assert!(!content.contains("anchors:"), "中部残块应被清除: {content}");
        assert!(!content.contains("沈砚之"));
        assert!(content.contains("第一段。"));
        assert!(content.contains("中段。"));
        assert!(content.contains("第二段。"));
    }

    #[test]
    fn repair_project_normalizes_head_without_id() {
        // 头部块缺 id: 字段（非规范 frontmatter）→ 重渲染规范化
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let fid = create_fragment(&paths, &sc_id, "正文。").unwrap();

        let frag_path = paths.fragments_dir().join(format!("{fid}.md"));
        std::fs::write(&frag_path, "---\nanchors:\n  - 沈砚之\n---\n\n正文。\n").unwrap();

        let report = repair_project(tmp.path()).unwrap();
        assert_eq!(report.fragments_cleaned, vec![fid.clone()]);
        let content = std::fs::read_to_string(&frag_path).unwrap();
        assert!(content.starts_with(&format!("---\nid: {fid}\n")), "应补 id: {content}");
    }

    #[test]
    fn fragment_id_not_reused_when_referenced() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();

        // 创建两个片段 + 断言引用第二个
        let _f1 = create_fragment(&paths, &sc_id, "一").unwrap();
        let f2 = create_fragment(&paths, &sc_id, "二").unwrap();
        let _a = add_assertion(&paths, &char_id, "holds", "i_0001", Some(&f2)).unwrap();

        // 删除被引用的片段文件（模拟外部引用残留）
        std::fs::remove_file(paths.fragments_dir().join(format!("{f2}.md"))).unwrap();

        // 新建片段：ID 不应复用被引用的（知识层引用高水位）
        let f3 = create_fragment(&paths, &sc_id, "三").unwrap();
        assert_ne!(f3, f2, "被引用过的 ID 不应回收复用");
    }

    #[test]
    fn create_thread_initializes_from_fragment_planted_at() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", Some(&char_id)).unwrap();
        let fid = create_fragment(&paths, &sc_id, "正文。").unwrap();

        // 埋设点传片段 ID（工具契约允许），应解析到所属场景并初始化
        let t_id = create_thread(&paths, "伏笔", "描述", "main", Some(&fid), None, None).unwrap();

        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread = file.threads.iter().find(|t| t.id == t_id).unwrap();
        assert!(
            !thread.knowledge.is_empty(),
            "片段埋设点也应初始化时间线: {:?}",
            thread.knowledge
        );
        // POV 角色在所属场景 known
        let has_pov = thread.knowledge.iter().any(|k| {
            matches!(
                &k.observer,
                story_project_format::threads_file::ObserverDef::Character { character }
                    if character == &char_id
            )
        });
        assert!(has_pov, "POV 应初始化: {:?}", thread.knowledge);
    }


    #[test]
    fn fragment_id_not_reused_even_without_references() {
        // 无引用的片段删除后，持久高水位仍阻止 ID 回收
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();

        let f1 = create_fragment(&paths, &sc_id, "一").unwrap();
        let f2 = create_fragment(&paths, &sc_id, "二").unwrap();
        assert_ne!(f1, f2);

        // 删除 f2（不走 delete_fragment，模拟外部直接删文件，无引用残留）
        std::fs::remove_file(paths.fragments_dir().join(format!("{f2}.md"))).unwrap();

        let f3 = create_fragment(&paths, &sc_id, "三").unwrap();
        assert_ne!(
            f3, f2,
            "持久高水位应阻止已分配 ID 的回收复用（即使无任何引用）"
        );
    }


    #[test]
    fn advance_thread_reveal_links_reader_knowledge() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let char_id = create_entity(&paths, EntityKind::Character, "角色").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", Some(&char_id)).unwrap();

        let t_id = create_thread(&paths, "伏笔", "描述", "main", Some(&sc_id), None, None).unwrap();

        // 推进到 revealed → reader 自动置为 known
        advance_thread(&paths, &t_id, "revealed", Some(&sc_id)).unwrap();

        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread = file.threads.iter().find(|t| t.id == t_id).unwrap();
        let reader_known = thread.knowledge.iter().any(|k| {
            matches!(&k.observer, story_project_format::threads_file::ObserverDef::Builtin(b) if b == "reader")
                && k.state_timeline.iter().any(|e| e.state == "known")
        });
        assert!(reader_known, "揭示后 reader 应自动 known: {:?}", thread.knowledge);
    }

    #[test]
    fn advance_thread_reveal_with_fragment_anchor_resolves_scene() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景", None).unwrap();
        let fid = create_fragment(&paths, &sc_id, "正文。").unwrap();

        let t_id = create_thread(&paths, "伏笔", "描述", "minor", None, None, None).unwrap();

        // 揭示锚点传片段 ID → 解析到所属场景记 reader
        advance_thread(&paths, &t_id, "resolved", Some(&fid)).unwrap();

        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread = file.threads.iter().find(|t| t.id == t_id).unwrap();
        let reader_known = thread.knowledge.iter().any(|k| {
            matches!(&k.observer, story_project_format::threads_file::ObserverDef::Builtin(b) if b == "reader")
                && k.state_timeline.iter().any(|e| e.state == "known")
        });
        assert!(reader_known, "片段锚点也应联动 reader: {:?}", thread.knowledge);
    }


    #[test]
    fn add_assertion_reports_coexisting_multi_valued() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // occupation 两条并存（多重职业合法），回执应带并存提示
        let first = add_assertion(&paths, "c_0001", "occupation", "画铺伙计", None).unwrap();
        assert!(first.coexisting.is_empty());
        let second = add_assertion(&paths, "c_0001", "occupation", "守坟人", None).unwrap();
        assert_eq!(second.coexisting, vec!["画铺伙计".to_string()]);
        assert!(second.superseded.is_empty(), "多值谓词不应自动覆盖");

        // 同值仍走查重
        let again = add_assertion(&paths, "c_0001", "occupation", "守坟人", None).unwrap();
        assert!(again.deduplicated);
    }


    #[test]
    fn advance_thread_rejects_regression() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());
        let ch_id = create_chapter(&paths, "章").unwrap();
        let sc1 = create_scene(&paths, &ch_id, "场景一", None).unwrap();
        let sc2 = create_scene(&paths, &ch_id, "场景二", None).unwrap();

        let t_id = create_thread(&paths, "伏笔", "描述", "main", Some(&sc1), None, None).unwrap();
        advance_thread(&paths, &t_id, "revealed", Some(&sc2)).unwrap();

        // revealed → planted 回退：应报错且不改写 planted_at
        let err = advance_thread(&paths, &t_id, "planted", Some(&sc2)).unwrap_err();
        assert!(err.to_string().contains("单向推进"), "got: {err}");

        let file = ThreadsFile::load_or_default(&paths.root().join("threads.toml")).unwrap();
        let thread = file.threads.iter().find(|t| t.id == t_id).unwrap();
        assert_eq!(thread.planted_at.as_deref(), Some(sc1.as_str()), "planted_at 不应被改写");
        assert_eq!(thread.revealed_at.as_deref(), Some(sc2.as_str()));

        // revealed → resolved 前进、abandoned 任意时刻均可
        advance_thread(&paths, &t_id, "resolved", Some(&sc2)).unwrap();
        advance_thread(&paths, &t_id, "abandoned", None).unwrap();
    }


    #[test]
    fn entity_id_not_reused_after_delete() {
        // 删除实体文件后（模拟外部删除），同类型新建不复用旧 ID
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let l1 = create_entity(&paths, EntityKind::Location, "山影城").unwrap();
        let l2 = create_entity(&paths, EntityKind::Location, "旧画铺").unwrap();
        assert_ne!(l1, l2);

        // 直接删文件（不走 delete_entity，无引用清理）
        std::fs::remove_file(paths.locations_dir().join(format!("{l2}.toml"))).unwrap();

        let l3 = create_entity(&paths, EntityKind::Location, "新城").unwrap();
        assert_ne!(l3, l2, "高水位应阻止已删实体 ID 复用");
        assert!(l3 > l2, "新 ID 应大于高水位: {l3} vs {l2}");
    }

    #[test]
    fn entity_id_respects_dangling_references() {
        // 断言仍引用已删实体 → 该 ID 不复用（防悬空引用改绑新实体）
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let c1 = create_entity(&paths, EntityKind::Character, "甲").unwrap();
        let _ = create_entity(&paths, EntityKind::Character, "乙").unwrap();
        let c3 = create_entity(&paths, EntityKind::Character, "丙").unwrap();

        // 旧格式断言直接引用 c3（不建证据条目，纯文本写入模拟历史数据）
        std::fs::create_dir_all(paths.knowledge_dir()).unwrap();
        std::fs::write(
            paths.knowledge_dir().join("assertions.toml"),
            format!(
                "[[assertion]]\nid = \"a_0001\"\nsubject = \"{c1}\"\npredicate = \"knows\"\nobject = \"{c3}\"\nstatus = \"active\"\n"
            ),
        )
        .unwrap();

        std::fs::remove_file(paths.characters_dir().join(format!("{c3}.toml"))).unwrap();

        let c4 = create_entity(&paths, EntityKind::Character, "丁").unwrap();
        assert_ne!(c4, c3, "被断言引用的已删 ID 不应复用");
    }

    // —— create_chapter 自动关联章节脉络 ——

    #[test]
    fn create_chapter_links_outline_by_title() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        // 预置脉络：带"第X章"前缀、无 chapter_id
        let mut outline = ChaptersFile::default();
        outline.chapters.push(ChapterOutlineDef {
            chapter_id: None,
            title: "第三章 上山".into(),
            volume_order: 0,
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
        save_chapters_file(&paths, &outline).unwrap();

        // 创建同名章节（标题归一化后匹配，前缀可有可无）
        let ch_id = create_chapter(&paths, "上山").unwrap();

        let file = load_chapters_file(&paths).unwrap();
        assert_eq!(file.chapters[0].chapter_id.as_deref(), Some(ch_id.as_str()));
    }

    // —— insert_fragment_after / move_fragment / merge_fragments ——

    #[test]
    fn insert_fragment_after_places_new_after_target() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let f1 = create_fragment(&paths, &sc_id, "片段一").unwrap();
        let f2 = create_fragment(&paths, &sc_id, "片段二").unwrap();
        // 当前顺序：[f1, f2]

        // 创建 f3 并插入到 f1 之后
        std::fs::write(
            paths.fragments_dir().join("f_9999.md"),
            "---\nid: f_9999\n---\n\n片段三\n",
        )
        .unwrap();
        insert_fragment_after(&paths, &sc_id, &f1, "f_9999").unwrap();

        let structure = load_structure(&paths).unwrap();
        let fragments = &structure.scene[&sc_id].fragments;
        // 顺序应为 [f1, f_9999, f2]
        assert_eq!(fragments, &vec![f1.clone(), "f_9999".to_string(), f2]);
    }

    #[test]
    fn insert_fragment_after_moves_existing_fragment() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let f1 = create_fragment(&paths, &sc_id, "片段一").unwrap();
        let f2 = create_fragment(&paths, &sc_id, "片段二").unwrap();
        // create_fragment 把新片段推到末尾 → [f1, f2, f3]
        // （split_fragment 的真实调用序列就是这个形态）
        let f3 = create_fragment(&paths, &sc_id, "片段三").unwrap();

        insert_fragment_after(&paths, &sc_id, &f1, &f3).unwrap();

        let structure = load_structure(&paths).unwrap();
        let fragments = &structure.scene[&sc_id].fragments;
        // 应移动到 f1 之后 → [f1, f3, f2]，而非因"已存在"守卫留在末尾
        assert_eq!(fragments, &vec![f1.clone(), f3.clone(), f2.clone()]);
    }

    #[test]
    fn insert_fragment_after_unknown_target_errors() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        create_fragment(&paths, &sc_id, "片段一").unwrap();

        let result = insert_fragment_after(&paths, &sc_id, "f_9999", "f_0002");
        assert!(result.is_err());
    }

    #[test]
    fn move_fragment_reorders_within_scene() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let f1 = create_fragment(&paths, &sc_id, "片段一").unwrap();
        let f2 = create_fragment(&paths, &sc_id, "片段二").unwrap();
        let f3 = create_fragment(&paths, &sc_id, "片段三").unwrap();
        // 当前顺序：[f1, f2, f3]

        // 把 f1 移到位置 1（移除 f1 后 [f2,f3]，插到 index 1 → [f2, f1, f3]）
        move_fragment(&paths, &sc_id, &f1, 1).unwrap();

        let structure = load_structure(&paths).unwrap();
        let fragments = &structure.scene[&sc_id].fragments;
        assert_eq!(fragments, &vec![f2.clone(), f1, f3]);
    }

    #[test]
    fn move_fragment_out_of_range_errors() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let f1 = create_fragment(&paths, &sc_id, "片段一").unwrap();
        // [f1]，移除后 []，插入 index 5 越界
        let result = move_fragment(&paths, &sc_id, &f1, 5);
        assert!(result.is_err());
    }

    #[test]
    fn merge_fragments_combines_text_and_anchors() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();

        // 创建 f1，手动加 anchors
        let f1 = create_fragment(&paths, &sc_id, "前半段。").unwrap();
        std::fs::write(
            paths.fragments_dir().join(format!("{f1}.md")),
            "---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n前半段。\n",
        )
        .unwrap();

        // 创建 f2，带不同 anchor
        let f2 = create_fragment(&paths, &sc_id, "后半段。").unwrap();
        std::fs::write(
            paths.fragments_dir().join(format!("{f2}.md")),
            format!(
                "---\nid: {f2}\nanchors:\n  - entity: c_0002\n    role: mentioned\n---\n\n后半段。\n"
            ),
        )
        .unwrap();

        // 合并 f2 到 f1
        merge_fragments(&paths, &f1, &f2).unwrap();

        // f2 文件应被删除
        assert!(!paths.fragments_dir().join(format!("{f2}.md")).exists());

        // f1 正文应包含两段
        let f1_content =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{f1}.md"))).unwrap();
        assert!(f1_content.contains("前半段。"));
        assert!(f1_content.contains("后半段。"));
        // 两个 anchors 都应保留
        assert!(f1_content.contains("c_0001"));
        assert!(f1_content.contains("c_0002"));

        // structure.toml 应不再包含 f2
        let structure = load_structure(&paths).unwrap();
        assert!(!structure.scene[&sc_id].fragments.contains(&f2));
    }

    #[test]
    fn merge_fragments_same_id_errors() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let ch_id = create_chapter(&paths, "测试章").unwrap();
        let sc_id = create_scene(&paths, &ch_id, "场景A", None).unwrap();
        let f1 = create_fragment(&paths, &sc_id, "片段一").unwrap();

        let result = merge_fragments(&paths, &f1, &f1);
        assert!(result.is_err());
    }

    // —— coerce_object_value（alive 布尔修复）——

    #[test]
    fn coerce_object_value_alive_true_variants() {
        // AI 常见误传：predicate 字面量 "alive" 作为值 → 应判为 true
        assert_eq!(
            coerce_object_value("alive", "alive"),
            toml::Value::Boolean(true)
        );
        assert_eq!(
            coerce_object_value("alive", "true"),
            toml::Value::Boolean(true)
        );
        assert_eq!(
            coerce_object_value("alive", "存活"),
            toml::Value::Boolean(true)
        );
        assert_eq!(
            coerce_object_value("alive", "活着"),
            toml::Value::Boolean(true)
        );
        // 大小写不敏感
        assert_eq!(
            coerce_object_value("alive", "TRUE"),
            toml::Value::Boolean(true)
        );
    }

    #[test]
    fn coerce_object_value_alive_false_variants() {
        assert_eq!(
            coerce_object_value("alive", "false"),
            toml::Value::Boolean(false)
        );
        assert_eq!(
            coerce_object_value("alive", "已故"),
            toml::Value::Boolean(false)
        );
        assert_eq!(
            coerce_object_value("alive", "死亡"),
            toml::Value::Boolean(false)
        );
        assert_eq!(
            coerce_object_value("alive", "dead"),
            toml::Value::Boolean(false)
        );
    }

    #[test]
    fn coerce_object_value_age_parses_integer() {
        assert_eq!(coerce_object_value("age", "17"), toml::Value::Integer(17));
        assert_eq!(coerce_object_value("age", "-5"), toml::Value::Integer(-5));
        // 非数字退回字符串
        assert_eq!(
            coerce_object_value("age", "不详"),
            toml::Value::String("不详".to_string())
        );
    }

    #[test]
    fn coerce_object_value_other_predicates_keep_string() {
        assert_eq!(
            coerce_object_value("holds", "i_0001"),
            toml::Value::String("i_0001".to_string())
        );
        assert_eq!(
            coerce_object_value("located_at", "l_0001"),
            toml::Value::String("l_0001".to_string())
        );
    }

    #[test]
    fn add_assertion_alive_writes_boolean_to_toml() {
        // 端到端：AI 传 "alive" 字符串，文件里应写入 object = true（布尔）
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let id = add_assertion(&paths, "c_0001", "alive", "alive", None).unwrap().id;
        assert_eq!(id, "a_0001");

        let content =
            std::fs::read_to_string(paths.knowledge_dir().join("assertions.toml")).unwrap();
        // TOML 布尔是小写 true，不是 "alive" 字符串
        assert!(content.contains("object = true"));
        assert!(!content.contains("object = \"alive\""));
    }

    #[test]
    fn chapter_outline_status_transitions() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters.push(ChapterOutlineDef {
            title: "第一章".into(),
            ..ChapterOutlineDef::default()
        });
        save_chapters_file(&paths, &chapters).unwrap();

        assert_eq!(
            set_chapter_outline_status(&paths, 1, "writing").unwrap(),
            "writing"
        );
        assert_eq!(
            set_chapter_outline_status(&paths, 1, "completed").unwrap(),
            "completed"
        );
        // 回退被拒绝
        let err = set_chapter_outline_status(&paths, 1, "writing").unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));
        // 非法值被拒绝
        assert!(set_chapter_outline_status(&paths, 1, "draft").is_err());
        // 越界
        assert!(set_chapter_outline_status(&paths, 9, "writing").is_err());
    }

    #[test]
    fn chapter_outline_backfill_and_beats() {
        let tmp = setup_test_project();
        let paths = ProjectPaths::new(tmp.path());

        let mut chapters = load_chapters_file(&paths).unwrap();
        chapters.chapters.push(ChapterOutlineDef {
            title: "第二章".into(),
            ..ChapterOutlineDef::default()
        });
        save_chapters_file(&paths, &chapters).unwrap();

        backfill_chapter_outline_id(&paths, 1, "ch_01").unwrap();
        update_chapter_outline_beats(&paths, 1, vec!["登场".into(), "冲突".into()]).unwrap();
        append_chapter_outline_scene(&paths, 1, "sc_01_01").unwrap();
        append_chapter_outline_scene(&paths, 1, "sc_01_01").unwrap(); // 幂等

        let reloaded = load_chapters_file(&paths).unwrap();
        let item = &reloaded.chapters[0];
        assert_eq!(item.chapter_id.as_deref(), Some("ch_01"));
        assert_eq!(item.beats, vec!["登场", "冲突"]);
        assert_eq!(item.scenes, vec!["sc_01_01"]);

        // 空串清除回填
        backfill_chapter_outline_id(&paths, 1, "").unwrap();
        let reloaded = load_chapters_file(&paths).unwrap();
        assert_eq!(reloaded.chapters[0].chapter_id, None);
    }
}
