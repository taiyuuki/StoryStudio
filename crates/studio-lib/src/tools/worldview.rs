//! 世界观工具。
//!
//! - 基础概念 / 体系设定：存 `worldview/` 目录（concepts.toml / systems.toml）；
//! - 地理 / 势力：已统一为实体（`entities/locations/`、`entities/organizations/`），
//!   此处只做聚合展示，增删改走 create_location / create_organization / delete_entity。

use std::collections::HashSet;
use std::path::Path;

use story_ai_runtime::{Tool, ToolDef};
use story_project_format::worldview_file::{
    ConceptDef, ConceptsFile, SystemDef, SystemsFile, WorldviewSectionFile,
};
use story_project_format::EntityFile;

use super::{check_index, get_index, get_json_array_opt, get_str, get_str_opt, normalize_object_items, object_schema, ToolContext};

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
            description: "读取世界观设定，可用 section 过滤：concepts=基础概念（worldview/concepts.toml）、\
            systems=体系设定（worldview/systems.toml）、locations=地理（entities/locations/）、\
            organizations=势力（entities/organizations/）。\
            长篇后期地点/势力较多时建议按需过滤（单个地点/势力也可用 search_entities 检索、\
            get_entity_detail 查详情），避免全量输出过长被截断。"
                .into(),
            parameters: object_schema(vec![(
                "section",
                "string",
                "要读取的分节：concepts / systems / locations / organizations，\
                逗号分隔可多选（如 \"concepts,systems\"）。省略 = 全部",
                false,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let selected = parse_worldview_sections(get_str_opt(arguments, "section"))?;
        let want = |name: &str| selected.as_ref().is_none_or(|s| s.contains(name));

        let paths = self.ctx.paths();

        let mut result = String::new();

        // Concepts
        if want("concepts") {
            let concepts: ConceptsFile = load_section(&paths.worldview_concepts_toml())?;
            render_concepts(&concepts, &mut result);
        }

        // Systems
        if want("systems") {
            let systems: SystemsFile = load_section(&paths.worldview_systems_toml())?;
            render_systems(&systems, &mut result);
        }

        // 地理（Location 实体；父地点 ID 解析为名称展示）
        if want("locations") {
            let locations = load_entities(&paths.locations_dir());
            render_locations(&locations, &mut result);
        }

        // 势力（Organization 实体）
        if want("organizations") {
            let organizations = load_entities(&paths.organizations_dir());
            render_organizations(&organizations, &mut result);
        }

        if result.is_empty() {
            if selected.is_some() {
                Ok("所选分节尚未创建或为空。".to_string())
            } else {
                Ok("世界观尚未创建或为空。使用 set_worldview_item 添加概念/体系，\
                create_location / create_organization 添加地点与势力。"
                    .to_string())
            }
        } else {
            Ok(result.trim_end().to_string())
        }
    }
}

/// 解析 get_worldview 的 section 过滤参数。
///
/// 返回 `None` = 不过滤（省略 / "all" / 空串）。
/// 支持逗号多选（中英文逗号均可）与单数别名（concept → concepts 等），
/// 未知取值整体报错并列出合法值。
fn parse_worldview_sections(raw: Option<String>) -> Result<Option<HashSet<String>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut set = HashSet::new();
    for part in raw.split([',', '，', ' ', '/']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let canonical = match part.to_ascii_lowercase().as_str() {
            "concepts" | "concept" => "concepts",
            "systems" | "system" => "systems",
            "locations" | "location" | "geo" | "geography" => "locations",
            "organizations" | "organization" | "orgs" | "factions" => "organizations",
            "all" => return Ok(None),
            other => {
                return Err(format!(
                    "未知 section: {other}（支持: concepts / systems / locations / organizations，\
                    逗号分隔多选；省略 = 全部）"
                ))
            }
        };
        set.insert(canonical.to_string());
    }
    if set.is_empty() {
        return Ok(None);
    }
    Ok(Some(set))
}

