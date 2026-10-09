//! 文档结构工具：编译检查、结构概览、章节/场景的增删改。

use story_ai_runtime::{Tool, ToolDef};
use story_ir::StableId;
use story_project_format::structure_toml::StoryTimeDef;

use super::{get_json_array_opt, get_str, get_str_opt, object_schema, ToolContext};

// ============================================================================
// compile_check
// ============================================================================

pub struct CompileCheckTool {
    ctx: ToolContext,
}
impl CompileCheckTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CompileCheckTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "compile_check".into(),
            description: "运行编译器，返回诊断结果（错误/警告/提示）。".into(),
            parameters: object_schema(vec![]),
        }
    }

    fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        let result = crate::compile::compile_project(&self.ctx.project_dir)
            .map_err(|e| format!("编译失败: {e}"))?;
        let diagnostics = result.diagnostics;
        if diagnostics.is_empty() {
            return Ok("编译通过，无诊断问题。".to_string());
        }

        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == story_compiler::Severity::Error)
            .collect();
        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == story_compiler::Severity::Warning)
            .collect();
        let hints: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                matches!(
                    d.severity,
                    story_compiler::Severity::Hint | story_compiler::Severity::Info
                )
            })
            .collect();

        let mut report = format!(
            "编译结果: {} 错误, {} 警告, {} 提示\n",
            errors.len(),
            warnings.len(),
            hints.len()
        );
        for e in &errors {
            report.push_str(&format!("  ❌ ERROR [{}]: {}\n", e.code, e.message));
        }
        for w in &warnings {
            report.push_str(&format!("  ⚠️ WARNING [{}]: {}\n", w.code, w.message));
        }
        for h in &hints {
            report.push_str(&format!("  💡 HINT [{}]: {}\n", h.code, h.message));
        }
        Ok(report)
    }
}

// ============================================================================
// get_structure
// ============================================================================

