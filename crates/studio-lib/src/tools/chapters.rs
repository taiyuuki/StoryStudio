//! 章节脉络工具：chapters.toml 的设计与增删改查。
//!
//! 桥接全局大纲（outline.toml）与章节实现（structure.toml）：
//! 把大纲展开成逐章的叙事脉络（目标/节拍/涉及角色线索/章末钩子），
//! 并记录卷-章分组与每章创作进度（planned/writing/completed）。

use std::path::Path;

use story_ai_runtime::{Tool, ToolDef};
use story_project_format::{
    is_valid_status, load_chapters_unified, save_outline_volumes, ChapterOutlineDef, ChaptersFile,
    VolumeGroupDef,
};

use super::{
    check_index, get_index, get_json_array_opt, get_str, get_str_array_opt, get_str_opt,
    normalize_object_items, object_schema, ToolContext,
};

// ============================================================================
// design_chapters
// ============================================================================

pub struct DesignChaptersTool {
    ctx: ToolContext,
}
impl DesignChaptersTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DesignChaptersTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "design_chapters".into(),
            description: "根据大纲和卷结构设计章节脉络，一次性追加多章到 chapters.toml。每章含标题、所属卷序号、梗概、目标、节拍、涉及角色/线索、章末钩子等。设计阶段无需 chapter_id（虚拟章），动笔时再 create_chapter 落实。卷分组通常无需提供：章节按 volume_order 自动归卷，卷分组为空时自动从 outline.toml 的卷大纲导入。".into(),
            // chapters / volumes 的每项是对象数组：items 显式声明为 object
            // （裸 "type": "array" 无 items 时部分模型默认按字符串数组传，
            // 序列化字符串项会在读取 title/name 时整链报「缺少参数」）
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "chapters": {
                        "type": "array",
                        "description": "章节数组，每项必须是原生 JSON 对象（不要把对象序列化成字符串）",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "章节标题" },
                                "volume_order": { "type": "integer", "description": "所属卷序号（1-based；0=未分卷）" },
                                "summary": { "type": "string", "description": "可选：本章梗概" },
                                "goal": { "type": "string", "description": "可选：本章目标" },
                                "beats": { "type": "array", "items": { "type": "string" }, "description": "可选：节拍列表" },
                                "characters": { "type": "array", "items": { "type": "string" }, "description": "可选：涉及角色（角色 ID 或名称，写入时自动归一为 ID）" },
                                "threads": { "type": "array", "items": { "type": "string" }, "description": "可选：涉及线索（线索 ID 或标题，写入时自动归一为 ID）" },
                                "hook": { "type": "string", "description": "可选：章末钩子" },
                                "notes": { "type": "string", "description": "可选：备注" }
                            },
                            "required": ["title"]
                        }
                    },
                    "volumes": {
                        "type": "array",
                        "description": "可选：卷分组数组，每项必须是原生 JSON 对象。若提供则整体替换大纲卷结构（写入 outline.toml）。项目已有卷时无需传，章节按 volume_order 直接归卷",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "卷名" },
                                "order": { "type": "integer", "description": "卷序号（1-based）" },
                                "theme": { "type": "string", "description": "可选：卷主题" },
                                "summary": { "type": "string", "description": "可选：卷梗概" }
                            },
                            "required": ["name"]
                        }
                    },
                    "replace": {
                        "type": "boolean",
                        "description": "是否替换现有章节脉络（默认 false=追加）。传 true 会清空已有章节，慎用：已有脉络中存在已动笔或已关联物理章节的条目时，必须再传 confirm=true 才会执行（建议先向用户说明）"
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "replace=true 且将清空已动笔/已关联的脉络时，传 true 显式确认放弃这些设计记录（不可逆）"
                    }
                },
                "required": ["chapters"]
            }),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // 兼容原生数组与 JSON 字符串两种形态（模型常无视 schema 声明）；
        // 项级再做对象归一化——字符串项宽松解析为对象
        let chapter_inputs = match get_json_array_opt(arguments, "chapters")? {
            Some(items) => normalize_object_items(items, "chapters")?,
            None => return Err("缺少参数: chapters".to_string()),
        };
        let volumes_json = get_json_array_opt(arguments, "volumes")?
            .map(|items| normalize_object_items(items, "volumes"))
            .transpose()?;
        let replace = arguments
            .get("replace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if chapter_inputs.is_empty() {
            return Err("chapters 不能为空数组".into());
        }

        let (mut file, mut volumes, path) = load_chapters(&self.ctx)?;

        // 可选：替换卷分组（唯一真相源在 outline.toml）
        let mut volume_note = String::new();
        if let Some(vol_inputs) = volumes_json {
            let mut vols = Vec::with_capacity(vol_inputs.len());
            for v in &vol_inputs {
                vols.push(VolumeGroupDef {
                    name: get_str(v, "name")?,
                    order: v
                        .get("order")
                        .and_then(|o| o.as_u64())
                        .unwrap_or((vols.len() as u64) + 1) as u32,
                    theme: get_str_opt(v, "theme"),
                    summary: get_str_opt(v, "summary"),
                });
            }
            // 卷序号 = 排序后的数组序（保持连续），落盘到 outline.toml
            vols.sort_by_key(|v| v.order);
            for (i, v) in vols.iter_mut().enumerate() {
                v.order = (i + 1) as u32;
            }
            save_outline_volumes(&self.ctx.paths(), &vols)
                .map_err(|e| format!("写入卷结构失败: {e}"))?;
            volumes = vols;
            volume_note = format!("，卷分组 {} 个（已写入大纲）", volumes.len());
        }

        if replace {
            // 软拦截：待清空的脉络里有"已动笔"或"已关联物理章节"的条目时，
            // 需要显式 confirm=true（不可逆丢失节拍/钩子/进度，Agent 应先 ask_user）
            let at_risk: Vec<&ChapterOutlineDef> = file
                .chapters
                .iter()
                .filter(|c| {
                    c.chapter_id.is_some()
                        || c.status != story_project_format::chapters_file::STATUS_PLANNED
                })
                .collect();
            if !at_risk.is_empty()
                && !arguments
                    .get("confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                let titles: Vec<&str> = at_risk.iter().map(|c| c.title.as_str()).take(5).collect();
                return Err(format!(
                    "replace 将清空 {} 条脉络，其中 {} 条已动笔或已关联物理章节（{}{}），\
                     不可逆丢失节拍/钩子/进度。确认放弃请传 confirm=true（建议先向用户说明），\
                     或改用默认追加模式 / 先 delete_chapter_outline 逐条清理",
                    file.chapters.len(),
                    at_risk.len(),
                    titles.join("、"),
                    if at_risk.len() > 5 { " 等" } else { "" }
                ));
            }
            file.chapters.clear();
        }
        let start_index = file.chapters.len() + 1;
        let mut ref_warnings: Vec<String> = Vec::new();

        for input in &chapter_inputs {
            let title = get_str(input, "title")?;
            let volume_order = input
                .get("volume_order")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let beats = parse_str_array(input, "beats")?;
            // 角色/线索引用归一：Agent 常传名字/标题，消费侧按 ID 查找——
            // 此处统一解析，解析不到的保留原值并把警告带回回执
            let (characters, warns) = crate::mutations::resolve_character_refs(
                &self.ctx.paths(),
                &parse_str_array(input, "characters")?,
            );
            ref_warnings.extend(warns);
            let (threads, warns) = crate::mutations::resolve_thread_refs(
                &self.ctx.paths(),
                &parse_str_array(input, "threads")?,
            );
            ref_warnings.extend(warns);

            file.chapters.push(ChapterOutlineDef {
                chapter_id: get_str_opt(input, "chapter_id"),
                title,
                volume_order,
                summary: get_str_opt(input, "summary"),
                goal: get_str_opt(input, "goal"),
                beats,
                characters,
                threads,
                scenes: Vec::new(),
                hook: get_str_opt(input, "hook"),
                notes: get_str_opt(input, "notes"),
                status: "planned".to_string(),
            });
        }

        let added = chapter_inputs.len();
        let end_index = file.chapters.len();
        let ensure_note = save_chapters(&self.ctx, &path, &mut file, &mut volumes)?;
        if volume_note.is_empty() && !ensure_note.is_empty() {
            volume_note = format!("，{ensure_note}");
        }

        let mut msg = format!(
            "已设计 {} 个章节脉络（序号 {}-{}），已写入 chapters.toml{}。各章状态为 planned，动笔时用 update_chapter_outline(index, status=\"writing\") 标记。",
            added, start_index, end_index, volume_note
        );
        if !ref_warnings.is_empty() {
            msg.push_str(&format!("\n⚠️ 引用解析：{}", ref_warnings.join("；")));
        }
        Ok(msg)
    }
}

