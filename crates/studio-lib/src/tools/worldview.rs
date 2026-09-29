//! 世界观工具：世界观（worldview.toml）的读取与条目增删改。

use std::path::{Path, PathBuf};

use story_ai_runtime::{Tool, ToolDef};
use story_project_format::worldview_file::{ConceptDef, FactionDef, PlaceDef, SystemDef};
use story_project_format::WorldviewFile;

use super::{check_index, get_str, get_str_opt, object_schema, ToolContext};

// ============================================================================
// get_worldview
// ============================================================================

pub struct GetWorldviewTool {
    ctx: ToolContext,
}
impl GetWorldviewTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetWorldviewTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_worldview".into(),
            description: "读取世界观设定（worldview.toml）：基础概念、体系设定、地理秘境、势力。"
                .into(),
            parameters: object_schema(vec![]),
        }
    }

    fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        let wv_path = self.ctx.paths().worldview_toml();

        if !wv_path.exists() {
            return Ok(
                "世界观文件 (worldview.toml) 尚未创建。使用 set_worldview_item 创建。".to_string(),
            );
        }

        let content = std::fs::read_to_string(&wv_path).map_err(|e| format!("读取失败: {e}"))?;

        if content.trim().is_empty() {
            return Ok("世界观文件为空。使用 set_worldview_item 添加内容。".to_string());
        }

        let wv = WorldviewFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;

        let mut result = String::new();

        // Concepts
        if !wv.concepts.is_empty() {
            result.push_str("## 基础概念\n");
            for (i, c) in wv.concepts.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, c.name));
                if let Some(ref k) = c.category {
                    result.push_str(&format!(" [{k}]"));
                }
                result.push('\n');
                if let Some(ref s) = c.summary {
                    result.push_str(&format!("   {s}\n"));
                }
                if let Some(ref d) = c.details {
                    result.push_str(&format!("   详情: {d}\n"));
                }
            }
            result.push('\n');
        }

        // Systems
        if !wv.systems.is_empty() {
            result.push_str("## 体系设定\n");
            for (i, s) in wv.systems.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, s.name));
                if let Some(ref k) = s.kind {
                    result.push_str(&format!(" [{k}]"));
                }
                result.push('\n');
                if let Some(ref l) = s.levels {
                    result.push_str(&format!("   等级: {l}\n"));
                }
                if let Some(ref r) = s.rules {
                    result.push_str(&format!("   规则: {r}\n"));
                }
                if let Some(ref sm) = s.summary {
                    result.push_str(&format!("   {sm}\n"));
                }
            }
            result.push('\n');
        }

        // Places
        if !wv.places.is_empty() {
            result.push_str("## 地理秘境\n");
            for (i, p) in wv.places.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, p.name));
                if let Some(ref k) = p.kind {
                    result.push_str(&format!(" [{k}]"));
                }
                if let Some(ref par) = p.parent {
                    result.push_str(&format!(" (上级: {par})"));
                }
                result.push('\n');
                if let Some(ref f) = p.features {
                    result.push_str(&format!("   特征: {f}\n"));
                }
                if let Some(ref s) = p.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
            result.push('\n');
        }

        // Factions
        if !wv.factions.is_empty() {
            result.push_str("## 势力\n");
            for (i, f) in wv.factions.iter().enumerate() {
                result.push_str(&format!("{}. {}", i + 1, f.name));
                if let Some(ref k) = f.kind {
                    result.push_str(&format!(" [{k}]"));
                }
                result.push('\n');
                if let Some(ref l) = f.leader {
                    result.push_str(&format!("   首领: {l}\n"));
                }
                if let Some(ref t) = f.territory {
                    result.push_str(&format!("   范围: {t}\n"));
                }
                if let Some(ref id) = f.ideology {
                    result.push_str(&format!("   理念: {id}\n"));
                }
                if let Some(ref s) = f.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
            result.push('\n');
        }

        if result.is_empty() {
            Ok("世界观文件存在但所有段落为空。".to_string())
        } else {
            Ok(result.trim_end().to_string())
        }
    }
}

// ============================================================================
// set_worldview_item
// ============================================================================

