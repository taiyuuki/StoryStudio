//! 大纲工具：实体更新、大纲（outline.toml）的读写与条目增删改。

use std::path::{Path, PathBuf};

use story_ai_runtime::{Tool, ToolDef};
use story_project_format::outline_file::{ArcDef, ConflictDef, StorylineDef, VolumeDef};
use story_project_format::{EntityFile, OutlineFile};

use super::{check_index, get_str, get_str_opt, object_schema, ToolContext};

// ============================================================================
// update_entity
// ============================================================================

pub struct UpdateEntityTool {
    ctx: ToolContext,
}
impl UpdateEntityTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateEntityTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_entity".into(),
            description: "更新已有实体的属性。根据实体 ID 前缀自动识别类型（c_=角色 / l_=地点 / i_=道具 / o_=组织），只更新提供的字段。".into(),
            parameters: object_schema(vec![
                ("entity_id", "string", "实体 ID", true),
                ("display_name", "string", "新显示名称", false),
                ("aliases", "string", "别名，逗号分隔", false),
                ("gender", "string", "性别（角色）", false),
                ("occupation", "string", "职业（角色）", false),
                ("birth_year", "string", "出生年份（角色，整数）", false),
                ("appearance", "string", "外貌描述（角色）", false),
                ("background", "string", "背景故事（角色）", false),
                ("personality", "string", "性格描述（角色，多行自由文本）", false),
                ("habits", "string", "习性：口头禅、惯用手、小动作等（角色，多行自由文本）", false),
                ("race", "string", "种族（角色）", false),
                ("parent", "string", "父地点 ID（地点）", false),
                ("geo_kind", "string", "地理类型（地点，如 settlement/city）", false),
                ("position", "string", "地理位置描述（地点）", false),
                ("item_kind", "string", "道具类型（道具，如 weapon/potion）", false),
                ("grade", "string", "品级（道具，如 legendary/common）", false),
                ("item_description", "string", "道具描述", false),
                ("abilities", "string", "能力/效果描述（道具）", false),
                ("unique", "boolean", "是否为唯一道具/神器（传 true/false）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;

        let entity_dir = match entity_id.chars().next() {
            Some('c') => self.ctx.paths().characters_dir(),
            Some('l') => self.ctx.paths().locations_dir(),
            Some('i') => self.ctx.paths().items_dir(),
            Some('o') => self.ctx.paths().organizations_dir(),
            _ => return Err(format!("无法识别实体 ID 前缀: {entity_id}")),
        };

        let entity_path = entity_dir.join(format!("{entity_id}.toml"));
        if !entity_path.exists() {
            return Err(format!("实体 '{entity_id}' 不存在"));
        }

        let content =
            std::fs::read_to_string(&entity_path).map_err(|e| format!("读取实体文件失败: {e}"))?;
        let mut entity =
            EntityFile::from_toml_str(&content).map_err(|e| format!("解析 TOML 失败: {e}"))?;

        let mut changes: Vec<&str> = Vec::new();

        if let Some(v) = get_str_opt(arguments, "display_name") {
            entity.display_name = v;
            changes.push("display_name");
        }
        if let Some(v) = get_str_opt(arguments, "aliases") {
            entity.aliases = v.split(',').map(|s| s.trim().to_string()).collect();
            changes.push("aliases");
        }

        if entity_id.starts_with('c') {
            let profile = entity.profile.get_or_insert_with(Default::default);
            if let Some(v) = get_str_opt(arguments, "gender") {
                profile.gender = Some(v);
                changes.push("gender");
            }
            if let Some(v) = get_str_opt(arguments, "occupation") {
                profile.occupation = Some(v);
                changes.push("occupation");
            }
            if let Some(v) = get_str_opt(arguments, "birth_year") {
                if let Ok(y) = v.parse::<i32>() {
                    profile.birth_year = Some(y);
                    changes.push("birth_year");
                }
            }
            if let Some(v) = get_str_opt(arguments, "appearance") {
                profile.appearance = Some(v);
                changes.push("appearance");
            }
            if let Some(v) = get_str_opt(arguments, "background") {
                profile.background = Some(v);
                changes.push("background");
            }
            if let Some(v) = get_str_opt(arguments, "race") {
                profile.race = Some(v);
                changes.push("race");
            }
            if let Some(v) = get_str_opt(arguments, "personality") {
                profile.personality = Some(v);
                changes.push("personality");
            }
            if let Some(v) = get_str_opt(arguments, "habits") {
                profile.habits = Some(v);
                changes.push("habits");
            }
        }

        if entity_id.starts_with('l') {
            let geo = entity.geography.get_or_insert_with(Default::default);
            if let Some(v) = get_str_opt(arguments, "parent") {
                geo.parent = Some(v);
                changes.push("parent");
            }
            if let Some(v) = get_str_opt(arguments, "geo_kind") {
                geo.kind = Some(v);
                changes.push("kind");
            }
            if let Some(v) = get_str_opt(arguments, "position") {
                geo.position = Some(v);
                changes.push("position");
            }
        }

        if entity_id.starts_with('i') {
            let item = entity.item.get_or_insert_with(Default::default);
            if let Some(v) = get_str_opt(arguments, "item_kind") {
                item.kind = Some(v);
                changes.push("kind");
            }
            if let Some(v) = get_str_opt(arguments, "grade") {
                item.grade = Some(v);
                changes.push("grade");
            }
            if let Some(v) = get_str_opt(arguments, "item_description") {
                item.description = Some(v);
                changes.push("description");
            }
            if let Some(v) = get_str_opt(arguments, "abilities") {
                item.abilities = Some(v);
                changes.push("abilities");
            }
            if let Some(v) = arguments.get("unique").and_then(|v| v.as_bool()) {
                item.unique = v;
                changes.push("unique");
            }
        }

        if changes.is_empty() {
            return Ok("未提供任何需要更新的字段。".to_string());
        }

        let toml_content = entity
            .to_toml_string()
            .map_err(|e| format!("序列化 TOML 失败: {e}"))?;
        std::fs::write(&entity_path, toml_content).map_err(|e| format!("写入实体文件失败: {e}"))?;

        Ok(format!("实体 '{entity_id}' 已更新: {}", changes.join(", ")))
    }
}

// ============================================================================
// get_outline
// ============================================================================

pub struct GetOutlineTool {
    ctx: ToolContext,
}
impl GetOutlineTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetOutlineTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_outline".into(),
            description: "读取故事大纲（outline.toml）：故事梗概、卷结构、角色弧线、故事线。"
                .into(),
            parameters: object_schema(vec![]),
        }
    }

    fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        let outline_path = self.ctx.paths().outline_toml();

        if !outline_path.exists() {
            return Ok(
                "大纲文件 (outline.toml) 尚未创建。使用 set_outline_item 创建。".to_string(),
            );
        }

        let content =
            std::fs::read_to_string(&outline_path).map_err(|e| format!("读取失败: {e}"))?;

        if content.trim().is_empty() {
            return Ok("大纲文件为空。使用 set_outline_item 添加内容。".to_string());
        }

        let outline = OutlineFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;

        let mut result = String::new();

        // Story meta
        let story = &outline.story;
        if story.title.is_some()
            || story.genre.is_some()
            || story.premise.is_some()
            || story.setting.is_some()
            || story.time_span.is_some()
            || story.audience.is_some()
            || story.target_words.is_some()
        {
            result.push_str("## 故事概要\n");
            if let Some(ref v) = story.title {
                result.push_str(&format!("标题: {v}\n"));
            }
            if let Some(ref v) = story.genre {
                result.push_str(&format!("类型: {v}\n"));
            }
            if let Some(ref v) = story.premise {
                result.push_str(&format!("前提: {v}\n"));
            }
            if let Some(ref v) = story.theme {
                result.push_str(&format!("主题: {v}\n"));
            }
            if let Some(ref v) = story.setting {
                result.push_str(&format!("背景设定: {v}\n"));
            }
            if let Some(ref v) = story.time_span {
                result.push_str(&format!("时间跨度: {v}\n"));
            }
            if let Some(ref v) = story.audience {
                result.push_str(&format!("目标读者: {v}\n"));
            }
            if let Some(v) = story.target_words {
                result.push_str(&format!("目标字数: {v}\n"));
            }
            result.push('\n');
        }

        // Volumes
        if !outline.volumes.is_empty() {
            result.push_str("## 卷结构\n");
            for (i, vol) in outline.volumes.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, vol.name));
                if let Some(ref t) = vol.theme {
                    result.push_str(&format!(" [主题: {t}]"));
                }
                result.push('\n');
                if let Some(ref s) = vol.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
            result.push('\n');
        }

        // Character arcs
        if !outline.arcs.is_empty() {
            result.push_str("## 角色弧线\n");
            for (i, arc) in outline.arcs.iter().enumerate() {
                result.push_str(&format!("{}. {} ({})", i + 1, arc.name, arc.character));
                if let Some(ref s) = arc.stages {
                    result.push_str(&format!(" [阶段: {s}]"));
                }
                if let Some(ref s) = arc.summary {
                    result.push_str(&format!(" — {s}"));
                }
                result.push('\n');
            }
            result.push('\n');
        }

        // Storylines
        if !outline.storylines.is_empty() {
            result.push_str("## 故事线\n");
            for (i, sl) in outline.storylines.iter().enumerate() {
                let thread_info = sl.thread.as_deref().unwrap_or("无线索");
                result.push_str(&format!("{}. {} ({})", i + 1, sl.name, thread_info));
                if let Some(ref s) = sl.summary {
                    result.push_str(&format!(" — {s}"));
                }
                result.push('\n');
            }
            result.push('\n');
        }

        // Conflicts
        if !outline.conflicts.is_empty() {
            result.push_str("## 冲突关系\n");
            for (i, c) in outline.conflicts.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, c.name));
                if let Some(ref k) = c.kind {
                    result.push_str(&format!(" [{k}]"));
                }
                if let Some(ref p) = c.parties {
                    result.push_str(&format!(": {p}"));
                }
                if let Some(ref s) = c.summary {
                    result.push_str(&format!(" — {s}"));
                }
                if let Some(ref cl) = c.climax {
                    result.push_str(&format!(" (高潮: {cl})"));
                }
                result.push('\n');
            }
            result.push('\n');
        }

        if result.is_empty() {
            Ok("大纲文件存在但所有段落为空。".to_string())
        } else {
            Ok(result.trim_end().to_string())
        }
    }
}

