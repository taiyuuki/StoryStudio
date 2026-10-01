//! 实体相关工具：创建/搜索/查询 角色、地点、道具、组织。

use story_ai_runtime::{Tool, ToolDef};
use story_ir::{SemanticGraph, StableId};
use story_project_format::EntityFile;

use crate::mutations::EntityKind;

use super::{format_brief, get_str, get_str_opt, object_schema, ToolContext};

/// 同名检测：创建实体后检查同名实体。
///
/// 同类型内重名才是冲突（Entity Linking 无法区分，编译器 duplicate_display_name
/// 告警，正文提及被丢弃）→ 返回警告与改错建议。跨类型同名（如「星辰学院」
/// 既是地点也是机构）是合法的「双面实体」，Linking 会各记一条提及 → 只给
/// 一句说明，不打警告。
fn duplicate_name_warning(
    paths: &story_project_format::ProjectPaths,
    kind: EntityKind,
    new_id: &str,
    name: &str,
) -> String {
    let same_names = |dir: &std::path::Path| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|entry| {
                let fname = entry.file_name().to_string_lossy().to_string();
                if !fname.ends_with(".toml") {
                    return None;
                }
                let id = fname.trim_end_matches(".toml").to_string();
                if id == new_id {
                    return None;
                }
                let content = std::fs::read_to_string(entry.path()).ok()?;
                let entity = EntityFile::from_toml_str(&content).ok()?;
                (entity.display_name == name).then_some(id)
            })
            .collect()
    };

    let same_kind = same_names(&kind.dir(paths));
    if !same_kind.is_empty() {
        return format!(
            " ⚠ 注意：同类型已存在同名实体 {}，正文自动出场识别将无法区分（duplicate_display_name 告警，提及被丢弃）。\
            建议为其中一方改名或补充别名区分。",
            same_kind.join(", ")
        );
    }

    // 跨类型同名：合法的双面实体，说明一句即可
    for other in [EntityKind::Character, EntityKind::Location, EntityKind::Item, EntityKind::Organization] {
        if other == kind {
            continue;
        }
        let ids = same_names(&other.dir(paths));
        if !ids.is_empty() {
            return format!(
                "（同名{}实体 {} 已存在：按「双面实体」处理，正文提及会同时计入两者。）",
                other.type_name_zh(),
                ids.join(", ")
            );
        }
    }
    String::new()
}

// ============================================================================
// create_character
// ============================================================================