pub struct GetStructureTool {
    ctx: ToolContext,
}
impl GetStructureTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetStructureTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_structure".into(),
            description: "获取项目文档结构概览：书籍 → 章节两级，每章附场景数。\
            传 chapter_id 可列出该章的场景明细（各场景标题与片段数）。".into(),
            parameters: object_schema(vec![(
                "chapter_id",
                "string",
                "可选：只展开该章节的场景列表（如 ch_0001）",
                false,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let parse_output = self.ctx.parse()?;
        let doc = &parse_output.ir.document;

        let (book_id, book_title) = doc
            .all_nodes()
            .find_map(|(id, n)| {
                if let story_ir::DocNode::Book(b) = n {
                    Some((id.clone(), b.title.clone()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| (StableId::new_unchecked("b_0001"), "未命名".to_string()));

        let chapter_ids = doc.children_of(&book_id);

        // 下钻模式：只展开指定章节的场景
        if let Some(ch_id) = get_str_opt(arguments, "chapter_id") {
            let sid = StableId::new_unchecked(&ch_id);
            if doc.get_chapter(&sid).is_none() {
                return Err(format!("章节 '{ch_id}' 不存在"));
            }
            let ch_title = doc
                .get_chapter(&sid)
                .map(|c| c.title.clone())
                .unwrap_or_default();
            let mut result = format!("章节: {ch_title} ({ch_id})\n");
            for sc_id in doc.children_of(&sid) {
                let sc_title = doc
                    .get_scene(sc_id)
                    .and_then(|s| s.title.as_deref())
                    .unwrap_or("无标题");
                let frag_count = doc.children_of(sc_id).len();
                result.push_str(&format!(
                    "  场景: {sc_title} ({sc_id}) [{frag_count} 片段]\n"
                ));
            }
            if doc.children_of(&sid).is_empty() {
                result.push_str("  （无场景）\n");
            }
            return Ok(result);
        }

        // 概览模式：场景折叠为计数（长篇几百章时保持输出有界）
        let mut result = format!("书籍: {book_title} ({book_id})\n");
        for ch_id in chapter_ids {
            let ch_title = doc
                .get_chapter(ch_id)
                .map(|c| c.title.as_str())
                .unwrap_or("?");
            result.push_str(&format!(
                "  章节: {ch_title} ({ch_id}) [{} 场景]\n",
                doc.children_of(ch_id).len()
            ));
        }
        if chapter_ids.is_empty() {
            result.push_str("  （无章节）\n");
        } else {
            result.push_str(&format!(
                "\n（共 {} 章；要看某章的场景明细，传 chapter_id 下钻）\n",
                chapter_ids.len()
            ));
        }
        Ok(result)
    }
}

// ============================================================================
// create_scene
// ============================================================================

pub struct CreateSceneTool {
    ctx: ToolContext,
}
impl CreateSceneTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateSceneTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_scene".into(),
            description: "在指定章节下创建新场景，自动生成场景 ID 并更新 structure.toml。\
            可选传入 initial_text 直接写入首个片段，建场与开笔一次完成，\
            省一次 write_fragment_text 往返。"
                .into(),
            parameters: object_schema(vec![
                ("chapter_id", "string", "所属章节 ID（如 ch_01）", true),
                ("title", "string", "场景标题", true),
                ("pov_character_id", "string", "POV 角色 ID", false),
                (
                    "initial_text",
                    "string",
                    "首个片段正文（可选；高速写作时建场即开笔，正文较长也可后续再写）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let chapter_id = get_str(arguments, "chapter_id")?;
        let title = get_str(arguments, "title")?;
        let pov = get_str_opt(arguments, "pov_character_id");
        let initial_text = get_str_opt(arguments, "initial_text");

        let paths = self.ctx.paths();
        let (new_id, fragment_id) = crate::mutations::create_scene_with_fragment(
            &paths,
            &chapter_id,
            &title,
            pov.as_deref(),
            initial_text.as_deref(),
            crate::stats::WriteSource::Ai,
        )
        .map_err(|e| e.to_string())?;

        let fragment_note = match &fragment_id {
            Some(fid) => format!("，首个片段已写入: {fid}（续写用 write_fragment_text / append_fragment_text）"),
            None => String::new(),
        };
        Ok(format!(
            "场景已创建: id={new_id}, title={title}, chapter={chapter_id}, pov={}{}",
            pov.as_deref().unwrap_or("未设置"),
            fragment_note
        ))
    }
}

// ============================================================================
// create_chapter
// ============================================================================

pub struct CreateChapterTool {
    ctx: ToolContext,
}
impl CreateChapterTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateChapterTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_chapter".into(),
            description: "创建新章节，自动生成章节 ID（ch_XX）并更新 structure.toml。".into(),
            parameters: object_schema(vec![(
                "title",
                "string",
                "章节标题（如：第一章 雨夜）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let title = get_str(arguments, "title")?;
        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_chapter(&paths, &title).map_err(|e| e.to_string())?;
        Ok(format!("章节已创建: id={new_id}, title={title}"))
    }
}

// ============================================================================
// update_chapter
// ============================================================================

pub struct UpdateChapterTool {
    ctx: ToolContext,
}
impl UpdateChapterTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateChapterTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_chapter".into(),
            description: "修改章节的标题或 symbol，只更新提供的字段。".into(),
            parameters: object_schema(vec![
                ("chapter_id", "string", "章节 ID（如 ch_01）", true),
                ("title", "string", "新标题", false),
                ("symbol", "string", "新 symbol（传空字符串清除）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let chapter_id = get_str(arguments, "chapter_id")?;
        let title = get_str_opt(arguments, "title");
        let symbol = get_str_opt(arguments, "symbol");

        if title.is_none() && symbol.is_none() {
            return Err("至少提供一个要修改的字段（title 或 symbol）".into());
        }

        let paths = self.ctx.paths();
        let changes = crate::mutations::update_chapter(
            &paths,
            &chapter_id,
            title.as_deref(),
            symbol.as_deref(),
        )
        .map_err(|e| e.to_string())?;

        Ok(format!("章节 {chapter_id} 已更新: {}", changes.join(", ")))
    }
}

// ============================================================================
// delete_chapter
// ============================================================================

pub struct DeleteChapterTool {
    ctx: ToolContext,
}
impl DeleteChapterTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteChapterTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_chapter".into(),
            description: "删除章节及其下所有场景和片段（级联删除，不可恢复）。".into(),
            parameters: object_schema(vec![(
                "chapter_id",
                "string",
                "要删除的章节 ID（如 ch_01）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let chapter_id = get_str(arguments, "chapter_id")?;
        let paths = self.ctx.paths();
        crate::mutations::delete_chapter(&paths, &chapter_id).map_err(|e| e.to_string())?;
        Ok(format!("章节 {chapter_id} 及其下所有场景、片段已删除"))
    }
}

// ============================================================================
// update_scene
// ============================================================================

pub struct UpdateSceneTool {
    ctx: ToolContext,
}
impl UpdateSceneTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateSceneTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_scene".into(),
            description:
                "修改场景的属性（标题、POV、情绪、叙事功能、场景摘要、故事时间），只更新提供的字段。".into(),
            parameters: object_schema(vec![
                ("scene_id", "string", "场景 ID（如 sc_01_02）", true),
                ("title", "string", "新标题", false),
                (
                    "pov_character_id",
                    "string",
                    "POV 角色 ID（传空字符串清除）",
                    false,
                ),
                (
                    "mood",
                    "string",
                    "情绪基调（如：紧张、温馨、悲壮）。传空字符串清除",
                    false,
                ),
                (
                    "function",
                    "string",
                    "叙事功能（如：铺垫、高潮、转折）。传空字符串清除",
                    false,
                ),
                (
                    "summary",
                    "string",
                    "场景摘要（一句话或一段话概括，可 AI 生成）。传空字符串清除",
                    false,
                ),
                (
                    "story_time",
                    "string",
                    "故事时间锚：数字（1023-9-15 / 1023）或历法标签（灵历3217年、某个雨夜）。\
                    标签按项目历法（worldview/timeline.toml 的 eras）比较，解析不了仅作展示。传空字符串清除",
                    false,
                ),
                (
                    "gap",
                    "string",
                    "与上一场景的时间间隔（如 3天、十年、同一夜；模糊描述合法）。传空字符串清除",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scene_id = get_str(arguments, "scene_id")?;
        let title = get_str_opt(arguments, "title");
        let pov_raw = arguments.get("pov_character_id").and_then(|v| v.as_str());
        let mood_raw = arguments.get("mood").and_then(|v| v.as_str());
        let function_raw = arguments.get("function").and_then(|v| v.as_str());
        let summary_raw = arguments.get("summary").and_then(|v| v.as_str());
        let story_time_raw = arguments.get("story_time").and_then(|v| v.as_str());
        let gap_raw = arguments.get("gap").and_then(|v| v.as_str());

        // story_time 解析: 数字 / 历法标签 / ""(清除)；gap 可与任一形态组合
        let mut story_time = match story_time_raw {
            None => None, // 参数未提供 = 不修改
            Some(s) => Some(parse_story_time(s)?),
        };
        if let Some(g) = gap_raw {
            let slot = story_time.get_or_insert_with(Default::default);
            let def = slot.get_or_insert_with(StoryTimeDef::default);
            def.gap = if g.is_empty() { None } else { Some(g.to_string()) };
        }

        if title.is_none()
            && pov_raw.is_none()
            && mood_raw.is_none()
            && function_raw.is_none()
            && summary_raw.is_none()
            && story_time.is_none()
        {
            return Err("至少提供一个要修改的字段".into());
        }

        let update = crate::mutations::SceneUpdate {
            title: title.as_deref(),
            pov: pov_raw.map(|s| if s.is_empty() { None } else { Some(s) }),
            mood: mood_raw.map(|s| if s.is_empty() { None } else { Some(s) }),
            function: function_raw.map(|s| if s.is_empty() { None } else { Some(s) }),
            summary: summary_raw.map(|s| if s.is_empty() { None } else { Some(s) }),
            story_time,
        };

        let paths = self.ctx.paths();
        let changes = crate::mutations::update_scene(&paths, &scene_id, &update)
            .map_err(|e| e.to_string())?;

        if changes.is_empty() {
            return Ok("未提供任何需要更新的字段。".to_string());
        }

        Ok(format!("场景 {scene_id} 已更新: {}", changes.join(", ")))
    }
}

// ============================================================================
// delete_scene
// ============================================================================

pub struct DeleteSceneTool {
    ctx: ToolContext,
}
impl DeleteSceneTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteSceneTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_scene".into(),
            description: "删除场景及其下所有片段（级联删除，不可恢复）。".into(),
            parameters: object_schema(vec![(
                "scene_id",
                "string",
                "要删除的场景 ID（如 sc_01_02）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scene_id = get_str(arguments, "scene_id")?;
        let paths = self.ctx.paths();
        crate::mutations::delete_scene(&paths, &scene_id).map_err(|e| e.to_string())?;
        Ok(format!("场景 {scene_id} 及其下所有片段已删除"))
    }
}

/// 解析故事时间字符串
///
/// 输入: "1925" / "1925-3" / "1925-3-15" / ""(清除)
/// 返回: None=清除, Some(def)=设值
fn parse_story_time(s: &str) -> Result<Option<StoryTimeDef>, String> {
    if s.is_empty() {
        return Ok(None); // 清除
    }
    // 数值锚：1023 / 1023-9 / 1023-9-15
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        let parts: Vec<&str> = s.split('-').collect();
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| format!("story_time 无效: {s}（年份须为数字）"))?;
        if !(1..=12).contains(&parts.len()) && parts.len() > 3 {
            return Err(format!("story_time 无效: {s}（最多 年-月-日 三段）"));
        }
        let month = match parts.get(1).filter(|x| !x.is_empty()) {
            None => None,
            Some(m) => Some(
                m.parse::<u8>()
                    .map_err(|_| format!("story_time 月份无效: {m}（1-12）"))?,
            ),
        };
        if let Some(m) = month {
            if !(1..=12).contains(&m) {
                return Err(format!("story_time 月份无效: {m}（1-12）"));
            }
        }
        let day = match parts.get(2).filter(|x| !x.is_empty()) {
            None => None,
            Some(d) => Some(
                d.parse::<u8>()
                    .map_err(|_| format!("story_time 日期无效: {d}（1-31）"))?,
            ),
        };
        if let Some(d) = day {
            if !(1..=31).contains(&d) {
                return Err(format!("story_time 日期无效: {d}（1-31）"));
            }
        }
        return Ok(Some(StoryTimeDef {
            year: Some(year),
            month,
            day,
            anchor: None,
            gap: None,
        }));
    }
    // 历法标签锚：任意文本（如 灵历3217年 / 某个雨夜），存 anchor 字段；
    // 可比较性由项目历法（worldview/timeline.toml 的 eras）决定，解析不了仅展示
    Ok(Some(StoryTimeDef {
        year: None,
        month: None,
        day: None,
        anchor: Some(s.to_string()),
        gap: None,
    }))
}

// ============================================================================
// save_scene_summaries — 批量落盘场景摘要
// ============================================================================

pub struct SaveSceneSummariesTool {
    ctx: ToolContext,
}
impl SaveSceneSummariesTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SaveSceneSummariesTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "save_scene_summaries".into(),
            description: "批量保存场景摘要到 structure.toml（一次调用替代多次 update_scene）。\
            适用于 get_story_recap 之后把生成的摘要统一落盘，避免下次回顾重复生成。"
                .into(),
            parameters: object_schema(vec![(
                "summaries",
                "string",
                "字符串形式的 JSON 数组（传原生数组亦可），\
                每项 {\"scene_id\": \"sc_01_01\", \"summary\": \"一句话摘要\"}",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // summaries 兼容字符串与原生数组两种形态（模型常无视 schema 的 "string" 声明）
        let items = match get_json_array_opt(arguments, "summaries")? {
            Some(items) if !items.is_empty() => items,
            Some(_) => return Err("summaries 不能为空数组".into()),
            None => return Err("缺少参数: summaries".into()),
        };

        let paths = self.ctx.paths();
        let mut saved = Vec::new();
        let mut errors = Vec::new();

        for item in &items {
            let scene_id = get_str(item, "scene_id")?;
            let summary = get_str(item, "summary")?;
            if summary.trim().is_empty() {
                errors.push(format!("{scene_id}: 摘要为空"));
                continue;
            }
            let update = crate::mutations::SceneUpdate {
                title: None,
                pov: None,
                mood: None,
                function: None,
                summary: Some(Some(summary.as_str())),
                story_time: None,
            };
            match crate::mutations::update_scene(&paths, &scene_id, &update) {
                Ok(_) => saved.push(scene_id),
                Err(e) => errors.push(format!("{scene_id}: {e}")),
            }
        }

        let mut out = format!("已保存 {} 条场景摘要: {}", saved.len(), saved.join(", "));
        if !errors.is_empty() {
            out.push_str(&format!("；未保存 {} 条（{}）", errors.len(), errors.join("；")));
        }
        Ok(out)
    }
}

// ============================================================================
// repair_project — 数据一致性修复
// ============================================================================

pub struct RepairProjectTool {
    ctx: ToolContext,
}
impl RepairProjectTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RepairProjectTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "repair_project".into(),
            description: "一次性修复历史数据的一致性问题：\
            1) 清除片段正文里残留的 YAML 块（曾因写入缺陷叠成双层 frontmatter，\
            会在读取正文时显示 --- anchors --- 字样）；\
            2) 把断言证据字段里的片段 ID 重建为证据登记条目（消除\"证据不存在\"告警）。\
            项目迁移或编译出现大量数据类告警时使用；幂等，可重复执行。"
                .into(),
            parameters: object_schema(vec![]),
        }
    }

    fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        let report = crate::mutations::repair_project(&self.ctx.project_dir)
            .map_err(|e| e.to_string())?;

        if report.fragments_cleaned.is_empty()
            && report.assertions_fixed == 0
            && report.evidence_skipped == 0
        {
            return Ok("检查完成：数据一致，无需修复。".to_string());
        }

        let mut out = String::from("修复完成：\n");
        if !report.fragments_cleaned.is_empty() {
            out.push_str(&format!(
                "  清除正文残留 YAML 块的片段 {} 个: {}\n",
                report.fragments_cleaned.len(),
                report.fragments_cleaned.join(", ")
            ));
        }
        if report.assertions_fixed > 0 {
            out.push_str(&format!(
                "  重建证据引用的断言 {} 条（片段 ID → 证据条目）\n",
                report.assertions_fixed
            ));
        }
        if report.evidence_skipped > 0 {
            out.push_str(&format!(
                "  {} 条证据指向不存在的片段，已保留待人工处理\n",
                report.evidence_skipped
            ));
        }
        Ok(out)
    }
}

// ============================================================================
// set_fragment_anchor / remove_fragment_anchor — 显式锚点管理
// ============================================================================

pub struct SetFragmentAnchorTool {
    ctx: ToolContext,
}
impl SetFragmentAnchorTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SetFragmentAnchorTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "set_fragment_anchor".into(),
            description: "为片段显式锚定实体（写入片段锚点列表，不依赖正文文字匹配）。\
            适用：以代词/身份称谓出场而文字匹配不到的角色（如「佣兵」「那人」），\
            或需要精确登记出道具/地点的出场。同一实体重复设置会更新角色而非重复添加。\
            一次锚定多个实体时用 entity_ids 批量传入，避免逐个调用。"
                .into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "片段 ID（如 f_0003）", true),
                ("entity_id", "string", "实体 ID（如 c_0008 / l_0002 / i_0001），单条锚定时必填", false),
                (
                    "entity_ids",
                    "array",
                    "批量锚定：数组，每项为实体 ID 字符串或 {\"entity_id\": \"...\", \"role\": \"...\"}。与 entity_id 二选一",
                    false,
                ),
                (
                    "role",
                    "string",
                    "锚点角色：appears（出场）/ mentioned（提及）/ referenced（指代），默认 appears",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let role = get_str_opt(arguments, "role").unwrap_or_else(|| "appears".to_string());

        // 批量：entity_ids 数组（ID 字符串或 {entity_id, role} 对象）
        if let Some(items) = get_json_array_opt(arguments, "entity_ids")? {
            if items.is_empty() {
                return Err("entity_ids 不能为空数组".into());
            }
            let mut anchors: Vec<(String, String)> = Vec::with_capacity(items.len());
            for item in &items {
                let pair = if let Some(id) = item.as_str() {
                    (id.to_string(), role.clone())
                } else {
                    (
                        get_str(item, "entity_id")?,
                        get_str_opt(item, "role").unwrap_or_else(|| role.clone()),
                    )
                };
                anchors.push(pair);
            }
            crate::mutations::set_fragment_anchors(&self.ctx.paths(), &fragment_id, &anchors)
                .map_err(|e| e.to_string())?;
            let list = anchors
                .iter()
                .map(|(id, r)| format!("{id}（{r}）"))
                .collect::<Vec<_>>()
                .join("、");
            return Ok(format!(
                "已锚定 {} 个实体到片段 {fragment_id}：{list}",
                anchors.len()
            ));
        }

        let entity_id = get_str(arguments, "entity_id")?;
        crate::mutations::set_fragment_anchor(&self.ctx.paths(), &fragment_id, &entity_id, &role)
            .map_err(|e| e.to_string())?;
        Ok(format!("已锚定：片段 {fragment_id} ← 实体 {entity_id}（{role}）"))
    }
}

pub struct RemoveFragmentAnchorTool {
    ctx: ToolContext,
}
impl RemoveFragmentAnchorTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RemoveFragmentAnchorTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "remove_fragment_anchor".into(),
            description: "移除片段上某实体的显式锚点。".into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "片段 ID", true),
                ("entity_id", "string", "实体 ID", true),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let entity_id = get_str(arguments, "entity_id")?;

        crate::mutations::remove_fragment_anchor(&self.ctx.paths(), &fragment_id, &entity_id)
            .map_err(|e| e.to_string())?;
        Ok(format!("已移除锚点：片段 {fragment_id} × 实体 {entity_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;
    use story_ai_runtime::Tool;

    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    #[test]
    fn structure_overview_collapses_scenes_and_drill_down() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());

        let paths = story_project_format::ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "渡口", None).unwrap();
        crate::mutations::create_fragment(&paths, &sc, "雾没散。").unwrap();

        // 概览：场景折叠为计数，不列场景行
        let out = GetStructureTool::new(ctx.clone())
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("[1 场景]"), "应显示场景计数: {out}");
        assert!(!out.contains("场景: 渡口"), "概览不应列出场景明细: {out}");
        assert!(out.contains("chapter_id 下钻"), "{out}");

        // 下钻：展开该章的场景
        let out = GetStructureTool::new(ctx.clone())
            .execute(&serde_json::json!({ "chapter_id": ch }))
            .unwrap();
        assert!(out.contains("场景: 渡口"), "下钻应列出场景: {out}");
        assert!(out.contains("[1 片段]"), "{out}");

        // 下钻不存在的章节
        let err = GetStructureTool::new(ctx)
            .execute(&serde_json::json!({ "chapter_id": "ch_9999" }))
            .unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }

    #[test]
    fn create_scene_with_initial_text() {
        // 建场即开笔：initial_text 直接落成首个片段，省一次工具往返
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());
        let paths = story_project_format::ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();

        let out = CreateSceneTool::new(ctx.clone())
            .execute(&serde_json::json!({
                "chapter_id": ch,
                "title": "试讲课",
                "pov_character_id": "c_0001",
                "initial_text": "粉笔落在石板上，苏格画下第一个圆。"
            }))
            .unwrap();
        assert!(out.contains("首个片段已写入"), "{out}");

        // 片段文件落盘且关联到场景
        let fragment_id = out
            .split("首个片段已写入: ")
            .nth(1)
            .unwrap()
            .split('（')
            .next()
            .unwrap()
            .trim()
            .to_string();
        let content = std::fs::read_to_string(dir.join("fragments").join(format!("{fragment_id}.md")))
            .unwrap();
        assert!(content.contains("粉笔落在石板上"), "{content}");
        let structure =
            story_project_format::StructureFile::from_toml_str(
                &std::fs::read_to_string(dir.join("structure.toml")).unwrap(),
            )
            .unwrap();
        let scene = structure
            .scene
            .iter()
            .find(|(_, s)| s.title.as_deref() == Some("试讲课"))
            .unwrap();
        assert!(
            scene.1.fragments.iter().any(|f| f == &fragment_id),
            "片段应关联到场景: {:?}",
            scene.1.fragments
        );

        // 不传 initial_text：行为与旧版一致，无片段
        let out2 = CreateSceneTool::new(ctx)
            .execute(&serde_json::json!({ "chapter_id": ch, "title": "空场景" }))
            .unwrap();
        assert!(!out2.contains("首个片段"), "{out2}");
        let structure =
            story_project_format::StructureFile::from_toml_str(
                &std::fs::read_to_string(dir.join("structure.toml")).unwrap(),
            )
            .unwrap();
        let empty_scene = structure
            .scene
            .iter()
            .find(|(_, s)| s.title.as_deref() == Some("空场景"))
            .unwrap();
        assert!(empty_scene.1.fragments.is_empty());
    }

    #[test]
    fn update_scene_accepts_era_label_and_gap() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());

        // 建章节/场景
        let paths = story_project_format::ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();

        // 历法标签 + 间隔（自由文本，历法未配置也合法）
        let tool = UpdateSceneTool::new(ctx);
        let res = tool
            .execute(&serde_json::json!({
                "scene_id": sc,
                "story_time": "灵历3217年秋",
                "gap": "3天"
            }))
            .unwrap();
        assert!(res.contains("已更新"), "{res}");

        // 落盘验证：anchor/gap 进 story_time 表
        let content = std::fs::read_to_string(dir.join("structure.toml")).unwrap();
        let anchor_line = content
            .lines()
            .find(|l| l.contains("anchor"))
            .unwrap_or(&content);
        assert!(anchor_line.contains("灵历3217年秋"), "{content}");
        assert!(content.contains("gap = \"3天\""), "{content}");

        // 数值锚仍走原路径
        tool.execute(&serde_json::json!({
            "scene_id": sc,
            "story_time": "1023-9-15"
        }))
        .unwrap();
        let content = std::fs::read_to_string(dir.join("structure.toml")).unwrap();
        assert!(content.contains("year = 1023"), "{content}");
        assert!(content.contains("month = 9"), "{content}");
        assert!(content.contains("day = 15"), "{content}");
    }

    /// 批量锚定：一次调用为片段锚定多个实体（混合 ID 字符串与 {entity_id, role}），
    /// 重复实体更新 role 而非重复添加。
    #[test]
    fn set_fragment_anchor_batch_entity_ids() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = story_project_format::ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "两人在雾里对峙。").unwrap();

        let tool = SetFragmentAnchorTool::new(ToolContext::new(dir.to_path_buf()));
        let out = tool
            .execute(&serde_json::json!({
                "fragment_id": fid,
                "entity_ids": "[{\"entity_id\": \"c_0001\", \"role\": \"appears\"}, \"l_0002\", {\"entity_id\": \"i_0003\", \"role\": \"mentioned\"}]"
            }))
            .unwrap();
        assert!(out.contains("已锚定 3 个实体"), "{out}");

        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert!(content.contains("entity: c_0001"), "{content}");
        assert!(content.contains("entity: l_0002"), "{content}");
        // 纯 ID 项走 role 默认值 appears
        assert!(content.contains("entity: i_0003"), "{content}");
        assert!(content.contains("role: mentioned"), "{content}");

        // 再次批量同一实体：更新 role，不重复添加，其他锚点保留
        let out = tool
            .execute(&serde_json::json!({
                "fragment_id": fid,
                "entity_ids": "[{\"entity_id\": \"c_0001\", \"role\": \"referenced\"}]"
            }))
            .unwrap();
        assert!(out.contains("已锚定 1 个实体"), "{out}");
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert_eq!(
            content.matches("entity: c_0001").count(),
            1,
            "重复实体不应产生第二个锚点: {content}"
        );
        assert!(content.contains("role: referenced"), "{content}");
        assert!(content.contains("entity: i_0003"), "{content}");

        // 非法 role 整体报错，不部分落盘
        let err = tool
            .execute(&serde_json::json!({
                "fragment_id": fid,
                "entity_ids": "[\"c_0001\", {\"entity_id\": \"c_0004\", \"role\": \"主角\"}]"
            }))
            .unwrap_err();
        assert!(err.contains("锚点角色"), "{err}");
    }

    #[test]
    fn save_scene_summaries_accepts_string_and_native_array() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = story_project_format::ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "渡口", None).unwrap();

        let tool = SaveSceneSummariesTool::new(ToolContext::new(dir.to_path_buf()));

        // 字符串形式的 JSON 数组（schema 声明的形态）
        let out = tool
            .execute(&serde_json::json!({
                "summaries": format!(
                    "[{{\"scene_id\": \"{sc}\", \"summary\": \"主角抵达渡口\"}}]"
                )
            }))
            .unwrap();
        assert!(out.contains("已保存 1 条"), "{out}");

        // 原生数组（模型常无视 "string" 声明直接传），同样落盘
        let out = tool
            .execute(&serde_json::json!({
                "summaries": [
                    { "scene_id": sc, "summary": "渡口雾夜初逢" },
                ]
            }))
            .unwrap();
        assert!(out.contains("已保存 1 条"), "{out}");
        let structure = std::fs::read_to_string(dir.join("structure.toml")).unwrap();
        assert!(structure.contains("渡口雾夜初逢"), "{structure}");

        // 缺参数与空数组分别给出明确错误
        let err = tool.execute(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("缺少参数"), "{err}");
        let err = tool
            .execute(&serde_json::json!({ "summaries": "[]" }))
            .unwrap_err();
        assert!(err.contains("不能为空数组"), "{err}");
    }
}