// ============================================================================
// set_outline_item
// ============================================================================

pub struct SetOutlineItemTool {
    ctx: ToolContext,
}
impl SetOutlineItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SetOutlineItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "set_outline_item".into(),
            description: "设置/添加大纲内容。通过 section 参数指定操作类型：story=故事元信息, volume=添加卷, arc=添加角色弧线, storyline=添加故事线, conflict=添加冲突关系。".into(),
            parameters: object_schema(vec![
                ("section", "string", "操作类型: story / volume / arc / storyline / conflict", true),
                ("title", "string", "故事标题（section=story）", false),
                ("genre", "string", "类型（section=story）", false),
                ("premise", "string", "前提设定（section=story）", false),
                ("theme", "string", "主题（section=story 或 volume）", false),
                ("setting", "string", "背景设定（section=story）", false),
                ("time_span", "string", "时间跨度（section=story）", false),
                ("audience", "string", "目标读者（section=story）", false),
                ("target_words", "string", "目标字数（section=story，如 800000 表示约 80 万字）", false),
                ("name", "string", "名称（section=volume/arc/storyline/conflict）", false),
                ("summary", "string", "摘要（section=volume/arc/storyline/conflict）", false),
                ("character", "string", "角色 ID（section=arc）", false),
                ("stages", "string", "弧线阶段描述（section=arc）", false),
                ("thread", "string", "关联的线索 ID（section=storyline，可选——故事线不强制绑定线索）", false),
                ("kind", "string", "冲突类型: character/faction/internal/cosmic（section=conflict）", false),
                ("parties", "string", "冲突双方描述（section=conflict）", false),
                ("climax", "string", "冲突高潮所在的卷（section=conflict）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        let outline_path = self.ctx.paths().outline_toml();

        let mut outline = if outline_path.exists() {
            let content =
                std::fs::read_to_string(&outline_path).map_err(|e| format!("读取失败: {e}"))?;
            if content.trim().is_empty() {
                OutlineFile::default()
            } else {
                OutlineFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?
            }
        } else {
            OutlineFile::default()
        };

        match section.as_str() {
            "story" => {
                if let Some(v) = get_str_opt(arguments, "title") {
                    outline.story.title = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "genre") {
                    outline.story.genre = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "premise") {
                    outline.story.premise = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "theme") {
                    outline.story.theme = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "setting") {
                    outline.story.setting = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "time_span") {
                    outline.story.time_span = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "audience") {
                    outline.story.audience = Some(v);
                }
                if let Some(v) = get_str_opt(arguments, "target_words") {
                    let n: u64 = v
                        .parse()
                        .map_err(|_| format!("target_words 必须是正整数，收到 '{v}'"))?;
                    outline.story.target_words = Some(n);
                }
            }
            "volume" => {
                let name = get_str(arguments, "name")?;
                let summary = get_str_opt(arguments, "summary");
                let theme = get_str_opt(arguments, "theme");

                outline.volumes.push(VolumeDef {
                    name,
                    summary,
                    theme,
                });
            }
            "arc" => {
                let character = get_str(arguments, "character")?;
                let name = get_str(arguments, "name")?;
                let stages = get_str_opt(arguments, "stages");
                let summary = get_str_opt(arguments, "summary");
                outline.arcs.push(ArcDef {
                    character,
                    name,
                    stages,
                    summary,
                });
            }
            "storyline" => {
                let thread = get_str_opt(arguments, "thread");
                let name = get_str(arguments, "name")?;
                let summary = get_str_opt(arguments, "summary");
                outline.storylines.push(StorylineDef {
                    thread,
                    name,
                    summary,
                });
            }
            "conflict" => {
                let name = get_str(arguments, "name")?;
                let kind = get_str_opt(arguments, "kind");
                let parties = get_str_opt(arguments, "parties");
                let summary = get_str_opt(arguments, "summary");
                let climax = get_str_opt(arguments, "climax");
                outline.conflicts.push(ConflictDef {
                    name,
                    kind,
                    parties,
                    summary,
                    climax,
                });
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: story, volume, arc, storyline, conflict）"
                ))
            }
        }

        let toml_content = outline
            .to_toml_string()
            .map_err(|e| format!("序列化失败: {e}"))?;
        std::fs::write(&outline_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;

        Ok(format!(
            "大纲已更新: section={section}, 已写入 outline.toml"
        ))
    }
}