pub struct CreateCharacterTool {
    ctx: ToolContext,
}
impl CreateCharacterTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateCharacterTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_character".into(),
            description: "创建新角色。生成角色 ID 并写入 entities/characters/ 目录。".into(),
            parameters: object_schema(vec![
                ("name", "string", "角色名称", true),
                ("aliases", "string", "别名，逗号分隔", false),
                ("gender", "string", "性别", false),
                ("age", "string", "年龄描述", false),
                ("role", "string", "在故事中的角色", false),
                ("appearance", "string", "角色外貌（多行自由文本）", false),
                ("background", "string", "角色背景（多行自由文本）", false),
                ("personality", "string", "性格描述（多行自由文本）", false),
                (
                    "habits",
                    "string",
                    "习性：口头禅、惯用手、小动作等（多行自由文本）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let name = get_str(arguments, "name")?;
        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_entity(&paths, EntityKind::Character, &name)
            .map_err(|e| e.to_string())?;

        // 追加可选字段（通过 update_entity 合并）
        let gender = get_str_opt(arguments, "gender");
        let appearance = get_str_opt(arguments, "appearance");
        let background = get_str_opt(arguments, "background");
        let personality = get_str_opt(arguments, "personality");
        let habits = get_str_opt(arguments, "habits");
        if gender.is_some()
            || appearance.is_some()
            || personality.is_some()
            || background.is_some()
            || habits.is_some()
            || get_str_opt(arguments, "aliases").is_some()
        {
            let entity_dir = paths.characters_dir();
            let entity_path = entity_dir.join(format!("{new_id}.toml"));
            let content =
                std::fs::read_to_string(&entity_path).map_err(|e| format!("读取失败: {e}"))?;
            let mut entity =
                EntityFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;
            if let Some(v) = get_str_opt(arguments, "aliases") {
                entity.aliases = v.split(',').map(|s| s.trim().to_string()).collect();
            }
            if gender.is_some() || appearance.is_some() || personality.is_some() || habits.is_some()
            {
                let profile = entity.profile.get_or_insert_with(Default::default);
                if let Some(g) = gender {
                    profile.gender = Some(g);
                }
                if let Some(a) = appearance {
                    profile.appearance = Some(a);
                }
                if let Some(b) = background {
                    profile.background = Some(b);
                }
                if let Some(p) = personality {
                    profile.personality = Some(p);
                }
                if let Some(h) = habits {
                    profile.habits = Some(h);
                }
            }
            let toml_content = entity
                .to_toml_string()
                .map_err(|e| format!("序列化失败: {e}"))?;
            std::fs::write(&entity_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;
        }

        let warning = duplicate_name_warning(&paths, EntityKind::Character, &new_id, &name);
        Ok(format!("角色已创建: id={new_id}, name={name}{warning}"))
    }
}

// ============================================================================
// create_location
// ============================================================================

pub struct CreateLocationTool {
    ctx: ToolContext,
}
impl CreateLocationTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateLocationTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_location".into(),
            description: "创建新地点（含世界观层面的地理设定：大陆/国家/城市/秘境等）。\
            生成地点 ID 并写入 entities/locations/ 目录。"
                .into(),
            parameters: object_schema(vec![
                ("name", "string", "地点名称（如：青云城）", true),
                ("aliases", "string", "别名，逗号分隔", false),
                ("parent", "string", "父地点 ID（构成地理层级树）", false),
                (
                    "geo_kind",
                    "string",
                    "地理类型（如：continent/nation/city/realm/landform）",
                    false,
                ),
                ("position", "string", "地理位置描述", false),
                ("features", "string", "特征描述（风貌、遗迹、气候等世界设定）", false),
                ("summary", "string", "一句话简介", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let name = get_str(arguments, "name")?;
        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_entity(&paths, EntityKind::Location, &name)
            .map_err(|e| e.to_string())?;

        // 追加可选字段
        let aliases = get_str_opt(arguments, "aliases");
        let parent = get_str_opt(arguments, "parent");
        let geo_kind = get_str_opt(arguments, "geo_kind");
        let position = get_str_opt(arguments, "position");
        let features = get_str_opt(arguments, "features");
        let summary = get_str_opt(arguments, "summary");
        if aliases.is_some()
            || parent.is_some()
            || geo_kind.is_some()
            || position.is_some()
            || features.is_some()
            || summary.is_some()
        {
            let entity_dir = paths.locations_dir();
            let entity_path = entity_dir.join(format!("{new_id}.toml"));
            let content =
                std::fs::read_to_string(&entity_path).map_err(|e| format!("读取失败: {e}"))?;
            let mut entity =
                EntityFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;
            if let Some(v) = aliases {
                entity.aliases = v.split(',').map(|s| s.trim().to_string()).collect();
            }
            let geo = entity.geography.get_or_insert_with(Default::default);
            if let Some(v) = parent {
                geo.parent = Some(v);
            }
            if let Some(v) = geo_kind {
                geo.kind = Some(v);
            }
            if let Some(v) = position {
                geo.position = Some(v);
            }
            if let Some(v) = features {
                geo.features = Some(v);
            }
            if let Some(v) = summary {
                geo.summary = Some(v);
            }
            let toml_content = entity
                .to_toml_string()
                .map_err(|e| format!("序列化失败: {e}"))?;
            std::fs::write(&entity_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;
        }

        let warning = duplicate_name_warning(&paths, EntityKind::Location, &new_id, &name);
        Ok(format!("地点已创建: id={new_id}, name={name}{warning}"))
    }
}

// ============================================================================
// create_item
// ============================================================================

pub struct CreateItemTool {
    ctx: ToolContext,
}
impl CreateItemTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateItemTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_item".into(),
            description: "创建新道具/物品。生成道具 ID 并写入 entities/items/ 目录。".into(),
            parameters: object_schema(vec![
                ("name", "string", "道具名称（如：寒霜剑）", true),
                ("aliases", "string", "别名，逗号分隔", false),
                (
                    "item_kind",
                    "string",
                    "道具类型（如：weapon/potion/artifact）",
                    false,
                ),
                (
                    "grade",
                    "string",
                    "品级（如：legendary/rare/common）",
                    false,
                ),
                ("description", "string", "道具描述", false),
                ("abilities", "string", "能力/效果描述", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let name = get_str(arguments, "name")?;
        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_entity(&paths, EntityKind::Item, &name)
            .map_err(|e| e.to_string())?;

        // 追加可选字段
        let aliases = get_str_opt(arguments, "aliases");
        let item_kind = get_str_opt(arguments, "item_kind");
        let grade = get_str_opt(arguments, "grade");
        let description = get_str_opt(arguments, "description");
        let abilities = get_str_opt(arguments, "abilities");
        if aliases.is_some()
            || item_kind.is_some()
            || grade.is_some()
            || description.is_some()
            || abilities.is_some()
        {
            let entity_dir = paths.items_dir();
            let entity_path = entity_dir.join(format!("{new_id}.toml"));
            let content =
                std::fs::read_to_string(&entity_path).map_err(|e| format!("读取失败: {e}"))?;
            let mut entity =
                EntityFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;
            if let Some(v) = aliases {
                entity.aliases = v.split(',').map(|s| s.trim().to_string()).collect();
            }
            let item = entity.item.get_or_insert_with(Default::default);
            if let Some(v) = item_kind {
                item.kind = Some(v);
            }
            if let Some(v) = grade {
                item.grade = Some(v);
            }
            if let Some(v) = description {
                item.description = Some(v);
            }
            if let Some(v) = abilities {
                item.abilities = Some(v);
            }
            let toml_content = entity
                .to_toml_string()
                .map_err(|e| format!("序列化失败: {e}"))?;
            std::fs::write(&entity_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;
        }

        let warning = duplicate_name_warning(&paths, EntityKind::Item, &new_id, &name);
        Ok(format!("道具已创建: id={new_id}, name={name}{warning}"))
    }
}

// ============================================================================
// create_organization
// ============================================================================

pub struct CreateOrganizationTool {
    ctx: ToolContext,
}
impl CreateOrganizationTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateOrganizationTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_organization".into(),
            description: "创建新组织/势力（宗门、国家、家族、公会等，含世界观层面的设定）。\
            生成组织 ID 并写入 entities/organizations/ 目录。"
                .into(),
            parameters: object_schema(vec![
                ("name", "string", "组织名称（如：天机阁）", true),
                ("aliases", "string", "别名，逗号分隔", false),
                (
                    "org_kind",
                    "string",
                    "组织类型（如：sect/nation/clan/guild/organization）",
                    false,
                ),
                ("leader", "string", "首领/代表人物", false),
                ("territory", "string", "势力范围", false),
                ("ideology", "string", "理念/宗旨", false),
                ("summary", "string", "简介（性质、宗旨、规模等）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let name = get_str(arguments, "name")?;
        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_entity(&paths, EntityKind::Organization, &name)
            .map_err(|e| e.to_string())?;

        // 追加可选字段（存入 [org] 段）
        let aliases = get_str_opt(arguments, "aliases");
        let org_kind = get_str_opt(arguments, "org_kind");
        let leader = get_str_opt(arguments, "leader");
        let territory = get_str_opt(arguments, "territory");
        let ideology = get_str_opt(arguments, "ideology");
        let summary = get_str_opt(arguments, "summary");
        if aliases.is_some()
            || org_kind.is_some()
            || leader.is_some()
            || territory.is_some()
            || ideology.is_some()
            || summary.is_some()
        {
            let entity_dir = paths.organizations_dir();
            let entity_path = entity_dir.join(format!("{new_id}.toml"));
            let content =
                std::fs::read_to_string(&entity_path).map_err(|e| format!("读取失败: {e}"))?;
            let mut entity =
                EntityFile::from_toml_str(&content).map_err(|e| format!("解析失败: {e}"))?;
            if let Some(v) = aliases {
                entity.aliases = v.split(',').map(|s| s.trim().to_string()).collect();
            }
            let org = entity.org.get_or_insert_with(Default::default);
            if let Some(v) = org_kind {
                org.kind = Some(v);
            }
            if let Some(v) = leader {
                org.leader = Some(v);
            }
            if let Some(v) = territory {
                org.territory = Some(v);
            }
            if let Some(v) = ideology {
                org.ideology = Some(v);
            }
            if let Some(v) = summary {
                org.summary = Some(v);
            }
            let toml_content = entity
                .to_toml_string()
                .map_err(|e| format!("序列化失败: {e}"))?;
            std::fs::write(&entity_path, toml_content).map_err(|e| format!("写入失败: {e}"))?;
        }

        let warning = duplicate_name_warning(&paths, EntityKind::Organization, &new_id, &name);
        Ok(format!("组织已创建: id={new_id}, name={name}{warning}"))
    }
}

// ============================================================================
// search_entities
// ============================================================================

pub struct SearchEntitiesTool {
    ctx: ToolContext,
}
impl SearchEntitiesTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SearchEntitiesTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_entities".into(),
            description: "搜索项目中的实体（角色/地点/道具/组织），返回匹配的 ID 和名称列表。\
            关键词匹配名称、别名、符号。可直接读磁盘文件，不依赖编译结果。\
            💡 query 可省略（或传 \"*\"）以列出全部实体——这是了解项目内容的第一步。"
                .into(),
            parameters: object_schema(vec![
                (
                    "query",
                    "string",
                    "搜索关键词（匹配名称/别名/符号）；省略或传 * 列出全部",
                    false,
                ),
                (
                    "entity_type",
                    "string",
                    "实体类型（characters/locations/items/organizations），可选过滤",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // query 可选：省略或 "*" 或空字符串 → 列出全部
        let query_raw = get_str_opt(arguments, "query");
        let query = match query_raw.as_deref() {
            None | Some("") | Some("*") | Some("＊") => String::new(), // 空 = 匹配全部
            Some(q) => q.to_lowercase(),
        };
        let match_all = query.is_empty();
        let entity_type = get_str_opt(arguments, "entity_type");
        let paths = self.ctx.paths();

        // 直接扫描磁盘上的实体文件，不依赖 IR 语义图（避免解析错误导致搜索失效）
        let entity_dirs: &[(&str, std::path::PathBuf)] = &[
            ("characters", paths.characters_dir()),
            ("locations", paths.locations_dir()),
            ("items", paths.items_dir()),
            ("organizations", paths.organizations_dir()),
        ];

        let mut results: Vec<String> = Vec::new();
        let mut scan_errors: Vec<String> = Vec::new();

        for (type_name, dir) in entity_dirs {
            // 按类型过滤
            if let Some(ref et) = entity_type {
                if et != type_name {
                    continue;
                }
            }
            if !dir.exists() {
                continue;
            }
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(e) => {
                    scan_errors.push(format!("读取目录 {} 失败: {e}", dir.display()));
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        scan_errors.push(format!("读取 {} 失败: {e}", path.display()));
                        continue;
                    }
                };
                let entity = match EntityFile::from_toml_str(&content) {
                    Ok(e) => e,
                    Err(e) => {
                        scan_errors.push(format!("解析 {} 失败: {e}", path.display()));
                        continue;
                    }
                };

                let display_lc = entity.display_name.to_lowercase();
                let aliases_lc: Vec<String> =
                    entity.aliases.iter().map(|a| a.to_lowercase()).collect();
                let symbol_lc = entity.symbol.as_deref().map(|s| s.to_lowercase());

                // 匹配 名称 / 别名 / 符号（match_all 时跳过过滤）
                let matched = match_all
                    || display_lc.contains(&query)
                    || aliases_lc.iter().any(|a| a.contains(&query))
                    || symbol_lc.as_deref().is_some_and(|s| s.contains(&query));

                if !matched {
                    continue;
                }

                // 输出: ID [type] 名称 (aliases) [+ symbol]
                let mut line = format!("{} [{}] {}", entity.id, type_name, entity.display_name);
                if !entity.aliases.is_empty() {
                    line.push_str(&format!(" (aliases: {})", entity.aliases.join(", ")));
                }
                if let Some(ref sym) = entity.symbol {
                    line.push_str(&format!(" ⟨{sym}⟩"));
                }
                results.push(line);
            }
        }

        if results.is_empty() {
            // 附带扫描错误信息，帮助诊断
            let mut msg = if match_all {
                "项目中没有任何实体文件。使用 create_character / create_location / create_item / create_organization 创建。"
                    .to_string()
            } else {
                format!("未找到匹配「{query}」的实体")
            };
            if !scan_errors.is_empty() {
                msg.push_str(&format!(
                    "\n（搜索时遇到 {} 个文件解析错误）",
                    scan_errors.len()
                ));
            }
            Ok(msg)
        } else {
            results.sort();
            let header = if match_all {
                format!("共 {} 个实体:", results.len())
            } else {
                format!("找到 {} 个匹配「{query}」的实体:", results.len())
            };
            let mut out = format!("{header}\n{}", results.join("\n"));
            if !scan_errors.is_empty() {
                out.push_str(&format!(
                    "\n（另有 {} 个文件解析失败已跳过）",
                    scan_errors.len()
                ));
            }
            Ok(out)
        }
    }
}

// ============================================================================
// get_entity_detail
// ============================================================================

pub struct GetEntityDetailTool {
    ctx: ToolContext,
}
impl GetEntityDetailTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetEntityDetailTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_entity_detail".into(),
            description: "获取指定实体的完整信息：基础信息、专属属性（角色档案/地理/道具/组织）、\
            自定义属性、出场记录。"
                .into(),
            parameters: object_schema(vec![("entity_id", "string", "实体 ID", true)]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;
        let paths = self.ctx.paths();

        // 优先读取原始 TOML 文件（携带完整的 profile/attributes，IR 会丢弃这些字段）
        let entity = match paths.file_for_id(&entity_id) {
            Some(path) if path.exists() => {
                let content =
                    std::fs::read_to_string(&path).map_err(|e| format!("读取实体文件失败: {e}"))?;
                Some(
                    EntityFile::from_toml_str(&content)
                        .map_err(|e| format!("解析实体失败: {e}"))?,
                )
            }
            _ => None,
        };

        // IR 用于出场记录扫描；即使原始文件不存在也尝试走 IR 兜底
        let parse_output = self.ctx.parse()?;
        let node = parse_output
            .ir
            .semantic
            .get(&StableId::new_unchecked(&entity_id));

        // 既无文件也无 IR 节点 → 实体不存在
        if entity.is_none() && node.is_none() {
            return Err(format!("实体 '{entity_id}' 不存在"));
        }

        let mut out = String::new();
        out.push_str("实体详情:\n");

        if let Some(ref ef) = entity {
            // —— 完整字段来自原始 TOML ——
            out.push_str(&format!("  ID: {}\n", ef.id));
            out.push_str(&format!("  名称: {}\n", ef.display_name));
            if let Some(ref sym) = ef.symbol {
                out.push_str(&format!("  符号: {sym}\n"));
            }
            if !ef.aliases.is_empty() {
                out.push_str(&format!("  别名: {}\n", ef.aliases.join(", ")));
            }

            // 按实体类型输出专属段
            if let Some(ref profile) = ef.profile {
                push_profile(&mut out, profile);
            }
            if let Some(ref geo) = ef.geography {
                push_geography(&mut out, geo);
            }
            if let Some(ref item) = ef.item {
                push_item(&mut out, item);
            }
            if let Some(ref org) = ef.org {
                push_org(&mut out, org);
            }
            if !ef.attributes.is_empty() {
                out.push_str("  自定义属性:\n");
                for (k, v) in &ef.attributes {
                    out.push_str(&format!("    {k}: {v}\n"));
                }
            }
        } else if let Some(sn) = node.as_ref() {
            // 无原始文件、仅有 IR 节点（罕见）：退化为基础信息
            out.push_str(&format!("  ID: {}\n", sn.id()));
            out.push_str(&format!("  名称: {}\n", sn.display_name()));
            if !sn.aliases().is_empty() {
                out.push_str(&format!("  别名: {}\n", sn.aliases().join(", ")));
            }
        }

        // —— 出场记录（LocationIndex：显式锚点 + 隐式文本匹配，与 find_mentions 同源；
        //      按片段去重，同一片段多次出现标 ×N）——
        let grouped = collect_appearances(&parse_output, &entity_id);
        let mut appearances = Vec::new();
        for (fid, count) in &grouped {
            let brief = parse_output
                .ir
                .document
                .get_fragment(&StableId::new_unchecked(fid))
                .map(|f| format_brief(&f.text, 60))
                .unwrap_or_default();
            let times = if *count > 1 { format!(" ×{count}") } else { String::new() };
            appearances.push(format!("  - {fid}{times}: \"{brief}\""));
        }
        out.push_str(&format!("\n  出场({} 个片段):\n", grouped.len()));
        out.push_str(&appearances.join("\n"));
        out.push_str("\n\n提示：可用 profile_check 核对该实体档案与正文描写的一致性（软提示）。");

        Ok(out)
    }
}

/// 收集实体的出场片段（LocationIndex 优先，空则兜底扫描显式锚点）。
///
/// 返回 (fragment_id, 出现次数)，按片段去重——get_entity_detail 展示与
/// profile_check 取证共用。
fn collect_appearances(
    parse_output: &story_parser::ParseOutput,
    entity_id: &str,
) -> Vec<(String, usize)> {
    let sid = StableId::new_unchecked(entity_id);
    let mut grouped: Vec<(String, usize)> = Vec::new();
    for a in parse_output.location_index.appearances_of(&sid) {
        let fid = a.fragment.as_str().to_string();
        match grouped.iter_mut().find(|(f, _)| *f == fid) {
            Some(entry) => entry.1 += 1,
            None => grouped.push((fid, 1)),
        }
    }
    if grouped.is_empty() {
        // 兜底：LocationIndex 为空时直接扫描显式 anchors
        for (fid, fnode) in parse_output.ir.document.all_fragments() {
            if fnode.anchors.iter().any(|a| a.entity == sid) {
                grouped.push((fid.as_str().to_string(), 1));
            }
        }
    }
    grouped
}

// ============================================================================
// profile_check — 档案与正文一致性核对（软提示）
// ============================================================================

pub struct ProfileCheckTool {
    ctx: ToolContext,
}
impl ProfileCheckTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

/// LLM 返回的冲突报告
#[derive(Debug, Default, serde::Deserialize)]
struct ProfileCheckLlm {
    #[serde(default)]
    conflicts: Vec<ProfileConflictLlm>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, serde::Deserialize)]
struct ProfileConflictLlm {
    field: String,
    profile_value: String,
    text_value: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    fragment_id: String,
}

impl Tool for ProfileCheckTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "profile_check".into(),
            description: "核对实体档案与正文描写的一致性（软提示，非错误）：把档案的事实性字段\
            （职务/身份/隶属/存亡/所在地/亲缘/时间线）与出场片段摘要对照，报告出入处\
            （如档案写「退休副局长」、正文里却以现职身份下令）。\
            档案过时也可能是有意的伏笔设计——报告只帮作者看见出入。\
            整章交付前可对主要角色各跑一遍。".into(),
            parameters: object_schema(vec![(
                "entity_id",
                "string",
                "要核对的实体 ID（如 c_0005）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;
        let provider = self
            .ctx
            .provider
            .as_ref()
            .ok_or_else(|| "profile_check 需要 provider，当前未配置".to_string())?;
        let paths = self.ctx.paths();

        // 档案：原始 TOML（完整字段）
        let entity_path = paths
            .file_for_id(&entity_id)
            .ok_or_else(|| format!("无效的实体 ID: {entity_id}"))?;
        if !entity_path.exists() {
            return Err(format!("实体 '{entity_id}' 不存在"));
        }
        let content = std::fs::read_to_string(&entity_path)
            .map_err(|e| format!("读取实体文件失败: {e}"))?;
        let entity =
            EntityFile::from_toml_str(&content).map_err(|e| format!("解析实体失败: {e}"))?;

        // 出场取证：片段摘要（上限 12 条，60 字摘要 + 次数）
        const APPEARANCE_CAP: usize = 12;
        let parse_output = self.ctx.parse()?;
        let grouped = collect_appearances(&parse_output, &entity_id);
        if grouped.is_empty() {
            return Ok(format!(
                "实体 {}「{}」暂无出场记录，无正文可比对。",
                entity_id, entity.display_name
            ));
        }

        let mut evidence = String::new();
        for (fid, count) in grouped.iter().take(APPEARANCE_CAP) {
            let brief = parse_output
                .ir
                .document
                .get_fragment(&StableId::new_unchecked(fid))
                .map(|f| format_brief(&f.text, 60))
                .unwrap_or_default();
            let times = if *count > 1 { format!(" ×{count}") } else { String::new() };
            evidence.push_str(&format!("- {fid}{times}: {brief}\n"));
        }
        if grouped.len() > APPEARANCE_CAP {
            evidence.push_str(&format!("（其余 {} 个片段略）\n", grouped.len() - APPEARANCE_CAP));
        }

        // 档案字段摘要（事实性字段拼进 prompt）
        let mut profile_text = format!("名称：{}\n", entity.display_name);
        if !entity.aliases.is_empty() {
            profile_text.push_str(&format!("别名：{}\n", entity.aliases.join("、")));
        }
        let mut section = |title: &str, pairs: Vec<(&str, Option<&String>)>| {
            let filled: Vec<String> = pairs
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| format!("{k}: {v}")))
                .collect();
            if !filled.is_empty() {
                profile_text.push_str(&format!("[{title}]\n{}\n", filled.join("\n")));
            }
        };
        if let Some(p) = &entity.profile {
            section(
                "角色档案",
                vec![
                    ("race", p.race.as_ref()),
                    ("gender", p.gender.as_ref()),
                    ("occupation", p.occupation.as_ref()),
                    ("appearance", p.appearance.as_ref()),
                    ("background", p.background.as_ref()),
                    ("personality", p.personality.as_ref()),
                ],
            );
        }
        if let Some(g) = &entity.geography {
            section(
                "地理",
                vec![
                    ("kind", g.kind.as_ref()),
                    ("position", g.position.as_ref()),
                    ("summary", g.summary.as_ref()),
                ],
            );
        }
        if let Some(i) = &entity.item {
            section(
                "道具",
                vec![("kind", i.kind.as_ref()), ("description", i.description.as_ref())],
            );
        }
        if let Some(o) = &entity.org {
            section(
                "组织",
                vec![
                    ("kind", o.kind.as_ref()),
                    ("leader", o.leader.as_ref()),
                    ("territory", o.territory.as_ref()),
                    ("summary", o.summary.as_ref()),
                ],
            );
        }

        let user = format!(
            "实体档案（{entity_id}）：\n{profile_text}\n出场片段摘要（阅读序）：\n{evidence}"
        );

        let request = story_ai_runtime::provider::GenerateRequest {
            system: crate::prompts::PROFILE_CHECK.to_string(),
            messages: vec![story_ai_runtime::provider::Message::user(user)],
            model: String::new(),
            temperature: 0.1,
            max_tokens: 16384,
            tools: Vec::new(),
        };
        let response = super::retry_generate(provider, &request)
            .map_err(|e| format!("一致性核对调用失败: {e}"))?;

        let report = parse_profile_check_response(&response.text);
        Ok(format_profile_check(&entity_id, &entity.display_name, &report))
    }
}

/// 宽松解析 LLM 的核对输出（直解 → 剥围栏 → 首尾花括号）
fn parse_profile_check_response(raw: &str) -> ProfileCheckLlm {
    let cleaned = raw.trim();
    let candidates = {
        let mut v = vec![cleaned.to_string()];
        let stripped = cleaned
            .trim_start_matches("```")
            .trim_start_matches("json")
            .trim();
        v.push(stripped.strip_suffix("```").unwrap_or(stripped).trim().to_string());
        if let (Some(s), Some(e)) = (cleaned.find('{'), cleaned.rfind('}')) {
            if e > s {
                v.push(cleaned[s..=e].to_string());
            }
        }
        v
    };
    for c in candidates {
        if let Ok(report) = serde_json::from_str::<ProfileCheckLlm>(&c) {
            return report;
        }
    }
    ProfileCheckLlm {
        conflicts: vec![],
        summary: format!("核对结果解析失败，原始内容：{cleaned}"),
    }
}

fn format_profile_check(entity_id: &str, name: &str, report: &ProfileCheckLlm) -> String {
    if report.conflicts.is_empty() {
        return format!(
            "档案一致性核对（{entity_id}「{name}」）：未见明显出入。{}",
            if report.summary.is_empty() {
                String::new()
            } else {
                report.summary.clone()
            }
        );
    }

    let mut out = format!(
        "档案一致性核对（{entity_id}「{name}」）：发现 {} 处出入（软提示——也可能是有意设计，如档案过时作为伏笔）：",
        report.conflicts.len()
    );
    for (idx, c) in report.conflicts.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. 字段 {}：档案「{}」 vs 正文「{}」",
            idx + 1,
            c.field,
            c.profile_value,
            c.text_value
        ));
        if !c.quote.is_empty() {
            out.push_str(&format!(
                "\n   原文「{}」（{}）",
                c.quote,
                if c.fragment_id.is_empty() {
                    "片段未知"
                } else {
                    &c.fragment_id
                }
            ));
        }
    }
    if !report.summary.is_empty() {
        out.push_str(&format!("\n总结：{}", report.summary));
    }
    out
}

