//! AI 建书 — 分步向导后端
//!
//! 设计原则：**生成是纯函数，保存才落盘**。每个阶段把 LLM 输出解析成
//! 可编辑的结构化草稿（Draft）返回给前端，用户改完后再调用对应的
//! `save_*` 确认提交——与全工作区"AI 输出确定性提交"的哲学一致。
//!
//! 步骤（对应前端向导）：
//! 1. 构思：`ai_book_concepts` → 3 个候选方向（尚无项目，不记 AI 账本）
//! 2. 开书：`create_book_project`（用户编辑后的概念 + 参数 → 建项目、
//!    写 outline.toml，参数随项目持久化）
//! 3. 设定：`ai_book_world` → 世界观/角色/关系/线索草稿；`save_book_world` 落盘
//! 4. 大纲：`ai_book_outline_batch` → 一批章节脉络草稿（长篇按批排纲，
//!    不一次排完）；`save_book_outline_batch` 追加 chapters.toml。
//!    写作过程中可反复调用续排（上下文含已完成章节梗概）。

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use story_ai_runtime::provider::{GenerateRequest, Message};
use story_ai_runtime::stream_generate_with_cancel;
use story_ai_runtime::CancelToken;
use story_project_format::{OutlineFile, ProjectPaths};

use crate::error::StudioError;
use crate::journal;
use crate::mutations::{self, EntityKind};

/// 默认单章字数
pub const DEFAULT_CHAPTER_WORDS: u64 = 2500;
/// 构思候选数
pub const CONCEPT_CANDIDATES: usize = 3;
/// 单批章节上限（防长篇一次排完）
pub const MAX_BATCH_CHAPTERS: u32 = 40;

// ============================================================================
// 参数 / 草稿类型（全部可 serde，前端编辑后原样传回）
// ============================================================================

/// 建书参数（第 1 步表单）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BookCreationOptions {
    /// 一句话灵感（必须非空）
    pub idea: String,
    /// 题材（None = 由 AI 判断）
    pub genre: Option<String>,
    /// 首批章节数（3..=40，0 = 默认 12）
    pub chapter_count: u32,
    /// 总字数目标
    pub target_words: Option<u64>,
    /// 单章字数
    pub chapter_words: Option<u64>,
    /// 面向人群（男频 / 女频 / 全年龄…）
    pub audience: Option<String>,
    /// 叙事视角（第一人称 / 第三人称限知…）
    pub pov: Option<String>,
    /// 文风基调（热血 / 轻松 / 暗黑 / 细腻…）
    pub style: Option<String>,
    /// 主要角色数（2..=12）
    pub character_count: Option<u32>,
}

impl BookCreationOptions {
    fn normalized(&self) -> Self {
        let mut o = self.clone();
        o.idea = o.idea.trim().to_string();
        o.genre = o
            .genre
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.contains("自动"))
            .map(|s| s.to_string());
        o.chapter_count = if o.chapter_count == 0 {
            12
        } else {
            o.chapter_count.clamp(3, MAX_BATCH_CHAPTERS)
        };
        o.chapter_words = o.chapter_words.filter(|w| (1000..=10000).contains(w));
        o.character_count = o.character_count.filter(|c| (2..=12).contains(c));
        o
    }
}

/// 构思候选（第 2 步编辑对象）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BookConcept {
    pub title: String,
    pub tagline: String,
    pub blurb: String,
    /// 核心设定与主线（写入 outline.story.premise）
    pub premise: String,
    pub theme: String,
    pub setting: String,
}

