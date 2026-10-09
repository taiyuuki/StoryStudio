//! 知识层与正文工具：断言、关系、线索、片段文本读写。

use story_ai_runtime::provider::{GenerateRequest, Message};
use story_ai_runtime::{Tool, ToolDef};
use story_ir::StableId;
use story_project_format::FragmentFile;

use super::{
    get_json_array_opt, get_str, get_str_opt, get_str_array_opt, get_u32_opt,
    normalize_object_items, object_schema, ToolContext,
};

// ============================================================================
// add_assertion
// ============================================================================

pub struct AddAssertionTool {
    ctx: ToolContext,
}
impl AddAssertionTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AddAssertionTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "add_assertion".into(),
            description: "添加知识库断言，写入 knowledge/assertions.toml。\
            ⚠️ 断言记录的是上帝视角的事实真相（作者已确定的世界现实），\
            不是正文当前呈现的表象，也不是未决悬案：正文写了角色死亡但实为假死时，\
            应按真相断言（假死即 alive=true），\"读者/角色以为他死了\"用线索可见性\
            （advance_thread / set_thread_knowledge）体现；身份、位置等表象与真相\
            不符时同理——真相入断言，认知差走线索。\
            object 值会根据 predicate 自动转换类型：alive 用 true/false，age 用整数，其他用字符串。\
            时变谓词（located_at/status/alive/age）会自动将同主体同谓词的旧断言置为 superseded，\
            所以\"角色当前位置/状态/生死/年龄\"直接添加新值即可，无需先删旧值；\
            需要显式时间边界的事实（如\"拜师之前身份是学徒\"）用 valid_from_scene/valid_until_scene \
            声明场景锚生效区间（与关系边的时变声明同构）。\
            可多值谓词（occupation/holds 等）允许并存（一人可有多重职业），\
            但并存时回执会提示旧值——同一职位的演进（如拟聘→正式）请用 retire_assertion 废弃旧值。\
            完全相同的断言不会重复创建。一次沉淀多条事实（如章节回顾）用 assertions 批量传入。"
                .into(),
            parameters: object_schema(vec![
                ("subject", "string", "断言主体（角色 ID），单条添加时必填", false),
                (
                    "predicate",
                    "string",
                    "谓词：alive/age/holds/located_at/member_of/identity/status，\
                    也可用自定义谓词记录事件型事实（如 death_cause=\"非病亡\"）；单条添加时必填",
                    false,
                ),
                (
                    "object",
                    "string",
                    "断言值：alive 用 \"true\"/\"false\"；age 用数字（如 \"17\"）；\
                    holds/located_at/member_of 用实体 ID；identity 及自定义谓词用文本。系统会按 predicate 自动转换类型。",
                    false,
                ),
                (
                    "evidence_fragment",
                    "string",
                    "证据片段 ID（须为已存在的片段；系统自动登记证据条目并摘录正文开头快照）",
                    false,
                ),
                (
                    "valid_from_scene",
                    "string",
                    "断言生效的场景/片段 ID（叙事锚）：该场景起成立；省略 = 自故事开始。单条添加时可用（批量暂不支持）",
                    false,
                ),
                (
                    "valid_until_scene",
                    "string",
                    "断言失效的场景/片段 ID（不含该场景）：到该场景前已不成立；省略 = 直到故事结束",
                    false,
                ),
                (
                    "assertions",
                    "object-array",
                    "批量添加：数组，每项为原生 JSON 对象 {\"subject\": \"...\", \"predicate\": \"...\", \"object\": \"...\", \"evidence_fragment\": \"...\"(可选)}。与 subject/predicate/object 二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let paths = self.ctx.paths();

        // 批量：assertions 数组（兼容字符串与原生数组形态）
        if let Some(items) = get_json_array_opt(arguments, "assertions")? {
            if items.is_empty() {
                return Err("assertions 不能为空数组".into());
            }
            // 项归一化：模型常把每项序列化成字符串传入
            let items = normalize_object_items(items, "assertions")?;
            let mut entries: Vec<(String, String, String, Option<String>)> =
                Vec::with_capacity(items.len());
            for item in &items {
                entries.push((
                    get_str(item, "subject")?,
                    get_str(item, "predicate")?,
                    get_str(item, "object")?,
                    get_str_opt(item, "evidence_fragment"),
                ));
            }
            let outcomes = crate::mutations::add_assertions(&paths, &entries)
                .map_err(|e| e.to_string())?;

            let mut superseded_all: Vec<String> = Vec::new();
            let mut coexisting_count = 0usize;
            let mut parts: Vec<String> = Vec::with_capacity(entries.len());
            for ((subject, predicate, _, _), outcome) in entries.iter().zip(&outcomes) {
                if outcome.deduplicated {
                    parts.push(format!("{subject} {predicate}：已存在，跳过"));
                } else {
                    parts.push(format!("{}：{subject} {predicate}", outcome.id));
                }
                superseded_all.extend(outcome.superseded.iter().cloned());
                if !outcome.coexisting.is_empty() {
                    coexisting_count += 1;
                }
            }
            let mut msg = format!("断言批量处理完成：{}", parts.join("；"));
            if !superseded_all.is_empty() {
                msg.push_str(&format!(
                    "；旧断言 {} 已自动置为 superseded",
                    superseded_all.join(", ")
                ));
            }
            if coexisting_count > 0 {
                msg.push_str(&format!(
                    " ⚠ {coexisting_count} 条存在同谓词并存断言；\
                     若是同一身份或职位的演进（如拟聘→正式），请用 retire_assertion 废弃旧值"
                ));
            }
            return Ok(msg);
        }

        let subject = get_str(arguments, "subject")?;
        let predicate = get_str(arguments, "predicate")?;
        let object_val = get_str(arguments, "object")?;
        let evidence = get_str_opt(arguments, "evidence_fragment");
        let valid_from = get_str_opt(arguments, "valid_from_scene");
        let valid_until = get_str_opt(arguments, "valid_until_scene");

        let outcome = crate::mutations::add_assertion_windowed(
            &paths,
            &subject,
            &predicate,
            &object_val,
            evidence.as_deref(),
            valid_from.as_deref(),
            valid_until.as_deref(),
        )
        .map_err(|e| e.to_string())?;

        if outcome.deduplicated {
            return Ok(format!(
                "断言已存在（无需重复添加）: id={}, subject={}, predicate={}",
                outcome.id, subject, predicate
            ));
        }
        let mut msg = format!(
            "断言已添加: id={}, subject={}, predicate={}",
            outcome.id, subject, predicate
        );
        if valid_from.is_some() || valid_until.is_some() {
            msg.push_str("（已声明生效区间，query_assertions 可见边界）");
        }
        if !outcome.superseded.is_empty() {
            msg.push_str(&format!(
                "；旧断言 {} 已自动置为 superseded",
                outcome.superseded.join(", ")
            ));
        }
        if !outcome.coexisting.is_empty() {
            msg.push_str(&format!(
                " ⚠ 该主体已有同谓词 active 断言：{} = {}。多重身份/持有并存无需处理；\
                若是同一身份或职位的演进（如拟聘→正式），请用 retire_assertion 废弃旧值",
                predicate,
                outcome.coexisting.join("、")
            ));
        }
        Ok(msg)
    }
}

// ============================================================================
// retire_assertion — 废弃断言
// ============================================================================

pub struct RetireAssertionTool {
    ctx: ToolContext,
}
impl RetireAssertionTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RetireAssertionTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "retire_assertion".into(),
            description: "废弃一条断言（status 置为 superseded，保留历史记录）。\
            剧情推翻旧设定时使用；located_at/status/alive/age 类新值会自动取代旧值，无需手动废弃。"
                .into(),
            parameters: object_schema(vec![(
                "assertion_id",
                "string",
                "要废弃的断言 ID（a_XXXX，可从 query_assertions 获取）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let assertion_id = get_str(arguments, "assertion_id")?;
        let paths = self.ctx.paths();
        crate::mutations::set_assertion_status(&paths, &assertion_id, "superseded")
            .map_err(|e| e.to_string())?;
        Ok(format!("断言 {assertion_id} 已废弃（superseded），不再参与当前状态"))
    }
}

// ============================================================================
// delete_assertion — 彻底删除断言
// ============================================================================

pub struct DeleteAssertionTool {
    ctx: ToolContext,
}
impl DeleteAssertionTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteAssertionTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_assertion".into(),
            description: "彻底删除一条断言（从 assertions.toml 移除，含历史记录）。\
            误建的断言用此工具清理；剧情推进导致的旧值失效请用 retire_assertion（保留历史）。"
                .into(),
            parameters: object_schema(vec![(
                "assertion_id",
                "string",
                "要删除的断言 ID（a_XXXX）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let assertion_id = get_str(arguments, "assertion_id")?;
        let paths = self.ctx.paths();
        crate::mutations::delete_assertion(&paths, &assertion_id)
            .map_err(|e| e.to_string())?;
        Ok(format!("断言 {assertion_id} 已彻底删除"))
    }
}

// ============================================================================
// refresh_evidence — 重锚证据摘录
// ============================================================================

pub struct RefreshEvidenceTool {
    ctx: ToolContext,
}
impl RefreshEvidenceTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RefreshEvidenceTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "refresh_evidence".into(),
            description: "重锚证据摘录：重读片段正文，刷新 evidence.toml 里的登记基线。\
            正文被合理编辑、evidence_mismatch 报\"内容已偏离登记时的摘录\"且核实事实\
            仍成立后使用，告警即消。按 fragment_id 刷新该片段的全部证据条目，\
            或按 evidence_id 刷新单条。"
                .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "片段 ID（f_XXXX）：刷新指向它的全部证据条目。与 evidence_id 至少提供其一",
                    false,
                ),
                (
                    "evidence_id",
                    "string",
                    "证据 ID（ev_XXXX）：只刷新这一条。与 fragment_id 至少提供其一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str_opt(arguments, "fragment_id");
        let evidence_id = get_str_opt(arguments, "evidence_id");

        let paths = self.ctx.paths();
        let refreshed = crate::mutations::refresh_evidence(
            &paths,
            evidence_id.as_deref(),
            fragment_id.as_deref(),
        )
        .map_err(|e| e.to_string())?;

        let detail = refreshed
            .iter()
            .map(|(ev, frag)| format!("{ev}({frag})"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "已刷新 {} 条证据摘录基线：{detail}",
            refreshed.len()
        ))
    }
}

// ============================================================================
// add_relation
// ============================================================================

pub struct AddRelationTool {
    ctx: ToolContext,
}
impl AddRelationTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AddRelationTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "add_relation".into(),
            description: "添加角色之间的关系，写入 relations.toml。关系是单向的（from → to）；\
            对称关系（rival_of/family_of 等）传 symmetric=true 会自动创建反向边（成对，删除时也成对删）。\
            关系随剧情演变（相识→反目→和解）时用 valid_from_scene/valid_until_scene 声明生效区间\
            （场景锚，按阅读序比较），过期关系不用删除、保留演变轨迹。\
            一次添加多条（如建档后补全人物关系网）用 relations 批量传入。"
                .into(),
            parameters: object_schema(vec![
                ("from", "string", "主体角色 ID（如 c_8A91D4），单条添加时必填", false),
                ("to", "string", "目标角色 ID（如 c_A1B2C3），单条添加时必填", false),
                (
                    "relation",
                    "string",
                    "关系类型，常用：mentor_of/family_of/lover_of/enemy_of/rival_of/\
                    subordinate_of/superior_of/knows/friend_of/apprentice_of/\
                    employed_by(受雇于)/employer_of(雇佣)/owner_of(拥有)/guardian_of(监护)。\
                    也可用其他自定义类型（如 business_with），语义自行保持一致",
                    false,
                ),
                (
                    "valid_from_scene",
                    "string",
                    "关系生效的场景/片段 ID（叙事锚）：该场景起关系成立；省略 = 自故事开始",
                    false,
                ),
                (
                    "valid_until_scene",
                    "string",
                    "关系失效的场景/片段 ID（不含该场景）：到该场景前关系已不成立；省略 = 直到故事结束",
                    false,
                ),
                (
                    "symmetric",
                    "boolean",
                    "对称关系声明：自动同时创建 to→from 的反向边（同一关系类型）。适用于 rival_of/family_of/lover_of 等；有向关系（mentor_of 等）保持 false",
                    false,
                ),
                (
                    "relations",
                    "object-array",
                    "批量添加：数组，每项为原生 JSON 对象 {\"from\",\"to\",\"relation\",\"valid_from_scene?\",\"valid_until_scene?\",\"symmetric?\"}。与单条参数二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let paths = self.ctx.paths();

        // 批量：relations 数组（兼容字符串与原生数组形态）
        if let Some(items) = get_json_array_opt(arguments, "relations")? {
            if items.is_empty() {
                return Err("relations 不能为空数组".into());
            }
            // 项归一化：模型常把每项序列化成字符串传入
            let items = normalize_object_items(items, "relations")?;
            let mut inputs: Vec<crate::mutations::RelationInput> =
                Vec::with_capacity(items.len());
            for item in &items {
                inputs.push(relation_input_from_json(item)?);
            }
            let ids = crate::mutations::add_relation_full(&paths, &inputs)
                .map_err(|e| e.to_string())?;
            return Ok(format!("已批量添加 {} 条关系（含对称反向边）：{}", ids.len(), ids.join(", ")));
        }

        let from = get_str(arguments, "from")?;
        let to = get_str(arguments, "to")?;
        let relation = get_str(arguments, "relation")?;

        let input = crate::mutations::RelationInput {
            from,
            to,
            relation,
            ..relation_input_from_json(arguments)?
        };
        let has_window = input.valid_from_scene.is_some() || input.valid_until_scene.is_some();
        let ids = crate::mutations::add_relation_full(&paths, &[input])
            .map_err(|e| e.to_string())?;
        let mut msg = format!("关系已添加: id={}", ids[0]);
        if ids.len() > 1 {
            msg.push_str(&format!("（含对称反向边 {}）", ids[1]));
        }
        if has_window {
            msg.push_str("（已声明生效区间，get_relations 可按 at_scene 过滤时变关系）");
        }
        Ok(msg)
    }
}

/// 从 JSON 对象解析 RelationInput 的可选字段（窗口 + 对称声明）。
///
/// 单条与批量共用；from/to/relation 由调用方按各自必填规则解析后填充。
fn relation_input_from_json(item: &serde_json::Value) -> Result<crate::mutations::RelationInput, String> {
    let get_scene = |key: &str| -> Result<Option<String>, String> {
        match item.get(key) {
            None => Ok(None),
            Some(v) if v.is_null() => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(Some)
                .ok_or_else(|| format!("{key} 应为场景 ID 字符串")),
        }
    };
    Ok(crate::mutations::RelationInput {
        from: String::new(),
        to: String::new(),
        relation: String::new(),
        valid_from_scene: get_scene("valid_from_scene")?,
        valid_until_scene: get_scene("valid_until_scene")?,
        symmetric: item
            .get("symmetric")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

// ============================================================================
// remove_relation
// ============================================================================

pub struct RemoveRelationTool {
    ctx: ToolContext,
}
impl RemoveRelationTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RemoveRelationTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "remove_relation".into(),
            description: "移除角色之间的关系边，从 relations.toml 删除。\
            若该边是对称声明的成对边（存在同类型的反向边），回执会提示孪生边 ID，需一并删除。\
            时变关系（有生效区间）过期后无需删除——保留演变轨迹即可。"
                .into(),
            parameters: object_schema(vec![(
                "relation_id",
                "string",
                "关系边 ID（se_ 前缀）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let relation_id = get_str(arguments, "relation_id")?;

        let paths = self.ctx.paths();
        let twin = crate::mutations::find_symmetric_twin(&paths, &relation_id);
        crate::mutations::remove_relation(&paths, &relation_id).map_err(|e| e.to_string())?;

        let mut msg = format!("关系已移除: {relation_id}");
        if let Some(twin_id) = twin {
            msg.push_str(&format!(
                "；⚠️ 存在同类型的反向边 {twin_id}（对称声明创建），如需彻底移除请一并 remove_relation"
            ));
        }
        Ok(msg)
    }
}

// ============================================================================
// update_relation
// ============================================================================

pub struct UpdateRelationTool {
    ctx: ToolContext,
}
impl UpdateRelationTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateRelationTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_relation".into(),
            description: "更新关系边的生效区间（时变关系）。\
            用场景/片段 ID 声明边界：valid_from_scene = 关系成立的起点，\
            valid_until_scene = 关系失效的位置（不含该场景）。\
            传空字符串清除对应边界（改为无界），省略 = 不修改。\
            例：师徒决裂 → update_relation(se_0001, valid_until_scene=\"sc_05_02\")。"
                .into(),
            parameters: object_schema(vec![
                ("relation_id", "string", "关系边 ID（se_ 前缀）", true),
                (
                    "valid_from_scene",
                    "string",
                    "生效场景/片段 ID（叙事锚）；空字符串清除该边界；省略 = 不修改",
                    false,
                ),
                (
                    "valid_until_scene",
                    "string",
                    "失效场景/片段 ID（不含该场景）；空字符串清除该边界；省略 = 不修改",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let relation_id = get_str(arguments, "relation_id")?;

        // 空串 = 清除边界；缺省 = 不修改（None 外层）
        let bound = |key: &str| -> Option<Option<String>> {
            arguments
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(Some)
                .or_else(|| {
                    // 显式传了空串 → 清除
                    arguments
                        .get(key)
                        .and_then(|v| v.as_str())
                        .map(|_| None)
                })
        };
        let from = bound("valid_from_scene");
        let until = bound("valid_until_scene");
        if from.is_none() && until.is_none() {
            return Err(
                "至少提供一个要修改的边界（valid_from_scene / valid_until_scene，空串表示清除）"
                    .into(),
            );
        }

        let paths = self.ctx.paths();
        crate::mutations::update_relation_valid(
            &paths,
            &relation_id,
            from.as_ref().map(|inner| inner.as_deref()),
            until.as_ref().map(|inner| inner.as_deref()),
        )
        .map_err(|e| e.to_string())?;

        let mut msg = format!("关系 {relation_id} 生效区间已更新");
        if let Some(Some(scene)) = &from {
            msg.push_str(&format!("（自 {scene} 起）"));
        }
        if let Some(None) = &from {
            msg.push_str("（起点已清除=自故事开始）");
        }
        if let Some(Some(scene)) = &until {
            msg.push_str(&format!("（至 {scene} 前止）"));
        }
        if let Some(None) = &until {
            msg.push_str("（终点已清除=直到故事结束）");
        }
        Ok(msg)
    }
}

// ============================================================================
// create_thread
// ============================================================================

pub struct CreateThreadTool {
    ctx: ToolContext,
}
impl CreateThreadTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for CreateThreadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "create_thread".into(),
            description: "创建叙事线索（伏笔/信息流），写入 threads.toml。".into(),
            parameters: object_schema(vec![
                ("title", "string", "线索标题", true),
                ("description", "string", "线索描述", true),
                ("priority", "string", "优先级（main/minor）", false),
                (
                    "scope",
                    "string",
                    "可见性范围：reader（默认，面向读者的明线，须先揭示后回收）/ \
                    pov（暗线：POV 角色私人闭环，允许直接回收，不要求先揭示）",
                    false,
                ),
                ("planted_at", "string", "埋设位置（场景 ID 或片段 ID，如 sc_01_01 / f_0003）", false),
                (
                    "due_volume",
                    "integer",
                    "计划埋设卷号（如 1 = 第一卷内应埋设）。到卷仍未埋会报「本卷该埋未埋」；\
                    不设则视为全书长线，仅在低级别提示",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let title = get_str(arguments, "title")?;
        let description = get_str(arguments, "description")?;
        let priority = get_str_opt(arguments, "priority").unwrap_or_else(|| "minor".to_string());
        let planted = get_str_opt(arguments, "planted_at");
        let due_volume = get_u32_opt(arguments, "due_volume");
        let scope = get_str_opt(arguments, "scope");

        let paths = self.ctx.paths();
        let new_id = crate::mutations::create_thread(
            &paths,
            &title,
            &description,
            &priority,
            planted.as_deref(),
            due_volume,
            scope.as_deref(),
        )
        .map_err(|e| e.to_string())?;

        Ok(format!("线索已创建: id={new_id}"))
    }
}

// ============================================================================
// get_threads
// ============================================================================

/// 从 ThreadDef 的 loose-typed 字段推导状态（与 ThreadStatus::derive 逻辑一致）。
pub(super) fn derive_thread_status(t: &story_project_format::threads_file::ThreadDef) -> &'static str {
    if t.abandoned {
        "abandoned"
    } else if t.resolved_at.is_some() {
        "resolved"
    } else if t.revealed_at.is_some() {
        "revealed"
    } else if t.planted_at.is_some() {
        "planted"
    } else {
        "planned"
    }
}

pub struct GetThreadsTool {
    ctx: ToolContext,
}
impl GetThreadsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetThreadsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_threads".into(),
            description: "查看叙事线索列表（伏笔/信息流）。可选按状态或优先级过滤，\
            不传过滤参数则返回全部。线索状态：planned(计划)/planted(已埋)/revealed(已揭示)/resolved(已回收)/abandoned(已放弃)。"
                .into(),
            parameters: object_schema(vec![
                (
                    "status",
                    "string",
                    "按状态过滤: planned / planted / revealed / resolved / abandoned",
                    false,
                ),
                (
                    "priority",
                    "string",
                    "按优先级过滤: main / minor",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let status_filter = get_str_opt(arguments, "status");
        let priority_filter = get_str_opt(arguments, "priority");

        let threads_path = self.ctx.paths().threads_toml();

        if !threads_path.exists() {
            return Ok("尚无线索（threads.toml 不存在）。使用 create_thread 创建。".to_string());
        }

        let content =
            std::fs::read_to_string(&threads_path).map_err(|e| format!("读取失败: {e}"))?;
        if content.trim().is_empty() {
            return Ok("尚无线索。使用 create_thread 创建。".to_string());
        }

        let file: story_project_format::ThreadsFile =
            story_project_format::ThreadsFile::from_toml_str(&content)
                .map_err(|e| format!("解析失败: {e}"))?;

        if file.threads.is_empty() {
            return Ok("尚无线索。使用 create_thread 创建。".to_string());
        }

        let lines: Vec<String> = file
            .threads
            .iter()
            .filter(|t| {
                if let Some(ref pf) = priority_filter {
                    if t.priority != *pf {
                        return false;
                    }
                }
                if let Some(ref sf) = status_filter {
                    if derive_thread_status(t) != sf.as_str() {
                        return false;
                    }
                }
                true
            })
            .map(|t| {
                let status = derive_thread_status(t);
                let scope = if t.scope.as_deref() == Some("pov") {
                    "/pov暗线"
                } else {
                    ""
                };
                let desc = t
                    .description
                    .as_deref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default();
                format!(
                    "{} [{}/{}{}] {}{}",
                    t.id,
                    t.priority,
                    status,
                    scope,
                    t.title,
                    desc
                )
            })
            .collect();

        if lines.is_empty() {
            Ok("没有匹配的线索。".to_string())
        } else {
            Ok(format!(
                "找到 {} 条线索:\n{}",
                lines.len(),
                lines.join("\n")
            ))
        }
    }
}

// ============================================================================
// update_thread
// ============================================================================

pub struct UpdateThreadTool {
    ctx: ToolContext,
}
impl UpdateThreadTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for UpdateThreadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "update_thread".into(),
            description: "修改已有线索的核心字段（title/description/goal/priority/tags/due_volume/scope），\
            只更新提供的字段。tags 以逗号分隔，整体覆盖。操作前先 get_threads 确认 ID。"
                .into(),
            parameters: object_schema(vec![
                ("thread_id", "string", "线索 ID（如 t_001）", true),
                ("title", "string", "线索标题", false),
                ("description", "string", "线索描述", false),
                ("goal", "string", "叙事目标（给 AI 的上下文）", false),
                ("priority", "string", "优先级: main / minor", false),
                (
                    "scope",
                    "string",
                    "可见性范围: reader（明线，默认）/ pov（暗线，回收不要求先揭示）",
                    false,
                ),
                ("tags", "array", "标签列表（原生数组或逗号分隔字符串，整体覆盖）", false),
                (
                    "due_volume",
                    "integer",
                    "计划埋设卷号（如 1 = 第一卷内应埋设）；传 0 表示清除、恢复全书长线",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let thread_id = get_str(arguments, "thread_id")?;
        let title = get_str_opt(arguments, "title");
        let description = get_str_opt(arguments, "description");
        let goal = get_str_opt(arguments, "goal");
        let priority = get_str_opt(arguments, "priority");
        let tags = get_str_array_opt(arguments, "tags")?;
        // None=不动；Some(n)=设为第 n 卷（0 清除）
        let due_volume = arguments.get("due_volume").map(|_| {
            get_u32_opt(arguments, "due_volume")
        });
        let scope = get_str_opt(arguments, "scope");

        let paths = self.ctx.paths();
        let changed = crate::mutations::update_thread(
            &paths,
            &thread_id,
            title.as_deref(),
            description.as_deref(),
            goal.as_deref(),
            priority.as_deref(),
            tags.as_deref(),
            due_volume,
            scope.as_deref(),
        )
        .map_err(|e| e.to_string())?;

        if changed.is_empty() {
            Ok(format!("线索 {thread_id} 未修改（未提供任何字段）"))
        } else {
            Ok(format!("线索 {thread_id} 已更新: {}", changed.join(", ")))
        }
    }
}

// ============================================================================
// advance_thread
// ============================================================================

pub struct AdvanceThreadTool {
    ctx: ToolContext,
}
impl AdvanceThreadTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AdvanceThreadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "advance_thread".into(),
            description: "推进线索的叙事状态。伏笔最核心的工具——用于标记线索的进展。\
            status 取值：planted(埋下)/revealed(揭示给读者)/resolved(回收闭环)/abandoned(放弃)。\
            planted/revealed/resolved 需要提供 scene_id 锚点；abandoned 不需要。\
            前向推进，不可回退。操作前先 get_threads 确认当前状态。\
            章节回顾等场景一次推进多条线索时，用 advances 批量传入。"
                .into(),
            parameters: object_schema(vec![
                ("thread_id", "string", "线索 ID（如 t_001）；单条推进时必填", false),
                (
                    "status",
                    "string",
                    "目标状态: planted / revealed / resolved / abandoned；单条推进时必填",
                    false,
                ),
                (
                    "scene_id",
                    "string",
                    "叙事锚点（场景 ID 或片段 ID，如 sc_01_01 / f_0003；planted/revealed/resolved 必填，abandoned 忽略）。\
                    揭示/收束时 reader（读者视角）会自动记为 known，无需手工补 set_thread_knowledge；\
                    角色层知情仍需按人物逐个设置",
                    false,
                ),
                (
                    "advances",
                    "object-array",
                    "批量推进：数组，每项为原生 JSON 对象 {\"thread_id\": \"...\", \"status\": \"...\", \"scene_id\": \"...\"(abandoned 可省)}。与 thread_id/status 二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let paths = self.ctx.paths();

        // 批量：advances 数组（兼容字符串与原生数组形态）
        if let Some(items) = get_json_array_opt(arguments, "advances")? {
            if items.is_empty() {
                return Err("advances 不能为空数组".into());
            }
            // 项归一化：模型常把每项序列化成字符串传入
            let items = normalize_object_items(items, "advances")?;
            let mut entries: Vec<(String, String, Option<String>)> =
                Vec::with_capacity(items.len());
            for item in &items {
                entries.push((
                    get_str(item, "thread_id")?,
                    get_str(item, "status")?,
                    get_str_opt(item, "scene_id"),
                ));
            }
            let results = crate::mutations::advance_threads(&paths, &entries)
                .map_err(|e| e.to_string())?;

            let parts: Vec<String> = entries
                .iter()
                .zip(&results)
                .map(|((thread_id, _, _), new_status)| format!("{thread_id}→{new_status}"))
                .collect();
            return Ok(format!(
                "已批量推进 {} 条线索：{}",
                results.len(),
                parts.join("；")
            ));
        }

        let thread_id = get_str(arguments, "thread_id")?;
        let status = get_str(arguments, "status")?;
        let scene_id = get_str_opt(arguments, "scene_id");

        let new_status =
            crate::mutations::advance_thread(&paths, &thread_id, &status, scene_id.as_deref())
                .map_err(|e| e.to_string())?;

        Ok(format!("线索 {thread_id} 状态已推进: {new_status}"))
    }
}

// ============================================================================
// delete_thread
// ============================================================================

pub struct DeleteThreadTool {
    ctx: ToolContext,
}
impl DeleteThreadTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteThreadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_thread".into(),
            description: "删除线索（按 ID）。删除不可恢复，引用此线索的条目会变成悬空引用。".into(),
            parameters: object_schema(vec![("thread_id", "string", "线索 ID（如 t_001）", true)]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let thread_id = get_str(arguments, "thread_id")?;

        let paths = self.ctx.paths();
        crate::mutations::delete_thread(&paths, &thread_id).map_err(|e| e.to_string())?;

        Ok(format!("线索已删除: {thread_id}"))
    }
}

// ============================================================================
// get_thread_detail
// ============================================================================

/// 将 ObserverDef 渲染为可读文本
pub(super) fn format_observer(o: &story_project_format::threads_file::ObserverDef) -> String {
    use story_project_format::threads_file::ObserverDef;
    match o {
        ObserverDef::Builtin(s) => match s.as_str() {
            "reader" => "读者".to_string(),
            "author" => "作者".to_string(),
            "omniscient" => "全知视角".to_string(),
            other => other.to_string(),
        },
        ObserverDef::Character { character } => format!("角色 {character}"),
        ObserverDef::Organization { organization } => format!("组织 {organization}"),
    }
}

/// 将 knowledge state 渲染为中文
pub(super) fn format_knowledge_state(s: &str) -> &str {
    match s {
        "unknown" => "未知",
        "suspected" => "怀疑",
        "known" => "知晓",
        other => other,
    }
}

pub struct GetThreadDetailTool {
    ctx: ToolContext,
}
impl GetThreadDetailTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetThreadDetailTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_thread_detail".into(),
            description: "查看单条线索的完整信息：标题、描述、目标、状态、叙事锚点、\
            可见性时间线（哪些角色/读者在哪个场景知道或不知道）、关联关系。"
                .into(),
            parameters: object_schema(vec![("thread_id", "string", "线索 ID（如 t_001）", true)]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let thread_id = get_str(arguments, "thread_id")?;
        let threads_path = self.ctx.paths().threads_toml();

        let content = std::fs::read_to_string(&threads_path)
            .map_err(|e| format!("读取 threads.toml 失败: {e}"))?;
        let file: story_project_format::ThreadsFile =
            story_project_format::ThreadsFile::from_toml_str(&content)
                .map_err(|e| format!("解析失败: {e}"))?;

        let t = file
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| format!("线索 '{thread_id}' 不存在"))?;

        let status = derive_thread_status(t);
        let mut out = String::new();
        out.push_str("线索详情:\n");
        out.push_str(&format!("  ID: {}\n", t.id));
        out.push_str(&format!("  标题: {}\n", t.title));
        if let Some(ref d) = t.description {
            out.push_str(&format!("  描述: {d}\n"));
        }
        if let Some(ref g) = t.goal {
            out.push_str(&format!("  目标: {g}\n"));
        }
        out.push_str(&format!("  优先级: {}\n", t.priority));
        out.push_str(&format!("  状态: {status}\n"));
        if !t.tags.is_empty() {
            out.push_str(&format!("  标签: {}\n", t.tags.join(", ")));
        }
        if let Some(ref s) = t.planted_at {
            out.push_str(&format!("  埋设: {s}\n"));
        }
        if let Some(ref s) = t.revealed_at {
            out.push_str(&format!("  揭示: {s}\n"));
        }
        if let Some(ref s) = t.resolved_at {
            out.push_str(&format!("  回收: {s}\n"));
        }
        if t.abandoned {
            out.push_str("  ⚠ 已放弃\n");
        }

        // 可见性
        if !t.knowledge.is_empty() {
            out.push_str("\n  可见性:\n");
            for k in &t.knowledge {
                let observer = format_observer(&k.observer);
                let timeline: Vec<String> = k
                    .state_timeline
                    .iter()
                    .map(|e| format!("{}({})", format_knowledge_state(&e.state), e.scene))
                    .collect();
                out.push_str(&format!("    {observer}: {}\n", timeline.join(" → ")));
            }
        }

        // 关联关系（只读展示）
        if !t.relationships.is_empty() {
            out.push_str("\n  关联关系:\n");
            for r in &t.relationships {
                let entities = r.entities.join(", ");
                let rel = r
                    .relation
                    .as_deref()
                    .map(|s| format!(" [{s}]"))
                    .unwrap_or_default();
                let desc = r
                    .description
                    .as_deref()
                    .map(|s| format!(" — {s}"))
                    .unwrap_or_default();
                out.push_str(&format!("    {entities}{rel}{desc}\n"));
            }
        }

        Ok(out)
    }
}

// ============================================================================
// set_thread_knowledge
// ============================================================================

pub struct SetThreadKnowledgeTool {
    ctx: ToolContext,
}
impl SetThreadKnowledgeTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SetThreadKnowledgeTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "set_thread_knowledge".into(),
            description: "记录线索的可见性变化——某观察者在某场景的知晓程度。\
            这是管理信息流的核心工具：跟踪谁（读者/角色/组织）在什么时候知道了什么。\
            同一观察者的记录会累积为时间线。\
            一场戏后多名角色同时获知用 observers 数组（同一线索/场景/状态批量应用）；\n            不同线索或不同状态的组合用 records 批量传入。"
                .into(),
            parameters: object_schema(vec![
                ("thread_id", "string", "线索 ID（如 t_001）；单条记录时必填", false),
                (
                    "observer",
                    "string",
                    "观察者：reader(读者) / author(作者) / omniscient(全知) / 角色ID(如c_001) / 组织ID(如o_001)；单条记录时必填",
                    false,
                ),
                (
                    "state",
                    "string",
                    "知晓程度：unknown(不知道) / suspected(怀疑) / known(知道)；单条记录时必填",
                    false,
                ),
                (
                    "scene_id",
                    "string",
                    "发生变化的场景 ID（如 sc_01_02）；单条记录时必填",
                    false,
                ),
                (
                    "observers",
                    "array",
                    "多名观察者便捷形态：观察者 ID 数组（如 [\"c_0001\",\"c_0002\"]），同一线索/场景/状态批量应用——一场戏后多人同时获知时用它，免去逐条拼 records",
                    false,
                ),
                (
                    "records",
                    "object-array",
                    "批量记录：数组，每项为原生 JSON 对象 {\"thread_id\": \"...\", \"observer\": \"...\", \"state\": \"...\", \"scene_id\": \"...\"}。与单条参数二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let paths = self.ctx.paths();

        // 批量：records 数组（兼容字符串与原生数组形态）
        if let Some(items) = get_json_array_opt(arguments, "records")? {
            if items.is_empty() {
                return Err("records 不能为空数组".into());
            }
            // 项归一化：模型常把每项序列化成字符串传入
            let items = normalize_object_items(items, "records")?;
            let mut entries: Vec<(String, String, String, String)> =
                Vec::with_capacity(items.len());
            for item in &items {
                entries.push((
                    get_str(item, "thread_id")?,
                    get_str(item, "observer")?,
                    get_str(item, "state")?,
                    get_str(item, "scene_id")?,
                ));
            }
            crate::mutations::set_thread_knowledge_batch(&paths, &entries)
                .map_err(|e| e.to_string())?;

            let parts: Vec<String> = entries
                .iter()
                .map(|(thread_id, observer, state, scene_id)| {
                    format!("{thread_id}：{observer} 在 {scene_id} 变为 {state}")
                })
                .collect();
            return Ok(format!(
                "已批量记录 {} 条可见性：{}",
                entries.len(),
                parts.join("；")
            ));
        }

        let thread_id = get_str(arguments, "thread_id")?;
        let state = get_str(arguments, "state")?;
        let scene_id = get_str(arguments, "scene_id")?;

        // observers 便捷形态：同一线索、同一场景、同一状态，多名观察者
        // 一场戏后集体获知——免去拼 N 条几乎相同的 records
        let observers: Vec<String> = match get_json_array_opt(arguments, "observers")? {
            Some(items) => {
                let list: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                if list.is_empty() {
                    return Err("observers 不能为空数组".into());
                }
                list
            }
            None => vec![get_str(arguments, "observer")?],
        };

        let entries: Vec<(String, String, String, String)> = observers
            .iter()
            .map(|o| {
                (
                    thread_id.clone(),
                    o.clone(),
                    state.clone(),
                    scene_id.clone(),
                )
            })
            .collect();
        crate::mutations::set_thread_knowledge_batch(&paths, &entries)
            .map_err(|e| e.to_string())?;

        Ok(format!(
            "可见性已记录: 线索 {thread_id} — {} 在 {scene_id} 变为 {state}",
            observers.join("、")
        ))
    }
}

// ============================================================================
// write_fragment_text
// ============================================================================

pub struct WriteFragmentTextTool {
    ctx: ToolContext,
}
impl WriteFragmentTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for WriteFragmentTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "write_fragment_text".into(),
            description:
                "写入或覆盖场景的正文片段。创建新片段时省略 fragment_id（自动生成新 ID）。\
            传入已有 fragment_id 则覆盖该片段的全部内容。\
            ⚠️ 要追加内容到已有片段请用 append_fragment_text，不要用此工具。\
            正文一般应达 800 字以上、包含完整叙事；刻意的短片段（收尾/过渡/暗线视角）\
            传 short=true 声明，跳过字数下限警示。注意：正文不要以 --- 分隔线起笔。"
                    .into(),
            parameters: object_schema(vec![
                ("scene_id", "string", "目标场景 ID（如 sc_01_01）", true),
                (
                    "text",
                    "string",
                    "完整的故事正文内容（至少 800 字，包含对话、动作、环境、心理描写）",
                    true,
                ),
                (
                    "fragment_id",
                    "string",
                    "已有片段 ID（仅覆盖已有片段时传入；创建新片段时省略）",
                    false,
                ),
                (
                    "short",
                    "boolean",
                    "声明为刻意短片段（收尾/过渡/暗线视角等），跳过 800 字下限警示。默认 false",
                    false,
                ),
                (
                    "scene_summary",
                    "string",
                    "顺手附带场景摘要（一两句话）：正文落盘的同时写入 scene.summary，省一次 update_scene 往返。摘要就在你刚写的正文里，提炼即可",
                    false,
                ),
                (
                    "scene_time",
                    "string",
                    "顺手附带故事时间：数字（1023-9-15 / 1023）或历法标签（灵历3217年），写入 scene.story_time",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scene_id = get_str(arguments, "scene_id")?;
        let text = get_str(arguments, "text")?;
        let fragment_id_opt = get_str_opt(arguments, "fragment_id");
        let scene_summary = get_str_opt(arguments, "scene_summary");
        let scene_time_raw = get_str_opt(arguments, "scene_time");


        let paths = self.ctx.paths();

        let (fragment_id, is_new) = if let Some(fid) = fragment_id_opt {
            // 覆盖已有片段：不存在则拒绝，避免造出脱离结构树的"孤儿片段"
            // （文件存在但不在任何场景里，get_structure 看不到、阅读顺序丢失）
            let frag_path = paths
                .file_for_id(&fid)
                .ok_or_else(|| format!("无效的片段 ID: {fid}"))?;
            if !frag_path.exists() {
                return Err(format!(
                    "片段 {fid} 不存在。创建新片段请省略 fragment_id（在场景 {scene_id} 下自动生成新 ID）；\
                    覆盖已有片段请传入 get_structure 中列出的 ID"
                ));
            }
            crate::mutations::write_fragment_text_source(
                &paths,
                &fid,
                &text,
                crate::stats::WriteSource::Ai,
            )
                .map_err(|e| e.to_string())?;
            (fid, false)
        } else {
            // 创建新片段（AI 来源记账——create_fragment 固定记手写，
            // 妙笔新建的正文会全部错记进手写账本）
            let fid = crate::mutations::create_fragment_anchored_source(
                &paths,
                &scene_id,
                &text,
                &[],
                crate::stats::WriteSource::Ai,
            )
            .map_err(|e| e.to_string())?;
            (fid, true)
        };

        let word_count = crate::workflow::count_words(&text) as usize;
        let declared_short = arguments
            .get("short")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let action = if is_new { "创建" } else { "更新" };
        let mut msg = format!(
            "片段已{action}: id={fragment_id}, scene={scene_id}, 字数={word_count}"
        );
        if word_count < 800 && !declared_short {
            msg.push_str(" ⚠ 低于建议下限 800 字，如非刻意留短（如收尾短章），建议扩写完整叙事；\
            刻意短片段（收尾/过渡/暗线视角）可传 short=true 声明，跳过此警示");
        }
        // 写入时理解锚定：LLM 指代消解，把"那人/佣兵"这类文字匹配认不到的
        // 出场落为显式锚点（best-effort，失败不影响已完成的写入）
        if let Some(provider) = self.ctx.provider.as_ref() {
            match super::anchoring::anchor_and_apply(&self.ctx, provider, &fragment_id) {
                Ok(o) => msg.push_str(&super::format_anchor_note(&o, &paths)),
                Err(e) => msg.push_str(&format!("\n（指代锚定跳过：{e}）")),
            }
        }

        // 写前档案推送：把正文中出现的实体的档案要点附在回执里，
        // 让创作者第一时间看见设定（如职业/种族），防正文与档案冲突
        let briefs = super::mentioned_entity_briefs(&paths, &text, 8);
        if !briefs.is_empty() {
            msg.push_str(&format!("\n出场实体（正文匹配，档案要点）：\n{}", {
                let lines: Vec<String> = briefs.iter().map(|b| format!("  - {b}")).collect();
                lines.join("\n")
            }));
        }
        // 覆盖前的旧文已留版本快照（节流），可回滚——回执明示，鼓励大胆覆盖
        if let Some(note) = crate::history::fresh_snapshot_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 顺手附带的场景元数据：同一次调用落盘，省一次 update_scene 往返
        let story_time = match &scene_time_raw {
            Some(raw) => match super::structure::parse_story_time(raw) {
                Ok(def) => def,
                Err(e) => return Err(format!("scene_time 解析失败: {e}（正文已写入，仅元数据未落）")),
            },
            None => None,
        };
        if scene_summary.is_some() || story_time.is_some() {
            let update = crate::mutations::SceneUpdate {
                title: None,
                pov: None,
                mood: None,
                function: None,
                summary: scene_summary.as_deref().map(Some),
                story_time: story_time.clone().map(Some),
            };
            match crate::mutations::update_scene(&paths, &scene_id, &update) {
                Ok(fields) => {
                    let fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                    msg.push_str(&format!("\n✅ 已同步场景元数据: {}", fields.join(", ")));
                }
                Err(e) => msg.push_str(&format!("\n⚠️ 场景元数据落盘失败: {e}（正文已写入）")),
            }
        }
        // 覆盖已有片段时：若它是断言证据，提示事实依据可能漂移（best-effort）
        if !is_new {
            if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, false) {
                msg.push_str(&note);
            }
        }
        // 首次写入正文自动推进脉络 planned → writing（best-effort）
        if let Some(note) = crate::mutations::auto_advance_outline_writing(&paths, &scene_id) {
            msg.push_str(&note);
        }
        // 本章所有场景都有正文时提示收尾（best-effort，不影响写入）
        if let Some(hint) = crate::mutations::chapter_completion_hint(&paths, &scene_id) {
            msg.push_str(&hint);
        }
        // 场景已有正文但缺 summary/story_time：提示顺手补齐（best-effort）
        if let Some(note) = crate::mutations::scene_metadata_gap_note(&paths, &scene_id) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// edit_fragment_text — 精准搜索-替换（局部修改）
// ============================================================================

pub struct EditFragmentTextTool {
    ctx: ToolContext,
}
impl EditFragmentTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for EditFragmentTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "edit_fragment_text".into(),
            description:
                "精准修改片段正文的局部内容：在指定片段中搜索 old_text，替换为 new_text。\
            适用于只改几句话或一个段落的场景（比 rewrite_fragment 更精确可控，不依赖 LLM 改写）。\
            ⚠️ 参数顺序勿颠倒：old_text 是正文里【已有】的文字，new_text 是【要改成】的文字。\
            ⚠️ old_text 应从正文中原样复制（先 read_scene_full_text / read_fragment_text 取原文）。\
            匹配为三级兜底：精确 → 归一化（容忍空白/换行/全角空格差异与中英文引号互换）\
            → 近似（与正文有 60% 以上文字重合的连续段落，适合引文被轻微转写的场景，\
            命中时回执会提示相似度，建议核对）。\
            默认要求 old_text 在正文中唯一出现；若多次出现需加 replace_all=true（否则报错并列出所有位置）。\
            要覆盖整篇正文请用 write_fragment_text，要续写请用 append_fragment_text。"
                    .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "要修改的片段 ID（如 f_0001）",
                    true,
                ),
                (
                    "old_text",
                    "string",
                    "要被替换的原文片段，从正文中原样复制。匹配三级兜底：精确 → 归一化（容忍空白/换行/引号差异）→ 近似（60% 文字重合）",
                    true,
                ),
                (
                    "new_text",
                    "string",
                    "替换后的新文本（可为空字符串以删除原文片段）",
                    true,
                ),
                (
                    "replace_all",
                    "boolean",
                    "old_text 多次出现时是否全部替换。默认 false（要求唯一匹配，多次出现会报错）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let old_text = get_str(arguments, "old_text")?;
        let new_text = arguments
            .get("new_text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "缺少参数: new_text".to_string())?;
        // new_text 允许为空（删除场景），但 get_str 会拒绝空串，故单独取值
        let replace_all = arguments
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_text == new_text {
            return Err("new_text 与 old_text 相同，无需替换".to_string());
        }

        // 读取并精确解析片段
        let paths = self.ctx.paths();
        let frag_path = paths
            .file_for_id(&fragment_id)
            .ok_or_else(|| format!("无效的 fragment ID: {fragment_id}"))?;
        if !frag_path.exists() {
            return Err(format!("片段 '{fragment_id}' 不存在"));
        }
        let content =
            std::fs::read_to_string(&frag_path).map_err(|e| format!("读取片段失败: {e}"))?;
        let frag = FragmentFile::parse(&content).map_err(|e| format!("解析片段失败: {e}"))?;

        // 匹配策略（三级，与 ai_check 的引文归因容错对齐）：
        // 精确字节匹配 → 去空白/标点归一化 → LCS 近似（引文被轻微转写时定位最接近段落）
        // 统一用字节范围 (from, to) 表示匹配结果
        let exact_ranges: Vec<(usize, usize)> = find_all_occurrences(&frag.text, &old_text)
            .iter()
            .map(|&pos| (pos, pos + old_text.len()))
            .collect();

        let (ranges, approx_similarity) = if !exact_ranges.is_empty() {
            (exact_ranges, None)
        } else {
            match find_fuzzy_ranges(&frag.text, &old_text) {
                Some(fuzzy) if !fuzzy.is_empty() => (fuzzy, None),
                _ => match find_lcs_range(&frag.text, &old_text) {
                    Some((range, similarity)) => (vec![range], Some(similarity)),
                    None => {
                        // 三级匹配全部失败 → 找最接近的片段帮助定位
                        let hint = find_closest_excerpt(&frag.text, &old_text);
                        let mut msg = "old_text 未在正文中找到（精确、归一化、近似三级匹配均失败；\
                            归一化已容忍空白/换行差异及常见中英文标点互换，\
                            近似要求与正文有 60% 以上文字重合且为连续段落）。\n\
                            可能原因：文字与原文出入过大，或引文拼接了正文里不相邻的内容。\n\
                            建议：用 read_scene_full_text / read_fragment_text 重新读取，复制精确文本。\n"
                            .to_string();
                        // 填反检测：old_text 找不到而 new_text 能在正文中精确找到，
                        // 十有八九是两个参数互换了（反向替换的前兆）
                        if !new_text.is_empty() && !find_all_occurrences(&frag.text, &new_text).is_empty()
                        {
                            msg.push_str(&format!(
                                "\n⚠️ 疑似 old_text 与 new_text 填反：new_text「{new_text}」\
                                在正文中找得到而 old_text 没有。请互换两个参数后重试。"
                            ));
                        }
                        if let Some(hint) = hint {
                            msg.push_str(&format!("\n最接近的正文片段（供参考）：\n  \"{hint}\""));
                        }
                        msg.push_str(&format!("\n你提供的 old_text:\n  \"{old_text}\""));
                        return Err(msg);
                    }
                },
            }
        };

        if let Some(sim) = approx_similarity {
            let pct = (sim * 100.0).round() as u8;
            eprintln!("[edit_fragment_text] {fragment_id} 使用了 LCS 近似匹配（相似度 {pct}%）");
        }

        if ranges.len() > 1 && !replace_all {
            // 歧义：列出所有出现位置，引导消歧
            let excerpts = build_range_excerpts(&ranges, &frag.text, 30);
            let mut msg = format!(
                "old_text 在正文中出现 {} 次，无法确定替换哪一个。\
                请扩大 old_text 范围使其唯一，或设 replace_all=true。\n",
                ranges.len()
            );
            msg.push_str(&excerpts);
            return Err(msg);
        }

        // 执行替换：按字节范围替换（精确和模糊统一处理）
        // 从后往前替换，避免位置偏移
        let mut new_body = frag.text.clone();
        for &(from, to) in ranges.iter().rev() {
            new_body.replace_range(from..to, &new_text);
        }

        let original_len = crate::workflow::count_words(&frag.text) as usize;
        let new_len = crate::workflow::count_words(&new_body) as usize;
        let replace_count = if replace_all { ranges.len() } else { 1 };

        // 写回（保留 frontmatter / 锚点）
        crate::mutations::write_fragment_text_source(
            &paths,
            &fragment_id,
            &new_body,
            crate::stats::WriteSource::Ai,
        )
            .map_err(|e| e.to_string())?;

        let mut msg = format!(
            "片段 {fragment_id} 已修改（原 {original_len} 字 → 新 {new_len} 字，替换 {replace_count} 处）"
        );
        if let Some(note) = crate::history::fresh_snapshot_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 近似匹配的替换不是逐字对应：明示请核对，防止"改错段落"被当成功
        if let Some(sim) = approx_similarity {
            let pct = (sim * 100.0).round() as u8;
            msg.push_str(&format!(
                "\n⚠️ 本次通过 LCS 近似匹配定位（相似度 {pct}%），替换的是与 old_text 最接近的段落而非逐字原文；\
                建议 read_fragment_text 核对替换结果是否符合预期"
            ));
        }
        // 断言证据片段被局部修改：事实依据可能漂移（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, false) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

/// 在 haystack 中查找 needle 的所有出现位置（字节索引，区分大小写）
fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut start = 0;
    while let Some(idx) = haystack[start..].find(needle) {
        let abs = start + idx;
        positions.push(abs);
        // 推进到本次命中之后，允许重叠外的连续匹配
        start = abs + needle.len();
    }
    positions
}

/// 模糊匹配：容忍空白/换行差异和 Unicode 标点变体（引号等）。
///
/// 规范化策略：
/// - **移除所有空白**（空格、换行、制表符、全角空格）—— 这样原文和 old_text
///   的空白差异完全消失，无论哪边多或少空白都能匹配
/// - 中文引号 `""''` 统一为 ASCII `"'`
/// - 其他字符保持不变
///
/// 然后在"无空白规范化视图"里精确匹配，映射回原始字节范围。
/// 这种方法逐字符对应，不会像 token 跳跃那样跨段误匹配。
///
/// 返回所有匹配的字节范围 (from, to)。
fn find_fuzzy_ranges(haystack: &str, needle: &str) -> Option<Vec<(usize, usize)>> {
    if needle.is_empty() {
        return None;
    }

    // 构建原文的无空白规范化视图：(规范化字符, 原始字节位置)
    let hay_view: Vec<(char, usize)> = haystack
        .char_indices()
        .filter_map(|(byte_idx, ch)| {
            let norm = normalize_char(ch);
            if norm.is_whitespace() {
                None // 移除所有空白
            } else {
                Some((norm, byte_idx))
            }
        })
        .collect();

    // 构建 needle 的无空白规范化字符串
    let needle_norm: String = needle
        .chars()
        .map(normalize_char)
        .filter(|c| !c.is_whitespace())
        .collect();

    if needle_norm.is_empty() {
        return None;
    }

    // 在规范化字符串里精确搜索
    let hay_str: String = hay_view.iter().map(|(c, _)| *c).collect();
    let needle_char_count = needle_norm.chars().count();
    let mut ranges = Vec::new();
    let mut search_byte_from = 0;
    while let Some(rel_byte_start) = hay_str[search_byte_from..].find(&needle_norm) {
        let abs_byte_start = search_byte_from + rel_byte_start;
        // 字节偏移 → 字符索引
        let char_start = hay_str[..abs_byte_start].chars().count();
        let char_end = char_start + needle_char_count;
        // 映射回原始字节范围
        let orig_from = hay_view[char_start].1;
        let orig_to = if char_end < hay_view.len() {
            hay_view[char_end].1
        } else {
            haystack.len()
        };
        ranges.push((orig_from, orig_to));
        // 下次搜索从匹配结束的字节位置开始（hay_str 里的字节偏移）
        search_byte_from = abs_byte_start + needle_norm.len();
    }

    if ranges.is_empty() {
        None
    } else {
        Some(ranges)
    }
}

/// 单字符规范化：标点变体统一为 ASCII 等价物
fn normalize_char(c: char) -> char {
    match c {
        '\u{201C}' | '\u{201D}' => '"',  // 中文双引号 "" → "
        '\u{2018}' | '\u{2019}' => '\'', // 中文单引号 '' → '
        '，' => ',',
        '。' => '.',
        '；' => ';',
        '：' => ':',
        '（' => '(',
        '）' => ')',
        '！' => '!',
        '？' => '?',
        _ => c,
    }
}

/// 近似匹配（第三级兜底）：needle 与正文的最长公共子序列对齐。
///
/// 用于前两级（精确 / 去空白归一化）均失配、但引文只是被轻微转写
/// （改一两字、省略、跳字）的场景——ai_check 的引文归因容忍这类转写
/// （LCS 覆盖 ≥60% 即归因），编辑侧对齐同级容错，才不会出现
/// "检测端能归因、编辑端必然失败"的能力错位。
///
/// 返回 (字节范围, 相似度)；similarity = LCS 覆盖 needle 归一化长度的比例。
/// 门槛：needle 归一化 ≥8 字、覆盖 ≥60%、且命中跨度 ≤ needle 长度的
/// 1.5 倍 + 8（零散命中横跨多段时整段替换的风险大于收益，宁缺毋滥）。
fn find_lcs_range(haystack: &str, needle: &str) -> Option<((usize, usize), f64)> {
    const MIN_NEEDLE_CHARS: usize = 8;

    // 复用 find_fuzzy_ranges 的归一化视图：空白移除 + 标点变体折叠
    let hay_view: Vec<(char, usize)> = haystack
        .char_indices()
        .filter_map(|(i, ch)| {
            let n = normalize_char(ch);
            (!n.is_whitespace()).then_some((n, i))
        })
        .collect();
    let needle_chars: Vec<char> = needle
        .chars()
        .map(normalize_char)
        .filter(|c| !c.is_whitespace())
        .collect();
    let n_len = needle_chars.len();
    if n_len < MIN_NEEDLE_CHARS || hay_view.is_empty() {
        return None;
    }
    let m = hay_view.len();

    // LCS 完整 DP 表（u16 足够：needle 长度远小于 u16 上限），供回溯定位区间
    let stride = m + 1;
    let mut dp = vec![0u16; (n_len + 1) * stride];
    for i in 1..=n_len {
        let nc = needle_chars[i - 1];
        let (prev, row) = dp.split_at_mut(i * stride);
        let prev = &prev[(i - 1) * stride..];
        let row = &mut row[..stride];
        for j in 1..=m {
            row[j] = if nc == hay_view[j - 1].0 {
                prev[j - 1] + 1
            } else {
                row[j - 1].max(prev[j])
            };
        }
    }
    let lcs_len = dp[n_len * stride + m] as usize;
    if lcs_len * 10 < n_len * 6 {
        return None; // 覆盖不足 60%：视作找不到
    }

    // 回溯收集正文侧命中的字符索引（逆序），首尾即替换区间
    let (mut i, mut j) = (n_len, m);
    let mut first_j = usize::MAX;
    let mut last_j = 0usize;
    let mut matched = 0usize;
    while i > 0 && j > 0 {
        if needle_chars[i - 1] == hay_view[j - 1].0 {
            first_j = first_j.min(j - 1);
            last_j = last_j.max(j - 1);
            matched += 1;
            i -= 1;
            j -= 1;
        } else if dp[(i - 1) * stride + j] >= dp[i * stride + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    debug_assert_eq!(matched, lcs_len);
    let span_len = last_j - first_j + 1;
    if span_len > n_len + n_len / 2 + 8 {
        return None; // 命中过于零散：拒绝整段替换
    }
    let from = hay_view[first_j].1;
    let to = hay_view[last_j].1 + hay_view[last_j].0.len_utf8();
    Some(((from, to), lcs_len as f64 / n_len as f64))
}

/// 为字节范围列表构造带上下文的摘录
fn build_range_excerpts(ranges: &[(usize, usize)], text: &str, context_chars: usize) -> String {
    let positions: Vec<usize> = ranges.iter().map(|(from, _)| *from).collect();
    let lengths: Vec<usize> = ranges.iter().map(|(from, to)| to - from).collect();
    let chars: Vec<char> = text.chars().collect();
    let char_positions: Vec<usize> = positions
        .iter()
        .map(|&byte_idx| text[..byte_idx.min(text.len())].chars().count())
        .collect();
    let total = chars.len();

    let mut out = String::new();
    for (i, &char_idx) in char_positions.iter().enumerate() {
        let query_len = lengths[i];
        let start = char_idx.saturating_sub(context_chars);
        let end = (char_idx + query_len + context_chars).min(total);

        let mut excerpt = String::new();
        if start > 0 {
            excerpt.push('…');
        }
        for (j, c) in chars[start..end].iter().enumerate() {
            let global = start + j;
            if global == char_idx {
                excerpt.push('【');
            }
            excerpt.push(*c);
            if global + 1 == char_idx + query_len {
                excerpt.push('】');
            }
        }
        if end < total {
            excerpt.push('…');
        }
        out.push_str(&format!("  位置{}: {}\n", i + 1, excerpt));
    }
    out
}

/// 在原文中找到与 old_text 最接近的片段（用于匹配失败时给 AI 提供参考）。
///
/// 策略：取 old_text 的前几个非空白字符在原文中搜索，找到则返回该位置周边的上下文。
fn find_closest_excerpt(haystack: &str, needle: &str) -> Option<String> {
    // 取 needle 的前几个非空白字符（字符级，非字节级）
    let probe_chars: Vec<char> = needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(10)
        .collect();
    if probe_chars.len() < 2 {
        return None;
    }
    // 用 probe 的前半部分字符搜索（字符安全的子串）
    let half = probe_chars.len() / 2;
    let search_count = half.max(2);
    let search_str: String = probe_chars.iter().take(search_count).collect();
    if let Some(idx) = haystack.find(&search_str) {
        // 返回该位置前后各 60 字符的上下文
        let char_start = haystack[..idx].chars().count();
        let chars: Vec<char> = haystack.chars().collect();
        let total = chars.len();
        let context = 60;
        let start = char_start.saturating_sub(context);
        let end = (char_start + search_str.chars().count() + context).min(total);
        let mut excerpt = String::new();
        if start > 0 {
            excerpt.push('…');
        }
        excerpt.extend(&chars[start..end]);
        if end < total {
            excerpt.push('…');
        }
        return Some(excerpt);
    }
    None
}

// ============================================================================
// read_fragment_text
// ============================================================================

pub struct ReadFragmentTextTool {
    ctx: ToolContext,
}
impl ReadFragmentTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for ReadFragmentTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "read_fragment_text".into(),
            description: "读取指定片段的正文内容（含元数据：片段 ID、锚点实体列表）。".into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "片段 ID（如 f_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let frag_path = self
            .ctx
            .paths()
            .fragments_dir()
            .join(format!("{fragment_id}.md"));

        let content =
            std::fs::read_to_string(&frag_path).map_err(|e| format!("读取片段失败: {e}"))?;

        let (body, anchors) = if let Some(rest) = content.strip_prefix("---") {
            if let Some(end) = rest.find("---") {
                let yaml_text = &content[3..3 + end];
                let body = content[3 + end + 3..]
                    .trim_start_matches('\n')
                    .trim()
                    .to_string();
                let anchors: Vec<String> = yaml_text
                    .lines()
                    .filter(|l| l.contains("entity:"))
                    .map(|l| l.trim().to_string())
                    .collect();
                (body, anchors)
            } else {
                (content, vec![])
            }
        } else {
            (content, vec![])
        };

        let mut result = format!(
            "片段 {fragment_id}:\n字数: {}\n",
            crate::workflow::count_words(&body)
        );
        if !anchors.is_empty() {
            result.push_str(&format!("锚点实体:\n{}\n", anchors.join("\n")));
        }
        result.push_str(&format!("\n正文:\n{body}"));
        Ok(result)
    }
}

// ============================================================================
// read_scene_full_text
// ============================================================================

/// 一次性读取整个场景的全部正文（按片段顺序拼合）。
///
/// 此前 Agent 同步一个场景需 get_scene_context（片段列表只有 100 字预览）
/// + N 次 read_fragment_text 往返；本工具把往返压到一次。
pub struct ReadSceneFullTextTool {
    ctx: ToolContext,
}
impl ReadSceneFullTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for ReadSceneFullTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "read_scene_full_text".into(),
            description: "一次性读取整个场景的全部正文：按片段顺序拼合，\
            每个片段标注 ID、字数与锚点实体。同步或修改场景前先调本工具通读全文；\
            需要 POV、章节脉络、出场角色等元数据时另调 get_scene_context。\
            避免逐片段调 read_fragment_text 的多次往返。"
                .into(),
            parameters: object_schema(vec![(
                "scene_id",
                "string",
                "场景 ID（如 sc_01_01）",
                true,
            )]),
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

        let chapter_title = parse_output
            .ir
            .document
            .chapter_of_scene(&sid)
            .and_then(|ch_id| parse_output.ir.document.get_chapter(ch_id))
            .map(|ch| ch.title.clone());

        let mut blocks = Vec::new();
        let mut total_words = 0usize;
        for fid in parse_output.ir.document.fragments_of_scene(&sid) {
            let Some(frag) = parse_output.ir.document.get_fragment(fid) else {
                continue;
            };
            let words = crate::workflow::count_words(&frag.text);
            total_words += words as usize;
            // 锚点渲染为 id(名称)，名称不可解析时只留 id
            let anchors: Vec<String> = frag
                .anchors
                .iter()
                .map(|a| {
                    match parse_output.ir.semantic.get(&a.entity) {
                        Some(node) => format!("{}({})", a.entity.as_str(), node.display_name()),
                        None => a.entity.as_str().to_string(),
                    }
                })
                .collect();
            let anchor_part = if anchors.is_empty() {
                String::new()
            } else {
                format!(" · 锚点: {}", anchors.join(", "))
            };
            blocks.push(format!(
                "── {fid}（{words} 字{anchor_part}）──\n{}",
                frag.text.trim()
            ));
        }

        let title = scene.title.as_deref().unwrap_or("无标题");
        if blocks.is_empty() {
            return Ok(format!("场景 {scene_id}《{title}》暂无片段。"));
        }
        let chapter_part = chapter_title
            .as_deref()
            .map(|t| format!("（{t}）"))
            .unwrap_or_default();
        Ok(format!(
            "场景 {scene_id}《{title}》{chapter_part}:\n片段 {} 个 · 共 {total_words} 字\n\n{}",
            blocks.len(),
            blocks.join("\n\n"),
        ))
    }
}

// ============================================================================
// append_fragment_text
// ============================================================================

pub struct AppendFragmentTextTool {
    ctx: ToolContext,
}
impl AppendFragmentTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AppendFragmentTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "append_fragment_text".into(),
            description: "向已有片段追加续写内容（不覆盖原有正文）。\
            用于扩写、继续创作、加长篇幅。追加内容一般应达 500 字以上，\
            过短会收到警示提示；局部修改请改用 edit_fragment_text。\
            建议先 read_fragment_text 了解当前内容，再决定续写方向。"
                .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "要追加内容的片段 ID（如 f_0001）",
                    true,
                ),
                (
                    "text",
                    "string",
                    "要追加的故事正文（至少 500 字，自然衔接已有内容）",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let text = get_str(arguments, "text")?;

        let paths = self.ctx.paths();
        crate::mutations::append_fragment_text(&paths, &fragment_id, &text)
            .map_err(|e| e.to_string())?;

        let word_count = crate::workflow::count_words(&text) as usize;
        let mut msg = format!("已向片段 {fragment_id} 追加 {word_count} 字内容");
        if word_count < 500 {
            msg.push_str(" ⚠ 低于建议下限 500 字，如非刻意短续，建议续写完整段落");
        }
        let briefs = super::mentioned_entity_briefs(&paths, &text, 8);
        if !briefs.is_empty() {
            msg.push_str(&format!("\n出场实体（正文匹配，档案要点）：\n{}", {
                let lines: Vec<String> = briefs.iter().map(|b| format!("  - {b}")).collect();
                lines.join("\n")
            }));
        }
        // 追加改变了正文 → 对全文重跑理解锚定（幂等）
        if let Some(provider) = self.ctx.provider.as_ref() {
            match super::anchoring::anchor_and_apply(&self.ctx, provider, &fragment_id) {
                Ok(o) => msg.push_str(&super::format_anchor_note(&o, &paths)),
                Err(e) => msg.push_str(&format!("\n（指代锚定跳过：{e}）")),
            }
        }
        // 覆盖前的旧文已留版本快照（节流），可回滚
        if let Some(note) = crate::history::fresh_snapshot_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 首次写入正文自动推进脉络 planned → writing（best-effort）
        if let Some(note) = crate::mutations::auto_advance_outline_writing(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 本章所有场景都有正文时提示收尾（best-effort，不影响追加）
        if let Some(hint) = crate::mutations::chapter_completion_hint(&paths, &fragment_id) {
            msg.push_str(&hint);
        }
        // 场景已有正文但缺 summary/story_time：提示顺手补齐（best-effort）
        if let Some(note) = crate::mutations::scene_metadata_gap_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// list_fragment_versions / restore_fragment_version — 版本历史自助
// ============================================================================

pub struct ListFragmentVersionsTool {
    ctx: ToolContext,
}
impl ListFragmentVersionsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for ListFragmentVersionsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "list_fragment_versions".into(),
            description: "列出片段的版本历史（时间戳、字数、开头摘录），新→旧。\
            覆盖/改写/删除前系统会自动留版本快照（节流：间隔 5 分钟内且字数变化\
            不足 200 不留版），每片段最多保留 20 版。\
            配合 restore_fragment_version 用时间戳回滚。"
                .into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "片段 ID（如 f_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let paths = self.ctx.paths();

        let versions = crate::history::list_versions(&paths, &fragment_id)
            .map_err(|e| e.to_string())?;
        if versions.is_empty() {
            return Ok(format!(
                "片段 {fragment_id} 没有版本快照（从未被覆盖/改写过，或变化未达留版阈值）"
            ));
        }

        let mut out = format!("片段 {fragment_id} 的版本历史（{} 版，新→旧）：\n", versions.len());
        for v in &versions {
            let excerpt = crate::history::read_version(&paths, &fragment_id, v.timestamp)
                .ok()
                .and_then(|c| {
                    let body = crate::fragment::fragment_body(&c);
                    let flat: String = body.split_whitespace().collect::<Vec<_>>().join("");
                    (!flat.is_empty()).then(|| {
                        let head: String = flat.chars().take(30).collect();
                        format!("「{head}…」")
                    })
                })
                .unwrap_or_default();
            out.push_str(&format!(
                "  版本 {}（{} 字）{excerpt}\n",
                v.timestamp, v.words
            ));
        }
        out.push_str("回滚用 restore_fragment_version(fragment_id, timestamp=版本号)；删除的片段同样可从历史恢复。");
        Ok(out)
    }
}

pub struct RestoreFragmentVersionTool {
    ctx: ToolContext,
}
impl RestoreFragmentVersionTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RestoreFragmentVersionTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "restore_fragment_version".into(),
            description: "把片段正文回滚到指定版本（时间戳来自 list_fragment_versions）。\
            恢复前会强制留底当前内容——恢复错了可以再恢复回来，操作可逆。\
            已删除的片段同样可恢复（文件会被重建）。"
                .into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "片段 ID（如 f_0001）", true),
                (
                    "timestamp",
                    "integer",
                    "目标版本的时间戳（list_fragment_versions 列出的版本号）",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let timestamp = arguments
            .get("timestamp")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "缺少或非法参数: timestamp（版本时间戳，见 list_fragment_versions）".to_string())?;

        let paths = self.ctx.paths();
        let body = crate::history::restore_version(&paths, &fragment_id, timestamp)
            .map_err(|e| e.to_string())?;
        let words = crate::workflow::count_words(&body);

        let mut msg = format!(
            "片段 {fragment_id} 已回滚到版本 {timestamp}（{words} 字）；\
            恢复前已留底当前内容，可再恢复回来"
        );
        // 回滚也是正文改动：证据片段提示同样适用（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, false) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// delete_fragment
// ============================================================================

pub struct DeleteFragmentTool {
    ctx: ToolContext,
}
impl DeleteFragmentTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for DeleteFragmentTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "delete_fragment".into(),
            description: "删除正文片段（.md 文件），并从所属场景的 fragments 列表移除。\
            不会级联清理该片段上的锚点引用。删除前请先 read_fragment_text 确认内容。\
            删除前会自动把该片段内容存进版本历史（片段菜单 → 版本历史 可找回）。"
                .into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "要删除的片段 ID（如 f_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let paths = self.ctx.paths();
        crate::mutations::delete_fragment(&paths, &fragment_id).map_err(|e| e.to_string())?;
        let mut msg = format!(
            "片段 {fragment_id} 已删除（已从所属场景的 fragments 列表移除）"
        );
        // 删除前强制留底——内容仍在版本历史中可找回
        if let Some(note) = crate::history::fresh_snapshot_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 删除的片段若是断言证据：证据悬空警示（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, true) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// rewrite_fragment
// ============================================================================

pub struct RewriteFragmentTool {
    ctx: ToolContext,
}
impl RewriteFragmentTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for RewriteFragmentTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "rewrite_fragment".into(),
            description: "用 LLM 改写指定片段的正文。传入改写指令（如\"增加环境描写\"、\
            \"把对话改得更简洁\"、\"切换为第一人称\"），工具会读取原文、调用 LLM 按指令改写、\
            然后覆盖写入（保留原有的锚点 frontmatter）。改写前请先 read_fragment_text 了解原文。"
                .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "要改写的片段 ID（如 f_0001）",
                    true,
                ),
                (
                    "instruction",
                    "string",
                    "改写指令，描述希望如何改写（如\"增加雨夜的环境描写，强化压抑氛围\"）",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let instruction = get_str(arguments, "instruction")?;

        // 读取原片段（精确解析，保留 frontmatter）
        let paths = self.ctx.paths();
        let frag_path = paths
            .file_for_id(&fragment_id)
            .ok_or_else(|| format!("无效的 fragment ID: {fragment_id}"))?;
        if !frag_path.exists() {
            return Err(format!("片段 '{fragment_id}' 不存在"));
        }
        let content =
            std::fs::read_to_string(&frag_path).map_err(|e| format!("读取片段失败: {e}"))?;
        let frag = FragmentFile::parse(&content).map_err(|e| format!("解析片段失败: {e}"))?;
        let original_text = frag.text.clone();
        let original_len = crate::workflow::count_words(&original_text) as usize;
        if original_text.trim().is_empty() {
            return Err(format!("片段 '{fragment_id}' 正文为空，无法改写"));
        }

        // 必须有 provider
        let provider = self.ctx.provider.as_ref().ok_or_else(|| {
            "rewrite_fragment 需要 provider，但当前未配置。请确保通过项目模式运行。".to_string()
        })?;

        // 构造 LLM 请求
        let system = "你是一位资深的小说编辑。请严格按照改写指令，对给定的小说正文片段进行改写。\
            要求：\n\
            1. 保持故事走向、人物性格和关键情节不变\n\
            2. 只输出改写后的完整正文，不要任何解释、标注或 frontmatter\n\
            3. 保持原文的叙事人称和时态\n\
            4. 改写后的篇幅可与原文不同，但应保持合理的叙事节奏，不要过度扩写\n\
            5. 必须输出完整正文，不要中途停止或省略";
        let user = format!("改写指令：{instruction}\n\n原正文：\n\n{original_text}");

        // 动态计算 max_tokens：改写输出与原文等长，另需思考模式配额（可达 ~10k）。
        // 下限 16384 覆盖思考 + 常规片段输出，长片段按输入放大，上限 32768。
        let estimated_output_tokens = (original_len.saturating_mul(3)) + 1024;
        let max_tokens = estimated_output_tokens.clamp(16384, 32768);

        let request = GenerateRequest {
            system: system.to_string(),
            messages: vec![Message::user(user)],
            model: String::new(),
            temperature: 0.7,
            max_tokens,
            tools: Vec::new(),
        };

        let response = super::retry_generate(provider, &request)
            .map_err(|e| format!("LLM 改写调用失败: {e}"))?;

        // 检测截断：若达到 max_tokens，说明输出不完整，不应写残缺内容
        if matches!(
            response.stop_reason,
            story_ai_runtime::provider::StopReason::MaxTokens
        ) {
            return Err(format!(
                "改写内容被截断（达到 max_tokens={max_tokens} 上限）。\
                原文 {original_len} 字过长，建议：\
                (1) 用 split_fragment 拆分后再改写；\
                (2) 缩小改写范围（只改写部分段落）；\
                (3) 换用支持更长输出的模型。"
            ));
        }

        // 剥思考链：部分端点把 reasoning 内联混入 content，防止英文推理写进正文
        let new_text = crate::llm_json::strip_think(&response.text).trim().to_string();
        if new_text.is_empty() {
            return Err("LLM 返回空内容，改写失败".to_string());
        }

        let new_len = crate::workflow::count_words(&new_text) as usize;

        // 覆盖写入（保留原 frontmatter/anchors）
        crate::mutations::write_fragment_text_source(
            &paths,
            &fragment_id,
            &new_text,
            crate::stats::WriteSource::Ai,
        )
            .map_err(|e| e.to_string())?;

        let mut msg = format!(
            "片段 {fragment_id} 已改写（原 {original_len} 字 → 新 {new_len} 字）"
        );
        // 改写前的原文已留版本快照——把"可反悔"说到明面上，鼓励大胆尝试
        if let Some(note) = crate::history::fresh_snapshot_note(&paths, &fragment_id) {
            msg.push_str(&note);
        }
        // 改写证据片段：事实依据可能漂移（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, false) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// split_fragment
// ============================================================================

pub struct SplitFragmentTool {
    ctx: ToolContext,
}
impl SplitFragmentTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for SplitFragmentTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "split_fragment".into(),
            description: "在指定标记处把一个片段拆成两个。原片段保留标记及其之前的内容，\
            标记之后的内容成为新片段（自动插入到原片段后面，保持叙事顺序）。\
            原片段的锚点全部保留在前半部分；后半段会自动重跑理解锚定\
            （文本匹配的名字出场由解析层自动识别，指代出场由锚定补齐）。\
            拆分前请先 read_fragment_text 确认标记位置。"
                .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "要拆分的片段 ID（如 f_0001）",
                    true,
                ),
                (
                    "split_at",
                    "string",
                    "拆分标记：正文中的某段文本（如\"林风转身离开。\"），在该文本之后拆分",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let split_at = get_str(arguments, "split_at")?;

        let paths = self.ctx.paths();
        let frag_path = paths
            .file_for_id(&fragment_id)
            .ok_or_else(|| format!("无效的 fragment ID: {fragment_id}"))?;
        if !frag_path.exists() {
            return Err(format!("片段 '{fragment_id}' 不存在"));
        }
        let content =
            std::fs::read_to_string(&frag_path).map_err(|e| format!("读取片段失败: {e}"))?;
        let frag = FragmentFile::parse(&content).map_err(|e| format!("解析片段失败: {e}"))?;

        // 在正文中查找标记
        let split_byte_idx = frag
            .text
            .find(&split_at)
            .ok_or_else(|| format!("拆分标记未在正文中找到: '{split_at}'"))?;
        // before 包含标记及其之前的全部内容；切点落在引号内时后移到
        // 最近的闭合引号，避免拆出"前半缺右引号、后半孤立右引号"的残缺对白
        let mut end = split_byte_idx + split_at.len();
        let raw_before = &frag.text[..end];
        for (open_q, close_q) in [('“', '”'), ('「', '」'), ('『', '』')] {
            if raw_before.matches(open_q).count() > raw_before.matches(close_q).count() {
                if let Some(rel) = frag.text[end..].find(close_q) {
                    end += rel + close_q.len_utf8();
                }
            }
        }
        // ASCII 双引号按奇偶配对
        if frag.text[..end].matches('"').count() % 2 == 1 {
            if let Some(rel) = frag.text[end..].find('"') {
                end += rel + 1;
            }
        }
        let before = frag.text[..end].to_string();
        // after 是切点之后的内容
        let after = frag.text[end..].trim_start_matches('\n').to_string();

        if after.trim().is_empty() {
            return Err("拆分标记之后无内容，无法拆分（标记可能已在正文末尾）".to_string());
        }

        // 从 IR 反查 scene_id
        let parse_output = self.ctx.parse()?;
        let sid = StableId::new_unchecked(&fragment_id);
        let scene_id = parse_output
            .ir
            .document
            .scene_of_fragment(&sid)
            .ok_or_else(|| format!("片段 '{fragment_id}' 未归属任何场景"))?;

        let scene_id_str = scene_id.as_str();

        // 原片段写前半
        crate::mutations::write_fragment_text_source(
            &paths,
            &fragment_id,
            &before,
            crate::stats::WriteSource::Ai,
        )
            .map_err(|e| e.to_string())?;

        // 创建新片段（含后半内容）—— create_fragment 会 push 到末尾
        let new_id = crate::mutations::create_fragment(&paths, scene_id_str, &after)
            .map_err(|e| e.to_string())?;

        // 把新片段移到原片段后面（保持顺序）
        crate::mutations::insert_fragment_after(&paths, scene_id_str, &fragment_id, &new_id)
            .map_err(|e| e.to_string())?;

        let mut msg = format!(
            "片段已拆分：{fragment_id} 保留前半（{} 字），{new_id} 为后半（{} 字）",
            crate::workflow::count_words(&before),
            crate::workflow::count_words(&after)
        );
        // 原片段只剩前半：若它是断言证据，事实依据可能已随后半移走（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &fragment_id, false) {
            msg.push_str(&note);
        }
        // 后半段重跑理解锚定（best-effort，不影响拆分结果）：显式锚点全留在
        // 前半，后半的指代出场（代词/称谓）与 appears/mentioned 角色需要重新
        // 识别——确定性文本匹配在解析层自动完成，这里补 LLM 理解层
        if let Some(provider) = self.ctx.provider.as_ref() {
            match super::anchoring::anchor_and_apply(&self.ctx, provider, &new_id) {
                Ok(o) => msg.push_str(&super::format_anchor_note(&o, &paths)),
                Err(e) => msg.push_str(&format!("\n（后半段指代锚定跳过：{e}）")),
            }
        }
        Ok(msg)
    }
}

// ============================================================================
// merge_fragments
// ============================================================================

pub struct MergeFragmentsTool {
    ctx: ToolContext,
}
impl MergeFragmentsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for MergeFragmentsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "merge_fragments".into(),
            description: "合并两个片段：把第二个片段的正文与锚点合并到第一个，然后删除第二个。\
            锚点会去重保留。建议合并相邻片段（非相邻片段合并可能影响叙事顺序）。\
            合并前请先 read_fragment_text 确认两者内容。"
                .into(),
            parameters: object_schema(vec![
                (
                    "primary_id",
                    "string",
                    "合并目标片段 ID（保留此片段，内容追加到其后）",
                    true,
                ),
                (
                    "secondary_id",
                    "string",
                    "被合并的片段 ID（内容合并到 primary 后删除）",
                    true,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let primary_id = get_str(arguments, "primary_id")?;
        let secondary_id = get_str(arguments, "secondary_id")?;

        let paths = self.ctx.paths();
        crate::mutations::merge_fragments(&paths, &primary_id, &secondary_id)
            .map_err(|e| e.to_string())?;

        let mut msg = format!(
            "片段 {secondary_id} 已合并到 {primary_id}（正文与锚点已合并，{secondary_id} 已删除）"
        );
        // 被合并的片段已删除：若它是断言证据，证据随片段悬空（best-effort）
        if let Some(note) = crate::mutations::evidence_usage_note(&paths, &secondary_id, true) {
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

// ============================================================================
// move_fragment
// ============================================================================

pub struct MoveFragmentTool {
    ctx: ToolContext,
}
impl MoveFragmentTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for MoveFragmentTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "move_fragment".into(),
            description: "调整片段在场景内的顺序。direction：up（前移一位）/ down（后移一位）/ \
            to_first（移到最前）/ to_last（移到最后）；跨多位移动用 position 直接指定\
            目标位置（1-based，提供时忽略 direction）。用于重排叙事节奏。"
                .into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "要移动的片段 ID", true),
                (
                    "direction",
                    "string",
                    "移动方向：up / down / to_first / to_last（与 position 二选一）",
                    false,
                ),
                (
                    "position",
                    "string",
                    "目标位置（1-based，1=最前；场景内有几个片段就有效到几）。\
                    跨多位移动时直接指定，提供时忽略 direction",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;

        let parse_output = self.ctx.parse()?;
        let sid = StableId::new_unchecked(&fragment_id);

        // 反查 scene_id 和当前位置
        let scene_id = parse_output
            .ir
            .document
            .scene_of_fragment(&sid)
            .ok_or_else(|| format!("片段 '{fragment_id}' 未归属任何场景"))?;
        let scene_id_str = scene_id.as_str();

        let fragments = parse_output.ir.document.fragments_of_scene(scene_id);
        let current_pos = fragments
            .iter()
            .position(|f| f.as_str() == fragment_id)
            .ok_or_else(|| format!("片段 '{fragment_id}' 不在场景的 fragments 列表中"))?;

        let total = fragments.len();
        // 计算目标位置（移除 current_pos 后的索引；插入后即最终 0-based 位置）
        let new_index = if let Some(position) = get_u32_opt(arguments, "position") {
            let position = position as usize;
            if position == 0 || position > total {
                return Err(format!(
                    "position 必须在 1..={total} 内（场景共 {total} 个片段），收到 {position}"
                ));
            }
            position - 1
        } else {
            let direction = get_str(arguments, "direction").map_err(|_| {
                "需要提供 direction（up/down/to_first/to_last）或 position（1-based 目标位置）"
                    .to_string()
            })?;
            match direction.as_str() {
                "up" => {
                    if current_pos == 0 {
                        return Err(format!(
                            "片段 {fragment_id} 已在最前，无法前移（本就是第 1 位；要移到其他位置请用 position）"
                        ));
                    }
                    current_pos - 1
                }
                "down" => {
                    if current_pos == total - 1 {
                        return Err(format!(
                            "片段 {fragment_id} 已在最后，无法后移（本就是第 {total} 位；要移到其他位置请用 position 指定 1..={total}）"
                        ));
                    }
                    current_pos + 1
                }
                "to_first" => 0,
                "to_last" => {
                    // 移除后数组长度为 total-1，追加即末尾
                    total - 1
                }
                other => {
                    return Err(format!(
                        "无效的 direction '{other}'（支持: up / down / to_first / to_last）"
                    ))
                }
            }
        };

        let paths = self.ctx.paths();
        crate::mutations::move_fragment(&paths, scene_id_str, &fragment_id, new_index)
            .map_err(|e| e.to_string())?;

        Ok(format!(
            "片段 {fragment_id} 已移动（位置 {} → {}）",
            current_pos + 1,
            new_index + 1
        ))
    }
}

// ============================================================================
// 认知矩阵 — 全景信息差视图（前端 ThreadPanel 用）
// ============================================================================

/// 矩阵列：一条线索（一个秘密）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixThreadDto {
    pub id: String,
    pub title: String,
    /// planned/planted/revealed/resolved/abandoned（锚点推导）
    pub status: &'static str,
    /// main/minor
    pub priority: String,
}

/// 矩阵行：一个观察者（读者/角色/组织）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixObserverDto {
    /// 展示名（读者 / 角色真名 / 组织名）
    pub label: String,
    /// reader | character | organization | other
    pub kind: String,
}

/// 单元格：某观察者对某线索的最新认知
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCellDto {
    /// unknown | suspected | known；空串 = 无记录（观察者未跟踪此线索）
    pub state: String,
    /// 认知定格的场景 ID（无记录为 None）
    pub scene: Option<String>,
    /// 「第N章·场景标题」定位标签
    pub scene_label: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeMatrixDto {
    pub threads: Vec<MatrixThreadDto>,
    pub observers: Vec<MatrixObserverDto>,
    /// cells[observer 行][thread 列]
    pub cells: Vec<Vec<MatrixCellDto>>,
}

/// 汇聚全部线索的观察者认知矩阵：行=观察者、列=线索、格=最新认知。
///
/// 长篇悬疑感来自信息不对称的精密调配，但可见性按条目分散记录时，
/// 「读者到底知道多少、反派知道多少、同行者知道多少」一眼看不见——
/// 这里把 threads.toml 的 knowledge 时间线摊平成一张表。单元格取该
/// 观察者时间线中**阅读序最靠后**的条目（时间线本身是逐条追加的，
/// 但补录旧场景时会乱序，按场景阅读序取末项才与读者视角一致）。
pub fn knowledge_matrix(
    paths: &story_project_format::ProjectPaths,
) -> Result<KnowledgeMatrixDto, String> {
    use story_project_format::threads_file::ObserverDef;

    // 尚无线索文件（新建项目）= 空矩阵
    let Ok(content) = std::fs::read_to_string(paths.threads_toml()) else {
        return Ok(KnowledgeMatrixDto { threads: Vec::new(), observers: Vec::new(), cells: Vec::new() });
    };
    let file = story_project_format::ThreadsFile::from_toml_str(&content)
        .map_err(|e| format!("解析 threads.toml 失败: {e}"))?;
    if file.threads.is_empty() {
        return Ok(KnowledgeMatrixDto { threads: Vec::new(), observers: Vec::new(), cells: Vec::new() });
    }

    // 阅读序（structure.toml：book.chapters → chapter.scenes），供时间线排序
    let structure = crate::mutations::load_structure(paths).ok();
    let mut scene_order: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut scene_label: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Some(structure) = &structure {
        for (ch_no, ch_id) in structure.book.chapters.iter().enumerate() {
            let Some(ch) = structure.chapter.get(ch_id) else { continue };
            for sc_id in &ch.scenes {
                let label = structure
                    .scene
                    .get(sc_id)
                    .and_then(|s| s.title.as_deref())
                    .unwrap_or("无标题");
                scene_order.insert(sc_id.clone(), scene_order.len());
                scene_label.insert(
                    sc_id.clone(),
                    format!("第{}章·{}", ch_no + 1, label),
                );
            }
        }
    }

    let threads: Vec<MatrixThreadDto> = file
        .threads
        .iter()
        .map(|t| MatrixThreadDto {
            id: t.id.clone(),
            title: t.title.clone(),
            status: derive_thread_status(t),
            priority: t.priority.clone(),
        })
        .collect();

    // 观察者清单：reader 固定首位，角色按 ID 序，组织随后
    let mut character_ids: Vec<String> = Vec::new();
    let mut organization_ids: Vec<String> = Vec::new();
    let mut has_reader = false;
    for t in &file.threads {
        for k in &t.knowledge {
            match &k.observer {
                ObserverDef::Builtin(s) if s == "reader" => has_reader = true,
                ObserverDef::Character { character } => {
                    if !character_ids.iter().any(|c| c == character) {
                        character_ids.push(character.clone());
                    }
                }
                ObserverDef::Organization { organization } => {
                    if !organization_ids.iter().any(|o| o == organization) {
                        organization_ids.push(organization.clone());
                    }
                }
                ObserverDef::Builtin(_) => {}
            }
        }
    }
    character_ids.sort();
    organization_ids.sort();

    let entity_label = |id: &str| -> String {
        paths
            .file_for_id(id)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| story_project_format::EntityFile::from_toml_str(&c).ok())
            .map(|e| e.display_name)
            .unwrap_or_else(|| id.to_string())
    };

    let mut observers: Vec<MatrixObserverDto> = Vec::new();
    if has_reader {
        observers.push(MatrixObserverDto { label: "读者".into(), kind: "reader".into() });
    }
    for cid in &character_ids {
        observers.push(MatrixObserverDto {
            label: entity_label(cid),
            kind: "character".into(),
        });
    }
    for oid in &organization_ids {
        observers.push(MatrixObserverDto {
            label: entity_label(oid),
            kind: "organization".into(),
        });
    }

    // 单元格：[观察者行][thread 列]
    let empty_cell = MatrixCellDto { state: String::new(), scene: None, scene_label: None };
    let mut cells: Vec<Vec<MatrixCellDto>> =
        vec![vec![empty_cell.clone(); file.threads.len()]; observers.len()];

    for (t_idx, t) in file.threads.iter().enumerate() {
        for k in &t.knowledge {
            // 时间线取阅读序最靠后的条目
            let latest = k
                .state_timeline
                .iter()
                .enumerate()
                .max_by_key(|(i, entry)| {
                    scene_order.get(&entry.scene).copied().unwrap_or_else(|| {
                        // 场景不在结构里（被删/坏引用）：退回记录顺序，靠后为准
                        usize::MAX / 2 + *i
                    })
                })
                .map(|(_, entry)| entry);
            let Some(latest) = latest else { continue };
            let row = match &k.observer {
                ObserverDef::Builtin(s) if s == "reader" => {
                    if has_reader { Some(0) } else { None }
                }
                ObserverDef::Character { character } => character_ids
                    .binary_search(character)
                    .ok()
                    .map(|i| i + usize::from(has_reader)),
                ObserverDef::Organization { organization } => organization_ids
                    .binary_search(organization)
                    .ok()
                    .map(|i| i + usize::from(has_reader) + character_ids.len()),
                ObserverDef::Builtin(_) => None,
            };
            if let Some(row) = row {
                cells[row][t_idx] = MatrixCellDto {
                    state: latest.state.clone(),
                    scene: Some(latest.scene.clone()),
                    scene_label: scene_label.get(&latest.scene).cloned(),
                };
            }
        }
    }

    Ok(KnowledgeMatrixDto { threads, observers, cells })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use story_ai_runtime::provider::{
        AiError, GenerateRequest, GenerateResponse, StopReason, TokenUsage,
    };
    use story_project_format::init::init_project;
    use tempfile::TempDir;

    /// 认知矩阵：观察者 × 线索，取阅读序最靠后的认知
    #[test]
    fn knowledge_matrix_flattens_observers_and_threads() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        let paths = story_project_format::ProjectPaths::new(tmp.path());

        std::fs::write(
            paths.fragments_dir().parent().unwrap().join("structure.toml"),
            r#"[book]
id = "b_root"
title = "测试"
chapters = ["ch_01", "ch_02"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01", "sc_02"]

[chapter.ch_02]
title = "第二章"
scenes = ["sc_03"]

[scene.sc_01]
title = "投宿"
fragments = []

[scene.sc_02]
title = "跟踪"
fragments = []

[scene.sc_03]
title = "摊牌"
fragments = []
"#,
        )
        .unwrap();

        std::fs::write(
            paths.threads_toml(),
            r#"[[thread]]
id = "t_0001"
title = "凶手身份"
priority = "main"
planted_at = "sc_01"

[[thread.knowledge]]
observer = "reader"
state_timeline = [
  { scene = "sc_01", state = "unknown" },
  { scene = "sc_03", state = "suspected" },
]

[[thread.knowledge]]
observer = { character = "c_0002" }
state_timeline = [
  { scene = "sc_02", state = "known" },
  # 补录的旧场景排在后面：按阅读序仍以 sc_02 为准
  { scene = "sc_01", state = "unknown" },
]
"#,
        )
        .unwrap();

        let matrix = knowledge_matrix(&paths).unwrap();
        assert_eq!(matrix.threads.len(), 1);
        assert_eq!(matrix.threads[0].status, "planted");
        // 读者行在前，角色行随后
        assert_eq!(matrix.observers.len(), 2);
        assert_eq!(matrix.observers[0].label, "读者");
        assert_eq!(matrix.observers[0].kind, "reader");
        // 无档案的角色回退显示 ID
        assert_eq!(matrix.observers[1].label, "c_0002");

        // 读者：阅读序最靠后 sc_03 → suspected
        assert_eq!(matrix.cells[0][0].state, "suspected");
        assert_eq!(matrix.cells[0][0].scene.as_deref(), Some("sc_03"));
        assert!(matrix.cells[0][0].scene_label.as_deref().unwrap().contains("摊牌"));
        // 角色：补录乱序时仍按阅读序取 sc_02 → known
        assert_eq!(matrix.cells[1][0].state, "known");
        assert_eq!(matrix.cells[1][0].scene.as_deref(), Some("sc_02"));
    }

    #[test]
    fn knowledge_matrix_empty_project() {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        let paths = story_project_format::ProjectPaths::new(tmp.path());
        let matrix = knowledge_matrix(&paths).unwrap();
        assert!(matrix.threads.is_empty());
        assert!(matrix.observers.is_empty());
    }

    fn setup_test_project() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    /// 写入一个含章节/场景的结构，并创建一个 fragment
    fn write_structure_with_scene(dir: &std::path::Path) {
        let structure = r#"[book]
id = "b_root"
title = "测试"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
fragments = []
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();
    }

    #[test]
    fn version_tools_list_and_restore() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let dir = tmp.path();
        let paths = story_project_format::ProjectPaths::new(dir);

        // v1 写入 → 强制留版 → 覆盖为 v2
        crate::mutations::create_fragment_anchored_source(
            &paths,
            "sc_01_01",
            "第一版正文，夜雨初起。",
            &[],
            crate::stats::WriteSource::Manual,
        )
        .unwrap();
        crate::history::force_snapshot(&paths, "f_0001");
        crate::mutations::write_fragment_text_source(
            &paths,
            "f_0001",
            "第二版正文，雨势转急，行人四散。",
            crate::stats::WriteSource::Ai,
        )
        .unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let list = ListFragmentVersionsTool::new(ctx.clone());
        let out = list
            .execute(&serde_json::json!({"fragment_id": "f_0001"}))
            .unwrap();
        assert!(out.contains("1 版"), "{out}");
        assert!(out.contains("第一版正文"), "应含版本摘录: {out}");
        assert!(out.contains("restore_fragment_version"), "{out}");

        // 取时间戳回滚，正文应回到第一版
        let ts = crate::history::list_versions(&paths, "f_0001").unwrap()[0].timestamp;
        let restore = RestoreFragmentVersionTool::new(ctx);
        let msg = restore
            .execute(&serde_json::json!({"fragment_id": "f_0001", "timestamp": ts}))
            .unwrap();
        assert!(msg.contains("已回滚"), "{msg}");

        let content = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("第一版正文"), "{content}");

        // 回滚本身也留了底（可再恢复回去）
        assert_eq!(crate::history::list_versions(&paths, "f_0001").unwrap().len(), 2);
    }

    #[test]
    fn write_receipt_hints_missing_scene_metadata() {
        // 场景无 summary/story_time：写入正文后回执应提示补齐
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = WriteFragmentTextTool::new(ctx);
        let res = tool
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "这是一段用于测试的正文内容，足够长以通过最小字数校验。"
            }))
            .unwrap();
        assert!(res.contains("缺少"), "应提示元数据缺失: {res}");
        assert!(res.contains("update_scene"), "应引导补齐动作: {res}");
    }

    #[test]
    fn add_relation_symmetric_creates_paired_edges() {        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = AddRelationTool::new(ctx);
        let msg = tool
            .execute(&serde_json::json!({
                "from": "c_0001",
                "to": "c_0002",
                "relation": "rival_of",
                "symmetric": true,
            }))
            .unwrap();
        assert!(msg.contains("对称反向边"), "{msg}");

        let content = std::fs::read_to_string(tmp.path().join("relations.toml")).unwrap();
        assert_eq!(content.matches("[[relation]]").count(), 2, "{content}");

        // remove_relation 提示孪生边
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let remove = RemoveRelationTool::new(ctx);
        let msg = remove
            .execute(&serde_json::json!({"relation_id": "se_0001"}))
            .unwrap();
        assert!(msg.contains("反向边 se_0002"), "应提示孪生边: {msg}");
    }

    #[test]
    fn update_relation_tool_sets_and_clears_window() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let dir = tmp.path();

        let ctx = ToolContext::new(dir.to_path_buf());
        let add = AddRelationTool::new(ctx.clone());
        add.execute(&serde_json::json!({
            "from": "c_0001", "to": "c_0002", "relation": "mentor_of"
        }))
        .unwrap();

        // 设置失效边界（师徒决裂）
        let tool = UpdateRelationTool::new(ctx.clone());
        let msg = tool
            .execute(&serde_json::json!({
                "relation_id": "se_0001",
                "valid_until_scene": "sc_01_01"
            }))
            .unwrap();
        assert!(msg.contains("至 sc_01_01 前止"), "{msg}");
        let content = std::fs::read_to_string(dir.join("relations.toml")).unwrap();
        assert!(content.contains("scene = \"sc_01_01\""), "{content}");

        // 空串清除边界
        let msg = tool
            .execute(&serde_json::json!({
                "relation_id": "se_0001",
                "valid_until_scene": ""
            }))
            .unwrap();
        assert!(msg.contains("终点已清除"), "{msg}");
        let content = std::fs::read_to_string(dir.join("relations.toml")).unwrap();
        assert!(!content.contains("scene"), "清除后不应再有场景锚: {content}");

        // 两个边界都不提供 → 报错
        assert!(tool
            .execute(&serde_json::json!({"relation_id": "se_0001"}))
            .is_err());
    }

    #[test]
    fn delete_fragment_removes_file_and_reference() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);

        let ctx = ToolContext::new(dir.to_path_buf());

        // 1. 先用 write_fragment_text 创建一个 fragment
        let create = WriteFragmentTextTool::new(ctx.clone());
        let created = create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "这是一段用于测试的正文内容，足够长以通过最小字数校验。"
            }))
            .unwrap();
        // 提取 fragment id（格式：... id=f_XXXX）
        let frag_id = created
            .split("id=")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .unwrap_or_default()
            .trim();
        assert!(
            frag_id.starts_with("f_"),
            "应生成 f_ 前缀的 id，实际: {created}"
        );

        // 确认文件存在
        let frag_path = dir.join(format!("fragments/{frag_id}.md"));
        assert!(frag_path.exists());

        // 确认 structure.toml 的 scene.fragments 已包含该 id
        let content = std::fs::read_to_string(dir.join("structure.toml")).unwrap();
        assert!(content.contains(frag_id));

        // 2. 删除 fragment
        let delete = DeleteFragmentTool::new(ctx.clone());
        let result = delete
            .execute(&serde_json::json!({ "fragment_id": frag_id }))
            .unwrap();
        assert!(result.contains("已删除"));
        assert!(result.contains(frag_id));

        // 3. 文件已被删除
        assert!(!frag_path.exists());

        // 4. structure.toml 的 scene.fragments 不再包含该 id
        let content = std::fs::read_to_string(dir.join("structure.toml")).unwrap();
        assert!(!content.contains(frag_id));
    }

    // —— write_fragment_text 记账来源 ——

    /// 妙笔新建片段（省略 fragment_id）必须记入 AI 账本。
    /// 曾走 create_fragment（固定手写），AI 新写的正文全被错记为手写。
    #[test]
    fn write_fragment_text_new_fragment_records_ai_words() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = WriteFragmentTextTool::new(ctx);
        // "妙笔写下的第一段正文" = 10 字（CJK 逐字计）
        tool.execute(&serde_json::json!({
            "scene_id": "sc_01_01",
            "text": "妙笔写下的第一段正文"
        }))
        .unwrap();

        let s = crate::stats::summary(dir, 7);
        assert_eq!(s.today.ai, 10);
        assert_eq!(s.today.manual, 0);
    }

    #[test]
    fn delete_fragment_nonexistent() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = DeleteFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({ "fragment_id": "f_9999" }));
        assert!(result.is_err());
    }

    // —— rewrite_fragment ——

    /// Mock provider 返回固定改写文本（可配置 stop_reason，模拟截断）
    #[derive(Debug)]
    struct MockRewriteProvider {
        response: String,
        stop_reason: StopReason,
    }

    impl story_ai_runtime::AiProvider for MockRewriteProvider {
        fn id(&self) -> &str {
            "mock-rewrite"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: self.stop_reason.clone(),
                tool_calls: Vec::new(),
            })
        }
    }

    #[test]
    fn rewrite_fragment_with_provider() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx_no_provider = ToolContext::new(tmp.path().to_path_buf());
        // 先创建一个有内容的片段
        let create = WriteFragmentTextTool::new(ctx_no_provider.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "林风走进山谷，看到古塔矗立在雾中。"
            }))
            .unwrap();

        // 取 fragment_id
        let dir = tmp.path();
        let frag_file = std::fs::read_dir(dir.join("fragments"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let frag_id = frag_file.file_name().to_str().unwrap().replace(".md", "");

        // 用带 provider 的 ctx 改写
        let provider = Arc::new(MockRewriteProvider {
            response: "林风踏入幽谷，一座古塔自浓雾中显现，塔身布满青苔。".to_string(),
            stop_reason: StopReason::EndTurn,
        }) as Arc<dyn story_ai_runtime::AiProvider>;
        let ctx = ToolContext::with_provider(dir.to_path_buf(), provider);
        let tool = RewriteFragmentTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": frag_id,
                "instruction": "增加环境描写"
            }))
            .unwrap();
        assert!(result.contains("已改写"));
        assert!(result.contains(frag_id.as_str()));

        // 正文应已被替换
        let content = std::fs::read_to_string(frag_file.path()).unwrap();
        assert!(content.contains("幽谷"));
        assert!(!content.contains("走进山谷"));
    }

    #[test]
    fn rewrite_fragment_without_provider_errors() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "原文本内容足够长用于测试。"
            }))
            .unwrap();

        let tool = RewriteFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "instruction": "改写"
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("provider"));
    }

    #[test]
    fn rewrite_fragment_nonexistent_errors() {
        let tmp = setup_test_project();
        let provider = Arc::new(MockRewriteProvider {
            response: "x".to_string(),
            stop_reason: StopReason::EndTurn,
        }) as Arc<dyn story_ai_runtime::AiProvider>;
        let ctx = ToolContext::with_provider(tmp.path().to_path_buf(), provider);
        let tool = RewriteFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_9999",
            "instruction": "改写"
        }));
        assert!(result.is_err());
    }

    #[test]
    fn rewrite_fragment_detects_truncation() {
        // 当 LLM 返回 MaxTokens（被截断）时，工具应报错而非写残缺内容
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx_no_provider = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx_no_provider.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "这是一段需要改写的原文，内容足够长。"
            }))
            .unwrap();

        // Mock provider 返回截断的 stop_reason
        let provider = Arc::new(MockRewriteProvider {
            response: "改写后的残缺内容（被截断）".to_string(),
            stop_reason: StopReason::MaxTokens,
        }) as Arc<dyn story_ai_runtime::AiProvider>;
        let ctx = ToolContext::with_provider(tmp.path().to_path_buf(), provider);
        let tool = RewriteFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "instruction": "改写"
        }));

        assert!(result.is_err(), "截断时应返回错误");
        let err = result.unwrap_err();
        assert!(err.contains("截断"), "错误信息应说明被截断: {err}");
        assert!(err.contains("split_fragment"), "应建议拆分片段: {err}");

        // 原文不应被覆盖（残缺内容不写盘）
        let content = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(
            content.contains("这是一段需要改写的原文"),
            "原文应保留（残缺内容不应写入），实际: {content}"
        );
    }

    // —— split_fragment ——

    #[test]
    fn split_fragment_at_marker() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "林风推开门。雨声骤响。他走入雨中。"
            }))
            .unwrap();

        let tool = SplitFragmentTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "split_at": "雨声骤响。"
            }))
            .unwrap();
        assert!(result.contains("已拆分"));
        assert!(result.contains("f_0001"));
        // 应生成了新片段 f_0002
        assert!(result.contains("f_0002"));

        // 验证 f_0001 保留前半（含标记）
        let f1 = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(f1.contains("林风推开门。"));
        assert!(f1.contains("雨声骤响。"));
        assert!(!f1.contains("他走入雨中。"));

        // f_0002 包含后半
        let f2 = std::fs::read_to_string(tmp.path().join("fragments/f_0002.md")).unwrap();
        assert!(f2.contains("他走入雨中。"));
    }

    #[test]
    fn split_fragment_inserts_tail_after_original() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "半夜，林风推开门。雨声骤响。他走入雨中。"
            }))
            .unwrap();
        create
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "天快亮了。" }))
            .unwrap();

        // 拆分第一个片段：后半（f_0003）应紧跟 f_0001，排在 f_0002（天快亮）之前
        let tool = SplitFragmentTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "split_at": "雨声骤响。"
            }))
            .unwrap();
        assert!(result.contains("已拆分"), "{result}");

        let content = std::fs::read_to_string(tmp.path().join("structure.toml")).unwrap();
        let structure = story_project_format::StructureFile::from_toml_str(&content).unwrap();
        assert_eq!(
            structure.scene["sc_01_01"].fragments,
            vec!["f_0001".to_string(), "f_0003".to_string(), "f_0002".to_string()],
            "后半应插在原片段之后、f_0002 之前，保持叙事顺序"
        );
    }

    #[test]
    fn split_fragment_marker_not_found() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "一段正文内容。"
            }))
            .unwrap();

        let tool = SplitFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "split_at": "不存在的标记"
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("未在正文中找到"));
    }

    // —— merge_fragments ——

    #[test]
    fn merge_fragments_combines() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        // 创建两个片段
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "第一段内容。"
            }))
            .unwrap();
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "第二段内容。"
            }))
            .unwrap();

        let tool = MergeFragmentsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "primary_id": "f_0001",
                "secondary_id": "f_0002"
            }))
            .unwrap();
        assert!(result.contains("已合并"));
        assert!(result.contains("f_0002"));

        // f_0002 应被删除
        assert!(!tmp.path().join("fragments/f_0002.md").exists());

        // f_0001 应包含两段
        let f1 = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(f1.contains("第一段内容。"));
        assert!(f1.contains("第二段内容。"));
    }

    #[test]
    fn merge_fragments_same_id_errors() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = MergeFragmentsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "primary_id": "f_0001",
            "secondary_id": "f_0001"
        }));
        assert!(result.is_err());
    }

    // —— move_fragment ——

    #[test]
    fn move_fragment_down() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "片段一" }))
            .unwrap();
        create
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "片段二" }))
            .unwrap();
        create
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "片段三" }))
            .unwrap();

        // 顺序：[f_0001, f_0002, f_0003]，把 f_0001 下移
        let tool = MoveFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "direction": "down"
        }));
        let result = result.unwrap();
        assert!(result.contains("已移动"));
        assert!(result.contains("1 → 2"));

        // 验证 structure 顺序变了
        let structure_content = std::fs::read_to_string(tmp.path().join("structure.toml")).unwrap();
        // f_0002 应在 f_0001 之前
        let pos2 = structure_content.find("f_0002").unwrap();
        let pos1 = structure_content.rfind("f_0001").unwrap();
        assert!(pos2 < pos1, "f_0002 应在 f_0001 之前");
    }

    #[test]
    fn move_fragment_invalid_direction_errors() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "片段一" }))
            .unwrap();

        let tool = MoveFragmentTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "direction": "sideways"
        }));
        assert!(result.is_err());
    }

    #[test]
    fn move_fragment_to_position() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        for text in ["片段一", "片段二", "片段三"] {
            create
                .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": text }))
                .unwrap();
        }

        // 顺序：[f_0001, f_0002, f_0003]，把 f_0001 跨两位移到第 3 位
        let tool = MoveFragmentTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "position": 3
            }))
            .unwrap();
        assert!(result.contains("1 → 3"), "{result}");

        let structure_content =
            std::fs::read_to_string(tmp.path().join("structure.toml")).unwrap();
        let pos2 = structure_content.find("f_0002").unwrap();
        let pos3 = structure_content.find("f_0003").unwrap();
        let pos1 = structure_content.rfind("f_0001").unwrap();
        assert!(pos2 < pos3 && pos3 < pos1, "顺序应为 f_0002, f_0003, f_0001");

        // 越界报错
        let tool = MoveFragmentTool::new(ToolContext::new(tmp.path().to_path_buf()));
        let err = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "position": 4
            }))
            .unwrap_err();
        assert!(err.contains("1..=3"), "{err}");
    }

    #[test]
    fn add_assertion_batch_accepts_native_array_and_rejects_wrong_type() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = AddAssertionTool::new(ctx);

        // 原生数组形态：模型常无视 schema 的 "string" 声明直接传数组，
        // 且字段顺序与声明顺序无关（evidence_fragment 在 subject 前）
        let result = tool
            .execute(&serde_json::json!({
                "assertions": [
                    { "evidence_fragment": null, "predicate": "alive", "subject": "c_0001", "object": "true" },
                    { "subject": "c_0002", "predicate": "age", "object": "17" }
                ]
            }))
            .unwrap();
        assert!(result.contains("批量"), "{result}");

        // 类型错误：明确报错，而不是静默误入单条分支报"缺少参数 subject"
        let err = tool
            .execute(&serde_json::json!({ "assertions": 42 }))
            .unwrap_err();
        assert!(err.contains("assertions 需为 JSON 数组"), "{err}");
    }

    /// 每项被模型序列化成字符串时应宽松解析回对象，而非报「缺少参数: subject」
    #[test]
    fn add_assertion_batch_accepts_string_items() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = AddAssertionTool::new(ctx);

        let result = tool
            .execute(&serde_json::json!({
                "assertions": [
                    "{\"subject\": \"c_0001\", \"predicate\": \"alive\", \"object\": \"true\"}",
                    { "subject": "c_0002", "predicate": "age", "object": "17" }
                ]
            }))
            .unwrap();
        assert!(result.contains("批量"), "{result}");

        // 字符串项解析失败要指明第几项
        let err = tool
            .execute(&serde_json::json!({ "assertions": ["不是 JSON 的散文"] }))
            .unwrap_err();
        assert!(err.contains("assertions 第 1 项"), "{err}");
    }

    #[test]
    fn write_fragment_short_declaration_skips_warning() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = WriteFragmentTextTool::new(ctx);

        // 未声明：短文本触发字数警示
        let r1 = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01", "text": "短片段。" }))
            .unwrap();
        assert!(r1.contains("低于建议下限"), "{r1}");

        // 声明 short=true（刻意短片段）：跳过警示
        let r2 = tool
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "另一个刻意短片段。",
                "short": true
            }))
            .unwrap();
        assert!(!r2.contains("低于建议下限"), "{r2}");
    }

    #[test]
    fn split_fragment_reanchors_tail() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());
        let dir = tmp.path();
        let paths = story_project_format::ProjectPaths::new(dir);
        let c1 = crate::mutations::create_entity(
            &paths,
            crate::mutations::EntityKind::Character,
            "林风",
        )
        .unwrap();

        // 锚定 LLM 固定返回：后半段的"他"锚到林风（appears）
        let provider = Arc::new(MockRewriteProvider {
            response: format!(
                "{{\"anchors\": [{{\"entity\": \"{c1}\", \"role\": \"appears\", \"quote\": \"他走入雨中。\"}}], \"unresolved\": []}}"
            ),
            stop_reason: StopReason::EndTurn,
        }) as Arc<dyn story_ai_runtime::AiProvider>;
        let ctx = ToolContext::with_provider(dir.to_path_buf(), provider);

        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "林风推开门。雨声骤响。他走入雨中。"
            }))
            .unwrap();

        let tool = SplitFragmentTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "split_at": "雨声骤响。"
            }))
            .unwrap();
        assert!(result.contains("已拆分"), "{result}");

        // 后半段（f_0002）补上了理解锚点——"锚点全留前半"不再导致出场失联
        let f2 = std::fs::read_to_string(dir.join("fragments/f_0002.md")).unwrap();
        assert!(f2.contains(&format!("entity: {c1}")), "{f2}");
        assert!(f2.contains("role: appears"), "{f2}");
    }

    // ------------------------------------------------------------------------
    // edit_fragment_text 单元测试
    // ------------------------------------------------------------------------

    /// 构造一个带正文的 fragment，返回 (TempDir, ctx, fragment_id)
    fn setup_fragment(body: &str) -> (TempDir, ToolContext, String) {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);
        let ctx = ToolContext::new(dir.to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": body,
            }))
            .unwrap();
        (tmp, ctx, "f_0001".to_string())
    }

    #[test]
    fn edit_fragment_replaces_unique_match() {
        let (_tmp, ctx, fid) =
            setup_fragment("林风推开门，雨水顺着发梢滴落。他抬头望向灰蒙蒙的天空。");

        let tool = EditFragmentTextTool::new(ctx.clone());
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "雨水顺着发梢滴落",
            "new_text": "冰雨如鞭般抽打在脸上",
        }));

        assert!(result.is_ok(), "应成功: {:?}", result);
        let msg = result.unwrap();
        assert!(msg.contains("替换 1 处"), "返回信息: {msg}");

        // 验证内容确实改了
        let read = ReadFragmentTextTool::new(ctx);
        let out = read
            .execute(&serde_json::json!({ "fragment_id": "f_0001" }))
            .unwrap();
        assert!(out.contains("冰雨如鞭般抽打在脸上"));
        assert!(!out.contains("雨水顺着发梢滴落"));
    }

    #[test]
    fn edit_fragment_errors_on_no_match() {
        let (_tmp, ctx, fid) = setup_fragment("林风推开门，走了出去。");

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "这段文本不存在",
            "new_text": "新文本",
        }));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("未在正文中找到"), "错误信息: {err}");
    }

    #[test]
    fn edit_fragment_detects_swapped_args() {
        // old_text/new_text 填反：要找的"新文"其实在正文里，
        // 要替换成的"旧文"反而不存在 → 报错并点名疑似填反
        let (_tmp, ctx, fid) = setup_fragment("林风推开门，雨水顺着发梢滴落。");

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "冰雨如鞭般抽打在脸上",
            "new_text": "雨水顺着发梢滴落",
        }));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("填反"), "应提示疑似填反: {err}");
    }

    #[test]
    fn edit_fragment_errors_on_multiple_matches_without_replace_all() {
        // old_text "他" 出现 3 次
        let body = "他抬头望天。他叹了口气。他转身离开。";
        let (_tmp, ctx, fid) = setup_fragment(body);

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "他",
            "new_text": "她",
        }));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("3 次"), "应提示出现次数: {err}");
        assert!(err.contains("位置1"), "应列出位置: {err}");
        assert!(err.contains("位置3"), "应列出所有位置: {err}");
    }

    #[test]
    fn edit_fragment_replace_all_replaces_every_occurrence() {
        let body = "他抬头望天。他叹了口气。他转身离开。";
        let (_tmp, ctx, fid) = setup_fragment(body);

        let tool = EditFragmentTextTool::new(ctx.clone());
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "他",
            "new_text": "她",
            "replace_all": true,
        }));

        assert!(result.is_ok(), "应成功: {:?}", result);
        let msg = result.unwrap();
        assert!(msg.contains("替换 3 处"), "应替换 3 处: {msg}");

        let read = ReadFragmentTextTool::new(ctx);
        let out = read
            .execute(&serde_json::json!({ "fragment_id": "f_0001" }))
            .unwrap();
        assert_eq!(out.matches("她").count(), 3);
        assert!(!out.contains("他抬头") && !out.contains("他叹") && !out.contains("他转"));
    }

    #[test]
    fn edit_fragment_rejects_identical_old_and_new() {
        let (_tmp, ctx, fid) = setup_fragment("林风推开门。");

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "林风",
            "new_text": "林风",
        }));

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("相同"));
    }

    #[test]
    fn edit_fragment_supports_deletion_via_empty_new_text() {
        let (_tmp, ctx, fid) = setup_fragment("林风推开门，走了出去，没有回头。");

        let tool = EditFragmentTextTool::new(ctx.clone());
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "，没有回头",
            "new_text": "",
        }));

        assert!(result.is_ok(), "空 new_text 应删除片段: {:?}", result);

        let read = ReadFragmentTextTool::new(ctx);
        let out = read
            .execute(&serde_json::json!({ "fragment_id": "f_0001" }))
            .unwrap();
        assert!(out.contains("走了出去。"));
        assert!(!out.contains("没有回头"));
    }

    #[test]
    fn edit_fragment_preserves_frontmatter_anchors() {
        // 直接写一个带 anchors 的 fragment 文件
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);
        let frag_content = "---\nid: f_0001\nanchors:\n  - entity: c_8A91D4\n    role: appears\n---\n\n林风推开门，雨水落了下来。\n";
        std::fs::write(dir.join("fragments/f_0001.md"), frag_content).unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = EditFragmentTextTool::new(ctx.clone());
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "old_text": "雨水落了下来",
            "new_text": "冰雨倾盆而下",
        }));
        assert!(result.is_ok(), "{:?}", result);

        // 锚点应保留
        let content = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("c_8A91D4"), "锚点应保留");
        assert!(content.contains("冰雨倾盆而下"));
    }

    // ------------------------------------------------------------------------
    // read_scene_full_text
    // ------------------------------------------------------------------------

    #[test]
    fn read_scene_full_text_joins_fragments_in_order() {
        let (tmp, ctx, _fid) = setup_fragment("第一段正文。");
        // 再写一个片段，验证顺序拼合与汇总
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "第二段正文。",
            }))
            .unwrap();

        let tool = ReadSceneFullTextTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();
        assert!(out.contains("第一段正文。"), "{out}");
        assert!(out.contains("第二段正文。"), "{out}");
        // 顺序：第一段先于第二段；两个片段各带分节头
        let p1 = out.find("第一段正文。").unwrap();
        let p2 = out.find("第二段正文。").unwrap();
        assert!(p1 < p2);
        assert!(out.contains("片段 2 个"), "{out}");
        assert!(out.contains("共 10 字"), "{out}"); // 标点不计：两段各 5 字
        assert!(out.contains("── f_0001"), "{out}");
        let _ = tmp;
    }

    #[test]
    fn read_scene_full_text_empty_scene_and_missing_scene() {
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path()); // 场景无片段
        let ctx = ToolContext::new(tmp.path().to_path_buf());

        let tool = ReadSceneFullTextTool::new(ctx);
        let empty = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();
        assert!(empty.contains("暂无片段"), "{empty}");

        let missing = tool.execute(&serde_json::json!({ "scene_id": "sc_99_99" }));
        assert!(missing.is_err());
    }

    // ------------------------------------------------------------------------
    // find_all_occurrences 纯函数测试
    // ------------------------------------------------------------------------

    #[test]
    fn find_all_occurrences_counts_overlapping_free_matches() {
        let positions = find_all_occurrences("他来了，他看了，他走了", "他");
        assert_eq!(positions.len(), 3);
    }

    #[test]
    fn find_all_occurrences_returns_empty_for_no_match() {
        let positions = find_all_occurrences("你好世界", "不存在");
        assert!(positions.is_empty());
    }

    #[test]
    fn find_all_occurrences_case_sensitive() {
        // 区分大小写：ABC 不匹配 abc
        let positions = find_all_occurrences("ABCabcABC", "ABC");
        assert_eq!(positions.len(), 2); // 仅匹配大写的两处
    }

    #[test]
    fn find_all_occurrences_empty_needle_returns_empty() {
        let positions = find_all_occurrences("任意文本", "");
        assert!(positions.is_empty());
    }

    // —— 模糊匹配 ——

    #[test]
    fn edit_fuzzy_matches_whitespace_diff() {
        // 精确匹配失败（空白差异），模糊匹配应成功
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "林风推开门，走了进去。屋里很暗。"
            }))
            .unwrap();

        // old_text 的空白和原文不同（多了换行和空格）
        let tool = EditFragmentTextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "old_text": "林风推开门，\n  走了进去。",
                "new_text": "林风推开大门，走了进去。"
            }))
            .unwrap();
        assert!(result.contains("已修改"));

        // 验证替换生效
        let content = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("推开大门"));
        assert!(content.contains("屋里很暗"));
    }

    #[test]
    fn edit_fuzzy_matches_fullwidth_punctuation_variants() {
        // 模型输出的半角标点（, . ( )）应匹配正文的全角标点（，。（））
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "他压低声音说（别出声），然后熄了灯。"
            }))
            .unwrap();

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "old_text": "他压低声音说(别出声), 然后熄了灯.",
                "new_text": "他低声道（嘘），吹熄了灯。"
            }))
            .unwrap();
        assert!(result.contains("已修改"), "{result}");

        let content = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("吹熄了灯"), "{content}");
        assert!(!content.contains("别出声"), "{content}");
    }

    #[test]
    fn edit_fuzzy_matches_quote_variants() {
        // 正文用中文引号，AI 的 old_text 用 ASCII 引号 → 模糊匹配应成功
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "他说\u{201c}用公式描述世界\u{201d}的种子早就埋进了骨子里。"
            }))
            .unwrap();

        // old_text 用 ASCII 引号
        let tool = EditFragmentTextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "old_text": "他说\"用公式描述世界\"的种子",
                "new_text": "他感到\"用公式描述世界\"的信念"
            }))
            .unwrap();
        assert!(result.contains("已修改"));

        let content = std::fs::read_to_string(tmp.path().join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("他感到"));
        // 替换范围包含了引号区域，new_text 用 ASCII 引号写入
        assert!(content.contains("\"用公式描述世界\""));
    }

    #[test]
    fn edit_fuzzy_no_false_positive_across_paragraphs() {
        // 确保模糊匹配不会跨段误匹配（日志中报告的问题）
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "最后一条线。\n\n屏住呼吸，手腕悬空，针管笔出发。\n\n最后一条线。\n\n屏住呼吸，手腕悬空，针管笔出发。"
            }))
            .unwrap();

        // old_text 包含两段之间的全部内容，应该精确匹配到（而非跳跃式误匹配）
        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "old_text": "最后一条线。\n\n屏住呼吸，手腕悬空，针管笔出发。\n\n最后一条线。\n\n屏住呼吸，手腕悬空，针管笔出发。",
            "new_text": "替换全部"
        }));
        // 应该成功替换（精确匹配或模糊匹配都能命中）
        assert!(result.is_ok(), "应匹配到完整范围: {:?}", result);
    }

    #[test]
    fn edit_fuzzy_finds_closest_excerpt_on_failure() {
        // 模糊匹配也失败时，错误信息应含"最接近的正文片段"
        let tmp = setup_test_project();
        write_structure_with_scene(tmp.path());

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let create = WriteFragmentTextTool::new(ctx.clone());
        create
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "林风推开门，走了进去。"
            }))
            .unwrap();

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "old_text": "完全不存在的文本内容",
            "new_text": "替换"
        }));
        assert!(result.is_err());
        let err = result.unwrap_err();
        // 可能包含最接近片段（取决于 find_closest_excerpt 是否找到部分匹配）
        // 至少应含"未在正文中找到"
        assert!(err.contains("未在正文中找到"));
    }

    // —— 第三级 LCS 近似匹配 ——

    #[test]
    fn edit_lcs_matches_paraphrased_old_text() {
        // 引文被轻微转写（顺着→沿着）：精确与归一化均失配，LCS 应兜底命中
        let (_tmp, ctx, fid) =
            setup_fragment("林风推开门，雨水顺着发梢滴落。他抬头望向灰蒙蒙的天空。");

        let tool = EditFragmentTextTool::new(ctx.clone());
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "雨水沿着发梢滴落",
            "new_text": "冰雨如鞭般抽打在脸上",
        }));

        let msg = result.expect("LCS 近似匹配应成功: ");
        assert!(msg.contains("近似匹配"), "回执应提示近似匹配: {msg}");
        assert!(msg.contains("相似度"), "回执应含相似度: {msg}");
        assert!(msg.contains("核对"), "回执应建议核对: {msg}");

        // 正文应是新文本，且被替换的正是原句而非邻句
        let read = ReadFragmentTextTool::new(ctx);
        let out = read
            .execute(&serde_json::json!({ "fragment_id": "f_0001" }))
            .unwrap();
        assert!(out.contains("冰雨如鞭般抽打在脸上"), "{out}");
        assert!(out.contains("林风推开门"), "邻句不应被波及: {out}");
        assert!(out.contains("灰蒙蒙的天空"), "邻句不应被波及: {out}");
    }

    #[test]
    fn edit_lcs_rejects_scattered_hits() {
        // 引文由正文里相隔很远的字拼成（覆盖率高但跨度大）：应拒绝整段替换
        let (_tmp, ctx, fid) = setup_fragment("甲乙丙丁戊。己庚辛壬癸。子丑寅卯辰。巳午未申酉。戌亥。");

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "甲丙戊庚壬丑卯巳未亥",
            "new_text": "替换",
        }));
        assert!(result.is_err(), "零散命中不应触发替换");
        assert!(result.unwrap_err().contains("未在正文中找到"));
    }

    #[test]
    fn edit_lcs_rejects_low_similarity() {
        // 文字重合度低（<60%）：三级匹配全部失败
        let (_tmp, ctx, fid) = setup_fragment("林风推开门，雨水顺着发梢滴落。");

        let tool = EditFragmentTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({
            "fragment_id": fid,
            "old_text": "皓月当空繁星点点夜色",
            "new_text": "替换",
        }));
        assert!(result.is_err());
    }

    #[test]
    fn write_receipt_pushes_mentioned_entity_profiles() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);

        let ctx = crate::tools::ToolContext::new(dir.to_path_buf());

        // 建角色（带职业/种族）——通过 update_entity 写档案
        let entity_toml = r#"id = "c_0001"