/// 渲染角色档案 [profile] 段
fn push_profile(out: &mut String, p: &story_project_format::entity_file::ProfileSection) {
    let mut lines: Vec<String> = Vec::new();
    if let Some(ref v) = p.race {
        lines.push(format!("种族: {v}"));
    }
    if let Some(ref v) = p.gender {
        lines.push(format!("性别: {v}"));
    }
    if let Some(ref v) = p.occupation {
        lines.push(format!("职业: {v}"));
    }
    if let Some(v) = p.birth_year {
        lines.push(format!("出生年份: {v}"));
    }
    if let Some(ref v) = p.appearance {
        lines.push(format!("外貌: {v}"));
    }
    if let Some(ref v) = p.background {
        lines.push(format!("背景: {v}"));
    }
    if let Some(ref v) = p.personality {
        lines.push(format!("性格: {v}"));
    }
    if let Some(ref v) = p.habits {
        lines.push(format!("习性: {v}"));
    }
    if !lines.is_empty() {
        out.push_str("  角色档案:\n");
        for l in lines {
            out.push_str(&format!("    {l}\n"));
        }
    }
}

/// 渲染地理信息 [geography] 段
fn push_geography(out: &mut String, g: &story_project_format::entity_file::GeographySection) {
    let mut lines: Vec<String> = Vec::new();
    if let Some(ref v) = g.kind {
        lines.push(format!("地理类型: {v}"));
    }
    if let Some(ref v) = g.parent {
        lines.push(format!("父地点: {v}"));
    }
    if let Some(ref v) = g.position {
        lines.push(format!("位置: {v}"));
    }
    if let Some(ref v) = g.features {
        lines.push(format!("特征: {v}"));
    }
    if let Some(ref v) = g.summary {
        lines.push(format!("简介: {v}"));
    }
    if !lines.is_empty() {
        out.push_str("  地理信息:\n");
        for l in lines {
            out.push_str(&format!("    {l}\n"));
        }
    }
}