// ============================================================================
// get_chapter_outline
// ============================================================================

pub struct GetChapterOutlineTool {
    ctx: ToolContext,
}
impl GetChapterOutlineTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetChapterOutlineTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_chapter_outline".into(),
            description:
                "读取章节脉络（chapters.toml），按卷分组展示各章的目标/节拍/涉及角色线索/状态。\
            章节超过 20 章时默认列未完成章节与最近 15 章已完成；可用 volume_order 按卷、\
            status 按状态过滤查看指定范围。写新章节前调用以获取本章设计意图。"
                    .into(),
            parameters: object_schema(vec![
                (
                    "volume_order",
                    "string",
                    "可选：只看某卷（1-based 卷序号）",
                    false,
                ),
                (
                    "status",
                    "string",
                    "可选：只看某状态（planned/writing/completed）",
                    false,
                ),
                (
                    "include_completed",
                    "boolean",
                    "是否包含已完成章节（默认 true）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let volume_filter = get_str_opt(arguments, "volume_order")
            .map(|s| s.parse::<u32>())
            .transpose()
            .map_err(|_| "volume_order 必须是正整数".to_string())?;
        let status_filter = get_str_opt(arguments, "status");
        if let Some(ref s) = status_filter {
            if !is_valid_status(s) {
                return Err(format!(
                    "status 必须是 planned/writing/completed，收到 '{s}'"
                ));
            }
        }
        let include_completed = arguments
            .get("include_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let (file, volumes, _path) = load_chapters(&self.ctx)?;

        if file.chapters.is_empty() && volumes.is_empty() {
            return Ok("章节脉络为空。使用 design_chapters 根据大纲设计章节脉络。".to_string());
        }

        // 卷序号 → 卷名
        let volume_name = |order: u32| -> String {
            volumes
                .iter()
                .find(|v| v.order == order)
                .map(|v| v.name.clone())
                .unwrap_or_else(|| format!("卷 {order}"))
        };

        let mut result = String::new();

        // 卷分组概览
        if !volumes.is_empty() {
            result.push_str("## 卷分组\n");
            for v in &volumes {
                result.push_str(&format!("{}. {}", v.order, v.name));
                if let Some(ref t) = v.theme {
                    result.push_str(&format!(" [主题: {t}]"));
                }
                result.push('\n');
                if let Some(ref s) = v.summary {
                    result.push_str(&format!("   {s}\n"));
                }
            }
            result.push('\n');
        }

        // 长篇智能默认：无任何过滤且章节很多时，只给「未完成 + 最近 15 章已完成」，
        // 防止几百章的脉络（每章含节拍/角色/钩子）一次性灌满上下文
        const SMART_THRESHOLD: usize = 20;
        const RECENT_COMPLETED_SHOWN: usize = 15;
        const MAX_PENDING_SHOWN: usize = 30;

        let smart_default = volume_filter.is_none()
            && status_filter.is_none()
            && include_completed
            && file.chapters.len() > SMART_THRESHOLD;

        let filtered: Vec<(usize, &ChapterOutlineDef)> = if smart_default {
            let recent_done: std::collections::HashSet<usize> = file
                .chapters
                .iter()
                .enumerate()
                .filter(|(_, c)| c.status == "completed")
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
                .iter()
                .rev()
                .take(RECENT_COMPLETED_SHOWN)
                .copied()
                .collect();
            let mut pending_shown = 0usize;
            file.chapters
                .iter()
                .enumerate()
                .filter(|(i, c)| c.status != "completed" || recent_done.contains(i))
                .filter(|(_, c)| {
                    // 未完成章节也可能很多（如刚批量设计完），同样限量
                    c.status == "completed" || {
                        pending_shown += 1;
                        pending_shown <= MAX_PENDING_SHOWN
                    }
                })
                .collect()
        } else {
            file.chapters
                .iter()
                .enumerate()
                .filter(|(_, c)| volume_filter.is_none_or(|vo| c.volume_order == vo))
                .filter(|(_, c)| status_filter.as_ref().is_none_or(|s| &c.status == s))
                .filter(|(_, c)| include_completed || c.status != "completed")
                .collect()
        };

        if filtered.is_empty() {
            let mut note = "没有匹配的章节脉络".to_string();
            if let Some(vo) = volume_filter {
                note.push_str(&format!("（卷 {vo}）"));
            }
            if let Some(ref s) = status_filter {
                note.push_str(&format!("（状态 {s}）"));
            }
            if !include_completed {
                note.push_str("（已排除 completed）");
            }
            return Ok(note);
        }

        result.push_str("## 章节脉络\n");
        for (idx, c) in &filtered {
            let status_mark = match c.status.as_str() {
                "completed" => "[完成]",
                "writing" => "[写作中]",
                _ => "[待写]",
            };
            result.push_str(&format!("{}. {} {}", idx + 1, c.title, status_mark));
            if c.volume_order > 0 {
                result.push_str(&format!("（{}）", volume_name(c.volume_order)));
            }
            if let Some(ref id) = c.chapter_id {
                result.push_str(&format!(" [{id}]"));
            }
            result.push('\n');

            if let Some(ref s) = c.summary {
                result.push_str(&format!("   梗概: {s}\n"));
            }
            if let Some(ref g) = c.goal {
                result.push_str(&format!("   目标: {g}\n"));
            }
            if !c.beats.is_empty() {
                result.push_str(&format!("   节拍: {}\n", c.beats.join(" → ")));
            }
            if !c.characters.is_empty() {
                result.push_str(&format!("   角色: {}\n", c.characters.join(", ")));
            }
            if !c.threads.is_empty() {
                result.push_str(&format!("   线索: {}\n", c.threads.join(", ")));
            }
            if let Some(ref h) = c.hook {
                result.push_str(&format!("   钩子: {h}\n"));
            }
            if let Some(ref n) = c.notes {
                result.push_str(&format!("   备注: {n}\n"));
            }
        }

        // 进度概览（无过滤时显示）
        if volume_filter.is_none() && status_filter.is_none() && include_completed {
            let total = file.chapters.len();
            let completed = file
                .chapters
                .iter()
                .filter(|c| c.status == "completed")
                .count();
            let writing = file
                .chapters
                .iter()
                .filter(|c| c.status == "writing")
                .count();
            let planned = file
                .chapters
                .iter()
                .filter(|c| c.status == "planned")
                .count();
            let next = file.chapters.iter().find(|c| c.status == "planned");
            result.push_str(&format!(
                "\n## 进度: {completed}/{total} 完成, {writing} 写作中, {planned} 待写\
                 （脉络规划信息，写作范围与量级以用户指令为准）\n",
            ));
            if let Some(n) = next {
                result.push_str(&format!("下一章: {}\n", n.title));
            }
        }

        // 智能默认模式的省略说明：引导按卷 / 按状态分批取
        if smart_default {
            let omitted = file.chapters.len() - filtered.len();
            result.push_str(&format!(
                "\n📖 共 {} 章脉络，本次省略 {} 章（多为早期已完成章节）。\
                 需要早期内容时用 volume_order 按卷或 status 过滤分批查看，不要一次取全量。\n",
                file.chapters.len(),
                omitted,
            ));
        }

        Ok(result.trim_end().to_string())
    }
}

// ============================================================================
// update_chapter_outline
// ============================================================================

pub struct UpdateChapterOutlineTool {
    ctx: ToolContext,
}
impl UpdateChapterOutlineTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateChapterOutlineTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_chapter_outline".into(),
            description: "修改单章脉络（用 index 指定，1-based，对应 get_chapter_outline 显示的编号）。可更新标题、梗概、目标、节拍、涉及角色/线索、钩子、备注、章节 ID、状态。status 仅允许 planned→writing→completed 单向推进。".into(),
            parameters: object_schema(vec![
                ("index", "integer", "章序号（1-based）", true),
                ("title", "string", "章节标题", false),
                ("volume_order", "string", "所属卷序号（1-based，0=未分卷）", false),
                ("summary", "string", "章节梗概", false),
                ("goal", "string", "本章目标", false),
                ("beats", "array", "节拍列表（如 [\"登场\",\"冲突\"]；逗号分隔字符串亦可），整体替换", false),
                ("characters", "array", "涉及角色（ID 或名称，写入时自动归一为 ID）列表，整体替换", false),
                ("threads", "array", "涉及线索（ID 或标题，写入时自动归一为 ID）列表，整体替换", false),
                ("hook", "string", "章末钩子", false),
                ("notes", "string", "其他备注", false),
                ("chapter_id", "string", "对齐 structure.toml 的章节 ID（ch_XX）；传空字符串清除", false),
                ("status", "string", "创作进度: planned/writing/completed（仅单向推进）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let index = get_index(arguments, "index")?;

        let (mut file, mut volumes, path) = load_chapters(&self.ctx)?;
        check_index("章节", index, file.chapters.len())?;
        let item = &mut file.chapters[index - 1];

        let mut changes: Vec<&str> = Vec::new();
        let mut ref_warnings: Vec<String> = Vec::new();

        if let Some(v) = get_str_opt(arguments, "title") {
            item.title = v;
            changes.push("title");
        }
        if let Some(v) = get_str_opt(arguments, "volume_order") {
            let n: u32 = v
                .parse()
                .map_err(|_| format!("volume_order 必须是非负整数，收到 '{v}'"))?;
            item.volume_order = n;
            changes.push("volume_order");
        }
        if let Some(v) = get_str_opt(arguments, "summary") {
            item.summary = Some(v);
            changes.push("summary");
        }
        if let Some(v) = get_str_opt(arguments, "goal") {
            item.goal = Some(v);
            changes.push("goal");
        }
        if let Some(v) = get_str_array_opt(arguments, "beats")? {
            item.beats = v;
            changes.push("beats");
        }
        if let Some(v) = get_str_array_opt(arguments, "characters")? {
            // 名字/ID 统一归一为角色 ID（消费侧按 ID 查找）
            let (resolved, warns) = crate::mutations::resolve_character_refs(&self.ctx.paths(), &v);
            ref_warnings.extend(warns);
            item.characters = resolved;
            changes.push("characters");
        }
        if let Some(v) = get_str_array_opt(arguments, "threads")? {
            // 标题/ID 统一归一为线索 ID（消费侧按 ID 查找）
            let (resolved, warns) = crate::mutations::resolve_thread_refs(&self.ctx.paths(), &v);
            ref_warnings.extend(warns);
            item.threads = resolved;
            changes.push("threads");
        }
        if let Some(v) = get_str_opt(arguments, "hook") {
            item.hook = Some(v);
            changes.push("hook");
        }
        if let Some(v) = get_str_opt(arguments, "notes") {
            item.notes = Some(v);
            changes.push("notes");
        }
        if let Some(raw) = arguments.get("chapter_id").and_then(|v| v.as_str()) {
            // 空字符串清除
            item.chapter_id = if raw.is_empty() {
                None
            } else {
                Some(raw.to_string())
            };
            changes.push("chapter_id");
        }
        if let Some(v) = get_str_opt(arguments, "status") {
            if !is_valid_status(&v) {
                return Err(format!(
                    "status 必须是 planned/writing/completed，收到 '{v}'"
                ));
            }
            // 单向推进校验
            let order = |s: &str| -> u8 {
                match s {
                    "planned" => 0,
                    "writing" => 1,
                    "completed" => 2,
                    _ => 0,
                }
            };
            if order(&v) < order(&item.status) {
                return Err(format!(
                    "状态仅允许单向推进（planned → writing → completed）：当前为 {}，不能改为 {}",
                    item.status, v
                ));
            }
            item.status = v;
            changes.push("status");
        }

        if changes.is_empty() {
            return Ok("未提供任何需要更新的字段。".to_string());
        }

        let ensure_note = save_chapters(&self.ctx, &path, &mut file, &mut volumes)?;

        let mut msg = format!(
            "章节脉络已修改: index={index}「{}」, 更新字段: {}",
            file.chapters[index - 1].title,
            changes.join(", ")
        );
        if !ref_warnings.is_empty() {
            msg.push_str(&format!("\n⚠️ 引用解析：{}", ref_warnings.join("；")));
        }
        if !ensure_note.is_empty() {
            msg.push_str(&format!("；{ensure_note}"));
        }
        Ok(msg)
    }
}

// ============================================================================
// delete_chapter_outline
// ============================================================================

pub struct DeleteChapterOutlineTool {
    ctx: ToolContext,
}
impl DeleteChapterOutlineTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteChapterOutlineTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_chapter_outline".into(),
            description: "删除单章脉络（用 index 指定，1-based）。仅删除 chapters.toml 中的设计记录，不影响 structure.toml 的章节实现。".into(),
            parameters: object_schema(vec![
                ("index", "integer", "章序号（1-based，对应 get_chapter_outline 显示的编号）", true),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let index = get_index(arguments, "index")?;

        let (mut file, mut volumes, path) = load_chapters(&self.ctx)?;
        check_index("章节", index, file.chapters.len())?;
        let removed = file.chapters.remove(index - 1);
        let ensure_note = save_chapters(&self.ctx, &path, &mut file, &mut volumes)?;

        let mut msg = format!(
            "已删除章节脉络: index={index}「{}」，剩余 {} 个（注意：未触碰 structure.toml）",
            removed.title,
            file.chapters.len()
        );
        // 双轨提示：被删脉络若已关联物理章节，设计记录没了但实现仍在
        if let Some(ch_id) = &removed.chapter_id {
            msg.push_str(&format!(
                "；⚠️ 该脉络原关联物理章节 {ch_id}（structure.toml 与正文保留不动，\
                如需一并删除请用 delete_chapter）"
            ));
        }
        if !ensure_note.is_empty() {
            msg.push_str(&format!("；{ensure_note}"));
        }
        Ok(msg)
    }
}

// ============================================================================
// populate_scenes_from_beats
// ============================================================================

/// 节拍标题头：前缀标记优先，无标记退回字数窗口断句。
///
/// 1. `【标题】…` / `[标题]…`：包裹段是作者显式给出的标题，直接提取；
/// 2. `标题：…`（全角/半角冒号）：冒号前 ≤12 字且不含句末标点视为标题；
/// 3. 在 8..=12 字窗口内的标点处断句（去掉断点标点），无标点才按
///    12 字符截断——避免「年轻审判官（伊莲娜，正文」这类词中腰斩。
fn beat_title_head(beat: &str) -> String {
    let src = beat.trim();
    // 前缀括号标记：内嵌段过长（>16 字）时对内嵌段继续走通用规则
    if let Some(inner) = bracket_marker_head(src) {
        if inner.chars().count() <= 16 {
            return inner.to_string();
        }
        return beat_title_head(inner);
    }
    if let Some(head) = colon_marker_head(src) {
        return head;
    }
    window_break_head(src)
}

/// 提取前缀括号标记 `【…】` / `[…]` 的内嵌段（须在开头，取首个闭合括号）。
fn bracket_marker_head(src: &str) -> Option<&str> {
    let (open, close) = match src.chars().next()? {
        '【' => ('【', '】'),
        '[' => ('[', ']'),
        _ => return None,
    };
    let close_pos = src[open.len_utf8()..].find(close)? + open.len_utf8();
    let inner = &src[open.len_utf8()..close_pos];
    (!inner.trim().is_empty()).then(|| inner.trim())
}

/// 提取 `标题：描述` 形态的标题：冒号在开头或太靠后（>12 字）、
/// 或冒号前含句末标点（正文句子的冒号，如引语）时不视为标记。
fn colon_marker_head(src: &str) -> Option<String> {
    let chars: Vec<char> = src.chars().collect();
    let pos = chars.iter().position(|c| *c == '：' || *c == ':')?;
    if pos == 0 || pos > 12 {
        return None;
    }
    let head: String = chars[..pos].iter().collect();
    let head = head.trim();
    if head.is_empty() || head.chars().any(|c| "。！？!?；;".contains(c)) {
        return None;
    }
    Some(head.to_string())
}

fn window_break_head(src: &str) -> String {
    const BREAKS: &[char] = &['，', '。', '、', '；', '：', ',', '（', '(', '—', ' '];
    let chars: Vec<char> = src.chars().collect();
    let max = 12.min(chars.len());
    for end in 8..=max {
        if BREAKS.contains(&chars[end - 1]) {
            return chars[..end - 1].iter().collect();
        }
    }
    chars[..max].iter().collect()
}

/// auto_titles 的模型提炼：一次批量调用为所有待命名节拍生成 4-6 字短标题。
///
/// 只负责调用与解析；失败由调用方兜底（回退 beat_title_head）。返回项数
/// 与节拍数一致，空串/超长等不合格项由调用方经 sanitize_llm_title 逐个回退。
fn generate_beat_titles(
    provider: &std::sync::Arc<dyn story_ai_runtime::provider::AiProvider>,
    beats: &[String],
) -> Result<Vec<String>, String> {
    use story_ai_runtime::provider::{GenerateRequest, Message};
    let numbered: Vec<String> = beats
        .iter()
        .enumerate()
        .map(|(i, b)| format!("{}. {b}", i + 1))
        .collect();
    let system = "你是网文场景命名器。为每个节拍提炼一个 4-6 字的场景短标题：\
概括该节拍的事件核心，动宾或偏正结构，不带标点、不带「第N幕」类前缀、\
不照抄节拍开头。只输出一个 JSON 字符串数组（顺序与数量同节拍），\
如 [\"遗蜕留书\",\"逍遥殿议事\"]，不要输出其他内容。";
    let request = GenerateRequest {
        system: system.into(),
        messages: vec![Message::user(numbered.join("\n"))],
        model: String::new(),
        temperature: 0.3,
        max_tokens: 48 * beats.len().max(1) + 64,
        tools: Vec::new(),
    };
    let resp = super::retry_generate(provider, &request)?;
    let raw = crate::llm_json::strip_think(&resp.text);
    crate::llm_json::ensure_not_truncated(resp.stop_reason, request.max_tokens as u32)?;
    let arr = crate::llm_json::parse_array(&raw)?;
    let titles: Vec<String> = arr
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    if titles.len() != beats.len() {
        return Err(format!(
            "标题数量不符（期望 {} 个，得到 {} 个）",
            beats.len(),
            titles.len()
        ));
    }
    Ok(titles)
}

/// 模型标题的落地清洗：去包裹引号/书名号、压平空白、超 12 字截断、空则 None
/// （None 位由调用方回退 beat_title_head）。
fn sanitize_llm_title(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c: char| {
        matches!(
            c,
            '"' | '“' | '”' | '「' | '」' | '『' | '』' | '《' | '》' | '\''
        )
    });
    let collapsed: String = trimmed.split_whitespace().collect();
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(12).collect())
}

pub struct PopulateScenesFromBeatsTool {
    ctx: ToolContext,
}
impl PopulateScenesFromBeatsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for PopulateScenesFromBeatsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "populate_scenes_from_beats".into(),
            description: "按章节脉络的节拍（beats）一键生成场景骨架：每个节拍创建一个待写场景\n            （叙事功能存完整节拍文本），并同步到脉络的 scenes 列表。\n            场景标题按优先级取：titles 显式指定（推荐——断句启发式产不出「遗蜕留书」这类语义标题）＞\n            节拍前缀标记（【标题】… / [标题]… / 标题：…）＞ 8-12 字窗口断句；\n            也可传 auto_titles=true 让模型从节拍批量提炼 4-6 字短标题（无 provider 或失败自动回退断句）。\n            POV 默认取本章脉络的第一个角色（本章主咖），可用 default_pov 覆盖。\n            前提：该章已 create_chapter 落实（chapter_id 已回填）。\n            生成的是骨架，写正文仍用 write_fragment_text 逐场景推进。"
                .into(),
            parameters: object_schema(vec![
                (
                    "index",
                    "integer",
                    "章序号（1-based，对应 get_chapter_outline 显示的编号）",
                    true,
                ),
                (
                    "titles",
                    "array",
                    "可选（推荐）：显式指定本次新生成场景的标题（字符串数组，按序对应第 i 个新场景；推荐 4-6 字语义短标题，如「遗蜕留书」，无需带「第N幕·」前缀）。提供数少于新场景时其余按节拍自动提取，多余忽略",
                    false,
                ),
                (
                    "auto_titles",
                    "boolean",
                    "可选：让模型从节拍文本批量提炼 4-6 字场景短标题（一次 LLM 调用；无 provider 或调用失败自动回退节拍断句）。显式 titles 对应位置优先于模型提炼。默认 false",
                    false,
                ),
                (
                    "default_pov",
                    "string",
                    "可选：新建场景的默认 POV（角色 ID 或名称，名称自动归一为 ID）。未传时自动取本章脉络的第一个角色（本章主咖）；本章无角色则不设 POV",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let index = get_index(arguments, "index")?;
        let titles = get_str_array_opt(arguments, "titles")?;
        let default_pov_input = get_str_opt(arguments, "default_pov");
        let paths = self.ctx.paths();

        let (file, _volumes, _path) = load_chapters(&self.ctx)?;
        check_index("章节", index, file.chapters.len())?;
        let entry = &file.chapters[index - 1];
        let chapter_id = entry.chapter_id.clone().ok_or_else(|| {
            format!(
                "第 {index} 章「{}」还是虚拟章（无 chapter_id），先 create_chapter 落实后再生成场景骨架",
                entry.title
            )
        })?;
        if entry.beats.is_empty() {
            return Err(format!(
                "第 {index} 章「{}」没有节拍（beats），无从而生成；先 update_chapter_outline 补节拍",
                entry.title
            ));
        }
        let beats = entry.beats.clone();
        let title = entry.title.clone();
        // 已生成的骨架数量（按脉络 scenes 记录，幂等基准）
        let existing = file.chapters[index - 1].scenes.len();

        // POV 优先级：显式 default_pov（名字归一为 ID）> 本章脉络第一个角色（本章主咖）
        let mut notes: Vec<String> = Vec::new();
        let default_pov = match &default_pov_input {
            Some(raw) => {
                let (resolved, warns) =
                    crate::mutations::resolve_character_refs(&paths, std::slice::from_ref(raw));
                notes.extend(warns);
                Some(resolved[0].clone())
            }
            None => entry.characters.first().cloned(),
        };

        let new_count = beats.len() - existing;
        if let Some(ts) = &titles {
            if ts.len() < new_count {
                notes.push(format!(
                    "titles 提供 {} 个，本次新建 {} 个场景，其余按节拍自动提取标题",
                    ts.len(),
                    new_count
                ));
            } else if ts.len() > new_count {
                notes.push(format!(
                    "titles 提供 {} 个，超出本次新建的 {} 个，多余忽略",
                    ts.len(),
                    new_count
                ));
            }
        }

        // auto_titles：标题由模型从节拍提炼（opt-in）。模型一律看到本章全部
        // 新节拍——标题要放在整章语境里统筹（避免同章标题撞车），显式 titles
        // 覆盖的位置不采用其结果。失败/缺项逐个回退 beat_title_head，不阻断
        // 骨架生成——截断启发式产不出「遗蜕留书」这类语义标题，但也不能让
        // 标题生成拖垮结构工具的确定性。
        let auto_titles = arguments
            .get("auto_titles")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut llm_titles: Vec<Option<String>> = vec![None; new_count];
        if auto_titles && new_count > 0 {
            let all_new_beats: Vec<String> = beats[existing..].to_vec();
            match self.ctx.provider.as_ref() {
                Some(provider) => match generate_beat_titles(provider, &all_new_beats) {
                    Ok(list) => {
                        let mut used = 0;
                        for (i, raw) in list.iter().enumerate() {
                            let explicit = titles
                                .as_ref()
                                .and_then(|ts| ts.get(i))
                                .is_some_and(|s| !s.trim().is_empty());
                            if explicit {
                                continue;
                            }
                            if let Some(t) = sanitize_llm_title(raw) {
                                llm_titles[i] = Some(t);
                                used += 1;
                            }
                        }
                        if used > 0 {
                            notes.push(format!(
                                "auto_titles：{used}/{new_count} 个新场景标题由模型提炼（显式 titles 优先），其余回退节拍断句"
                            ));
                        } else {
                            notes.push("auto_titles 返回标题均为空，全部回退节拍断句".into());
                        }
                    }
                    Err(e) => notes.push(format!("auto_titles 调用失败，标题回退节拍断句: {e}")),
                },
                None => {
                    notes.push("auto_titles 需要 provider（当前未配置），标题按节拍断句提取".into())
                }
            }
        }

        let mut created = Vec::new();
        for (i, beat) in beats.iter().skip(existing).enumerate() {
            // 标题优先级：显式 titles > 模型提炼（auto_titles）> 前缀标记/窗口断句
            let head = titles
                .as_ref()
                .and_then(|ts| ts.get(i))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| llm_titles.get(i).cloned().flatten())
                .unwrap_or_else(|| beat_title_head(beat));
            let scene_title = format!("第{}幕·{}", existing + i + 1, head);
            let scene_id = crate::mutations::create_scene(
                &paths,
                &chapter_id,
                &scene_title,
                default_pov.as_deref(),
            )
            .map_err(|e| e.to_string())?;
            // 叙事功能存完整节拍文本，写作时上下文自含
            let update = crate::mutations::SceneUpdate {
                title: None,
                pov: None,
                mood: None,
                function: Some(Some(beat.as_str())),
                summary: None,
                story_time: None,
            };
            let _ = crate::mutations::update_scene(&paths, &scene_id, &update);
            crate::mutations::append_chapter_outline_scene(&paths, index, &scene_id)
                .map_err(|e| e.to_string())?;
            created.push(format!("{scene_id}「{scene_title}」"));
        }

        if created.is_empty() {
            Ok(format!(
                "第 {index} 章「{title}」的 {} 个节拍均已生成过场景（{existing} 个），无新增",
                beats.len()
            ))
        } else {
            let mut msg = format!(
                "第 {index} 章「{title}」按 {} 个节拍生成 {} 个场景骨架：{}",
                beats.len(),
                created.len(),
                created.join("、")
            );
            if let Some(pov) = &default_pov {
                let src = if default_pov_input.is_some() {
                    "指定"
                } else {
                    "本章主咖"
                };
                msg.push_str(&format!("；默认 POV: {pov}（{src}）"));
            } else {
                msg.push_str("；未设 POV（本章脉络无角色且未传 default_pov）");
            }
            for note in notes {
                msg.push_str(&format!("；⚠️ {note}"));
            }
            Ok(msg)
        }
    }
}

// ============================================================================
// helpers
// ============================================================================

/// 读取 chapters.toml，返回 (ChaptersFile, 统一卷列表, path)。
/// 卷的唯一真相源是 outline.toml：读取时经 load_chapters_unified
/// 完成遗留副本的合并迁移（含章节 volume_order 与线索 due_volume 重映射）。
/// 文件不存在或为空时返回 default。
fn load_chapters(
    ctx: &ToolContext,
) -> Result<(ChaptersFile, Vec<VolumeGroupDef>, std::path::PathBuf), String> {
    let path = ctx.paths().chapters_toml();
    let (file, volumes) = load_chapters_unified(&ctx.paths());
    Ok((file, volumes, path))
}

/// 卷占位兜底（保存前调用）：章节引用的卷序号超出大纲卷结构时，
/// 补齐「第N卷」占位卷（统一卷序连续，中间缺号也一并补上），
/// 避免 AI 的归属意图静默沉入「未分卷」。用户可在脉络面板改名。
/// 返回 (给 AI 的说明文字, 是否修改了卷列表)。
fn ensure_volume_groups(volumes: &mut Vec<VolumeGroupDef>, file: &ChaptersFile) -> (String, bool) {
    let max_ref = file
        .chapters
        .iter()
        .map(|c| c.volume_order)
        .filter(|o| *o > 0)
        .max()
        .unwrap_or(0);
    let have = volumes.len() as u32;
    if max_ref <= have {
        return (String::new(), false);
    }

    let mut added = Vec::new();
    for order in (have + 1)..=max_ref {
        added.push(format!("第{order}卷"));
        volumes.push(VolumeGroupDef {
            name: format!("第{order}卷"),
            order,
            theme: None,
            summary: None,
        });
    }
    (
        format!("章节引用了未定义的卷，已补建占位卷: {}", added.join("、")),
        true,
    )
}

/// 将 ChaptersFile 写回 chapters.toml；卷列表若有补齐则同步落盘 outline.toml。
/// 返回兜底说明文字（无动作时为空串）。
fn save_chapters(
    ctx: &ToolContext,
    path: &Path,
    file: &mut ChaptersFile,
    volumes: &mut Vec<VolumeGroupDef>,
) -> Result<String, String> {
    let (note, volumes_dirty) = ensure_volume_groups(volumes, file);
    if volumes_dirty {
        save_outline_volumes(&ctx.paths(), volumes).map_err(|e| format!("写入卷结构失败: {e}"))?;
    }
    let toml_content = file
        .to_toml_string()
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(path, toml_content).map_err(|e| format!("写入失败: {e}"))?;
    Ok(note)
}

/// 从 JSON Value 中读取字符串数组字段
fn parse_str_array(obj: &serde_json::Value, key: &str) -> Result<Vec<String>, String> {
    match obj.get(key) {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("{key} 数组元素必须是字符串"))
            })
            .collect(),
        Some(other) => Err(format!("{key} 必须是字符串数组，收到 {other}")),
    }
}