pub struct SetWorldviewItemTool {
    ctx: ToolContext,
}
impl SetWorldviewItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SetWorldviewItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "set_worldview_item".into(),
            description: "添加世界观条目。通过 section 参数指定类型：concept=基础概念, system=体系设定, place=地理秘境, faction=势力。永远是追加新条目。".into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system / place / faction", true),
                ("name", "string", "名称（全部类型通用）", true),
                ("category", "string", "概念分类（section=concept）", false),
                ("summary", "string", "摘要（全部类型通用）", false),
                ("details", "string", "详细说明（section=concept）", false),
                ("kind", "string", "类型: system 用 cultivation/magic/technology/power; place 用 continent/nation/city/realm/landform; faction 用 sect/nation/clan/guild/organization", false),
                ("levels", "string", "等级划分，如「炼气→筑基→金丹」（section=system）", false),
                ("rules", "string", "规则与代价（section=system）", false),
                ("parent", "string", "上级区域（section=place）", false),
                ("features", "string", "特征描述（section=place）", false),
                ("leader", "string", "首领/代表人物（section=faction）", false),
                ("territory", "string", "势力范围（section=faction）", false),
                ("ideology", "string", "理念/宗旨（section=faction）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        let wv_path = self.ctx.paths().worldview_toml();

        let mut wv = if wv_path.exists() {
            let content =
                std::fs::read_to_string(&wv_path).map_err(|e| format!("读取失败: {e}"))?;
            if content.trim().is_empty() {
                WorldviewFile::default()
            } else {
                WorldviewFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?
            }
        } else {
            WorldviewFile::default()
        };

        match section.as_str() {
            "concept" => {
                let name = get_str(arguments, "name")?;
                let category = get_str_opt(arguments, "category");
                let summary = get_str_opt(arguments, "summary");
                let details = get_str_opt(arguments, "details");
                wv.concepts.push(ConceptDef {
                    name,
                    category,
                    summary,
                    details,
                });
            }
            "system" => {
                let name = get_str(arguments, "name")?;
                let kind = get_str_opt(arguments, "kind");
                let levels = get_str_opt(arguments, "levels");
                let rules = get_str_opt(arguments, "rules");
                let summary = get_str_opt(arguments, "summary");
                wv.systems.push(SystemDef {
                    name,
                    kind,
                    levels,
                    rules,
                    summary,
                });
            }
            "place" => {
                let name = get_str(arguments, "name")?;
                let kind = get_str_opt(arguments, "kind");
                let parent = get_str_opt(arguments, "parent");
                let features = get_str_opt(arguments, "features");
                let summary = get_str_opt(arguments, "summary");
                wv.places.push(PlaceDef {
                    name,
                    kind,
                    parent,
                    features,
                    summary,
                });
            }
            "faction" => {
                let name = get_str(arguments, "name")?;
                let kind = get_str_opt(arguments, "kind");
                let leader = get_str_opt(arguments, "leader");
                let territory = get_str_opt(arguments, "territory");
                let ideology = get_str_opt(arguments, "ideology");
                let summary = get_str_opt(arguments, "summary");
                wv.factions.push(FactionDef {
                    name,
                    kind,
                    leader,
                    territory,
                    ideology,
                    summary,
                });
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: concept, system, place, faction）"
                ))
            }
        }

        let toml_content = wv
            .to_toml_string()
            .map_err(|e| format!("序列化失败: {e}"))?;
        std::fs::write(&wv_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;

        Ok(format!(
            "世界观已更新: section={section}, 已写入 worldview.toml"
        ))
    }
}

// ============================================================================
// update_worldview_item
// ============================================================================