/// 世界观 / 人物设定草稿（第 3 步编辑对象）
///
/// 地理与势力直接以实体（Location / Organization）形式编辑，
/// 保存时逐个建档；世界观文件只落概念、体系与纪元。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorldDraft {
    pub concepts: Vec<ConceptItem>,
    pub systems: Vec<SystemItem>,
    pub eras: Vec<String>,
    pub characters: Vec<CharacterItem>,
    pub locations: Vec<LocationItem>,
    pub organizations: Vec<OrganizationItem>,
    pub relations: Vec<RelationItem>,
    pub threads: Vec<ThreadItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConceptItem {
    pub name: String,
    pub category: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemItem {
    pub name: String,
    /// cultivation / magic / technology / power
    pub kind: String,
    /// 等级链（如 炼气→筑基→金丹）
    pub levels: String,
    pub rules: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CharacterItem {
    pub name: String,
    /// protagonist / love_interest / antagonist / mentor / support
    pub role: String,
    pub gender: String,
    pub occupation: String,
    pub personality: String,
    pub background: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocationItem {
    pub name: String,
    /// continent / nation / city / realm / landform
    pub kind: String,
    /// 上级地点名（保存时解析为已建档的 Location ID）
    pub parent: String,
    /// 特征描述（世界设定）
    pub features: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrganizationItem {
    pub name: String,
    /// sect / nation / clan / guild / organization
    pub kind: String,
    pub leader: String,
    pub territory: String,
    pub ideology: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RelationItem {
    pub from: String,
    pub to: String,
    pub relation: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadItem {
    pub title: String,
    pub description: String,
    /// main / minor
    pub priority: String,
}

/// 章节脉络批次草稿（第 4 步编辑对象；写作中续排时复用）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutlineBatchDraft {
    /// 本批新增的卷分组（可为空 = 续用现有卷）
    pub volumes: Vec<VolumeItem>,
    pub chapters: Vec<ChapterItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VolumeItem {
    pub name: String,
    pub order: u32,
    pub theme: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChapterItem {
    pub title: String,
    pub volume_order: u32,
    pub summary: String,
    pub goal: String,
    pub beats: Vec<String>,
    pub characters: Vec<String>,
    pub hook: String,
}

/// 设定保存回执
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSaveReport {
    pub characters: usize,
    pub locations: usize,
    pub organizations: usize,
    pub relations: usize,
    pub threads: usize,
}

// ============================================================================
// 取消（进程内唯一在途阶段调用）
// ============================================================================

static CANCEL_SLOT: OnceLock<RwLock<Option<CancelToken>>> = OnceLock::new();

/// 取消进行中的建书阶段调用
pub fn cancel_book_creation() -> bool {
    let slot = CANCEL_SLOT.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = slot.read() {
        if let Some(token) = guard.as_ref() {
            token.cancel();
            return true;
        }
    }
    false
}

/// 包装一个阶段调用：注册取消令牌，结束/出错后注销
fn with_cancel<T>(f: impl FnOnce(&CancelToken) -> Result<T, StudioError>) -> Result<T, StudioError> {
    let token = CancelToken::new();
    let slot = CANCEL_SLOT.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = slot.write() {
        *guard = Some(token.clone());
    }
    let result = f(&token);
    if let Ok(mut guard) = slot.write() {
        *guard = None;
    }
    result
}

// ============================================================================
// 第 1 步：构思（3 个候选；尚无项目，不记 AI 账本）
// ============================================================================

pub fn ai_book_concepts(
    options: &BookCreationOptions,
    provider: Arc<dyn story_ai_runtime::AiProvider>,
) -> Result<Vec<BookConcept>, StudioError> {
    let options = options.normalized();
    if options.idea.is_empty() {
        return Err(StudioError::Config(
            "灵感不能为空，先写下一句话想法".to_string(),
        ));
    }
    with_cancel(|cancel| {
        let json = llm_json(provider, cancel, None, concept_prompt(&options))?;
        parse_concepts(&json)
    })
}

fn parse_concepts(v: &serde_json::Value) -> Result<Vec<BookConcept>, StudioError> {
    let pick = |item: &serde_json::Value| BookConcept {
        title: opt_str(item, "title").unwrap_or_default(),
        tagline: opt_str(item, "tagline").unwrap_or_default(),
        blurb: opt_str(item, "blurb").unwrap_or_default(),
        premise: opt_str(item, "premise").unwrap_or_default(),
        theme: opt_str(item, "theme").unwrap_or_default(),
        setting: opt_str(item, "setting").unwrap_or_default(),
    };
    // 标准形态：{"concepts": [...]}
    let list: Option<Vec<BookConcept>> = v
        .get("concepts")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().map(pick).collect());
    let mut concepts = match list {
        Some(l) if !l.is_empty() => l,
        // 容错：模型直接给了单个对象 → 视为唯一候选
        _ if v.get("title").is_some() => vec![pick(v)],
        _ => {
            return Err(StudioError::Ai(
                "构思输出里没有候选（缺 concepts 数组或 title 字段），请重试".into(),
            ))
        }
    };
    // 只留有书名的候选，最多 3 个
    concepts.retain(|c| !c.title.is_empty());
    concepts.truncate(CONCEPT_CANDIDATES);
    if concepts.is_empty() {
        return Err(StudioError::Ai("构思候选都没有书名，请重试".into()));
    }
    Ok(concepts)
}

fn concept_prompt(o: &BookCreationOptions) -> String {
    let mut extras = String::new();
    if let Some(a) = &o.audience {
        extras.push_str(&format!("\n【面向人群】{a}"));
    }
    if let Some(p) = &o.pov {
        extras.push_str(&format!("\n【叙事视角】{p}"));
    }
    if let Some(s) = &o.style {
        extras.push_str(&format!("\n【文风基调】{s}"));
    }
    if let Some(w) = o.target_words {
        extras.push_str(&format!("\n【体量目标】约 {} 万字", w / 10000));
    }
    if let Some(c) = o.chapter_words {
        extras.push_str(&format!("\n【单章篇幅】约 {c} 字"));
    }
    let guide = o
        .genre
        .as_deref()
        .and_then(|g| crate::tools::guide::resolve_guide_text(g, None))
        .map(|g| format!("\n【题材创作指导（要点）】\n{}\n", truncate_chars(&g, 1600)))
        .unwrap_or_default();
    let genre_line = o
        .genre
        .as_deref()
        .map(|g| format!("\n【题材】{g}"))
        .unwrap_or_else(|| "\n【题材】由你判断最合适的题材".to_string());

    format!(
        "你是一位资深网文主编，擅长把一句话灵感打磨成能长跑的系列企划。\n\
        \n【灵感】{}\n{genre_line}{extras}\n{guide}\n\
        请给出 {CONCEPT_CANDIDATES} 个差异明显的候选方向（题材切入点、主角处境、卖点各不相同）。\n\
        只输出一个 JSON 对象（字符串内的引号用「」），字段：\n\
        - concepts：数组，{CONCEPT_CANDIDATES} 项，每项：\n\
        \x20 - title：书名，2-8 字，有记忆点\n\
        \x20 - tagline：一句话卖点\n\
        \x20 - blurb：给读者看的简介，80-150 字\n\
        \x20 - premise：核心设定与主线脉络，200-300 字（写清世界基调、主角处境、核心冲突与长线悬念）\n\
        \x20 - theme：主题词\n\
        \x20 - setting：故事发生地概述",
        o.idea
    )
}

// ============================================================================
// 第 2 步：开书（编辑后的概念 + 参数 → 项目落盘）
// ============================================================================

/// 用确认后的概念建项目，返回项目根目录。
///
/// 参数随项目持久化到 outline.toml（audience / target_words / pov / style /
/// chapter_words），之后逐章成书与续排大纲都会遵守这些约束。
pub fn create_book_project(
    parent_dir: &Path,
    concept: &BookConcept,
    options: &BookCreationOptions,
) -> Result<String, StudioError> {
    let title = concept.title.trim();
    if title.is_empty() {
        return Err(StudioError::Config("书名不能为空".to_string()));
    }
    if concept.premise.trim().is_empty() {
        return Err(StudioError::Config("主线不能为空".to_string()));
    }
    let o = options.normalized();

    let root = crate::project::create_project(parent_dir, title)?;
    let paths = ProjectPaths::new(&root);

    let mut outline = OutlineFile::default();
    outline.story.title = Some(title.to_string());
    outline.story.genre = o.genre.clone();
    outline.story.premise = Some(concept.premise.trim().to_string());
    if !concept.theme.trim().is_empty() {
        outline.story.theme = Some(concept.theme.trim().to_string());
    }
    if !concept.setting.trim().is_empty() {
        outline.story.setting = Some(concept.setting.trim().to_string());
    }
    outline.story.audience = o.audience.clone();
    outline.story.pov = o.pov.clone();
    outline.story.style = o.style.clone();
    outline.story.chapter_words = o.chapter_words;
    outline.story.target_words = Some(
        o.target_words
            .unwrap_or(DEFAULT_CHAPTER_WORDS * o.chapter_count as u64),
    );
    write_toml(&paths, "outline", &paths.root().join("outline.toml"), &outline)?;

    Ok(root.display().to_string())
}

// ============================================================================
// 第 3 步：设定（草稿 / 保存）
// ============================================================================

/// 生成设定草稿（纯函数，不落盘）。用**用户编辑后**的概念做上下文。
pub fn ai_book_world(
    concept: &BookConcept,
    options: &BookCreationOptions,
    provider: Arc<dyn story_ai_runtime::AiProvider>,
) -> Result<WorldDraft, StudioError> {
    let o = options.normalized();
    let n_chars = o.character_count.unwrap_or(6);
    with_cancel(|cancel| {
        let json = llm_json(provider, cancel, None, world_prompt(concept, &o, n_chars))?;
        Ok(parse_world_draft(&json, n_chars))
    })
}

fn world_prompt(concept: &BookConcept, o: &BookCreationOptions, n_chars: u32) -> String {
    let mut extras = String::new();
    if let Some(a) = &o.audience {
        extras.push_str(&format!("【面向人群】{a}\n"));
    }
    if let Some(p) = &o.pov {
        extras.push_str(&format!("【叙事视角】{p}\n"));
    }
    if let Some(s) = &o.style {
        extras.push_str(&format!("【文风基调】{s}\n"));
    }
    let genre_line = o
        .genre
        .as_deref()
        .map(|g| format!("【题材】{g}\n"))
        .unwrap_or_default();
    format!(
        "你是一位世界观与人物设定的行家，为下面这本新书搭建设定。\n\
        \n【书名】《{}》\n{genre_line}{extras}【主线】{}\n【主题】{}\n\
        \n只输出一个 JSON 对象（字符串内的引号用「」），字段：\n\
        - concepts：[{{name, category, summary}}] 3-6 项基础概念（世界规则/纪元/种族等）\n\
        - systems：[{{name, kind, levels, rules, summary}}] 0-3 项力量体系（kind 取 cultivation/magic/technology/power；levels 写等级链；rules 写规则与代价）\n\
        - locations：[{{name, kind, parent, features, summary}}] 3-8 个地点，覆盖世界地理与剧情发生地（kind 取 continent/nation/city/realm/landform；大区域在前、具体地点在后；parent 填上级地点名）\n\
        - organizations：[{{name, kind, leader, territory, ideology, summary}}] 0-4 个组织势力（kind 取 sect/nation/clan/guild/organization）\n\
        - eras：[纪元名] 0-3 个（故事内历法）\n\
        - characters：[{{name, role, gender, occupation, personality, background, aliases:[别名]}}] 恰好 {n_chars} 个主要角色（role 取 protagonist/love_interest/antagonist/mentor/support；必须含 1 名主角与 1 名主要反派）\n\
        - relations：[{{from, to, relation}}] 角色间关系（from/to 用角色名；relation 如 师徒/宿敌/兄妹）\n\
        - threads：[{{title, description, priority}}] 2-5 条叙事线索（伏笔/悬念；priority 取 main/minor）",
        concept.title, concept.premise, concept.theme
    )
}

fn parse_world_draft(v: &serde_json::Value, max_chars: u32) -> WorldDraft {
    let s = |x: &serde_json::Value, k: &str| opt_str(x, k).unwrap_or_default();
    // prompt 规定世界观段嵌在 "worldview" 下；容错也接受平铺形态
    let wv = v.get("worldview").unwrap_or(v);
    let arr = |key: &str| -> Vec<serde_json::Value> {
        wv.get(key)
            .or_else(|| v.get(key))
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let n_chars = max_chars as usize;

    let mut draft = WorldDraft {
        concepts: arr("concepts")
            .iter()
            .take(8)
            .map(|c| ConceptItem {
                name: s(c, "name"),
                category: s(c, "category"),
                summary: s(c, "summary"),
            })
            .collect(),
        systems: arr("systems")
            .iter()
            .take(4)
            .map(|x| SystemItem {
                name: s(x, "name"),
                kind: s(x, "kind"),
                levels: s(x, "levels"),
                rules: s(x, "rules"),
                summary: s(x, "summary"),
            })
            .collect(),
        eras: wv
            .get("eras")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .take(4)
                    .map(|x| x.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        characters: Vec::new(),
        locations: arr("locations")
            .iter()
            .take(10)
            .map(|x| LocationItem {
                name: s(x, "name"),
                kind: s(x, "kind"),
                parent: s(x, "parent"),
                features: s(x, "features"),
                summary: s(x, "summary"),
            })
            .collect(),
        organizations: arr("organizations")
            .iter()
            .take(6)
            .map(|x| OrganizationItem {
                name: s(x, "name"),
                kind: s(x, "kind"),
                leader: s(x, "leader"),
                territory: s(x, "territory"),
                ideology: s(x, "ideology"),
                summary: s(x, "summary"),
            })
            .collect(),
        relations: arr("relations")
            .iter()
            .take(20)
            .map(|x| RelationItem {
                from: s(x, "from"),
                to: s(x, "to"),
                relation: s(x, "relation"),
            })
            .collect(),
        threads: arr("threads")
            .iter()
            .take(8)
            .map(|x| ThreadItem {
                title: s(x, "title"),
                description: s(x, "description"),
                priority: s(x, "priority"),
            })
            .collect(),
    };

    // 角色：留有名字的，去重，限 max_chars
    for c in arr("characters") {
        let name = s(&c, "name");
        if name.is_empty() || draft.characters.len() >= n_chars {
            continue;
        }
        if draft.characters.iter().any(|x| x.name == name) {
            continue;
        }
        let aliases = c
            .get("aliases")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::trim)
                    .filter(|x| !x.is_empty() && *x != name)
                    .map(|x| x.to_string())
                    .collect()
            })
            .unwrap_or_default();
        draft.characters.push(CharacterItem {
            name,
            role: s(&c, "role"),
            gender: s(&c, "gender"),
            occupation: s(&c, "occupation"),
            personality: s(&c, "personality"),
            background: s(&c, "background"),
            aliases,
        });
    }
    // 无名条目清理
    draft.concepts.retain(|x| !x.name.is_empty());
    draft.systems.retain(|x| !x.name.is_empty());
    draft.locations.retain(|x| !x.name.is_empty());
    draft.organizations.retain(|x| !x.name.is_empty());
    draft
        .relations
        .retain(|x| !x.from.is_empty() && !x.to.is_empty() && !x.relation.is_empty());
    draft.threads.retain(|x| !x.title.is_empty());
    draft
}

/// 保存设定草稿（确定性落盘：worldview / entities / relations / threads）。
///
/// 注意：重复调用会**追加**（不覆盖已有实体），向导只在第 3 步保存一次；
/// 事后修改请用设定面板或妙笔工具。
pub fn save_book_world(root: &Path, draft: &WorldDraft) -> Result<WorldSaveReport, StudioError> {
    let paths = ProjectPaths::new(root);

    // worldview/ 目录（每部分一个文件）
    write_toml(
        &paths,
        "worldview/concepts",
        &paths.worldview_concepts_toml(),
        &story_project_format::ConceptsFile {
            concepts: draft
                .concepts
                .iter()
                .map(|c| story_project_format::ConceptDef {
                    name: c.name.clone(),
                    category: non_empty(&c.category),
                    summary: non_empty(&c.summary),
                    details: None,
                })
                .collect(),
        },
    )?;
    write_toml(
        &paths,
        "worldview/systems",
        &paths.worldview_systems_toml(),
        &story_project_format::SystemsFile {
            systems: draft
                .systems
                .iter()
                .map(|x| story_project_format::SystemDef {
                    name: x.name.clone(),
                    kind: non_empty(&x.kind),
                    levels: non_empty(&x.levels),
                    rules: non_empty(&x.rules),
                    summary: non_empty(&x.summary),
                })
                .collect(),
        },
    )?;
    write_toml(
        &paths,
        "worldview/timeline",
        &paths.worldview_timeline_toml(),
        &story_project_format::TimelineFile {
            eras: draft.eras.clone(),
        },
    )?;

    // 角色实体（含档案 + 别名）
    let mut char_ids: Vec<(String, String)> = Vec::new();
    for c in &draft.characters {
        if c.name.trim().is_empty() {
            continue;
        }
        let id = mutations::create_entity(&paths, EntityKind::Character, c.name.trim())?;
        if let Some(mut entity) = load_entity(&paths, &id) {
            entity.aliases = c.aliases.clone();
            let profile = entity.profile.get_or_insert_with(Default::default);
            if profile.gender.is_none() {
                profile.gender = non_empty(&c.gender);
            }
            if profile.occupation.is_none() {
                profile.occupation = non_empty(&c.occupation);
            }
            if profile.personality.is_none() {
                profile.personality = non_empty(&c.personality);
            }
            if profile.background.is_none() {
                profile.background = non_empty(&c.background);
            }
            let _ = write_entity(&paths, &entity);
        }
        char_ids.push((c.name.trim().to_string(), id));
    }

    // 地点实体（世界地理与剧情地点统一建档；parent 为地点名，二遍解析为 ID）
    let mut locations = 0usize;
    let mut loc_ids: Vec<(String, String)> = Vec::new();
    for l in &draft.locations {
        if l.name.trim().is_empty() {
            continue;
        }
        let name = l.name.trim().to_string();
        let id = mutations::create_entity(&paths, EntityKind::Location, &name)?;
        if let Some(mut entity) = load_entity(&paths, &id) {
            let geo = entity.geography.get_or_insert_with(Default::default);
            geo.kind = non_empty(&l.kind);
            geo.features = non_empty(&l.features);
            geo.summary = non_empty(&l.summary);
            let _ = write_entity(&paths, &entity);
        }
        loc_ids.push((name, id));
        locations += 1;
    }
    // 第二遍：上级地名 → Location ID（解析不到的忽略）
    for l in &draft.locations {
        let parent = l.parent.trim();
        if parent.is_empty() {
            continue;
        }
        let Some((_, child_id)) = loc_ids.iter().find(|(n, _)| n == l.name.trim()) else {
            continue;
        };
        let Some((_, parent_id)) = loc_ids.iter().find(|(n, _)| n == parent) else {
            continue;
        };
        if let Some(mut entity) = load_entity(&paths, child_id) {
            if let Some(geo) = entity.geography.as_mut() {
                geo.parent = Some(parent_id.clone());
                let _ = write_entity(&paths, &entity);
            }
        }
    }

    // 组织实体（世界观势力统一建档）
    let mut organizations = 0usize;
    for o in &draft.organizations {
        if o.name.trim().is_empty() {
            continue;
        }
        let id = mutations::create_entity(&paths, EntityKind::Organization, o.name.trim())?;
        if let Some(mut entity) = load_entity(&paths, &id) {
            let org = entity.org.get_or_insert_with(Default::default);
            org.kind = non_empty(&o.kind);
            org.leader = non_empty(&o.leader);
            org.territory = non_empty(&o.territory);
            org.ideology = non_empty(&o.ideology);
            org.summary = non_empty(&o.summary);
            let _ = write_entity(&paths, &entity);
        }
        organizations += 1;
    }

    // 关系（名字 → id；端点不存在的边丢弃）
    let mut relations = 0usize;
    for r in &draft.relations {
        let (Some((_, from_id)), Some((_, to_id))) = (
            char_ids.iter().find(|(n, _)| n == r.from.trim()),
            char_ids.iter().find(|(n, _)| n == r.to.trim()),
        ) else {
            continue;
        };
        if mutations::add_relation(&paths, from_id, to_id, r.relation.trim()).is_ok() {
            relations += 1;
        }
    }

    // 叙事线索
    let mut threads = 0usize;
    for t in &draft.threads {
        if t.title.trim().is_empty() {
            continue;
        }
        let priority = if t.priority == "main" { "main" } else { "minor" };
        if mutations::create_thread(&paths, t.title.trim(), &t.description, priority, None).is_ok() {
            threads += 1;
        }
    }

    Ok(WorldSaveReport {
        characters: char_ids.len(),
        locations,
        organizations,
        relations,
        threads,
    })
}

// ============================================================================
// 第 4 步：章节脉络批次（草稿 / 追加保存；写作中可反复续排）
// ============================================================================

/// 生成下一批章节脉络草稿（纯函数，不落盘）。
///
/// 上下文：outline 主线与约束 + 已有卷结构 + 最近脉络 + 已完成章节梗概 +
/// 主要角色名——写作中途续排时，AI 对走向的判断比开书时准。
pub fn ai_book_outline_batch(
    root: &Path,
    count: u32,
    provider: Arc<dyn story_ai_runtime::AiProvider>,
) -> Result<OutlineBatchDraft, StudioError> {
    let count = if count == 0 { 12 } else { count }.clamp(1, MAX_BATCH_CHAPTERS);
    let context = outline_context(root)?;
    with_cancel(|cancel| {
        let json = llm_json(provider, cancel, Some(root), outline_prompt(&context, count))?;
        Ok(parse_outline_draft(&json, count as usize))
    })
}

/// 续排上下文（独立出来便于测试）
pub(crate) struct OutlineContext {
    pub premise: String,
    pub genre: Option<String>,
    pub pov: Option<String>,
    pub style: Option<String>,
    pub audience: Option<String>,
    pub chapter_words: Option<u64>,
    pub character_names: Vec<String>,
    pub thread_titles: Vec<String>,
    /// 已有卷（name, order, 已排章数）
    pub volumes: Vec<(String, u32, usize)>,
    /// 最近若干章脉络（标题, 梗概, 钩子）旧 → 新
    pub recent_chapters: Vec<(String, String, String)>,
    /// 已完成章节的梗概（旧 → 新，最近 8 章）
    pub completed_summaries: Vec<String>,
    pub existing_chapters: usize,
}

fn outline_context(root: &Path) -> Result<OutlineContext, StudioError> {
    let paths = ProjectPaths::new(root);
    let outline = OutlineFile::from_toml_str(
        &std::fs::read_to_string(paths.root().join("outline.toml")).map_err(|_| {
            StudioError::NotFound("outline.toml 不存在——请先完成开书步骤".into())
        })?,
    )
    .map_err(|e| StudioError::Config(format!("outline.toml 解析失败: {e}")))?;

    // 主要角色名
    let mut character_names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths.characters_dir()) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(c) = std::fs::read_to_string(&p) {
                if let Ok(entity) = story_project_format::EntityFile::from_toml_str(&c) {
                    character_names.push(entity.display_name);
                }
            }
        }
    }
    character_names.sort();

    // 线索标题
    let thread_titles: Vec<String> = std::fs::read_to_string(paths.threads_toml())
        .ok()
        .and_then(|c| story_project_format::ThreadsFile::from_toml_str(&c).ok())
        .map(|f| f.threads.iter().map(|t| t.title.clone()).collect())
        .unwrap_or_default();

    // 章节脉络
    let chapters_file = std::fs::read_to_string(paths.chapters_toml())
        .ok()
        .and_then(|c| story_project_format::ChaptersFile::from_toml_str(&c).ok())
        .unwrap_or_default();

    let volumes: Vec<(String, u32, usize)> = chapters_file
        .volumes
        .iter()
        .map(|v| {
            let n = chapters_file
                .chapters
                .iter()
                .filter(|c| c.volume_order == v.order)
                .count();
            (v.name.clone(), v.order, n)
        })
        .collect();

    let recent_chapters: Vec<(String, String, String)> = chapters_file
        .chapters
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|c| {
            (
                c.title.clone(),
                c.summary.clone().unwrap_or_default(),
                c.hook.clone().unwrap_or_default(),
            )
        })
        .collect();

    let mut completed_summaries: Vec<String> = chapters_file
        .chapters
        .iter()
        .filter(|c| c.status == "completed")
        .map(|c| {
            format!(
                "《{}》：{}",
                c.title,
                c.summary.clone().unwrap_or_default()
            )
        })
        .collect();
    let keep = completed_summaries.len().saturating_sub(8);
    completed_summaries.drain(0..keep);

    Ok(OutlineContext {
        premise: outline.story.premise.clone().unwrap_or_default(),
        genre: outline.story.genre.clone(),
        pov: outline.story.pov.clone(),
        style: outline.story.style.clone(),
        audience: outline.story.audience.clone(),
        chapter_words: outline.story.chapter_words,
        character_names,
        thread_titles,
        volumes,
        recent_chapters,
        completed_summaries,
        existing_chapters: chapters_file.chapters.len(),
    })
}

fn outline_prompt(ctx: &OutlineContext, count: u32) -> String {
    let mut head = String::new();
    if let Some(g) = &ctx.genre {
        head.push_str(&format!("【题材】{g}\n"));
    }
    if let Some(a) = &ctx.audience {
        head.push_str(&format!("【面向人群】{a}\n"));
    }
    if let Some(p) = &ctx.pov {
        head.push_str(&format!("【叙事视角】{p}\n"));
    }
    if let Some(s) = &ctx.style {
        head.push_str(&format!("【文风基调】{s}\n"));
    }
    if let Some(w) = ctx.chapter_words {
        head.push_str(&format!("【单章篇幅】约 {w} 字\n"));
    }

    let mut existing = String::new();
    if !ctx.volumes.is_empty() {
        let vols: Vec<String> = ctx
            .volumes
            .iter()
            .map(|(n, o, c)| format!("{o}. {n}（已排 {c} 章）"))
            .collect();
        existing.push_str(&format!("\n【卷结构】{}\n", vols.join("；")));
    }
    if !ctx.recent_chapters.is_empty() {
        existing.push_str("\n【最近的章节脉络（旧 → 新）】\n");
        for (t, s, h) in &ctx.recent_chapters {
            existing.push_str(&format!("- 《{t}》{s}"));
            if !h.is_empty() {
                existing.push_str(&format!("（钩子：{h}）"));
            }
            existing.push('\n');
        }
    }
    if !ctx.completed_summaries.is_empty() {
        existing.push_str("\n【已写完的章节（梗概，旧 → 新）】\n");
        for s in &ctx.completed_summaries {
            existing.push_str(&format!("- {s}\n"));
        }
    }
    let next_index = ctx.existing_chapters + 1;

    format!(
        "你是一位精通节奏设计的网文大纲师，为这本书续排接下来的章节脉络。\n\
        \n【书的主线】{}\n{head}{existing}【主要角色】{}\n【叙事线索】{}\n\
        \n接下来是第 {next_index} 章起，请排 {} 章的脉络。\
        若最后一卷已排超过 12 章，请在 volumes 里给出下一卷；否则 volumes 留空数组。\n\
        只输出一个 JSON 对象（字符串内的引号用「」），字段：\n\
        - volumes：[{{name, order, theme, summary}}] 新增卷（order 接续现有最大卷号）\n\
        - chapters：[{{title, volume_order, summary, goal, beats:[...], characters:[出场角色名], hook}}] \
        恰好 {count} 章，每章 beats 3-6 个（每个一句话）；要与前面的钩子衔接、章章有推进，\
        推进叙事线索但不要在批内仓促收束长线悬念",
        ctx.premise,
        if ctx.character_names.is_empty() {
            "（尚未建角色）".to_string()
        } else {
            ctx.character_names.join("、")
        },
        if ctx.thread_titles.is_empty() {
            "（尚未建线索）".to_string()
        } else {
            ctx.thread_titles.join("、")
        },
        count,
    )
}

fn parse_outline_draft(v: &serde_json::Value, target: usize) -> OutlineBatchDraft {
    let s = |x: &serde_json::Value, k: &str| opt_str(x, k).unwrap_or_default();
    let str_arr = |item: &serde_json::Value, key: &str| -> Vec<String> {
        item.get(key)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(|x| x.to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut volumes: Vec<VolumeItem> = v
        .get("volumes")
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            let name = s(item, "name");
            if name.is_empty() {
                return None;
            }
            Some(VolumeItem {
                name,
                order: item.get("order").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                theme: s(item, "theme"),
                summary: s(item, "summary"),
            })
        })
        .take(8)
        .collect();
    for (i, vol) in volumes.iter_mut().enumerate() {
        if vol.order == 0 {
            vol.order = (i + 1) as u32;
        }
    }

    let chapters: Vec<ChapterItem> = v
        .get("chapters")
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            let title = s(item, "title");
            if title.is_empty() {
                return None;
            }
            Some(ChapterItem {
                title,
                volume_order: item
                    .get("volume_order")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(1) as u32,
                summary: s(item, "summary"),
                goal: s(item, "goal"),
                beats: str_arr(item, "beats"),
                characters: str_arr(item, "characters"),
                hook: s(item, "hook"),
            })
        })
        .take(target)
        .collect();

    OutlineBatchDraft { volumes, chapters }
}