/// 解析 JSON 字符串形式的字符串数组（宽松解析）
/// 统一走 [`crate::llm_json`] 的容错梯度：剥思考链 → 原文 → 剥 ```json 围栏 →
/// 截取首个 '[' 到最后一个 ']' → 修复字符串内部未转义的 ASCII 引号。
pub(crate) fn parse_json_array_lenient(
    raw: &str,
    field: &str,
) -> Result<Vec<serde_json::Value>, String> {
    crate::llm_json::parse_array(raw).map_err(|e| format!("{field}{e}"))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ToolContext) {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        (tmp, ctx)
    }

    /// 脉络引用归一（回归：threads/characters 传标题/名字时被原样存储，
    /// 消费侧按 ID 查找 → get_scene_context 报「threads.toml 中不存在」、
    /// prepare_writing 角色速查静默缺人）。
    #[test]
    fn design_and_update_resolve_thread_and_character_refs() {
        let (_tmp, ctx) = setup();
        let paths = ctx.paths();

        // 夹具：一条线索 + 一个角色（threads.toml 的 serde 字段名是单数 [[thread]]）
        std::fs::write(
            paths.threads_toml(),
            "[[thread]]\nid = \"t_001\"\ntitle = \"旧案之谜\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(paths.characters_dir()).unwrap();
        std::fs::write(
            paths.characters_dir().join("c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"林风\"\n",
        )
        .unwrap();

        // design：标题/名字/ID/未知混传
        let res = DesignChaptersTool::new(ctx.clone())
            .execute(&serde_json::json!({
                "chapters": [{
                    "title": "雨夜",
                    "characters": ["林风", "路人甲"],
                    "threads": ["旧案之谜", "t_001", "不存在之谜"]
                }]
            }))
            .unwrap();
        assert!(res.contains("不存在之谜"), "未知线索应在回执警告: {res}");
        assert!(res.contains("路人甲"), "未知角色应在回执警告: {res}");

        let content = std::fs::read_to_string(paths.chapters_toml()).unwrap();
        assert!(
            content.contains("\"t_001\""),
            "线索标题应归一为 ID: {content}"
        );
        assert!(
            content.contains("\"c_0001\""),
            "角色名应归一为 ID: {content}"
        );
        assert!(
            !content.contains("\"旧案之谜\""),
            "已解析的标题不应残留: {content}"
        );
        assert!(
            content.contains("\"不存在之谜\""),
            "未知引用按原样保留（不静默丢数据）: {content}"
        );

        // update：同样归一
        UpdateChapterOutlineTool::new(ctx.clone())
            .execute(&serde_json::json!({
                "index": 1,
                "threads": ["旧案之谜"],
                "characters": ["林风"]
            }))
            .unwrap();
        let content = std::fs::read_to_string(paths.chapters_toml()).unwrap();
        assert!(
            content.contains("\"t_001\""),
            "update 路径也应归一线索标题: {content}"
        );
        assert!(
            content.contains("\"c_0001\""),
            "update 路径也应归一角色名: {content}"
        );
    }

    #[test]
    fn long_outline_defaults_to_smart_scope() {
        let (_tmp, ctx) = setup();
        // 25 章：前 18 章已完成、后 7 章待写 → 触发智能默认（阈值 20）
        let mut toml = String::new();
        for i in 1..=25 {
            let status = if i <= 18 { "completed" } else { "planned" };
            toml.push_str(&format!(
                "[[chapters]]\ntitle = \"第{i}章\"\nvolume_order = 1\nstatus = \"{status}\"\nsummary = \"梗概{i}\"\n\n"
            ));
        }
        toml.push_str("[[volumes]]\nname = \"第一卷\"\norder = 1\n");
        std::fs::write(ctx.paths().chapters_toml(), toml).unwrap();

        let tool = GetChapterOutlineTool::new(ctx.clone());

        // 默认：只显示 7 个待写 + 最近 15 个已完成，早期 3 章被省略并给出提示
        let out = tool.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("省略 3 章"), "应省略 18-15=3 章: {out}");
        assert!(!out.contains("第1章"), "最早的已完成章应被省略: {out}");
        assert!(out.contains("第4章"), "最近 15 个已完成应保留: {out}");
        assert!(out.contains("第25章"), "待写章节应全部保留: {out}");
        assert!(out.contains("不要一次取全量"), "应引导分批查看: {out}");

        // 显式按状态过滤：不受智能默认影响，能拿到全部 completed
        let out = tool
            .execute(&serde_json::json!({"status": "completed"}))
            .unwrap();
        assert!(out.contains("第1章"), "status 过滤应返回全部已完成: {out}");
        assert!(!out.contains("省略"), "{out}");
    }

    #[test]
    fn short_outline_unchanged() {
        let (tmp, ctx) = setup();
        std::fs::write(
            ctx.paths().chapters_toml(),
            "[[chapters]]\ntitle = \"第一章\"\nvolume_order = 1\n",
        )
        .unwrap();
        let out = GetChapterOutlineTool::new(ctx.clone())
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("第一章"));
        assert!(!out.contains("省略"), "短脉络不应触发智能默认: {out}");
        drop(tmp);
    }

    /// 原生数组形态的 characters/threads 必须真正落盘（回归：字符串单形态入口
    /// 曾把原生数组静默吞掉，导致脉络/场景上下文里角色与线索全部缺席）。
    #[test]
    fn update_chapter_outline_native_array_persists() {
        let (_tmp, ctx) = setup();
        DesignChaptersTool::new(ctx.clone())
            .execute(&serde_json::json!({
                // chapters 也用原生数组形态
                "chapters": [
                    {"title": "雨夜", "volume_order": 1, "chapter_id": "ch_01"}
                ]
            }))
            .unwrap();

        // 模型无视 schema 传原生数组（事故形态）
        let update = UpdateChapterOutlineTool::new(ctx.clone());
        let res = update
            .execute(&serde_json::json!({
                "index": 1,
                "characters": ["c_0001", "c_0005"],
                "threads": ["t_004", "t_007"],
            }))
            .unwrap();
        assert!(res.contains("characters"), "回执应列出更新字段: {res}");
        assert!(res.contains("threads"), "回执应列出更新字段: {res}");

        // 逗号串形态同样生效
        update
            .execute(&serde_json::json!({"index": 1, "beats": "登场, 冲突"}))
            .unwrap();

        // 读取侧验证落盘
        let out = GetChapterOutlineTool::new(ctx.clone())
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("角色: c_0001, c_0005"), "{out}");
        assert!(out.contains("线索: t_004, t_007"), "{out}");
        assert!(out.contains("节拍: 登场 → 冲突"), "{out}");

        // 落盘文件本身
        let content = std::fs::read_to_string(ctx.paths().chapters_toml()).unwrap();
        assert!(
            content.contains("\"t_004\"") || content.contains("t_004"),
            "{content}"
        );
    }

    #[test]
    fn design_get_update_delete_flow() {
        let (_tmp, ctx) = setup();

        // 1. design_chapters：设计 2 章 + 1 卷
        let design = DesignChaptersTool::new(ctx.clone());
        let chapters_json = r#"[
            {"title":"雨夜","volume_order":1,"summary":"投宿客栈","goal":"引出身份疑点","beats":["登场","冲突"],"characters":["c_001"],"threads":["t_001"],"hook":"旧案"},
            {"title":"渡河","volume_order":1,"goal":"渡河遇险"}
        ]"#;
        let volumes_json = r#"[{"name":"第一卷：风起","order":1,"theme":"离别"}]"#;
        let args = serde_json::json!({
            "chapters": chapters_json,
            "volumes": volumes_json,
        });
        let res = design.execute(&args).unwrap();
        assert!(res.contains("已设计 2 个章节脉络"));

        // 2. get_chapter_outline：读取验证
        let get = GetChapterOutlineTool::new(ctx.clone());
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("第一卷：风起"));
        assert!(out.contains("雨夜"));
        assert!(out.contains("渡河"));
        assert!(out.contains("[待写]"));
        assert!(out.contains("登场 → 冲突"));
        assert!(out.contains("进度: 0/2 完成"));

        // 3. update_chapter_outline：标记第 1 章为 writing + 回填 chapter_id
        let update = UpdateChapterOutlineTool::new(ctx.clone());
        let res = update
            .execute(&serde_json::json!({
                "index": "1",
                "status": "writing",
                "chapter_id": "ch_01",
            }))
            .unwrap();
        assert!(res.contains("status"));
        assert!(res.contains("chapter_id"));

        // 4. get 过滤：只看 writing（此时第 1 章 writing，第 2 章 planned）
        let out = get
            .execute(&serde_json::json!({"status": "writing"}))
            .unwrap();
        assert!(out.contains("雨夜"), "writing 过滤应显示第1章, got: {out}");
        assert!(!out.contains("渡河"));

        // 5. 状态回退校验：completed 后不能回退到 writing
        update
            .execute(&serde_json::json!({"index":"1","status":"completed"}))
            .unwrap();
        let err = update
            .execute(&serde_json::json!({"index":"1","status":"writing"}))
            .unwrap_err();
        assert!(err.contains("单向推进"));

        // 6. delete_chapter_outline：删第 2 章（数字形态 index，schema 声明为 integer）
        let delete = DeleteChapterOutlineTool::new(ctx.clone());
        let res = delete.execute(&serde_json::json!({"index": 2})).unwrap();
        assert!(res.contains("已删除章节脉络"));
        assert!(res.contains("剩余 1 个"));

        // 7. 验证 chapters.toml 落盘内容正确
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("进度: 1/1 完成"));
    }

    #[test]
    fn design_chapters_accepts_string_items() {
        // 用户真实案例：模型无视 items 的 object 声明，把每章/每卷序列化成
        // 字符串传入。旧实现在 get_str(input, "title") 处报「缺少参数: title」；
        // 现按 llm_json 宽松梯度把字符串项解析回对象，与原生对象项混用亦可。
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        let res = design
            .execute(&serde_json::json!({
                "chapters": [
                    "{\"title\": \"雨夜\", \"volume_order\": 1, \"summary\": \"投宿客栈\", \"hook\": \"旧案\"}",
                    {"title": "渡河", "volume_order": 1}
                ],
                "volumes": ["{\"name\": \"第一卷 风起\", \"order\": 1}"]
            }))
            .unwrap();
        assert!(res.contains("已设计 2 个章节脉络"), "{res}");

        let (file, vols, _) = load_chapters(&ctx).unwrap();
        assert_eq!(vols.len(), 1);
        assert_eq!(vols[0].name, "第一卷 风起");
        assert_eq!(file.chapters[0].title, "雨夜");
        assert_eq!(file.chapters[0].summary.as_deref(), Some("投宿客栈"));
        assert_eq!(file.chapters[0].hook.as_deref(), Some("旧案"));
        assert_eq!(file.chapters[1].title, "渡河");
    }

    #[test]
    fn design_chapters_string_item_garbage_errors_with_index() {
        // 字符串项解析失败要指明第几项，而非误导性的「缺少参数: title」
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        let err = design
            .execute(&serde_json::json!({
                "chapters": [
                    {"title": "正常章"},
                    "完全不是 JSON 的散文"
                ]
            }))
            .unwrap_err();
        assert!(err.contains("chapters 第 2 项"), "应指明项序: {err}");
    }

    #[test]
    fn design_chapters_reference_outline_volumes_without_passing_them() {
        let (_tmp, ctx) = setup();
        // 大纲层已有卷结构（唯一真相源），妙笔只传章节不传 volumes
        std::fs::write(
            ctx.paths().outline_toml(),
            "[[volumes]]\nname = \"第一卷 风起\"\nsummary = \"离乡\"\ntheme = \"离别\"\n\n\
             [[volumes]]\nname = \"第二卷 潮生\"\n",
        )
        .unwrap();

        let design = DesignChaptersTool::new(ctx.clone());
        design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"雨夜","volume_order":1},{"title":"渡河","volume_order":2}]"#,
            }))
            .unwrap();

        // 章节直接按 volume_order 引用大纲卷，无需导入提示
        let (file, vols, _) = load_chapters(&ctx).unwrap();
        assert_eq!(vols.len(), 2);
        assert_eq!(vols[0].name, "第一卷 风起");
        assert_eq!(file.chapters[0].volume_order, 1);
        assert_eq!(file.chapters[1].volume_order, 2);
        // 卷只存大纲，chapters.toml 不再有副本
        let content = std::fs::read_to_string(ctx.paths().chapters_toml()).unwrap();
        assert!(!content.contains("[[volumes]]"), "{content}");

        // 妙笔读取脉络时能看到卷分组
        let get = GetChapterOutlineTool::new(ctx.clone());
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("第一卷 风起"), "{out}");
        assert!(out.contains("第二卷 潮生"), "{out}");
    }

    #[test]
    fn design_chapters_placeholder_volume_without_outline() {
        let (_tmp, ctx) = setup();
        // 无大纲卷结构，章节引用卷 1/3 → 补齐连续占位卷（1、2、3）
        let design = DesignChaptersTool::new(ctx.clone());
        let res = design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"雨夜","volume_order":1},{"title":"远行","volume_order":3}]"#,
            }))
            .unwrap();
        assert!(res.contains("占位卷"), "{res}");
        assert!(res.contains("第1卷、第2卷、第3卷"), "{res}");

        // 占位卷落盘到大纲（唯一真相源），chapters.toml 保持无卷副本
        let (_, vols, _) = load_chapters(&ctx).unwrap();
        let orders: Vec<u32> = vols.iter().map(|v| v.order).collect();
        assert_eq!(orders, vec![1, 2, 3], "{orders:?}");
        assert!(vols.iter().any(|v| v.name == "第3卷"));
    }

    #[test]
    fn all_unassigned_chapters_leave_volumes_empty() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"雨夜","volume_order":0},{"title":"渡河"}]"#,
            }))
            .unwrap();
        let content = std::fs::read_to_string(ctx.paths().chapters_toml()).unwrap();
        assert!(!content.contains("[[volumes]]"), "{content}");
        let (file, _vols, _) = load_chapters(&ctx).unwrap();
        assert!(file.volumes.is_empty());
    }

    #[test]
    fn lenient_parse_handles_fences_and_stray_quotes() {
        // 1. 代码围栏包裹
        let fenced = "```json\n[{\"title\":\"开篇\"}]\n```";
        let v = parse_json_array_lenient(fenced, "chapters").unwrap();
        assert_eq!(v[0]["title"], "开篇");

        // 2. 前后缀说明文字
        let with_prose = "以下是章节设计：\n[{\"title\":\"终章\"}]\n如上。";
        let v = parse_json_array_lenient(with_prose, "chapters").unwrap();
        assert_eq!(v[0]["title"], "终章");

        // 3. 字符串内部未转义的 ASCII 引号（模型把中文引号写成 "）
        let stray = "[{\"title\":\"坟前\",\"beats\":[\"他说\"开匣\"去\",\"看碑\"]}]";
        let v = parse_json_array_lenient(stray, "chapters").unwrap();
        assert_eq!(v[0]["beats"][0], "他说\"开匣\"去");

        // 4. 仍是垃圾时明确报错
        assert!(parse_json_array_lenient("这不是 JSON", "chapters").is_err());
    }

    #[test]
    fn design_chapters_accepts_stray_quote_beats() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        let chapters = "[{\"title\":\"第二章 开匣\",\"beats\":[\"砚舟点破\"信皮是描的\"\",\"父亲说明早上山\"]}]";
        let res = design
            .execute(&serde_json::json!({"chapters": chapters}))
            .unwrap();
        assert!(res.contains("已设计 1 个章节脉络"));

        let get = GetChapterOutlineTool::new(ctx.clone());
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(
            out.contains("信皮是描的"),
            "beats 应保留原文引号, got: {out}"
        );
    }

    #[test]
    fn design_append_vs_replace() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        let one = r#"[{"title":"第一章"}]"#;

        // 第一次：1 章
        design
            .execute(&serde_json::json!({"chapters": one}))
            .unwrap();
        // 第二次追加：再 1 章（默认追加）
        design
            .execute(&serde_json::json!({"chapters": one}))
            .unwrap();
        let get = GetChapterOutlineTool::new(ctx.clone());
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("进度: 0/2"));

        // 第三次 replace：清空后 1 章（全部为 planned 且未关联 → 无需 confirm）
        design
            .execute(&serde_json::json!({"chapters": one, "replace": true}))
            .unwrap();
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("进度: 0/1"));
    }

    /// replace 软拦截：待清空脉络含已动笔/已关联条目时需显式 confirm。
    #[test]
    fn design_replace_guard_requires_confirm_for_writing_chapters() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        let one = r#"[{"title":"第一章 上山"}]"#;
        design
            .execute(&serde_json::json!({"chapters": one}))
            .unwrap();
        let update = UpdateChapterOutlineTool::new(ctx.clone());
        update
            .execute(&serde_json::json!({"index": 1, "status": "writing"}))
            .unwrap();

        // 不带 confirm → 拒绝并列明风险
        let err = design
            .execute(
                &serde_json::json!({"chapters": r#"[{"title":"新版第一章"}]"#, "replace": true}),
            )
            .unwrap_err();
        assert!(err.contains("confirm=true"), "{err}");
        assert!(err.contains("已动笔"), "{err}");

        // 脉络未被清空
        let get = GetChapterOutlineTool::new(ctx.clone());
        assert!(get
            .execute(&serde_json::json!({}))
            .unwrap()
            .contains("上山"));

        // 带 confirm=true → 执行清空替换
        design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"新版第一章"}]"#,
                "replace": true,
                "confirm": true
            }))
            .unwrap();
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("新版第一章"), "{out}");
        assert!(!out.contains("上山"), "{out}");
    }

    /// populate_scenes_from_beats：按节拍生成场景骨架，幂等。
    #[test]
    fn populate_scenes_from_beats_creates_skeleton() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"上山","beats":["山门对峙","夜探藏经阁","断崖顿悟"]}]"#
            }))
            .unwrap();

        // 虚拟章直接生成 → 报错引导 create_chapter
        let tool = PopulateScenesFromBeatsTool::new(ctx.clone());
        let err = tool.execute(&serde_json::json!({"index": 1})).unwrap_err();
        assert!(err.contains("create_chapter"), "{err}");

        // 落实物理章（同名自动回填）后生成：3 节拍 → 3 场景，脉络 scenes 同步
        let paths = story_project_format::ProjectPaths::new(&ctx.project_dir);
        crate::mutations::create_chapter(&paths, "上山").unwrap();
        let out = tool.execute(&serde_json::json!({"index": 1})).unwrap();
        assert!(out.contains("3 个场景骨架"), "{out}");
        assert!(out.contains("第1幕"), "{out}");

        // 幂等：再次调用无新增
        let out = tool.execute(&serde_json::json!({"index": 1})).unwrap();
        assert!(out.contains("无新增"), "{out}");

        // 脉络 scenes 列表已同步 3 个场景 ID
        let file = crate::mutations::load_chapters_file(&paths).unwrap();
        assert_eq!(file.chapters[0].scenes.len(), 3);
    }

    // —— auto_titles：模型提炼场景短标题 ——

    /// Mock provider，返回固定文本（模拟模型输出）
    #[derive(Debug)]
    struct MockTitleProvider {
        response: String,
    }

    impl story_ai_runtime::provider::AiProvider for MockTitleProvider {
        fn id(&self) -> &str {
            "mock-title"
        }
        fn generate(
            &self,
            _request: &story_ai_runtime::provider::GenerateRequest,
        ) -> Result<story_ai_runtime::provider::GenerateResponse, story_ai_runtime::provider::AiError>
        {
            use story_ai_runtime::provider::{GenerateResponse, StopReason, TokenUsage};
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    fn setup_populate_project(
        provider: Option<std::sync::Arc<dyn story_ai_runtime::provider::AiProvider>>,
    ) -> (TempDir, ToolContext) {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        let ctx = match provider {
            Some(p) => ToolContext::with_provider(tmp.path().to_path_buf(), p),
            None => ToolContext::new(tmp.path().to_path_buf()),
        };
        let design = DesignChaptersTool::new(ctx.clone());
        design
            .execute(&serde_json::json!({
                "chapters": r#"[{"title":"上山","beats":["【山门对峙】林风与守门弟子起冲突","苏白在云梦大泽问道峰留下坐化假象，凡人苏白被迫接掌空壳宗门"]}]"#
            }))
            .unwrap();
        let paths = story_project_format::ProjectPaths::new(&ctx.project_dir);
        crate::mutations::create_chapter(&paths, "上山").unwrap();
        (tmp, ctx)
    }

    #[test]
    fn populate_auto_titles_uses_llm_with_fallback() {
        // 模型返回两条标题：第 2 条（无显式 titles 覆盖的节拍）用模型提炼
        let (_tmp, ctx) = setup_populate_project(Some(std::sync::Arc::new(MockTitleProvider {
            response: r#"["遗蜕留书","逍遥殿议事"]"#.to_string(),
        })));
        let tool = PopulateScenesFromBeatsTool::new(ctx.clone());
        // 显式 titles 只覆盖第 1 个场景 → 第 2 个走模型提炼
        let out = tool
            .execute(&serde_json::json!({
                "index": 1,
                "titles": ["显式标题"],
                "auto_titles": true
            }))
            .unwrap();
        assert!(out.contains("第1幕·显式标题"), "{out}");
        assert!(out.contains("第2幕·逍遥殿议事"), "{out}");
        assert!(out.contains("1/2 个新场景标题由模型提炼"), "{out}");

        // 场景标题已落盘
        let paths = story_project_format::ProjectPaths::new(&ctx.project_dir);
        let structure = story_project_format::StructureFile::from_toml_str(
            &std::fs::read_to_string(paths.structure_toml()).unwrap(),
        )
        .unwrap();
        let titles: Vec<&str> = structure
            .scene
            .values()
            .filter_map(|s| s.title.as_deref())
            .collect();
        assert!(titles.contains(&"第2幕·逍遥殿议事"), "{titles:?}");
    }

    #[test]
    fn populate_auto_titles_falls_back_on_bad_output() {
        // 模型输出不是合法 JSON → 全部回退节拍断句，不阻断骨架生成
        let (_tmp, ctx) = setup_populate_project(Some(std::sync::Arc::new(MockTitleProvider {
            response: "抱歉，我不会命名。".to_string(),
        })));
        let tool = PopulateScenesFromBeatsTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({ "index": 1, "auto_titles": true }))
            .unwrap();
        assert!(out.contains("2 个场景骨架"), "{out}");
        assert!(out.contains("auto_titles 调用失败"), "{out}");
        // 第 1 节拍有【】标记 → 前缀提取仍生效
        assert!(out.contains("第1幕·山门对峙"), "{out}");
    }

    #[test]
    fn populate_auto_titles_without_provider_notes_fallback() {
        let (_tmp, ctx) = setup_populate_project(None);
        let tool = PopulateScenesFromBeatsTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({ "index": 1, "auto_titles": true }))
            .unwrap();
        assert!(out.contains("需要 provider"), "{out}");
        assert!(out.contains("第1幕·山门对峙"), "{out}");
    }

    #[test]
    fn sanitize_llm_title_strips_quotes_and_caps_length() {
        assert_eq!(
            sanitize_llm_title("  \"遗蜕留书\" "),
            Some("遗蜕留书".to_string())
        );
        assert_eq!(
            sanitize_llm_title("「逍遥殿议事」"),
            Some("逍遥殿议事".to_string())
        );
        assert_eq!(sanitize_llm_title("   "), None);
        assert_eq!(sanitize_llm_title(""), None);
        // 超 12 字截断
        assert_eq!(
            sanitize_llm_title("一个非常非常非常长的场景标题名称溢出"),
            Some("一个非常非常非常长的场景".to_string())
        );
    }

    /// 节拍标题头提取：前缀标记优先（【】/[]/冒号），无标记退回窗口断句。
    #[test]
    fn beat_title_head_prefers_prefix_markers() {
        // 【标题】… / [标题]…：包裹段即标题，不再截断正文
        assert_eq!(
            beat_title_head("【清点账目】苏白在主殿清点账目，发现亏空"),
            "清点账目"
        );
        assert_eq!(beat_title_head("[夜探]潜入藏经阁翻查旧档"), "夜探");
        // 标题：描述（全角/半角冒号均可，冒号前 ≤12 字）
        assert_eq!(
            beat_title_head("山门对峙：林风与守门弟子起冲突"),
            "山门对峙"
        );
        assert_eq!(beat_title_head("断崖顿悟:于断崖处领悟剑意"), "断崖顿悟");
        // 冒号前含句末标点 → 正文句子的冒号（如引语），退回窗口断句
        assert_eq!(
            beat_title_head("苏白连夜赶回主殿，在殿前拦住长老：质问"),
            "苏白连夜赶回主殿"
        );
        // 无标记：保持旧行为——窗口断句或 12 字硬截断
        assert_eq!(
            beat_title_head("夜探藏经阁，发现空匣"),
            "夜探藏经阁，发现空匣"
        );
        assert_eq!(
            beat_title_head("苏白在断云峰的无为宗主殿与长老对峙"),
            "苏白在断云峰的无为宗主殿"
        );
        assert_eq!(beat_title_head("断崖顿悟"), "断崖顿悟");
    }

    /// populate 的新参数：titles 显式标题、default_pov 指定/自动推断。
    #[test]
    fn populate_scenes_titles_and_default_pov() {
        let (_tmp, ctx) = setup();
        let paths = ctx.paths();
        // 角色档案：default_pov 按名字传入时的归一基准
        std::fs::create_dir_all(paths.characters_dir()).unwrap();
        std::fs::write(
            paths.characters_dir().join("c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"林风\"\n",
        )
        .unwrap();

        let design = DesignChaptersTool::new(ctx.clone());
        design
            .execute(&serde_json::json!({
                "chapters": r#"[
                    {"title":"上山","beats":["【山门对峙】林风与守门弟子起冲突","夜探藏经阁，发现空匣","断崖顿悟"],"characters":["林风"]},
                    {"title":"渡河","beats":["摆渡人索要旧铜钱","河心旋涡卷走行囊"],"characters":["林风"]},
                    {"title":"夜雨","beats":["客栈投宿","查房","夜半异响"],"characters":[]}
                ]"#
            }))
            .unwrap();
        let ch1 = crate::mutations::create_chapter(&paths, "上山").unwrap();
        let ch2 = crate::mutations::create_chapter(&paths, "渡河").unwrap();
        let ch3 = crate::mutations::create_chapter(&paths, "夜雨").unwrap();

        let tool = PopulateScenesFromBeatsTool::new(ctx.clone());

        // 自动路径：标题走节拍前缀标记/窗口断句，POV 取本章脉络第一个角色
        let out = tool.execute(&serde_json::json!({"index": 1})).unwrap();
        assert!(out.contains("第1幕·山门对峙"), "{out}");
        assert!(out.contains("第2幕·夜探藏经阁，发现空匣"), "{out}");
        assert!(out.contains("第3幕·断崖顿悟"), "{out}");
        assert!(out.contains("默认 POV: c_0001（本章主咖）"), "{out}");

        // 显式路径：titles 覆盖标题、default_pov 传名字归一为 ID
        let out = tool
            .execute(&serde_json::json!({
                "index": 2,
                "titles": ["铜钱", "旋涡"],
                "default_pov": "林风"
            }))
            .unwrap();
        assert!(out.contains("第1幕·铜钱"), "{out}");
        assert!(out.contains("第2幕·旋涡"), "{out}");
        assert!(out.contains("默认 POV: c_0001（指定）"), "{out}");

        // titles 少于新场景数：其余按节拍自动提取并附说明；本章无角色 → 不设 POV
        let out = tool
            .execute(&serde_json::json!({"index": 3, "titles": ["投宿"]}))
            .unwrap();
        assert!(out.contains("第1幕·投宿"), "{out}");
        assert!(out.contains("第2幕·查房"), "{out}");
        assert!(out.contains("其余按节拍自动提取标题"), "{out}");
        assert!(out.contains("未设 POV"), "{out}");

        // POV 已真正落盘到 structure.toml（而非仅回执说明）
        let structure = crate::mutations::load_structure(&paths).unwrap();
        for ch in [&ch1, &ch2] {
            let povs: Vec<Option<String>> = structure.chapter[ch]
                .scenes
                .iter()
                .map(|s| structure.scene[s].pov.clone())
                .collect();
            assert!(
                povs.iter().all(|p| p.as_deref() == Some("c_0001")),
                "{ch} 的场景 POV 应为 c_0001: {povs:?}"
            );
        }
        let povs3: Vec<Option<String>> = structure.chapter[&ch3]
            .scenes
            .iter()
            .map(|s| structure.scene[s].pov.clone())
            .collect();
        assert!(
            povs3.iter().all(|p| p.is_none()),
            "无角色章不应设 POV: {povs3:?}"
        );
    }

    #[test]
    fn get_empty_chapters() {
        let (_tmp, ctx) = setup();
        let get = GetChapterOutlineTool::new(ctx.clone());
        let out = get.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("章节脉络为空"));
    }

    #[test]
    fn design_invalid_json_errors() {
        let (_tmp, ctx) = setup();
        let design = DesignChaptersTool::new(ctx.clone());
        // chapters 参数为合法 JSON 字符串，但内容不是数组
        let err = design
            .execute(&serde_json::json!({"chapters": "{\"bad\": 1}"}))
            .unwrap_err();
        assert!(err.contains("不是 JSON 数组"), "{err}");
    }
}
