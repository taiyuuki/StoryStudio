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
    check_index, get_index, get_json_array_opt, get_str, get_str_opt, get_str_array_opt,
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
                                "characters": { "type": "array", "items": { "type": "string" }, "description": "可选：涉及角色" },
                                "threads": { "type": "array", "items": { "type": "string" }, "description": "可选：涉及线索" },
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
                let titles: Vec<&str> =
                    at_risk.iter().map(|c| c.title.as_str()).take(5).collect();
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

        for input in &chapter_inputs {
            let title = get_str(input, "title")?;
            let volume_order = input
                .get("volume_order")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let beats = parse_str_array(input, "beats")?;
            let characters = parse_str_array(input, "characters")?;
            let threads = parse_str_array(input, "threads")?;

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

        Ok(format!(
            "已设计 {} 个章节脉络（序号 {}-{}），已写入 chapters.toml{}。各章状态为 planned，动笔时用 update_chapter_outline(index, status=\"writing\") 标记。",
            added, start_index, end_index, volume_note
        ))
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
            description: "读取章节脉络（chapters.toml），按卷分组展示各章的目标/节拍/涉及角色线索/状态。\
            章节超过 20 章时默认列未完成章节与最近 15 章已完成；可用 volume_order 按卷、\
            status 按状态过滤查看指定范围。写新章节前调用以获取本章设计意图。".into(),
            parameters: object_schema(vec![
                ("volume_order", "string", "可选：只看某卷（1-based 卷序号）", false),
                ("status", "string", "可选：只看某状态（planned/writing/completed）", false),
                ("include_completed", "boolean", "是否包含已完成章节（默认 true）", false),
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
                    c.status == "completed"
                        || {
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
                "\n## 进度: {completed}/{total} 完成, {writing} 写作中, {planned} 待写\n",
            ));
            if let Some(n) = next {
                result.push_str(&format!("下一章待写: {}\n", n.title));
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
                ("characters", "array", "涉及角色 ID 列表，整体替换", false),
                ("threads", "array", "涉及线索 ID 列表，整体替换", false),
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
            item.characters = v;
            changes.push("characters");
        }
        if let Some(v) = get_str_array_opt(arguments, "threads")? {
            item.threads = v;
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
                    "状态只能单向推进（{} → {}），当前为 {}，不能回退到 {}",
                    item.status, v, item.status, v
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
        format!(
            "章节引用了未定义的卷，已补建占位卷: {}",
            added.join("、")
        ),
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
        save_outline_volumes(&ctx.paths(), volumes)
            .map_err(|e| format!("写入卷结构失败: {e}"))?;
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
pub(crate) fn parse_json_array_lenient(raw: &str, field: &str) -> Result<Vec<serde_json::Value>, String> {
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
        assert!(content.contains("\"t_004\"") || content.contains("t_004"), "{content}");
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
        assert!(out.contains("信皮是描的"), "beats 应保留原文引号, got: {out}");
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
            .execute(&serde_json::json!({"chapters": r#"[{"title":"新版第一章"}]"#, "replace": true}))
            .unwrap_err();
        assert!(err.contains("confirm=true"), "{err}");
        assert!(err.contains("已动笔"), "{err}");

        // 脉络未被清空
        let get = GetChapterOutlineTool::new(ctx.clone());
        assert!(get.execute(&serde_json::json!({})).unwrap().contains("上山"));

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