/// 渲染道具信息 [item] 段
fn push_item(out: &mut String, it: &story_project_format::entity_file::ItemSection) {
    let mut lines: Vec<String> = Vec::new();
    if let Some(ref v) = it.kind {
        lines.push(format!("类型: {v}"));
    }
    if let Some(ref v) = it.grade {
        lines.push(format!("品级: {v}"));
    }
    if it.unique {
        lines.push("唯一道具".to_string());
    }
    if let Some(ref v) = it.description {
        lines.push(format!("描述: {v}"));
    }
    if let Some(ref v) = it.abilities {
        lines.push(format!("能力: {v}"));
    }
    if !lines.is_empty() {
        out.push_str("  道具信息:\n");
        for l in lines {
            out.push_str(&format!("    {l}\n"));
        }
    }
}

/// 渲染组织信息 [org] 段
fn push_org(out: &mut String, o: &story_project_format::entity_file::OrgSection) {
    let mut lines: Vec<String> = Vec::new();
    if let Some(ref v) = o.kind {
        lines.push(format!("类型: {v}"));
    }
    if let Some(ref v) = o.leader {
        lines.push(format!("首领: {v}"));
    }
    if let Some(ref v) = o.territory {
        lines.push(format!("范围: {v}"));
    }
    if let Some(ref v) = o.ideology {
        lines.push(format!("理念: {v}"));
    }
    if let Some(ref v) = o.summary {
        lines.push(format!("简介: {v}"));
    }
    if let Some(ref v) = o.location {
        lines.push(format!("所在地: {v}"));
    }
    if !lines.is_empty() {
        out.push_str("  组织信息:\n");
        for l in lines {
            out.push_str(&format!("    {l}\n"));
        }
    }
}