// ============================================================================
// update_outline_item
// ============================================================================

pub struct UpdateOutlineItemTool {
    ctx: ToolContext,
}
impl UpdateOutlineItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateOutlineItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_outline_item".into(),
            description: "修改大纲中已有的条目（卷/弧线/故事线/冲突）。先 get_outline 获取序号，再用 index 指定要修改的条目，只更新提供的字段。".into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: volume / arc / storyline / conflict（不支持 story）", true),
                ("index", "string", "条目序号（1-based，对应 get_outline 显示的编号）", true),
                ("name", "string", "名称（全部类型通用）", false),
                ("summary", "string", "摘要（volume/arc/storyline/conflict）", false),
                ("theme", "string", "主题（volume）", false),
                ("character", "string", "角色 ID（arc）", false),
                ("stages", "string", "弧线阶段描述（arc）", false),
                ("thread", "string", "线索 ID（storyline）", false),
                ("kind", "string", "冲突类型: character/faction/internal/cosmic（conflict）", false),
                ("parties", "string", "冲突双方描述（conflict）", false),
                ("climax", "string", "冲突高潮所在的卷（conflict）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        if section == "story" {
            return Err(
                "story 是单值段，请使用 set_outline_item(section=story) 修改故事元信息".into(),
            );
        }
        let index: usize = get_str(arguments, "index")?
            .parse()
            .map_err(|_| "index 必须是正整数（1-based）".to_string())?;
        if index == 0 {
            return Err("index 从 1 开始计数".into());
        }

        let (mut outline, outline_path) = load_outline(&self.ctx)?;

        let mut changes: Vec<&str> = Vec::new();

        match section.as_str() {
            "volume" => {
                check_index("卷", index, outline.volumes.len())?;
                let item = &mut outline.volumes[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
                if let Some(v) = get_str_opt(arguments, "theme") {
                    item.theme = Some(v);
                    changes.push("theme");
                }
            }
            "arc" => {
                check_index("弧线", index, outline.arcs.len())?;
                let item = &mut outline.arcs[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "character") {
                    item.character = v;
                    changes.push("character");
                }
                if let Some(v) = get_str_opt(arguments, "stages") {
                    item.stages = Some(v);
                    changes.push("stages");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
            }
            "storyline" => {
                check_index("故事线", index, outline.storylines.len())?;
                let item = &mut outline.storylines[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "thread") {
                    item.thread = Some(v);
                    changes.push("thread");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
            }
            "conflict" => {
                check_index("冲突", index, outline.conflicts.len())?;
                let item = &mut outline.conflicts[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "kind") {
                    item.kind = Some(v);
                    changes.push("kind");
                }
                if let Some(v) = get_str_opt(arguments, "parties") {
                    item.parties = Some(v);
                    changes.push("parties");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
                if let Some(v) = get_str_opt(arguments, "climax") {
                    item.climax = Some(v);
                    changes.push("climax");
                }
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: volume, arc, storyline, conflict）"
                ))
            }
        }

        if changes.is_empty() {
            return Ok("未提供任何需要更新的字段。".to_string());
        }

        save_outline(&outline_path, &outline)?;

        Ok(format!(
            "大纲已修改: section={section}, index={index}, 更新字段: {}",
            changes.join(", ")
        ))
    }
}

