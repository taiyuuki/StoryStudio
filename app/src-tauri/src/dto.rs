//! 前端 DTO — 全部 camelCase 序列化，与前端 TS 类型一一对应

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use story_project_format::entity_file::{ItemSection, ProfileSection};
use story_project_format::fragment_file::AnchorEntry;
use story_project_format::relation_file::ValidityDef;
use story_project_format::structure_toml::StoryTimeDef;
use story_project_format::threads_file::ThreadDef;
use story_project_format::{ChapterDef, EntityFile, FragmentFile, RelationsFile, ThreadsFile};

// ============================================================================
// Workspace — 打开项目后的全量快照
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDto {
    pub root: String,
    pub name: String,
    pub author: Option<String>,
    pub language: Option<String>,
    pub book_id: String,
    pub book_title: String,
    pub chapters: Vec<ChapterDto>,
    pub entities: Vec<EntityDto>,
    pub relations: Vec<RelationDto>,
    pub threads: Vec<ThreadDto>,
    pub total_words: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterDto {
    pub id: String,
    pub title: String,
    pub scenes: Vec<SceneDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneDto {
    pub id: String,
    pub chapter_id: String,
    pub title: Option<String>,
    pub pov: Option<String>,
    pub mood: Option<String>,
    pub summary: Option<String>,
    pub fragment_count: usize,
    pub word_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityDto {
    pub id: String,
    /// character | location | item | organization
    pub kind: String,
    pub display_name: String,
    pub symbol: Option<String>,
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geography: Option<GeographyDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemDto>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub race: Option<String>,
    pub gender: Option<String>,
    pub occupation: Option<String>,
    pub birth_year: Option<i32>,
    pub appearance: Option<String>,
    pub background: Option<String>,
    pub personality: Option<String>,
    pub habits: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeographyDto {
    pub parent: Option<String>,
    pub kind: Option<String>,
    pub position: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ItemDto {
    pub kind: Option<String>,
    pub grade: Option<String>,
    pub description: Option<String>,
    pub abilities: Option<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationDto {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relation: String,
    pub valid: Option<ValidityDef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDto {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub goal: Option<String>,
    /// main | minor
    pub priority: String,
    pub tags: Vec<String>,
    pub planted_at: Option<String>,
    pub revealed_at: Option<String>,
    pub resolved_at: Option<String>,
    pub abandoned: bool,
}

// ============================================================================
// SceneDetail — 单个场景的编辑数据（含片段正文）
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneDetailDto {
    pub id: String,
    pub chapter_id: String,
    pub chapter_title: String,
    pub title: Option<String>,
    pub pov: Option<String>,
    pub mood: Option<String>,
    pub function: Option<String>,
    pub summary: Option<String>,
    pub story_time: Option<StoryTimeDef>,
    pub fragments: Vec<FragmentDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentDto {
    pub id: String,
    pub text: String,
    pub anchors: Vec<AnchorDto>,
    pub word_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorDto {
    pub entity: String,
    pub role: String,
}

// ============================================================================
// Compile — 诊断
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileDto {
    /// false = 解析失败，面板显示 error 消息
    pub ok: bool,
    pub error: Option<String>,
    pub diagnostics: Vec<DiagDto>,
    pub stats: StatsDto,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub fragment_count: usize,
    pub entity_count: usize,
    pub edge_count: usize,
    pub assertion_count: usize,
    pub evidence_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagDto {
    /// error | warning | hint | info
    pub severity: String,
    pub code: String,
    pub message: String,
    pub rule: Option<String>,
    pub entity: Option<String>,
    pub fragment: Option<String>,
}

// ============================================================================
// 场景属性补丁 — Option<Option<T>> 三态语义（None=不改 / null=清空 / 值=设置）
// ============================================================================

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ScenePatchDto {
    pub title: Option<Option<String>>,
    pub pov: Option<Option<String>>,
    pub mood: Option<Option<String>>,
    pub function: Option<Option<String>>,
    pub summary: Option<Option<String>>,
    pub story_time: Option<Option<StoryTimeDef>>,
}

// ============================================================================
// AI 配置 — camelCase 镜像（库类型为 snake_case）
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigDto {
    pub profiles: Vec<AiProfileDto>,
    pub active_profile: Option<String>,
    /// 用途 → 档案 id 分配（write / agent / workflow…），
    /// 未列出的用途回退激活档案
    pub purpose_profiles: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProfileDto {
    pub id: String,
    /// 来源预设 id（deepseek 等；空 = 手动配置）
    pub preset: Option<String>,
    pub name: String,
    /// anthropic | openai
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub api_url_full: Option<bool>,
    /// 思考深度：auto | off | low | medium | high
    /// auto = 不发送思考参数，跟随模型默认（适用于必须开启思考的模型）
    pub thinking: Option<String>,
    /// 单次生成请求超时（秒）；None = 默认 300
    pub timeout_secs: Option<u64>,
    /// OpenAI 接口模式：chat（Chat Completions，兼容最广）| responses（OpenAI 原生，GPT-5/o 系列）
    /// 仅 provider="openai" 生效；anthropic 恒为 None
    pub api_mode: Option<String>,
    /// 私有参数（deep merge 进请求体，支持 provider 私有字段）
    pub extra_body: Option<serde_json::Value>,
    /// 自定义请求头（追加，不覆盖认证头）
    pub extra_headers: Option<std::collections::BTreeMap<String, String>>,
    /// 上次连接测试结果
    pub last_test: Option<story_studio_lib::ai::ProfileTestRecord>,
}

fn thinking_to_str(t: &story_ai_runtime::ThinkingConfig) -> String {
    use story_ai_runtime::ThinkingConfig;
    match t {
        ThinkingConfig::Auto => "auto",
        ThinkingConfig::Off => "off",
        ThinkingConfig::Low => "low",
        ThinkingConfig::Medium => "medium",
        ThinkingConfig::High => "high",
        ThinkingConfig::Budget(_) => "medium",
    }
    .to_string()
}

fn thinking_from_str(s: &str) -> Option<story_ai_runtime::ThinkingConfig> {
    use story_ai_runtime::ThinkingConfig;
    match s {
        "auto" => Some(ThinkingConfig::Auto),
        "off" => Some(ThinkingConfig::Off),
        "low" => Some(ThinkingConfig::Low),
        "medium" => Some(ThinkingConfig::Medium),
        "high" => Some(ThinkingConfig::High),
        _ => None,
    }
}

impl From<&story_studio_lib::ai::GlobalAiConfig> for AiConfigDto {
    fn from(c: &story_studio_lib::ai::GlobalAiConfig) -> Self {
        AiConfigDto {
            profiles: c
                .profiles
                .as_ref()
                .map(|ps| {
                    ps.iter()
                        .map(|p| AiProfileDto {
                            id: p.id.clone(),
                            preset: p.preset.clone(),
                            name: p.name.clone(),
                            provider: p.provider.clone(),
                            model: p.model.clone(),
                            api_key: p.api_key.clone(),
                            api_url: p.api_url.clone(),
                            api_url_full: p.api_url_full,
                            thinking: p.thinking.as_ref().map(thinking_to_str),
                            timeout_secs: p.timeout_secs,
                            api_mode: p.api_mode.as_ref().map(|m| match m {
                                story_ai_runtime::ApiMode::Responses => "responses".to_string(),
                                story_ai_runtime::ApiMode::Chat => "chat".to_string(),
                            }),
                            extra_body: p.extra_body.clone(),
                            extra_headers: p
                                .extra_headers
                                .as_ref()
                                .map(|hs| {
                                    hs.iter().cloned().collect::<std::collections::BTreeMap<_, _>>()
                                }),
                            last_test: p.last_test.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            active_profile: c.active_profile.clone(),
            purpose_profiles: c.purpose_profiles.clone(),
        }
    }
}

impl From<&AiProfileDto> for story_studio_lib::ai::ProviderProfile {
    fn from(p: &AiProfileDto) -> Self {
        story_studio_lib::ai::ProviderProfile {
            id: p.id.clone(),
            preset: p.preset.clone(),
            last_test: p.last_test.clone(),
            name: p.name.clone(),
            provider: p.provider.clone(),
            model: p.model.clone(),
            api_key: p.api_key.clone(),
            api_url: p.api_url.clone(),
            api_url_full: p.api_url_full,
            extra_body: p.extra_body.clone(),
            extra_headers: p
                .extra_headers
                .as_ref()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            thinking: p.thinking.as_deref().and_then(thinking_from_str),
            timeout_secs: p.timeout_secs,
            // anthropic provider 不使用该字段（恒 None）
            api_mode: if p.provider.as_deref() == Some("openai") {
                match p.api_mode.as_deref() {
                    Some("responses") => Some(story_ai_runtime::ApiMode::Responses),
                    _ => Some(story_ai_runtime::ApiMode::Chat),
                }
            } else {
                None
            },
        }
    }
}

impl From<&AiConfigDto> for story_studio_lib::ai::GlobalAiConfig {
    fn from(c: &AiConfigDto) -> Self {
        story_studio_lib::ai::GlobalAiConfig {
            profiles: Some(c.profiles.iter().map(Into::into).collect()),
            active_profile: c.active_profile.clone(),
            purpose_profiles: c.purpose_profiles.clone(),
            ..Default::default()
        }
    }
}

// ============================================================================
// 转换辅助 — 库类型 → DTO
// ============================================================================

/// 字数统计（全工作区统一口径：CJK 每字 1、ASCII 词 1、标点不计）
pub fn count_words(text: &str) -> u64 {
    story_studio_lib::workflow::count_words(text) as u64
}

pub fn to_entity_dto(kind: &str, ef: &EntityFile) -> EntityDto {
    EntityDto {
        id: ef.id.clone(),
        kind: kind.to_string(),
        display_name: ef.display_name.clone(),
        symbol: ef.symbol.clone(),
        aliases: ef.aliases.clone(),
        profile: ef.profile.as_ref().map(profile_to_dto),
        geography: ef.geography.as_ref().map(|g| GeographyDto {
            parent: g.parent.clone(),
            kind: g.kind.clone(),
            position: g.position.clone(),
        }),
        item: ef.item.as_ref().map(|i| ItemDto {
            kind: i.kind.clone(),
            grade: i.grade.clone(),
            description: i.description.clone(),
            abilities: i.abilities.clone(),
            unique: i.unique,
        }),
        attributes: ef.attributes.clone(),
    }
}

fn profile_to_dto(p: &ProfileSection) -> ProfileDto {
    ProfileDto {
        race: p.race.clone(),
        gender: p.gender.clone(),
        occupation: p.occupation.clone(),
        birth_year: p.birth_year,
        appearance: p.appearance.clone(),
        background: p.background.clone(),
        personality: p.personality.clone(),
        habits: p.habits.clone(),
    }
}

fn profile_from_dto(p: &ProfileDto) -> ProfileSection {
    ProfileSection {
        race: p.race.clone(),
        gender: p.gender.clone(),
        occupation: p.occupation.clone(),
        birth_year: p.birth_year,
        appearance: p.appearance.clone(),
        background: p.background.clone(),
        personality: p.personality.clone(),
        habits: p.habits.clone(),
    }
}

pub fn item_from_dto(i: &ItemDto) -> ItemSection {
    ItemSection {
        kind: i.kind.clone(),
        grade: i.grade.clone(),
        description: i.description.clone(),
        abilities: i.abilities.clone(),
        unique: i.unique,
    }
}

pub fn to_thread_dto(t: &ThreadDef) -> ThreadDto {
    ThreadDto {
        id: t.id.clone(),
        title: t.title.clone(),
        description: t.description.clone(),
        goal: t.goal.clone(),
        priority: t.priority.clone(),
        tags: t.tags.clone(),
        planted_at: t.planted_at.clone(),
        revealed_at: t.revealed_at.clone(),
        resolved_at: t.resolved_at.clone(),
        abandoned: t.abandoned,
    }
}

pub fn to_fragment_dto(file: &FragmentFile) -> FragmentDto {
    FragmentDto {
        id: file.frontmatter.id.clone(),
        text: file.text.clone(),
        anchors: file
            .frontmatter
            .anchors
            .iter()
            .map(|a: &AnchorEntry| AnchorDto {
                entity: a.entity.clone(),
                role: a.role.clone(),
            })
            .collect(),
        word_count: count_words(&file.text),
    }
}

pub fn to_relation_dto(r: &story_project_format::RelationDef) -> RelationDto {
    RelationDto {
        id: r.id.clone(),
        from: r.from.clone(),
        to: r.to.clone(),
        relation: r.relation.clone(),
        valid: r.valid.clone(),
    }
}

pub fn read_file(path: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))
}

/// 从 TOML 加载，不存在或为空则 Default（镜像 mutations.rs 的私有辅助）
pub fn load_toml<T>(path: &std::path::Path) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let content = read_file(path)?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }
    toml::from_str(&content).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
}

/// 读取一个 fragment 文件
pub fn load_fragment_file(root: &Path, fragment_id: &str) -> Result<FragmentFile, String> {
    let paths = story_project_format::ProjectPaths::new(root);
    let path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| format!("无效的片段 ID: {fragment_id}"))?;
    if !path.exists() {
        return Err(format!("片段文件不存在: {}", path.display()));
    }
    let content = read_file(&path)?;
    FragmentFile::parse(&content).map_err(|e| format!("片段 {fragment_id} 解析失败: {e}"))
}

/// 保存实体文件（EntityFile ← DTO，id 决定路径）
pub fn save_entity_file(root: &Path, dto: &EntityDto) -> Result<(), String> {
    use story_studio_lib::mutations::EntityKind;

    let kind = EntityKind::from_id(&dto.id).map_err(|e| e.to_string())?;
    let mut ef = match story_project_format::ProjectPaths::new(root).file_for_id(&dto.id) {
        Some(path) if path.exists() => {
            let content = read_file(&path)?;
            EntityFile::from_toml_str(&content).map_err(|e| format!("解析实体失败: {e}"))?
        }
        _ => EntityFile::new_character(&dto.id, &dto.display_name),
    };

    ef.id = dto.id.clone();
    ef.display_name = dto.display_name.clone();
    ef.symbol = dto.symbol.clone().filter(|s| !s.is_empty());
    ef.aliases = dto
        .aliases
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if kind == EntityKind::Character {
        ef.profile = Some(profile_from_dto(dto.profile.as_ref().unwrap_or(&ProfileDto::default())));
    } else {
        ef.profile = None;
    }
    if kind == EntityKind::Location {
        ef.geography = dto.geography.clone().map(|g| {
            story_project_format::entity_file::GeographySection {
                parent: g.parent,
                kind: g.kind,
                position: g.position,
            }
        });
    } else {
        ef.geography = None;
    }
    if kind == EntityKind::Item {
        ef.item = Some(item_from_dto(dto.item.as_ref().unwrap_or(&ItemDto::default())));
    } else {
        ef.item = None;
    }

    let dir = kind.dir(&story_project_format::ProjectPaths::new(root));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let toml_content = ef.to_toml_string().map_err(|e| format!("序列化实体失败: {e}"))?;
    std::fs::write(dir.join(format!("{}.toml", dto.id)), toml_content)
        .map_err(|e| format!("写入实体文件失败: {e}"))
}

/// 汇总项目全量工作区数据
pub fn build_workspace(root: &Path, info: &story_studio_lib::ProjectInfo) -> Result<WorkspaceDto, String> {
    let paths = story_project_format::ProjectPaths::new(root);

    let structure = story_project_format::StructureFile::from_toml_str(
        &read_file(&paths.structure_toml())?,
    )
    .map_err(|e| format!("structure.toml 解析失败: {e}"))?;
    let chapters = load_chapter_dtos(root)?;

    // 实体：扫描四类目录
    let mut entities = Vec::new();
    for (kind, dir) in [
        ("character", paths.characters_dir()),
        ("location", paths.locations_dir()),
        ("item", paths.items_dir()),
        ("organization", paths.organizations_dir()),
    ] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(content) = read_file(&p) {
                if let Ok(ef) = EntityFile::from_toml_str(&content) {
                    entities.push(to_entity_dto(kind, &ef));
                }
            }
        }
    }
    entities.sort_by(|a, b| a.display_name.cmp(&b.display_name));

    // 关系
    let relations = load_toml::<RelationsFile>(&paths.relations_toml())?
        .relations
        .iter()
        .map(to_relation_dto)
        .collect();

    // 叙事线索
    let threads = load_toml::<ThreadsFile>(&paths.threads_toml())?
        .threads
        .iter()
        .map(to_thread_dto)
        .collect();

    let total_words = chapters
        .iter()
        .flat_map(|c| c.scenes.iter())
        .map(|s| s.word_count)
        .sum();

    Ok(WorkspaceDto {
        root: root.display().to_string(),
        name: info.name.clone(),
        author: info.author.clone(),
        language: info.language.clone(),
        book_id: structure.book.id.clone(),
        book_title: structure.book.title.clone(),
        chapters,
        entities,
        relations,
        threads,
        total_words,
    })
}

/// 加载章节结构（Book → Chapter → Scene），计算每场景字数
pub fn load_chapter_dtos(root: &Path) -> Result<Vec<ChapterDto>, String> {
    let paths = story_project_format::ProjectPaths::new(root);
    let structure = story_project_format::StructureFile::from_toml_str(
        &read_file(&paths.structure_toml())?,
    )
    .map_err(|e| format!("structure.toml 解析失败: {e}"))?;

    let mut chapters = Vec::new();
    for ch_id in &structure.book.chapters {
        let ch_def: &ChapterDef = structure.chapter.get(ch_id).ok_or_else(|| {
            format!("structure.toml 不一致: 章节 {ch_id} 在 book.chapters 中但缺少定义")
        })?;
        let mut scenes = Vec::new();
        for sc_id in &ch_def.scenes {
            let sc_def = structure.scene.get(sc_id).ok_or_else(|| {
                format!("structure.toml 不一致: 场景 {sc_id} 在章节 scenes 中但缺少定义")
            })?;
            let words: u64 = sc_def
                .fragments
                .iter()
                .filter_map(|f| load_fragment_file(root, f).ok())
                .map(|ff| count_words(&ff.text))
                .sum();
            scenes.push(SceneDto {
                id: sc_id.clone(),
                chapter_id: ch_id.clone(),
                title: sc_def.title.clone(),
                pov: sc_def.pov.clone(),
                mood: sc_def.mood.clone(),
                summary: sc_def.summary.clone(),
                fragment_count: sc_def.fragments.len(),
                word_count: words,
            });
        }
        chapters.push(ChapterDto {
            id: ch_id.clone(),
            title: ch_def.title.clone(),
            scenes,
        });
    }
    Ok(chapters)
}