pub struct UpdateWorldviewItemTool {
    ctx: ToolContext,
}
impl UpdateWorldviewItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateWorldviewItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_worldview_item".into(),
            description: "修改世界观中已有的条目（基础概念/体系设定/地理秘境/势力）。先 get_worldview 获取序号，再用 index 指定要修改的条目，只更新提供的字段。".into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system / place / faction", true),
                ("index", "string", "条目序号（1-based，对应 get_worldview 显示的编号）", true),
                ("name", "string", "名称（全部类型通用）", false),
                ("category", "string", "概念分类（concept）", false),
                ("summary", "string", "摘要（全部类型通用）", false),
                ("details", "string", "详细说明（concept）", false),
                ("kind", "string", "类型（system/place/faction）", false),
                ("levels", "string", "等级划分（system）", false),
                ("rules", "string", "规则与代价（system）", false),
                ("parent", "string", "上级区域（place）", false),
                ("features", "string", "特征描述（place）", false),
                ("leader", "string", "首领/代表人物（faction）", false),
                ("territory", "string", "势力范围（faction）", false),
                ("ideology", "string", "理念/宗旨（faction）", false),
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

        let (mut wv, wv_path) = load_worldview(&self.ctx)?;
        let mut changes: Vec<&str> = Vec::new();

        match section.as_str() {
            "concept" => {
                check_index("基础概念", index, wv.concepts.len())?;
                let item = &mut wv.concepts[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "category") {
                    item.category = Some(v);
                    changes.push("category");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
                if let Some(v) = get_str_opt(arguments, "details") {
                    item.details = Some(v);
                    changes.push("details");
                }
            }
            "system" => {
                check_index("体系设定", index, wv.systems.len())?;
                let item = &mut wv.systems[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "kind") {
                    item.kind = Some(v);
                    changes.push("kind");
                }
                if let Some(v) = get_str_opt(arguments, "levels") {
                    item.levels = Some(v);
                    changes.push("levels");
                }
                if let Some(v) = get_str_opt(arguments, "rules") {
                    item.rules = Some(v);
                    changes.push("rules");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
            }
            "place" => {
                check_index("地理秘境", index, wv.places.len())?;
                let item = &mut wv.places[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "kind") {
                    item.kind = Some(v);
                    changes.push("kind");
                }
                if let Some(v) = get_str_opt(arguments, "parent") {
                    item.parent = Some(v);
                    changes.push("parent");
                }
                if let Some(v) = get_str_opt(arguments, "features") {
                    item.features = Some(v);
                    changes.push("features");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
            }
            "faction" => {
                check_index("势力", index, wv.factions.len())?;
                let item = &mut wv.factions[index - 1];
                if let Some(v) = get_str_opt(arguments, "name") {
                    item.name = v;
                    changes.push("name");
                }
                if let Some(v) = get_str_opt(arguments, "kind") {
                    item.kind = Some(v);
                    changes.push("kind");
                }
                if let Some(v) = get_str_opt(arguments, "leader") {
                    item.leader = Some(v);
                    changes.push("leader");
                }
                if let Some(v) = get_str_opt(arguments, "territory") {
                    item.territory = Some(v);
                    changes.push("territory");
                }
                if let Some(v) = get_str_opt(arguments, "ideology") {
                    item.ideology = Some(v);
                    changes.push("ideology");
                }
                if let Some(v) = get_str_opt(arguments, "summary") {
                    item.summary = Some(v);
                    changes.push("summary");
                }
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: concept, system, place, faction）"
                ))
            }
        }

        save_worldview(&wv_path, &wv)?;

        Ok(format!(
            "世界观条目已更新: section={section}, index={index}，修改字段: {}",
            changes.join(", ")
        ))
    }
}

// ============================================================================
// delete_worldview_item
// ============================================================================

pub struct DeleteWorldviewItemTool {
    ctx: ToolContext,
}
impl DeleteWorldviewItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteWorldviewItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_worldview_item".into(),
            description: "删除世界观中已有的条目（基础概念/体系设定/地理秘境/势力）。先 get_worldview 获取序号，再用 index 指定要删除的条目。".into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system / place / faction", true),
                ("index", "string", "条目序号（1-based，对应 get_worldview 显示的编号）", true),
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

        let (mut wv, wv_path) = load_worldview(&self.ctx)?;

        let (removed_name, remaining) = match section.as_str() {
            "concept" => {
                check_index("基础概念", index, wv.concepts.len())?;
                let removed = wv.concepts.remove(index - 1);
                (removed.name, wv.concepts.len())
            }
            "system" => {
                check_index("体系设定", index, wv.systems.len())?;
                let removed = wv.systems.remove(index - 1);
                (removed.name, wv.systems.len())
            }
            "place" => {
                check_index("地理秘境", index, wv.places.len())?;
                let removed = wv.places.remove(index - 1);
                (removed.name, wv.places.len())
            }
            "faction" => {
                check_index("势力", index, wv.factions.len())?;
                let removed = wv.factions.remove(index - 1);
                (removed.name, wv.factions.len())
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: concept, system, place, faction）"
                ))
            }
        };

        save_worldview(&wv_path, &wv)?;

        Ok(format!(
            "已删除世界观条目: section={section}, index={index}「{removed_name}」，剩余 {remaining} 个"
        ))
    }
}

// ============================================================================
// worldview helpers
// ============================================================================

/// 读取 worldview.toml，返回 (WorldviewFile, path)。文件不存在或为空时返回 default。
fn load_worldview(ctx: &ToolContext) -> Result<(WorldviewFile, PathBuf), String> {
    let wv_path = ctx.paths().worldview_toml();
    let wv = if wv_path.exists() {
        let content = std::fs::read_to_string(&wv_path).map_err(|e| format!("读取失败: {e}"))?;
        if content.trim().is_empty() {
            WorldviewFile::default()
        } else {
            WorldviewFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?
        }
    } else {
        WorldviewFile::default()
    };
    Ok((wv, wv_path))
}

/// 将 WorldviewFile 写回 worldview.toml
fn save_worldview(path: &Path, wv: &WorldviewFile) -> Result<(), String> {
    let toml_content = wv
        .to_toml_string()
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(path, toml_content).map_err(|e| format!("写入失败: {e}"))
}