// ============================================================================
// delete_outline_item
// ============================================================================

pub struct DeleteOutlineItemTool {
    ctx: ToolContext,
}
impl DeleteOutlineItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteOutlineItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_outline_item".into(),
            description: "删除大纲中已有的条目（卷/弧线/故事线/冲突）。先 get_outline 获取序号，再用 index 指定要删除的条目。".into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: volume / arc / storyline / conflict", true),
                ("index", "string", "条目序号（1-based，对应 get_outline 显示的编号）", true),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        let index: usize = get_str(arguments, "index")?
            .parse()
            .map_err(|_| "index 必须是正整数（1-based）".to_string())?;
        if index == 0 {
            return Err("index 从 1 开始计数".into());
        }

        let (mut outline, outline_path) = load_outline(&self.ctx)?;

        let (removed_name, remaining) = match section.as_str() {
            "volume" => {
                check_index("卷", index, outline.volumes.len())?;
                let removed = outline.volumes.remove(index - 1);
                (removed.name, outline.volumes.len())
            }
            "arc" => {
                check_index("弧线", index, outline.arcs.len())?;
                let removed = outline.arcs.remove(index - 1);
                (removed.name, outline.arcs.len())
            }
            "storyline" => {
                check_index("故事线", index, outline.storylines.len())?;
                let removed = outline.storylines.remove(index - 1);
                (removed.name, outline.storylines.len())
            }
            "conflict" => {
                check_index("冲突", index, outline.conflicts.len())?;
                let removed = outline.conflicts.remove(index - 1);
                (removed.name, outline.conflicts.len())
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: volume, arc, storyline, conflict）"
                ))
            }
        };

        save_outline(&outline_path, &outline)?;

        Ok(format!(
            "已删除大纲条目: section={section}, index={index}「{removed_name}」，剩余 {remaining} 个"
        ))
    }
}

// ============================================================================
// outline helpers
// ============================================================================

/// 读取 outline.toml，返回 (OutlineFile, path)。文件不存在或为空时返回 default。
fn load_outline(ctx: &ToolContext) -> Result<(OutlineFile, PathBuf), String> {
    let outline_path = ctx.paths().outline_toml();
    let outline = if outline_path.exists() {
        let content =
            std::fs::read_to_string(&outline_path).map_err(|e| format!("读取失败: {e}"))?;
        if content.trim().is_empty() {
            OutlineFile::default()
        } else {
            OutlineFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?
        }
    } else {
        OutlineFile::default()
    };
    Ok((outline, outline_path))
}

/// 将 OutlineFile 写回 outline.toml
fn save_outline(path: &Path, outline: &OutlineFile) -> Result<(), String> {
    let toml_content = outline
        .to_toml_string()
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(path, toml_content).map_err(|e| format!("写入失败: {e}"))
}