/// 保存章节脉络批次（追加 chapters.toml；volume_order 不在现有卷范围内
/// 的章节归入最后一卷）。
pub fn save_book_outline_batch(root: &Path, draft: &OutlineBatchDraft) -> Result<usize, StudioError> {
    if draft.chapters.is_empty() {
        return Err(StudioError::Config("没有可保存的章节".to_string()));
    }
    let paths = ProjectPaths::new(root);
    let chapters_path = paths.chapters_toml();
    let mut file = std::fs::read_to_string(&chapters_path)
        .ok()
        .and_then(|c| story_project_format::ChaptersFile::from_toml_str(&c).ok())
        .unwrap_or_default();

    // 追加新卷（order 冲突时顺延）
    let mut max_volume = file.volumes.iter().map(|v| v.order).max().unwrap_or(0);
    for v in &draft.volumes {
        if v.name.is_empty() || file.volumes.iter().any(|x| x.name == v.name) {
            continue;
        }
        let order = if v.order > max_volume {
            v.order
        } else {
            max_volume + 1
        };
        max_volume = order.max(max_volume);
        file.volumes.push(story_project_format::VolumeGroupDef {
            name: v.name.clone(),
            order,
            theme: non_empty(&v.theme),
            summary: non_empty(&v.summary),
        });
    }
    if file.volumes.is_empty() {
        // 无卷结构时兜底建第一卷
        file.volumes.push(story_project_format::VolumeGroupDef {
            name: "第一卷".to_string(),
            order: 1,
            theme: None,
            summary: None,
        });
    }

    let valid_orders: Vec<u32> = file.volumes.iter().map(|v| v.order).collect();
    // 向导表单收集的是角色名；workflow 按 StableId 解析 POV 与角色卡，这里统一映射为实体 ID
    //（名字或别名匹配，匹配不到的丢弃——避免写入悬空名字导致 POV 失效）
    let char_ids = character_id_map(&paths);
    for c in &draft.chapters {
        let volume_order = if valid_orders.contains(&c.volume_order) {
            c.volume_order
        } else {
            *valid_orders.last().unwrap_or(&1)
        };
        file.chapters.push(story_project_format::ChapterOutlineDef {
            chapter_id: None,
            title: c.title.clone(),
            volume_order,
            summary: non_empty(&c.summary),
            goal: non_empty(&c.goal),
            beats: c.beats.clone(),
            characters: c
                .characters
                .iter()
                .filter_map(|n| char_ids.get(n.trim()))
                .cloned()
                .collect(),
            threads: Vec::new(),
            scenes: Vec::new(),
            hook: non_empty(&c.hook),
            notes: None,
            status: "planned".to_string(),
        });
    }
    let saved = draft.chapters.len();
    write_toml(&paths, "chapters", &chapters_path, &file)?;
    Ok(saved)
}