display_name = "琳恩"
aliases = ["琳恩讲师"]

[profile]
occupation = "法阵课讲师"
race = "星辰精灵"
"#;
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            entity_toml,
        )
        .unwrap();

        let tool = WriteFragmentTextTool::new(ctx);
        let res = tool
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "琳恩站上讲台，法阵在指尖亮起。"
            }))
            .unwrap();
        assert!(
            res.contains("出场实体"),
            "回执应附出场实体: {res}"
        );
        assert!(
            res.contains("琳恩（c_0001，法阵课讲师，星辰精灵）"),
            "档案要点格式: {res}"
        );

        // 别名命中同样推送
        let res2 = tool
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "琳恩讲师推了推眼镜。"
            }))
            .unwrap();
        assert!(res2.contains("琳恩（c_0001"), "别名命中: {res2}");
    }

    #[test]
    fn split_aligns_to_quote_boundary() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);
        let ctx = crate::tools::ToolContext::new(dir.to_path_buf());

        // 正文含一段对白，拆分标记落在引号内
        let lq = '\u{201C}'; // 全角左引号
        let rq = '\u{201D}'; // 全角右引号
        let line = format!("{lq}我去讨个人情。{rq}");
        let text = format!("他站定，说：{line}然后转身走了很远。");
        let tool = WriteFragmentTextTool::new(ctx.clone());
        tool.execute(&serde_json::json!({
            "scene_id": "sc_01_01",
            "text": text
        }))
        .unwrap();

        let split = SplitFragmentTool::new(ctx);
        let res = split
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "split_at": "我去讨个人情。"
            }))
            .unwrap();
        assert!(res.contains("片段已拆分"), "{res}");

        let half1 = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        let half2 = std::fs::read_to_string(dir.join("fragments/f_0002.md")).unwrap();
        assert!(half1.contains(&line), "前半应带完整引号对: {half1}");
        assert!(!half2.contains(&format!("{rq}然后")), "切点应已后移过闭合引号: {half2}");
    }

    #[test]
    fn fragment_anchor_roundtrip() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        write_structure_with_scene(dir);
        let ctx = crate::tools::ToolContext::new(dir.to_path_buf());

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"洛恩\"\naliases = []\n",
        )
        .unwrap();
        let write = WriteFragmentTextTool::new(ctx.clone());
        write
            .execute(&serde_json::json!({
                "scene_id": "sc_01_01",
                "text": "那名佣兵靠在门边，一言不发。"
            }))
            .unwrap();

        let set = crate::tools::structure::SetFragmentAnchorTool::new(ctx.clone());
        let res = set
            .execute(&serde_json::json!({
                "fragment_id": "f_0001",
                "entity_id": "c_0001",
                "role": "appears"
            }))
            .unwrap();
        assert!(res.contains("已锚定"), "{res}");

        let content = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        assert!(content.contains("entity: c_0001"), "{content}");
        assert!(content.contains("role: appears"), "{content}");

        let rm = crate::tools::structure::RemoveFragmentAnchorTool::new(ctx);
        rm.execute(&serde_json::json!({
            "fragment_id": "f_0001",
            "entity_id": "c_0001"
        }))
        .unwrap();
        let content = std::fs::read_to_string(dir.join("fragments/f_0001.md")).unwrap();
        assert!(!content.contains("entity: c_0001"), "{content}");
    }

    /// 批量添加关系：一次写入多条，ID 连续自增；单条调用续接编号。
    #[test]
    fn add_relation_batch_then_single() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = AddRelationTool::new(ctx);

        let out = tool
            .execute(&serde_json::json!({
                "relations": "[{\"from\": \"c_0001\", \"to\": \"c_0002\", \"relation\": \"mentor_of\"}, {\"from\": \"c_0002\", \"to\": \"c_0003\", \"relation\": \"friend_of\"}]"
            }))
            .unwrap();
        assert!(out.contains("已批量添加 2 条关系"), "{out}");
        assert!(out.contains("se_0001"), "{out}");
        assert!(out.contains("se_0002"), "{out}");

        // 单条路径续接批量后的自增编号
        let out = tool
            .execute(&serde_json::json!({
                "from": "c_0001", "to": "c_0003", "relation": "enemy_of"
            }))
            .unwrap();
        assert!(out.contains("se_0003"), "{out}");
    }

    /// 批量添加断言：时变谓词自动取代旧值、批内重复跳过。
    #[test]
    fn add_assertion_batch_supersedes_and_dedupes() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = AddAssertionTool::new(ctx);

        // 预置旧位置断言（a_0001）
        tool.execute(&serde_json::json!({
            "subject": "c_0001", "predicate": "located_at", "object": "l_0001"
        }))
        .unwrap();

        // 批量：1) 新位置（取代 a_0001）2) 新角色状态 3) 与 2) 完全相同 → 跳过
        let out = tool
            .execute(&serde_json::json!({
                "assertions": "[{\"subject\": \"c_0001\", \"predicate\": \"located_at\", \"object\": \"l_0002\"}, {\"subject\": \"c_0002\", \"predicate\": \"status\", \"object\": \"在逃\"}, {\"subject\": \"c_0002\", \"predicate\": \"status\", \"object\": \"在逃\"}]"
            }))
            .unwrap();
        assert!(out.contains("a_0002：c_0001 located_at"), "{out}");
        assert!(out.contains("a_0003：c_0002 status"), "{out}");
        assert!(out.contains("已存在，跳过"), "{out}");
        assert!(out.contains("a_0001 已自动置为 superseded"), "{out}");

        // 落盘验证：3 条断言，a_0001 已 superseded
        let content =
            std::fs::read_to_string(dir.join("knowledge/assertions.toml")).unwrap();
        assert_eq!(content.matches("id = ").count(), 3, "{content}");
        assert!(content.contains("superseded"), "{content}");
    }

}