fn render_concepts(concepts: &ConceptsFile, result: &mut String) {
    if !concepts.concepts.is_empty() {
        result.push_str("## 基础概念\n");
        for (i, c) in concepts.concepts.iter().enumerate() {
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
}

fn render_systems(systems: &SystemsFile, result: &mut String) {
    if !systems.systems.is_empty() {
        result.push_str("## 体系设定\n");
        for (i, s) in systems.systems.iter().enumerate() {
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
}

/// 地理（Location 实体；父地点 ID 解析为名称展示）
fn render_locations(locations: &[EntityFile], result: &mut String) {
    if !locations.is_empty() {
        let name_of = |id: &str| {
            locations
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.display_name.clone())
                .unwrap_or_else(|| id.to_string())
        };
        result.push_str("## 地理\n");
        for (i, e) in locations.iter().enumerate() {
            result.push_str(&format!("{}. {}（{}）", i + 1, e.display_name, e.id));
            if let Some(ref geo) = e.geography {
                if let Some(ref k) = geo.kind {
                    result.push_str(&format!(" [{k}]"));
                }
                if let Some(ref p) = geo.parent {
                    result.push_str(&format!(" (上级: {})", name_of(p)));
                }
            }
            result.push('\n');
            if let Some(ref geo) = e.geography {
                if let Some(ref f) = geo.features {
                    result.push_str(&format!("   特征: {f}\n"));
                }
                if let Some(ref s) = geo.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
        }
        result.push('\n');
    }
}

/// 势力（Organization 实体）
fn render_organizations(organizations: &[EntityFile], result: &mut String) {
    if !organizations.is_empty() {
        result.push_str("## 势力\n");
        for (i, e) in organizations.iter().enumerate() {
            result.push_str(&format!("{}. {}（{}）", i + 1, e.display_name, e.id));
            if let Some(ref org) = e.org {
                if let Some(ref k) = org.kind {
                    result.push_str(&format!(" [{k}]"));
                }
            }
            result.push('\n');
            if let Some(ref org) = e.org {
                if let Some(ref l) = org.leader {
                    result.push_str(&format!("   首领: {l}\n"));
                }
                if let Some(ref t) = org.territory {
                    result.push_str(&format!("   范围: {t}\n"));
                }
                if let Some(ref idg) = org.ideology {
                    result.push_str(&format!("   理念: {idg}\n"));
                }
                if let Some(ref s) = org.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
        }
        result.push('\n');
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
            description: "添加世界观条目（设计层文档）。section 指定类型：concept=基础概念, system=体系设定。\
            永远是追加新条目。地理与势力不是世界观条目——用 create_location / create_organization。\
            生成整套世界观时用 items 批量传入，一次写完。"
                .into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system（单条添加时必填）", false),
                ("name", "string", "名称（全部类型通用）", false),
                ("category", "string", "概念分类（section=concept）", false),
                ("summary", "string", "摘要（全部类型通用）", false),
                ("details", "string", "详细说明（section=concept）", false),
                ("kind", "string", "体系类型: cultivation/magic/technology/power（section=system）", false),
                ("levels", "string", "等级划分，如「炼气→筑基→金丹」（section=system）", false),
                ("rules", "string", "规则与代价（section=system）", false),
                (
                    "items",
                    "object-array",
                    "批量添加：数组，每项为原生 JSON 对象 {\"section\": \"concept|system\", \"name\": \"...\", ...各类型字段}。与 section+单条字段二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let paths = self.ctx.paths();

        let items: Vec<serde_json::Value> = match get_json_array_opt(arguments, "items")? {
            Some(arr) if arr.is_empty() => return Err("items 不能为空数组".into()),
            // 项归一化：模型常把每项序列化成字符串传入
            Some(arr) => normalize_object_items(arr, "items")?,
            // 单条：把扁平参数当作一条 item 处理（保持原语义）
            None => vec![arguments.clone()],
        };

        // 分节文件懒加载：批量混合 concept/system 时各读一次、各写一次
        let mut concepts: Option<ConceptsFile> = None;
        let mut systems: Option<SystemsFile> = None;
        let mut sections: Vec<String> = Vec::with_capacity(items.len());

        for item in &items {
            let section = get_str(item, "section")?;
            match section.as_str() {
                "concept" => {
                    if concepts.is_none() {
                        concepts = Some(load_section(&paths.worldview_concepts_toml())?);
                    }
                    let file = concepts.as_mut().unwrap();
                    file.concepts.push(ConceptDef {
                        name: get_str(item, "name")?,
                        category: get_str_opt(item, "category"),
                        summary: get_str_opt(item, "summary"),
                        details: get_str_opt(item, "details"),
                    });
                }
                "system" => {
                    if systems.is_none() {
                        systems = Some(load_section(&paths.worldview_systems_toml())?);
                    }
                    let file = systems.as_mut().unwrap();
                    file.systems.push(SystemDef {
                        name: get_str(item, "name")?,
                        kind: get_str_opt(item, "kind"),
                        levels: get_str_opt(item, "levels"),
                        rules: get_str_opt(item, "rules"),
                        summary: get_str_opt(item, "summary"),
                    });
                }
                "place" | "faction" => {
                    return Err(
                        "地理与势力已统一为实体：请用 create_location / create_organization 创建，\
                        delete_entity 删除，get_entity_detail 查看"
                            .into(),
                    )
                }
                other => {
                    return Err(format!(
                        "未知 section 类型: {other}（支持: concept, system）"
                    ))
                }
            }
            sections.push(section);
        }

        if let Some(file) = &concepts {
            save_section(&paths.worldview_concepts_toml(), file)?;
        }
        if let Some(file) = &systems {
            save_section(&paths.worldview_systems_toml(), file)?;
        }

        if items.len() == 1 {
            let file = match sections[0].as_str() {
                "concept" => "worldview/concepts.toml",
                _ => "worldview/systems.toml",
            };
            return Ok(format!(
                "世界观已更新: section={}，已写入 {file}",
                sections[0]
            ));
        }
        let concept_n = sections.iter().filter(|s| s.as_str() == "concept").count();
        let system_n = sections.iter().filter(|s| s.as_str() == "system").count();
        Ok(format!(
            "世界观已批量更新：基础概念 +{concept_n}，体系设定 +{system_n}（已写入 worldview/ 目录）"
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
            description: "修改世界观中已有的条目（基础概念/体系设定）。先 get_worldview 获取序号，\
            再用 index 指定要修改的条目，只更新提供的字段。地理与势力的修改走实体工具。"
                .into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system", true),
                ("index", "integer", "条目序号（1-based，对应 get_worldview 显示的编号）", true),
                ("name", "string", "名称（全部类型通用）", false),
                ("category", "string", "概念分类（concept）", false),
                ("summary", "string", "摘要（全部类型通用）", false),
                ("details", "string", "详细说明（concept）", false),
                ("kind", "string", "体系类型（system）", false),
                ("levels", "string", "等级划分（system）", false),
                ("rules", "string", "规则与代价（system）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        let index = get_index(arguments, "index")?;

        let paths = self.ctx.paths();
        let mut changes: Vec<&str> = Vec::new();

        match section.as_str() {
            "concept" => {
                let path = paths.worldview_concepts_toml();
                let mut file: ConceptsFile = load_section(&path)?;
                check_index("基础概念", index, file.concepts.len())?;
                let item = &mut file.concepts[index - 1];
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
                save_section(&path, &file)?;
            }
            "system" => {
                let path = paths.worldview_systems_toml();
                let mut file: SystemsFile = load_section(&path)?;
                check_index("体系设定", index, file.systems.len())?;
                let item = &mut file.systems[index - 1];
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
                save_section(&path, &file)?;
            }
            "place" | "faction" => {
                return Err(
                    "地理与势力已统一为实体：详情修改请直接编辑实体文件，或用 get_entity_detail 查看 ID 后处理"
                        .into(),
                )
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: concept, system）"
                ))
            }
        }

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
            description: "删除世界观中已有的条目（基础概念/体系设定）。先 get_worldview 获取序号，\
            再用 index 指定要删除的条目。地理与势力的删除走 delete_entity。"
                .into(),
            parameters: object_schema(vec![
                ("section", "string", "条目类型: concept / system", true),
                ("index", "integer", "条目序号（1-based，对应 get_worldview 显示的编号）", true),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let section = get_str(arguments, "section")?;
        let index = get_index(arguments, "index")?;

        let paths = self.ctx.paths();

        let (removed_name, remaining) = match section.as_str() {
            "concept" => {
                let path = paths.worldview_concepts_toml();
                let mut file: ConceptsFile = load_section(&path)?;
                check_index("基础概念", index, file.concepts.len())?;
                let removed = file.concepts.remove(index - 1);
                let remaining = file.concepts.len();
                save_section(&path, &file)?;
                (removed.name, remaining)
            }
            "system" => {
                let path = paths.worldview_systems_toml();
                let mut file: SystemsFile = load_section(&path)?;
                check_index("体系设定", index, file.systems.len())?;
                let removed = file.systems.remove(index - 1);
                let remaining = file.systems.len();
                save_section(&path, &file)?;
                (removed.name, remaining)
            }
            "place" | "faction" => {
                return Err(
                    "地理与势力已统一为实体：请用 delete_entity 删除（可先用 get_worldview 查看 ID）"
                        .into(),
                )
            }
            other => {
                return Err(format!(
                    "未知 section 类型: {other}（支持: concept, system）"
                ))
            }
        };

        Ok(format!(
            "已删除世界观条目: section={section}, index={index}「{removed_name}」，剩余 {remaining} 个"
        ))
    }
}

// ============================================================================
// helpers
// ============================================================================

/// 读取单个世界观分节文件（不存在或为空时返回 default）
fn load_section<T: WorldviewSectionFile>(path: &Path) -> Result<T, String> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    if content.trim().is_empty() {
        return Ok(T::default());
    }
    T::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))
}

/// 将分节文件写回 worldview/ 目录（自动创建目录）
fn save_section<T: WorldviewSectionFile>(path: &Path, section: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let content = section
        .to_toml_string()
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("写入失败: {e}"))
}

/// 读取实体目录下的全部实体文件（按文件名排序；损坏文件跳过）
fn load_entities(dir: &Path) -> Vec<EntityFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|c| EntityFile::from_toml_str(&c).ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;

    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    /// 批量写入世界观：一次调用混合 concept/system，逐段落盘；单条路径不受影响。
    #[test]
    fn set_worldview_item_batch_mixed_sections() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let tool = SetWorldviewItemTool::new(ToolContext::new(dir.to_path_buf()));

        let out = tool
            .execute(&serde_json::json!({
                "items": "[{\"section\": \"concept\", \"name\": \"红档\", \"summary\": \"禁忌档案\"}, {\"section\": \"concept\", \"name\": \"灰域\"}, {\"section\": \"system\", \"name\": \"灵潮\", \"kind\": \"magic\", \"levels\": \"微澜→惊涛\"}]"
            }))
            .unwrap();
        assert!(out.contains("基础概念 +2"), "{out}");
        assert!(out.contains("体系设定 +1"), "{out}");

        // 单条路径仍可用
        let out = tool
            .execute(&serde_json::json!({
                "section": "system", "name": "械力", "kind": "technology"
            }))
            .unwrap();
        assert!(out.contains("section=system"), "{out}");

        let concepts =
            std::fs::read_to_string(dir.join("worldview/concepts.toml")).unwrap();
        assert!(concepts.contains("红档"), "{concepts}");
        assert!(concepts.contains("灰域"), "{concepts}");
        let systems =
            std::fs::read_to_string(dir.join("worldview/systems.toml")).unwrap();
        assert!(systems.contains("灵潮"), "{systems}");
        assert!(systems.contains("械力"), "{systems}");
    }

    /// 模型常无视 items 的 object 声明，把每项序列化成字符串传入——
    /// 应宽松解析回对象而非报「缺少参数」
    #[test]
    fn set_worldview_item_batch_accepts_string_items() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let tool = SetWorldviewItemTool::new(ToolContext::new(dir.to_path_buf()));

        let out = tool
            .execute(&serde_json::json!({
                "items": [
                    "{\"section\": \"concept\", \"name\": \"红档\"}",
                    {"section": "concept", "name": "灰域"}
                ]
            }))
            .unwrap();
        assert!(out.contains("基础概念 +2"), "{out}");

        let concepts =
            std::fs::read_to_string(dir.join("worldview/concepts.toml")).unwrap();
        assert!(concepts.contains("红档"), "{concepts}");
        assert!(concepts.contains("灰域"), "{concepts}");
    }

    /// 批量中混入未知 section：整体报错，不落任何条目。
    #[test]
    fn set_worldview_item_batch_rejects_unknown_section_atomically() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let tool = SetWorldviewItemTool::new(ToolContext::new(dir.to_path_buf()));

        let err = tool
            .execute(&serde_json::json!({
                "items": "[{\"section\": \"concept\", \"name\": \"红档\"}, {\"section\": \"faction\", \"name\": \"雾盟\"}]"
            }))
            .unwrap_err();
        assert!(err.contains("create_organization"), "{err}");
        assert!(
            !dir.join("worldview/concepts.toml").exists(),
            "批量整体失败时不应落盘任何条目"
        );
    }

    /// section 过滤：单选 / 逗号多选 / 单数别名 / 未知值报错 / 省略全量。
    #[test]
    fn get_worldview_section_filter() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());

        // 准备：1 概念 + 1 体系 + 1 地点
        SetWorldviewItemTool::new(ctx.clone())
            .execute(&serde_json::json!({
                "items": "[{\"section\": \"concept\", \"name\": \"红档\", \"summary\": \"禁忌档案\"},\
                          {\"section\": \"system\", \"name\": \"灵潮\", \"kind\": \"magic\"}]"
            }))
            .unwrap();
        crate::tools::CreateLocationTool::new(ctx.clone())
            .execute(&serde_json::json!({"name": "青云城"}))
            .unwrap();

        let tool = GetWorldviewTool::new(ctx);
        // 省略 = 全部（默认行为不变）
        let all = tool.execute(&serde_json::json!({})).unwrap();
        assert!(all.contains("## 基础概念"), "{all}");
        assert!(all.contains("## 体系设定"), "{all}");
        assert!(all.contains("## 地理"), "{all}");

        // 单选：只概念
        let out = tool
            .execute(&serde_json::json!({"section": "concepts"}))
            .unwrap();
        assert!(out.contains("## 基础概念"), "{out}");
        assert!(!out.contains("## 体系设定"), "{out}");
        assert!(!out.contains("## 地理"), "{out}");

        // 逗号多选 + 单数别名
        let out = tool
            .execute(&serde_json::json!({"section": "concept,systems"}))
            .unwrap();
        assert!(out.contains("## 基础概念"), "{out}");
        assert!(out.contains("## 体系设定"), "{out}");
        assert!(!out.contains("## 地理"), "{out}");

        // unknown → 明确报错
        let err = tool
            .execute(&serde_json::json!({"section": "characters"}))
            .unwrap_err();
        assert!(err.contains("未知 section"), "{err}");

        // "all" 等价省略
        let out = tool.execute(&serde_json::json!({"section": "all"})).unwrap();
        assert!(out.contains("## 地理"), "{out}");
    }
}