// ============================================================================
// LLM 调用与解析
// ============================================================================

/// 一次 LLM 调用 → 宽松解析为 JSON 对象。
///
/// `ledger_root` 为 Some 时记 AI 账本（构思阶段尚无项目，跳过记账）。
fn llm_json(
    provider: Arc<dyn story_ai_runtime::AiProvider>,
    cancel: &CancelToken,
    ledger_root: Option<&Path>,
    prompt: String,
) -> Result<serde_json::Value, StudioError> {
    let request = GenerateRequest {
        system: "你为中文网络小说创作服务，只输出规定格式的 JSON，不输出任何解释或代码围栏以外的文字。".into(),
        messages: vec![Message::user(prompt)],
        model: String::new(),
        temperature: 0.7,
        // 思考模式下需要预算余量，否则 JSON 会被截断
        max_tokens: 16384,
        tools: Vec::new(),
    };

    let started = std::time::Instant::now();
    let result =
        stream_generate_with_cancel(provider.clone(), &request, cancel, Arc::new(|_| {}));
    if let Some(root) = ledger_root {
        match &result {
            Ok(resp) => crate::ai_ledger::record(
                root,
                "AI 建书·大纲",
                provider.id(),
                resp.usage.total() as u64,
                started.elapsed().as_millis() as u64,
                true,
                None,
            ),
            Err(ref e) => crate::ai_ledger::record(
                root,
                "AI 建书·大纲",
                provider.id(),
                0,
                started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            ),
        }
    }

    let response = result.map_err(|e| StudioError::Ai(format!("AI 调用失败: {e}")))?;
    parse_json_object_lenient(&response.text).map_err(|e| {
        StudioError::Ai(format!(
            "输出无法解析为 JSON（{e}）。请重试，或换一个更稳定的模型"
        ))
    })
}