// ============================================================================
// delete_entity
// ============================================================================

pub struct DeleteEntityTool {
    ctx: ToolContext,
}
impl DeleteEntityTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteEntityTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_entity".into(),
            description: "删除实体（角色/地点/道具/组织）。只删除实体自己的文件，\
            不会级联清理断言、关系、POV 或锚点引用——这些悬空引用会在 compile_check 时被诊断出来。\
            删除前请先用 get_entity_detail 确认。不可恢复。"
                .into(),
            parameters: object_schema(vec![(
                "entity_id",
                "string",
                "要删除的实体 ID（如 c_0001 / l_0001 / i_0001 / o_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;
        let paths = self.ctx.paths();
        crate::mutations::delete_entity(&paths, &entity_id).map_err(|e| e.to_string())?;
        Ok(format!(
            "实体 {entity_id} 已删除（相关断言/关系/锚点引用未清理，建议执行 compile_check 检查悬空引用）"
        ))
    }
}

// ============================================================================
// merge_entities
// ============================================================================

pub struct MergeEntitiesTool {
    ctx: ToolContext,
}
impl MergeEntitiesTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for MergeEntitiesTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "merge_entities".into(),
            description: "合并实体：把 from_entity 的一切引用重定向到 into_entity 并删除 from\
            （两者须同类型）。典型场景：匿名占位实体（如「九一真凶」）在真名揭晓后\
            并入正式角色档案。from 的档案名与别名转入目标的别名，其余档案字段不合并。\
            重定向范围：片段锚点、关系边（合并产生的自环删除）、断言、场景 POV、\
            章节脉络出场表、线索观察者、outline 角色弧线、未锚定指代归拢目标。".into(),
            parameters: object_schema(vec![
                (
                    "from_entity_id",
                    "string",
                    "被合并的实体 ID（如匿名占位实体）",
                    true,
                ),
                (
                    "into_entity_id",
                    "string",
                    "保留的目标实体 ID",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let from_id = get_str(arguments, "from_entity_id")?;
        let into_id = get_str(arguments, "into_entity_id")?;
        let paths = self.ctx.paths();
        let report =
            crate::mutations::merge_entities(&paths, &from_id, &into_id).map_err(|e| e.to_string())?;
        Ok(report.summary(&from_id, &into_id))
    }
}

// ============================================================================
// get_scene_context
// ============================================================================

pub struct GetSceneContextTool {
    ctx: ToolContext,
}
impl GetSceneContextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetSceneContextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_scene_context".into(),
            description: "获取指定场景的完整上下文：POV、出场角色、片段列表、所属章节。".into(),
            parameters: object_schema(vec![("scene_id", "string", "场景 ID（如 sc_01_01）", true)]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scene_id = get_str(arguments, "scene_id")?;
        let parse_output = self.ctx.parse()?;
        let sid = StableId::new_unchecked(&scene_id);
        let scene = parse_output
            .ir
            .document
            .get_scene(&sid)
            .ok_or_else(|| format!("场景 '{scene_id}' 不存在"))?;

        // 所属章节：同时拿到 title 与 chapter_id（后者用于匹配 chapters.toml 中的脉络）
        let chapter_info = parse_output
            .ir
            .document
            .chapter_of_scene(&sid)
            .and_then(|ch_id| {
                parse_output
                    .ir
                    .document
                    .get_chapter(ch_id)
                    .map(|ch| (ch_id.as_str().to_string(), ch.title.clone()))
            });

        let mut fragments = Vec::new();
        for fid in parse_output.ir.document.fragments_of_scene(&sid) {
            if let Some(frag) = parse_output.ir.document.get_fragment(fid) {
                fragments.push(format!("  {}: \"{}\"", fid, format_brief(&frag.text, 100)));
            }
        }

        let pov_id = scene.pov.as_ref();
        let pov_name = pov_id.and_then(|id| {
            parse_output
                .ir
                .semantic
                .get(id)
                .map(|n| n.display_name().to_string())
        });

        // 本章涉及角色清单：从 chapters.toml 的脉络里按 chapter_id 匹配读取。
        // 失败时静默跳过——此为增强上下文，不阻断主流程。
        let pov_id_str = pov_id.map(|s| s.as_str());
        let characters_block = chapter_info.as_ref().and_then(|(ch_id_str, _)| {
            collect_chapter_characters(&self.ctx, ch_id_str, pov_id_str, &parse_output.ir.semantic)
        });

        let pov_display = match (pov_name.as_deref(), pov_id_str) {
            (Some(name), Some(id)) => format!("{name}（{id}）"),
            (Some(name), None) => name.to_string(),
            (None, Some(id)) => format!("（{id} 未在档案中）"),
            (None, None) => "未设置".to_string(),
        };

        let mut result = format!(
            "场景 {scene_id}:\n  标题: {}\n  所属章节: {}\n  POV: {}\n  氛围: {}\n  片段数: {}\n",
            scene.title.as_deref().unwrap_or("无"),
            chapter_info
                .as_ref()
                .map(|(_, t)| t.as_str())
                .unwrap_or("未知"),
            pov_display,
            scene.mood.as_deref().unwrap_or("未设置"),
            fragments.len(),
        );
        // 故事时间（数值锚 / 历法标签 + 与上一场景的间隔）
        let mut time_label = String::new();
        if let Some(cal) = &scene.story_time {
            time_label = cal.year.to_string();
            if let Some(m) = cal.month {
                time_label.push_str(&format!("-{m:02}"));
                if let Some(d) = cal.day {
                    time_label.push_str(&format!("-{d:02}"));
                }
            }
        }
        if let Some(anchor) = &scene.story_time_anchor {
            if !time_label.is_empty() {
                time_label.push('，');
            }
            time_label.push_str(anchor);
        }
        if let Some(gap) = &scene.story_time_gap {
            if !time_label.is_empty() {
                time_label.push('，');
            }
            time_label.push_str(&format!("距上一场景 {gap}"));
        }
        if !time_label.is_empty() {
            result.push_str(&format!("  故事时间: {time_label}\n"));
        }
        if let Some(block) = characters_block {
            result.push_str("  本章涉及角色:\n");
            result.push_str(&block);
        }
        if !fragments.is_empty() {
            result.push_str("\n片段:\n");
            result.push_str(&fragments.join("\n"));
        }

        // 动笔前的文风提醒：在动作发生的瞬间 prime 模型，比堆在系统提示深处更有效。
        // 内容与 AGENT_SYSTEM_PROMPT 的文风准则呼应，保持精简，避免喧宾夺主。
        result.push_str(
            "\n\n[写作要点] 白描为主，少修辞，少破折号/省略号；情绪靠动作对话暗示，\
不滥写生理反应；在场角色都要按各自性格行事；对话要承载功能、声线区分；\
留一个未完成的张力；锁定 POV 视角，只写其五感所及；本章末尾设翻页钩子。",
        );
        Ok(result)
    }
}

// ============================================================================
// helpers
// ============================================================================

/// 读取 chapters.toml 中匹配 `chapter_id` 的章节脉络，渲染其 `characters` 列表。
///
/// 每个 ID 尝试在 semantic graph 里解析出显示名与 symbol；解析失败的标注"档案缺失"。
/// POV 角色若也出现在列表中，追加 `[POV]` 标记，便于 AI 区分主视角与其余出场角色。
///
/// 返回 `None` 表示：chapters.toml 不存在/解析失败/无匹配章节/characters 为空。
fn collect_chapter_characters(
    ctx: &ToolContext,
    chapter_id: &str,
    pov_id: Option<&str>,
    semantic: &SemanticGraph,
) -> Option<String> {
    let file = crate::mutations::load_chapters_file(&ctx.paths()).ok()?;
    let outline = file
        .chapters
        .iter()
        .find(|c| c.chapter_id.as_deref() == Some(chapter_id))?;
    if outline.characters.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(outline.characters.len());
    for cid in &outline.characters {
        let resolved = semantic.get(&StableId::new_unchecked(cid));
        let entry = match resolved {
            Some(node) => {
                let name = node.display_name();
                match node.symbol() {
                    Some(sym) => format!("{name}（{cid} / {sym}）"),
                    None => format!("{name}（{cid}）"),
                }
            }
            None => format!("{cid}（档案缺失）"),
        };
        let marked = if pov_id == Some(cid.as_str()) {
            format!("    - {entry} [POV]")
        } else {
            format!("    - {entry}")
        };
        lines.push(marked);
    }
    Some(lines.join("\n") + "\n")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use story_ai_runtime::Tool;
    use story_project_format::init::init_project;

    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    /// 写入一个带完整 profile 的角色文件
    fn write_character(dir: &Path, id: &str) {
        let path = dir.join("entities/characters").join(format!("{id}.toml"));
        let content = format!(
            r#"id = "{id}"
display_name = "林风"
aliases = ["林公子", "风少"]
symbol = "LF"

[profile]
race = "人族"
gender = "男"
occupation = "剑客"
birth_year = -120
appearance = "白衣胜雪，眉目清冷"
background = "前朝皇子，幼年流落江湖"

[attributes]
weapon = "寒霜剑"
level = "筑基期"
"#
        );
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn get_entity_detail_returns_full_profile() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_character(dir, "c_0001");

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = GetEntityDetailTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "entity_id": "c_0001" }))
            .unwrap();

        // 角色档案字段全部可见
        assert!(result.contains("种族: 人族"));
        assert!(result.contains("性别: 男"));
        assert!(result.contains("职业: 剑客"));
        assert!(result.contains("出生年份: -120"));
        assert!(result.contains("外貌: 白衣胜雪"));
        assert!(result.contains("背景: 前朝皇子"));
        // 自定义属性可见
        assert!(result.contains("weapon: 寒霜剑"));
        assert!(result.contains("level: 筑基期"));
        // 别名和符号
        assert!(result.contains("林公子"));
        assert!(result.contains("符号: LF"));
    }

    #[test]
    fn get_entity_detail_item_with_abilities() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let item_path = dir.join("entities/items").join("i_0001.toml");
        std::fs::write(
            &item_path,
            r#"id = "i_0001"
display_name = "寒霜剑"

[item]
kind = "weapon"
grade = "legendary"
unique = true
description = "千年玄冰铸就"
abilities = "冰封万物"
"#,
        )
        .unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = GetEntityDetailTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "entity_id": "i_0001" }))
            .unwrap();

        // description/abilities 以前被 IR 丢弃，现在应可见
        assert!(result.contains("描述: 千年玄冰铸就"));
        assert!(result.contains("能力: 冰封万物"));
        assert!(result.contains("品级: legendary"));
        assert!(result.contains("唯一道具"));
    }

    #[test]
    fn get_entity_detail_nonexistent() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetEntityDetailTool::new(ctx);
        let result = tool.execute(&serde_json::json!({ "entity_id": "c_9999" }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    /// 写入多个实体文件用于搜索测试
    fn write_search_fixtures(dir: &Path) {
        // 角色
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            r#"id = "c_0001"
display_name = "林风"
aliases = ["林公子", "风少"]
symbol = "LF"

[profile]
gender = "男"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0002.toml"),
            r#"id = "c_0002"
display_name = "苏雨晴"
aliases = ["苏姑娘"]
"#,
        )
        .unwrap();
        // 地点
        std::fs::write(
            dir.join("entities/locations/l_0001.toml"),
            r#"id = "l_0001"
display_name = "青云城"
aliases = ["青云"]
"#,
        )
        .unwrap();
        // 道具
        std::fs::write(
            dir.join("entities/items/i_0001.toml"),
            r#"id = "i_0001"
display_name = "寒霜剑"
aliases = ["霜剑"]

[item]
kind = "weapon"
"#,
        )
        .unwrap();
    }

    #[test]
    fn search_by_display_name() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool
            .execute(&serde_json::json!({ "query": "林风" }))
            .unwrap();
        assert!(result.contains("c_0001"));
        assert!(result.contains("林风"));
    }

    #[test]
    fn search_by_alias() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool
            .execute(&serde_json::json!({ "query": "风少" }))
            .unwrap();
        assert!(result.contains("c_0001"), "应通过别名匹配到林风");
    }

    #[test]
    fn search_by_symbol() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool.execute(&serde_json::json!({ "query": "LF" })).unwrap();
        assert!(result.contains("c_0001"), "应通过符号匹配到林风");
    }

    #[test]
    fn search_with_type_filter() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));

        // 搜索"剑"应只返回道具
        let result = tool
            .execute(&serde_json::json!({ "query": "剑", "entity_type": "items" }))
            .unwrap();
        assert!(result.contains("i_0001"));
        assert!(!result.contains("c_"));

        // 类型过滤排除不匹配的
        let result = tool
            .execute(&serde_json::json!({ "query": "林", "entity_type": "locations" }))
            .unwrap();
        assert!(result.contains("未找到"));
    }

    #[test]
    fn search_case_insensitive() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool.execute(&serde_json::json!({ "query": "lf" })).unwrap();
        assert!(result.contains("c_0001"));
    }

    #[test]
    fn search_partial_match() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        // "风" 同时匹配"林风"和"寒霜剑"（别名"霜剑"不含"风"，只匹配林风）
        let result = tool.execute(&serde_json::json!({ "query": "风" })).unwrap();
        assert!(result.contains("林风"));
    }

    #[test]
    fn profile_check_response_parses_conflicts() {
        // 康明远式案例：档案「退休副局长」 vs 正文现职行为
        let raw = r#"{"conflicts":[{"field":"occupation","profile_value":"退休副局长、市警察协会顾问","text_value":"现职局长，到场下令专案重启","quote":"康明远宣布专案重启","fragment_id":"f_0001"}],"summary":"档案与正文职务描述存在出入。"}"#;
        let report = parse_profile_check_response(raw);
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].field, "occupation");
        assert_eq!(report.conflicts[0].fragment_id, "f_0001");

        let out = format_profile_check("c_0005", "康明远", &report);
        assert!(out.contains("发现 1 处出入"), "{out}");
        assert!(out.contains("退休副局长"), "{out}");
        assert!(out.contains("f_0001"), "{out}");
        assert!(out.contains("软提示"), "{out}");
    }

    #[test]
    fn profile_check_response_parses_fenced_and_empty() {
        let fenced = "```json\n{\"conflicts\":[],\"summary\":\"未见明显出入。\"}\n```";
        let report = parse_profile_check_response(fenced);
        assert!(report.conflicts.is_empty());
        assert_eq!(report.summary, "未见明显出入。");

        let out = format_profile_check("c_0001", "某角色", &report);
        assert!(out.contains("未见明显出入"), "{out}");

        // 完全无法解析 → 不 panic，summary 提示解析失败
        let report = parse_profile_check_response("这不是 JSON");
        assert!(report.conflicts.is_empty());
        assert!(report.summary.contains("解析失败"));
    }

    #[test]
    fn search_skips_corrupt_files() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        // 写入一个损坏的 TOML 文件
        std::fs::write(
            tmp.path().join("entities/characters/c_0099.toml"),
            "this is not valid toml {{{",
        )
        .unwrap();

        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool
            .execute(&serde_json::json!({ "query": "林风" }))
            .unwrap();
        // 正常文件仍能搜到
        assert!(result.contains("c_0001"));
        // 提示有解析错误
        assert!(result.contains("解析失败"));
    }

    #[test]
    fn search_empty_project() {
        let tmp = setup_test_project();
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let result = tool
            .execute(&serde_json::json!({ "query": "anything" }))
            .unwrap();
        assert!(result.contains("未找到"));
    }

    #[test]
    fn list_all_no_query() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        // 不传 query → 列出全部
        let empty_args = serde_json::Value::Object(serde_json::Map::new());
        let result = tool.execute(&empty_args).unwrap();
        assert!(
            result.contains("共 4 个实体"),
            "应列出全部 4 个实体，实际: {result}"
        );
        assert!(result.contains("林风"));
        assert!(result.contains("苏雨晴"));
        assert!(result.contains("青云城"));
        assert!(result.contains("寒霜剑"));
    }

    #[test]
    fn list_all_wildcard() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        // 传 * → 列出全部（通配符兼容）
        let result = tool.execute(&serde_json::json!({ "query": "*" })).unwrap();
        assert!(result.contains("共 4 个实体"));
    }

    #[test]
    fn list_all_with_type_filter() {
        let tmp = setup_test_project();
        write_search_fixtures(tmp.path());
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        // 不传 query + entity_type=characters → 只列角色
        let result = tool
            .execute(&serde_json::json!({ "entity_type": "characters" }))
            .unwrap();
        assert!(result.contains("林风"));
        assert!(result.contains("苏雨晴"));
        assert!(!result.contains("青云城")); // 地点不出现
        assert!(!result.contains("寒霜剑")); // 道具不出现
    }

    #[test]
    fn list_all_empty_project() {
        let tmp = setup_test_project();
        let tool = SearchEntitiesTool::new(ToolContext::new(tmp.path().to_path_buf()));
        // 空项目 + 不传 query → 明确提示没有实体文件
        let empty_args = serde_json::Value::Object(serde_json::Map::new());
        let result = tool.execute(&empty_args).unwrap();
        assert!(
            result.contains("没有任何实体文件"),
            "应提示项目没有实体，实际: {result}"
        );
    }

    /// 端到端：用 AI 的 create_character 真实创建角色，再搜索
    /// 模拟 AI 的实际工作流程，验证创建→搜索链路完整
    #[test]
    fn e2e_create_then_search() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());

        // 1. AI 通过工具创建角色（完全模拟真实调用）
        let create = CreateCharacterTool::new(ctx.clone());
        let created = create
            .execute(&serde_json::json!({
                "name": "叶知秋",
                "aliases": "叶先生,秋叔",
                "gender": "男",
                "description": "隐居的剑术宗师"
            }))
            .unwrap();
        assert!(created.contains("角色已创建"));

        // 确认文件确实写入了正确目录
        let char_dir = dir.join("entities/characters");
        let files: Vec<_> = std::fs::read_dir(&char_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "应有 1 个角色文件，实际: {files:?}");

        // 2. AI 搜索角色名
        let search = SearchEntitiesTool::new(ctx.clone());
        let result = search
            .execute(&serde_json::json!({ "query": "叶知秋" }))
            .unwrap();
        assert!(
            result.contains("叶知秋"),
            "应搜到角色名，实际输出: {result}"
        );

        // 3. AI 搜索别名
        let result = search
            .execute(&serde_json::json!({ "query": "秋叔" }))
            .unwrap();
        assert!(
            result.contains("叶知秋"),
            "应通过别名搜到，实际输出: {result}"
        );

        // 4. AI 用 entity_type 过滤
        let result = search
            .execute(&serde_json::json!({
                "query": "叶",
                "entity_type": "characters"
            }))
            .unwrap();
        assert!(result.contains("叶知秋"));

        // 5. 模糊搜索（部分匹配）
        let result = search
            .execute(&serde_json::json!({ "query": "叶" }))
            .unwrap();
        assert!(result.contains("叶知秋"));
    }

    // —— delete_entity ——

    #[test]
    fn delete_entity_removes_file() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_character(dir, "c_0001");
        let char_path = dir.join("entities/characters/c_0001.toml");
        assert!(char_path.exists());

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = DeleteEntityTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "entity_id": "c_0001" }))
            .unwrap();
        assert!(result.contains("已删除"));
        assert!(result.contains("c_0001"));
        // 文件确实被删除
        assert!(!char_path.exists());
    }

    #[test]
    fn delete_entity_nonexistent() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = DeleteEntityTool::new(ctx);
        let result = tool.execute(&serde_json::json!({ "entity_id": "c_9999" }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    #[test]
    fn delete_entity_invalid_id_prefix() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = DeleteEntityTool::new(ctx);
        // 无效前缀，mutations 层会拒绝
        let result = tool.execute(&serde_json::json!({ "entity_id": "x_0001" }));
        assert!(result.is_err());
    }

    // —— get_scene_context ——

    /// 写入极简角色档案（仅 id + display_name，避免 symbol 重复）
    fn write_minimal_character(dir: &Path, id: &str, display_name: &str) {
        let path = dir.join("entities/characters").join(format!("{id}.toml"));
        let content = format!(
            r#"id = "{id}"
display_name = "{display_name}"
aliases = []
"#
        );
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn get_scene_context_includes_chapter_characters() {
        let tmp = setup_test_project();
        let dir = tmp.path();

        // 两个角色：c_0001 是 POV，c_0002 是其他出场角色
        write_minimal_character(dir, "c_0001", "林风");
        write_minimal_character(dir, "c_0002", "苏婉");

        // structure.toml：一章一场景，pov 指向 c_0001
        let structure = r#"[book]
id = "b_root"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章 雨夜"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "客栈投宿"
pov = "c_0001"
mood = "紧张"
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        // chapters.toml：本章脉络标注涉及两个角色
        let chapters = r#"[[chapters]]
chapter_id = "ch_01"
title = "第一章 雨夜"
characters = ["c_0001", "c_0002"]
status = "writing"
"#;
        std::fs::write(dir.join("chapters.toml"), chapters).unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = GetSceneContextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();

        // POV 行带角色 ID
        assert!(result.contains("POV: 林风（c_0001）"), "got: {result}");
        // 本章涉及角色段存在，两个角色都出现
        assert!(result.contains("本章涉及角色"), "got: {result}");
        assert!(result.contains("林风"), "got: {result}");
        assert!(result.contains("苏婉"), "got: {result}");
        assert!(result.contains("c_0002"), "got: {result}");
        // POV 角色在涉及角色列表中带 [POV] 标记
        assert!(
            result.contains("林风（c_0001") && result.contains("[POV]"),
            "got: {result}"
        );
    }

    #[test]
    fn get_scene_context_without_chapters_toml() {
        // 没有 chapters.toml（或脉络中无匹配章节）时，不应输出"本章涉及角色"段
        let tmp = setup_test_project();
        let dir = tmp.path();

        write_minimal_character(dir, "c_0001", "林风");

        let structure = r#"[book]
id = "b_root"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
pov = "c_0001"
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        // chapters.toml 保持 init 后的空状态（无 [[chapters]]）

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = GetSceneContextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();

        assert!(result.contains("POV: 林风"), "got: {result}");
        assert!(
            !result.contains("本章涉及角色"),
            "无脉络时不应输出涉及角色段, got: {result}"
        );
    }

    #[test]
    fn get_scene_context_character_missing_profile() {
        // chapters.toml 标注的角色在 entities 中不存在时，标注"档案缺失"
        let tmp = setup_test_project();
        let dir = tmp.path();

        write_minimal_character(dir, "c_0001", "林风");
        // 注意：不创建 c_0003

        let structure = r#"[book]
id = "b_root"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
pov = "c_0001"
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        let chapters = r#"[[chapters]]
chapter_id = "ch_01"
title = "第一章"
characters = ["c_0001", "c_0003"]
status = "writing"
"#;
        std::fs::write(dir.join("chapters.toml"), chapters).unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = GetSceneContextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();

        assert!(result.contains("档案缺失"), "got: {result}");
        assert!(result.contains("c_0003"), "got: {result}");
    }

    #[test]
    fn create_same_name_entity_warns() {
        // 先建地点「山影城」，再建同名组织 → 合法双面实体：说明而非告警
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let location = CreateLocationTool::new(ctx.clone());
        let res = location
            .execute(&serde_json::json!({"name": "山影城"}))
            .unwrap();
        assert!(!res.contains("⚠"), "首个实体不应告警: {res}");

        let org = CreateOrganizationTool::new(ctx.clone());
        let res = org
            .execute(&serde_json::json!({"name": "山影城"}))
            .unwrap();
        assert!(
            !res.contains("⚠") && res.contains("双面实体"),
            "跨类型同名应说明双面实体而非告警: {res}"
        );

        // 同名角色与地点、组织都不同类型 → 同样是双面实体说明
        let ch = CreateCharacterTool::new(ctx.clone());
        let res = ch
            .execute(&serde_json::json!({"name": "山影城"}))
            .unwrap();
        assert!(!res.contains("⚠"), "跨类型同名角色不应告警: {res}");

        // 再建第二个同名地点 → 同类型撞名，真冲突，应告警
        let location2 = CreateLocationTool::new(ctx.clone());
        let res = location2
            .execute(&serde_json::json!({"name": "山影城"}))
            .unwrap();
        assert!(
            res.contains("⚠") && res.contains("同类型已存在同名实体"),
            "同类型重名应告警: {res}"
        );
    }

    #[test]
    fn get_entity_detail_counts_implicit_mentions() {
        // 正文隐式提及角色名（无显式锚点）→ 出场记录应有计数
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let create = CreateCharacterTool::new(ctx.clone());
        let res = create.execute(&serde_json::json!({"name": "林七"})).unwrap();
        let id = res.split("id=").nth(1).unwrap().split(',').next().unwrap().to_string();

        // 建章节/场景并写入提及该角色的片段（无显式锚点，靠隐式匹配）
        let paths = story_project_format::ProjectPaths::new(tmp.path());
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        crate::mutations::create_fragment(
            &paths,
            &sc,
            "林七推开门，屋里没人。林七又喊了一声。",
        )
        .unwrap();

        let detail = GetEntityDetailTool::new(ctx.clone());
        let out = detail.execute(&serde_json::json!({"entity_id": id})).unwrap();
        assert!(
            out.contains("出场(1 个片段)"),
            "隐式提及应计入出场: {out}"
        );
    }
}