/// 宽松解析模型输出的 JSON 对象：原文 → 剥代码围栏 → 截取首 '{' 到末 '}'
pub(crate) fn parse_json_object_lenient(raw: &str) -> Result<serde_json::Value, String> {
    let t = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        if v.is_object() {
            return Ok(v);
        }
    }
    // 剥 ```json 围栏
    let body = if let Some(stripped) = t.strip_prefix("```") {
        stripped
            .trim_start_matches("json")
            .trim_start_matches("JSON")
            .trim_start()
            .strip_suffix("```")
            .map(str::trim)
            .unwrap_or(t)
    } else {
        t
    };
    if let (Some(s), Some(e)) = (body.find('{'), body.rfind('}')) {
        if e > s {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body[s..=e]) {
                if v.is_object() {
                    return Ok(v);
                }
            }
        }
    }
    Err("输出中找不到 JSON 对象".to_string())
}

fn opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn non_empty(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s.trim().to_string())
    }
}

/// 角色名 / 别名 → StableId 映射（含别名，供名字形态的草稿字段落盘为 ID）
fn character_id_map(paths: &ProjectPaths) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(paths.characters_dir()) else {
        return map;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("toml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(entity) = story_project_format::EntityFile::from_toml_str(&content) else {
            continue;
        };
        map.entry(entity.display_name).or_insert_with(|| entity.id.clone());
        for alias in entity.aliases {
            map.entry(alias).or_insert_with(|| entity.id.clone());
        }
    }
    map
}

/// 按字符数截断（中文安全）
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

// ============================================================================
// 落盘辅助
// ============================================================================

fn write_toml<T: serde::Serialize>(
    paths: &ProjectPaths,
    op: &str,
    path: &std::path::PathBuf,
    value: &T,
) -> Result<(), StudioError> {
    let content = toml::to_string(value)
        .map_err(|e| StudioError::Config(format!("序列化 {op}.toml 失败: {e}")))?;
    journal::write_verified(paths.root(), op, path, &content)
}

fn load_entity(paths: &ProjectPaths, id: &str) -> Option<story_project_format::EntityFile> {
    let path = paths.file_for_id(id)?;
    let content = std::fs::read_to_string(path).ok()?;
    story_project_format::EntityFile::from_toml_str(&content).ok()
}

fn write_entity(
    paths: &ProjectPaths,
    entity: &story_project_format::EntityFile,
) -> Result<(), StudioError> {
    let path = paths
        .file_for_id(&entity.id)
        .ok_or_else(|| StudioError::Config("无效实体 ID".into()))?;
    let content = entity
        .to_toml_string()
        .map_err(|e| StudioError::Config(format!("序列化实体失败: {e}")))?;
    journal::write_verified(paths.root(), "entity_write", &path, &content)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ai_runtime::provider::{AiError, GenerateResponse, StopReason, TokenUsage};
    use story_ai_runtime::AiProvider;
    use std::sync::Mutex;

    /// 按脚本顺序回放响应的 Mock Provider
    #[derive(Debug)]
    struct ScriptedProvider {
        responses: Mutex<Vec<String>>,
    }

    impl AiProvider for ScriptedProvider {
        fn id(&self) -> &str {
            "scripted"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            let mut q = self.responses.lock().unwrap();
            if q.is_empty() {
                return Err(AiError::Api {
                    status: 500,
                    message: "脚本响应耗尽".into(),
                });
            }
            Ok(GenerateResponse {
                text: q.remove(0),
                usage: TokenUsage::default(),
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    fn scripted(responses: Vec<String>) -> Arc<dyn AiProvider> {
        Arc::new(ScriptedProvider {
            responses: Mutex::new(responses),
        })
    }

    const CONCEPTS_JSON: &str = r#"{"concepts":[
        {"title":"拾灯录","tagline":"守灯人最后一个徒弟","blurb":"灵灯将熄，少年携灯走遍山海。","premise":"灵灯逐盏熄灭，守灯学徒阿澈须在灯灭前点燃新灯，却发现熄灯的正是祖师。","theme":"传承与背叛","setting":"架空东方大陆"},
        {"title":"灯下客","tagline":"每个来客都藏着一盏灯","blurb":"客栈掌柜收集故事也收集灯。","premise":"长明镇客栈掌柜以故事换灯油，客人各有隐秘。","theme":"人情","setting":"小镇客栈"},
        {"title":"烬火行","tagline":"从灰烬里重新点火","blurb":"王朝焚书后，点灯人造反。","premise":"焚书王朝与点灯人组织的暗战。","theme":"自由","setting":"王朝末年"}
    ]}"#;

    const WORLD_JSON: &str = r#"{"worldview":{"concepts":[{"name":"灵灯","category":"世界规则","summary":"以灯载魂"}],"systems":[{"name":"守灯术","kind":"magic","levels":"照夜→引星→续日","rules":"点灯折寿"}],"eras":["灯历"]},
      "characters":[{"name":"阿澈","role":"protagonist","gender":"男","occupation":"守灯学徒","personality":"执拗","background":"孤儿入山门","aliases":["小澈"]},{"name":"沈青梧","role":"love_interest","gender":"女","occupation":"游方医","personality":"冷静","background":"医毒双修"},{"name":"老瞎子","role":"antagonist","gender":"男","occupation":"祖师","personality":"阴鸷","background":"假装失明"}],
      "locations":[{"name":"云隐洲","kind":"continent","features":"多雨多灯","summary":"故事所在大陆"},{"name":"长明镇","kind":"city","parent":"云隐洲","summary":"故事起点"}],
      "organizations":[{"name":"守灯阁","kind":"sect","leader":"祖师","territory":"云隐洲","summary":"守灯人宗门"}],
      "relations":[{"from":"阿澈","to":"沈青梧","relation":"同行"},{"from":"阿澈","to":"不存在的人","relation":"无效"}],
      "threads":[{"title":"祖师熄灯之谜","description":"灯为何一盏盏熄灭","priority":"main"},{"title":"阿澈的寿数","description":"点灯折寿的代价","priority":"minor"}]}"#;

    const OUTLINE_BATCH_JSON: &str = r#"{"volumes":[{"name":"第一卷 灯下","order":1,"theme":"启程","summary":"离乡"}],
      "chapters":[
        {"title":"灯灭之夜","volume_order":1,"summary":"山门最后一盏灯熄","goal":"引出主线","beats":["灯灭","下山","遇袭"],"characters":["阿澈"],"hook":"来客身份"},
        {"title":"长明镇","volume_order":1,"summary":"初到长明镇","goal":"结识沈青梧","beats":["入镇","义诊","同行"],"characters":["阿澈","沈青梧"],"hook":"灯渣"},
        {"title":"灯渣","volume_order":1,"summary":"追查灯渣来源","goal":"指向祖师","beats":["化验","对质","怀疑"],"characters":["阿澈","老瞎子"],"hook":"祖师真面目"}
      ]}"#;

    const OUTLINE_BATCH2_JSON: &str = r#"{"volumes":[{"name":"第二卷 上山","order":2,"theme":"对峙","summary":"上山对质"}],
      "chapters":[
        {"title":"上山","volume_order":2,"summary":"夜上守灯阁","goal":"逼近真相","beats":["潜入","发现","被困"],"characters":["阿澈","老瞎子"],"hook":"阁中灯海"}
      ]}"#;

    fn options() -> BookCreationOptions {
        BookCreationOptions {
            idea: "守灯少年".into(),
            genre: Some("玄幻".into()),
            chapter_count: 3,
            target_words: Some(1_000_000),
            chapter_words: Some(3000),
            audience: Some("男频".into()),
            pov: Some("第三人称限知".into()),
            style: Some("热血".into()),
            character_count: Some(3),
        }
    }

    #[test]
    fn concepts_parse_three_candidates() {
        let concepts =
            ai_book_concepts(&options(), scripted(vec![CONCEPTS_JSON.to_string()])).unwrap();
        assert_eq!(concepts.len(), 3);
        assert_eq!(concepts[0].title, "拾灯录");
        assert_eq!(concepts[2].tagline, "从灰烬里重新点火");
    }

    #[test]
    fn concepts_single_object_fallback() {
        let single = r#"{"title":"独苗","tagline":"t","blurb":"b","premise":"p","theme":"th","setting":"st"}"#;
        let concepts =
            ai_book_concepts(&options(), scripted(vec![single.to_string()])).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].title, "独苗");
    }

    #[test]
    fn empty_idea_rejected() {
        let mut o = options();
        o.idea = "   ".into();
        let err = ai_book_concepts(&o, scripted(vec![])).unwrap_err();
        assert!(err.to_string().contains("灵感"));
    }

    #[test]
    fn create_project_persists_params() {
        let parent = tempfile::tempdir().unwrap();
        let concept = BookConcept {
            title: "拾灯录".into(),
            tagline: "t".into(),
            blurb: "b".into(),
            premise: "主线：灵灯将熄".into(),
            theme: "传承".into(),
            setting: "东方".into(),
        };
        let root = create_book_project(parent.path(), &concept, &options()).unwrap();
        let outline = OutlineFile::from_toml_str(
            &std::fs::read_to_string(Path::new(&root).join("outline.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(outline.story.title.as_deref(), Some("拾灯录"));
        assert_eq!(outline.story.premise.as_deref(), Some("主线：灵灯将熄"));
        assert_eq!(outline.story.audience.as_deref(), Some("男频"));
        assert_eq!(outline.story.pov.as_deref(), Some("第三人称限知"));
        assert_eq!(outline.story.style.as_deref(), Some("热血"));
        assert_eq!(outline.story.chapter_words, Some(3000));
        assert_eq!(outline.story.target_words, Some(1_000_000));
    }

    #[test]
    fn world_draft_parse_and_save() {
        let parent = tempfile::tempdir().unwrap();
        let concept = BookConcept {
            title: "拾灯录".into(),
            premise: "主线".into(),
            ..Default::default()
        };
        let root = create_book_project(parent.path(), &concept, &options()).unwrap();

        // 草稿解析：角色限 3 个、无效关系丢弃、空条目清理
        let draft =
            ai_book_world(&concept, &options(), scripted(vec![WORLD_JSON.to_string()])).unwrap();
        assert_eq!(draft.characters.len(), 3);
        assert_eq!(draft.characters[0].aliases, vec!["小澈"]);
        // 解析只清理空字段；端点校验发生在保存时（见下方 report 断言）
        assert_eq!(draft.relations.len(), 2);
        assert_eq!(draft.threads.len(), 2);

        // 保存落盘
        let report = save_book_world(Path::new(&root), &draft).unwrap();
        assert_eq!(report.characters, 3);
        assert_eq!(report.locations, 2);
        assert_eq!(report.organizations, 1);
        assert_eq!(report.relations, 1);
        assert_eq!(report.threads, 2);

        let paths = ProjectPaths::new(Path::new(&root));
        let che = std::fs::read_to_string(paths.characters_dir().join("c_0001.toml")).unwrap();
        assert!(che.contains("阿澈") && che.contains("小澈") && che.contains("执拗"));
        let concepts = std::fs::read_to_string(paths.worldview_concepts_toml()).unwrap();
        assert!(concepts.contains("灵灯"));
        let systems = std::fs::read_to_string(paths.worldview_systems_toml()).unwrap();
        assert!(systems.contains("守灯术"));
        let timeline = std::fs::read_to_string(paths.worldview_timeline_toml()).unwrap();
        assert!(timeline.contains("灯历"));
        // 地理与势力统一为实体：云隐洲（大陆）→ l_0001，长明镇（上级云隐洲）→ l_0002
        let loc1 = std::fs::read_to_string(paths.locations_dir().join("l_0001.toml")).unwrap();
        assert!(loc1.contains("云隐洲") && loc1.contains("continent") && loc1.contains("多雨多灯"));
        let loc2 = std::fs::read_to_string(paths.locations_dir().join("l_0002.toml")).unwrap();
        assert!(loc2.contains("长明镇") && loc2.contains("parent = \"l_0001\""));
        let org1 = std::fs::read_to_string(paths.organizations_dir().join("o_0001.toml")).unwrap();
        assert!(org1.contains("守灯阁") && org1.contains("祖师") && org1.contains("sect"));
        let relations = std::fs::read_to_string(paths.relations_toml()).unwrap();
        assert!(relations.contains("c_0001") && relations.contains("c_0002"));
        let threads = std::fs::read_to_string(paths.threads_toml()).unwrap();
        assert!(threads.contains("祖师熄灯之谜"));
    }

    #[test]
    fn outline_batch_append_and_incremental_context() {
        let parent = tempfile::tempdir().unwrap();
        let concept = BookConcept {
            title: "拾灯录".into(),
            premise: "主线：灵灯将熄，阿澈寻真相".into(),
            ..Default::default()
        };
        let root = create_book_project(parent.path(), &concept, &options()).unwrap();
        let root = Path::new(&root);

        // 先落设定（角色供续排上下文引用）
        let world = ai_book_world(&concept, &options(), scripted(vec![WORLD_JSON.into()])).unwrap();
        save_book_world(root, &world).unwrap();

        // 第一批：3 章 + 1 卷
        let draft = ai_book_outline_batch(root, 3, scripted(vec![OUTLINE_BATCH_JSON.into()]))
            .unwrap();
        assert_eq!(draft.chapters.len(), 3);
        assert_eq!(draft.volumes.len(), 1);
        let saved = save_book_outline_batch(root, &draft).unwrap();
        assert_eq!(saved, 3);

        // 标记第 1 章完成（模拟写作进度）
        let paths = ProjectPaths::new(root);
        let mut file = story_project_format::ChaptersFile::from_toml_str(
            &std::fs::read_to_string(paths.chapters_toml()).unwrap(),
        )
        .unwrap();
        file.chapters[0].status = "completed".into();
        std::fs::write(paths.chapters_toml(), file.to_toml_string().unwrap()).unwrap();

        // 增量上下文：含已完成梗概、最近脉络、卷结构、角色名
        let ctx = outline_context(root).unwrap();
        assert_eq!(ctx.existing_chapters, 3);
        assert!(ctx
            .completed_summaries
            .iter()
            .any(|s| s.contains("灯灭之夜")));
        assert!(ctx.recent_chapters.last().unwrap().0.contains("灯渣"));
        assert!(ctx.volumes[0].0.contains("灯下"));
        assert!(ctx.character_names.contains(&"阿澈".to_string()));

        // 第二批：追加 1 章 + 新卷（order=2）
        let draft2 = ai_book_outline_batch(root, 1, scripted(vec![OUTLINE_BATCH2_JSON.into()]))
            .unwrap();
        save_book_outline_batch(root, &draft2).unwrap();

        let file = story_project_format::ChaptersFile::from_toml_str(
            &std::fs::read_to_string(paths.chapters_toml()).unwrap(),
        )
        .unwrap();
        assert_eq!(file.chapters.len(), 4, "第二批应追加而非覆盖");
        assert_eq!(file.volumes.len(), 2);
        assert_eq!(file.chapters[3].volume_order, 2);
        assert_eq!(file.chapters[3].status, "planned");
        // 角色名已映射为实体 ID（workflow 按 ID 解析 POV 与角色卡）
        assert_eq!(file.chapters[0].characters, vec!["c_0001"]);
        assert_eq!(file.chapters[1].characters, vec!["c_0001", "c_0002"]);
    }

    #[test]
    fn outline_volume_order_fallback_to_last_volume() {
        let parent = tempfile::tempdir().unwrap();
        let concept = BookConcept {
            title: "书".into(),
            premise: "主线".into(),
            ..Default::default()
        };
        let root = create_book_project(parent.path(), &concept, &BookCreationOptions::default())
            .unwrap();
        // 草稿声称 volume_order=9（不存在）→ 归入第一卷
        let draft = OutlineBatchDraft {
            volumes: vec![VolumeItem {
                name: "第一卷".into(),
                order: 1,
                ..Default::default()
            }],
            chapters: vec![ChapterItem {
                title: "某章".into(),
                volume_order: 9,
                ..Default::default()
            }],
        };
        save_book_outline_batch(Path::new(&root), &draft).unwrap();
        let file = story_project_format::ChaptersFile::from_toml_str(
            &std::fs::read_to_string(
                ProjectPaths::new(Path::new(&root)).chapters_toml(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(file.chapters[0].volume_order, 1);
    }

    #[test]
    fn outline_requires_project() {
        let parent = tempfile::tempdir().unwrap();
        let err = ai_book_outline_batch(parent.path(), 3, scripted(vec![])).unwrap_err();
        assert!(err.to_string().contains("outline.toml"));
    }

    #[test]
    fn concept_prompt_includes_params_and_guide() {
        let p = concept_prompt(&options());
        assert!(p.contains("男频"));
        assert!(p.contains("第三人称限知"));
        assert!(p.contains("热血"));
        assert!(p.contains("100 万字"));
        assert!(p.contains("题材创作指导"), "玄幻应命中内置 fantasy 指导");
    }

    #[test]
    fn lenient_object_parse() {
        let fenced = "```json\n{\"title\":\"书\"}\n```";
        assert_eq!(
            parse_json_object_lenient(fenced).unwrap()["title"],
            serde_json::json!("书")
        );
        let with_prose = "结果如下：\n{\"a\":1}\n以上。";
        assert!(parse_json_object_lenient(with_prose).is_ok());
        assert!(parse_json_object_lenient("这不是 JSON").is_err());
        assert!(parse_json_object_lenient("[1,2]").is_err());
    }
}
