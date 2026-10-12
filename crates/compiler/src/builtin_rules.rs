//! Built-in Rules — 内置检查规则 (RFC-0004 §5)
//!
//! v0.1 内置规则：
//!   Structure 阶段：IrInvariantChecker, OrphanReference, DuplicateSymbol, AnchorTargetExists

use story_ir::IdPrefix;
use story_ir::{DocNode, StableId};

use crate::diagnostic::{Severity, SourceLocation};
use crate::rule::{Dependency, Rule, RuleContext, RulePhase};

// ============================================================================
// IrInvariantChecker — IR 不变量检查 (RFC-0002 §3.3)
// ============================================================================

#[derive(Debug)]
pub struct IrInvariantChecker;

impl Rule for IrInvariantChecker {
    fn id(&self) -> &str {
        "ir_invariants"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Structure
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Structure]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let ir = ctx.ir;

        // Invariant 5: 所有 SemanticEdge 端点存在
        for (edge_id, edge) in ir.semantic.all_edges() {
            if !ir.semantic.contains(&edge.from) && !ir.document.contains(&edge.from) {
                ctx.error(
                    self.id(),
                    "ir.edge_endpoints_exist",
                    format!("边 {edge_id} 的 from '{}' 不存在", edge.from),
                    SourceLocation::entity(edge.from.clone()),
                );
            }
            if !ir.semantic.contains(&edge.to) && !ir.document.contains(&edge.to) {
                ctx.error(
                    self.id(),
                    "ir.edge_endpoints_exist",
                    format!("边 {edge_id} 的 to '{}' 不存在", edge.to),
                    SourceLocation::entity(edge.to.clone()),
                );
            }
        }

        // Invariant 6: Document Graph 链表一致性（A.next=B → B.prev=A）
        for (id, node) in ir.document.all_nodes() {
            if let DocNode::Fragment(frag) = node {
                if let Some(ref next_id) = frag.next {
                    if let Some(DocNode::Fragment(next_frag)) = ir.document.get(next_id) {
                        if next_frag.prev.as_ref() != Some(id) {
                            ctx.error(
                                self.id(),
                                "ir.linklist_inconsistent",
                                format!(
                                    "Fragment {} 的 next={} 但 {} 的 prev 不匹配",
                                    id, next_id, next_id
                                ),
                                SourceLocation::fragment(id.clone()),
                            );
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// OrphanReference — 引用完整性 (RFC-0004 §5.2)
// ============================================================================

#[derive(Debug)]
pub struct OrphanReference;

impl Rule for OrphanReference {
    fn id(&self) -> &str {
        "orphan_reference"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Structure
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Structure, Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let ir = ctx.ir;

        // 检查 Fragment 的 anchors 引用的 Entity 是否存在
        for (_id, frag) in ir.document.all_fragments() {
            for anchor in &frag.anchors {
                if !ir.semantic.contains(&anchor.entity) {
                    ctx.error(
                        self.id(),
                        "orphan_anchor_target",
                        format!(
                            "Fragment {} 的 anchor 引用了不存在的实体 {}",
                            frag.id, anchor.entity
                        ),
                        SourceLocation::fragment(frag.id.clone()),
                    );
                }
            }
        }

        // 检查 Scene 的 POV 引用的 Character 是否存在
        for (id, node) in ir.document.all_nodes() {
            if let DocNode::Scene(scene) = node {
                if let Some(ref pov_id) = scene.pov {
                    if !ir.semantic.contains(pov_id) {
                        ctx.error(
                            self.id(),
                            "orphan_pov_target",
                            format!("Scene {} 的 pov 引用了不存在的角色 {}", id, pov_id),
                            SourceLocation::structure(id.clone()),
                        );
                    }
                }
            }
        }
    }
}

// ============================================================================
// DuplicateSymbol — Symbol 冲突检测
// ============================================================================

#[derive(Debug)]
pub struct DuplicateSymbol;

impl Rule for DuplicateSymbol {
    fn id(&self) -> &str {
        "duplicate_symbol"
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Structure
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        // 同名但类型不同的实体（如「星辰学院」既是地点也是机构）是合法的
        // 「双面实体」：Entity Linking 对正文提及各记一条，不构成歧义。
        // 只有同类型内重名才真正冲突——Linking 无法区分，提及会被丢弃。
        use std::collections::BTreeMap;
        let mut name_to_entities: BTreeMap<(&'static str, String), Vec<StableId>> = BTreeMap::new();

        for entity in ctx.ir.semantic.all_entities() {
            name_to_entities
                .entry((entity.type_name(), entity.display_name().to_string()))
                .or_default()
                .push(entity.id().clone());
        }

        for ((kind, name), entities) in name_to_entities {
            if entities.len() > 1 {
                ctx.warning(
                    self.id(),
                    "duplicate_display_name",
                    format!(
                        "多个{kind}实体共享同名 '{}'，Entity Linking 无法区分（提及将被丢弃）: {:?}",
                        name,
                        entities.iter().map(|e| e.as_str()).collect::<Vec<_>>()
                    ),
                    SourceLocation::entity(entities[0].clone()),
                );
            }
        }
    }
}

// ============================================================================
// AnchorTargetExists — anchor 目标存在性（补充检查）
// ============================================================================

#[derive(Debug)]
pub struct AnchorTargetExists;

impl Rule for AnchorTargetExists {
    fn id(&self) -> &str {
        "anchor_target_exists"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Structure
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Fragment, Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        // 这个规则与 OrphanReference 的 anchor 检查重叠，
        // 但它在 Fragment 变化时单独触发（依赖 Fragment）。
        // v0.1 保留作为示例，实际可合并到 OrphanReference。
        let _ = ctx; // 无额外操作
    }
}

// ============================================================================
// AssertionContradiction — 断言矛盾检测 (RFC-0015 §8)
// ============================================================================

/// Predicate 的基数性——决定同一时间允许几个值
enum Cardinality {
    /// 同一时间只能有一个值——不同 object 且 valid 重叠 = 矛盾
    /// 如 age（同一时间只能有一个年龄）、alive（只能活或死）
    Single,
    /// 同一时间可以有多个值——不同 object 且 valid 重叠 = 正常
    /// 如 holds（可同时持剑和伞）、knows（可同时认识多人）
    /// status（可同时中毒和受伤）、located_at（可同时在某地点的某个区域）
    /// identity（可同时有公开身份和隐藏身份）
    Multi,
}

fn predicate_cardinality(predicate: &str) -> Cardinality {
    match predicate {
        // Single：同一时间唯一值
        "age" | "alive" => Cardinality::Single,
        // Multi：同一时间可多值
        "holds" | "knows" | "status" | "located_at" | "member_of" | "identity" => {
            Cardinality::Multi
        }
        // 自定义谓词默认 Multi：事件型/属性型事实（如 death_cause、wound）
        // 天然可多条共存；按 Single 会在 AI 补录历史事实时大量误报矛盾。
        // 时变谓词的新旧切换由 mutations::add_assertion 自动 supersede 处理。
        _ => Cardinality::Multi,
    }
}

/// 两条 active 断言，同 subject + predicate，不同 object，valid 区间重叠 → Error
/// （仅对 Single 基数性的 predicate 检测；Multi 基数性允许多值共存）
#[derive(Debug)]
pub struct AssertionContradiction;

impl Rule for AssertionContradiction {
    fn id(&self) -> &str {
        "assertion_contradiction"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Assertion, Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };

        // 按 (subject, predicate) 分组
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<(String, String), Vec<&story_knowledge::Assertion>> =
            BTreeMap::new();

        for (_id, assertion) in kb.all_assertions() {
            if assertion.status != story_knowledge::AssertionStatus::Active {
                continue;
            }
            let key = (
                assertion.subject.as_str().to_string(),
                assertion.predicate.as_str().to_string(),
            );
            groups.entry(key).or_default().push(assertion);
        }

        // 检查每组内的矛盾（仅对 Single 基数性的 predicate）
        for ((subject, predicate), assertions) in &groups {
            // Multi 基数性：允许多值共存，跳过矛盾检测
            if matches!(predicate_cardinality(predicate), Cardinality::Multi) {
                continue;
            }

            for i in 0..assertions.len() {
                for j in (i + 1)..assertions.len() {
                    let a = assertions[i];
                    let b = assertions[j];

                    // 不同 object + valid 重叠 = 矛盾
                    if a.object != b.object && a.valid.overlaps(&b.valid) {
                        ctx.error(
                            self.id(),
                            "assertion.contradiction",
                            format!(
                                "实体 {} 的 {} 断言矛盾：{:?}（{}）vs {:?}（{}），时间区间重叠",
                                subject, predicate, a.object, a.id, b.object, b.id
                            ),
                            SourceLocation::assertion(a.id.clone()),
                        );
                    }
                }
            }
        }
    }
}

// ============================================================================
// OrphanAssertion — 孤儿断言检测 (RFC-0015 §8)
// ============================================================================

/// 断言引用的 subject 实体在 IR 中不存在 → Warning
#[derive(Debug)]
pub struct OrphanAssertion;

impl Rule for OrphanAssertion {
    fn id(&self) -> &str {
        "orphan_assertion"
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Assertion, Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };

        for (id, assertion) in kb.all_assertions() {
            // superseded/contradicted 是历史记录而非当前状态，悬空不报警——
            // 否则长期项目会随断言沉淀积累出恒定噪音
            if !matches!(assertion.status, story_knowledge::AssertionStatus::Active) {
                continue;
            }
            if !ctx.ir.semantic.contains(&assertion.subject) {
                ctx.warning(
                    self.id(),
                    "orphan_assertion",
                    format!("断言 {} 引用的实体 {} 不存在", id, assertion.subject),
                    SourceLocation::assertion(id.clone()),
                );
            }
            // object 为实体引用（located_at/holds/member_of 等实体型谓词，
            // 值形如 StableId 时）→ 校验目标实体存在，防删除实体后悬空
            if let story_knowledge::ObjectValue::Reference(target) = &assertion.object {
                if !ctx.ir.semantic.contains(target) {
                    ctx.warning(
                        self.id(),
                        "orphan_assertion",
                        format!(
                            "断言 {} 的 {} 指向不存在的实体 {}（实体可能已被删除）",
                            id, assertion.predicate, target
                        ),
                        SourceLocation::assertion(id.clone()),
                    );
                }
            }
        }
    }
}

// ============================================================================
// DeadCharacterRevived — 死人复活检测 (RFC-0004 §5.3, RFC-0009 §8.1)
// ============================================================================

/// 角色死亡（alive=false）后，在更晚的时间点仍有 appears 出场 → Error
#[derive(Debug)]
pub struct DeadCharacterRevived;

impl Rule for DeadCharacterRevived {
    fn id(&self) -> &str {
        "dead_character_revived"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[
            Dependency::Assertion,
            Dependency::Entity,
            Dependency::Fragment,
        ]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };
        let loc_idx = match ctx.location_index {
            Some(idx) => idx,
            None => return,
        };

        use story_knowledge::{AssertionStatus, Predicate};

        // 1. 找到所有"已死亡"断言
        for (_id, death) in kb.all_assertions() {
            if death.status != AssertionStatus::Active {
                continue;
            }
            if death.predicate != Predicate::alive() {
                continue;
            }
            if death.object.as_bool() != Some(false) {
                continue;
            }

            let char_id = &death.subject;
            let death_time = match &death.valid.from {
                Some(t) => t.clone(),
                None => continue,
            };

            // 2. 查找该角色的所有 appears 出场
            for appearance in loc_idx.appearances_of(char_id) {
                if appearance.role != story_ir::AnchorRole::Appears {
                    continue;
                }

                // 3. 获取该 Fragment 所属 Scene 的时间
                let scene_time = scene_time_of_fragment(ctx, &appearance.fragment);
                let scene_time = match scene_time {
                    Some(t) => t,
                    None => continue, // 无时间标注，跳过（无法判断）
                };

                // 4. 出场时间 > 死亡时间 = 复活
                if scene_time.is_before(&death_time) || scene_time == death_time {
                    continue; // 死亡前或同时的出场是合法的
                }

                ctx.error(
                    self.id(),
                    "dead_character_revived",
                    format!(
                        "角色 {} 在 {} 后死亡，但在更晚的时间点仍有出场（Fragment {}）",
                        char_id, death_time, appearance.fragment
                    ),
                    SourceLocation::fragment(appearance.fragment.clone()),
                );
            }
        }
    }
}

// ============================================================================
// AgeInconsistency — 年龄不一致检测 (RFC-0009 §7.1)
// ============================================================================

/// 同一角色的年龄断言，相邻时间段的年龄差与时间差不符 → Error
#[derive(Debug)]
pub struct AgeInconsistency;

impl Rule for AgeInconsistency {
    fn id(&self) -> &str {
        "age_inconsistency"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Assertion, Dependency::Entity]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };

        use std::collections::BTreeMap;
        use story_ir::TimeCoord;
        use story_knowledge::{AssertionStatus, Predicate};

        // 按 subject 分组，收集所有 age 断言
        let mut by_char: BTreeMap<String, Vec<&story_knowledge::Assertion>> = BTreeMap::new();
        for (_id, a) in kb.all_assertions() {
            if a.status != AssertionStatus::Active {
                continue;
            }
            if a.predicate != Predicate::age() {
                continue;
            }
            by_char
                .entry(a.subject.as_str().to_string())
                .or_default()
                .push(a);
        }

        for (char_id, assertions) in &by_char {
            // 提取 (from_year, age) 对
            let mut points: Vec<(i32, i64, &story_knowledge::Assertion)> = Vec::new();
            for a in assertions {
                let age = match a.object.as_integer() {
                    Some(i) => i,
                    None => continue,
                };
                let from_year = match &a.valid.from {
                    Some(story_ir::StoryTime {
                        coord: TimeCoord::Absolute(cal),
                        ..
                    }) => cal.year,
                    _ => continue,
                };
                points.push((from_year, age, a));
            }

            // 按 from_year 排序
            points.sort_by_key(|(year, _, _)| *year);

            // 检查相邻断言的年龄差
            for window in points.windows(2) {
                let (year1, age1, _a1) = window[0];
                let (year2, age2, a2) = window[1];

                let years_passed = year2 - year1;
                let age_diff = age2 - age1;

                if age_diff != years_passed as i64 {
                    ctx.error(
                        self.id(),
                        "age_inconsistency",
                        format!(
                            "角色 {} 在 {} 年 {} 岁，{} 年 {} 岁——年龄差 {} 与时间差 {} 不符",
                            char_id, year1, age1, year2, age2, age_diff, years_passed
                        ),
                        SourceLocation::assertion(a2.id.clone()),
                    );
                }
            }
        }
    }
}

// ============================================================================
// ValidityWindowIllegal — 有效区间倒置 (RFC-0009 §7)
// ============================================================================

/// valid_until 早于 valid_from → Error
#[derive(Debug)]
pub struct ValidityWindowIllegal;

impl Rule for ValidityWindowIllegal {
    fn id(&self) -> &str {
        "validity_window_illegal"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Assertion]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };

        for (id, assertion) in kb.all_assertions() {
            if let (Some(from), Some(until)) = (&assertion.valid.from, &assertion.valid.until) {
                // until 早于 from = 倒置
                if until.is_before(from) {
                    ctx.error(
                        self.id(),
                        "validity_window_illegal",
                        format!(
                            "断言 {} 的 valid_until（{}）早于 valid_from（{}）",
                            id, until, from
                        ),
                        SourceLocation::assertion(id.clone()),
                    );
                }
            }
        }
    }
}

// ============================================================================
// EvidenceMismatch — 证据失配 (RFC-0015 §8)
// ============================================================================

/// 去除全部空白字符（摘录登记时空白已折叠为单空格，正文换行差异不应误报）
fn strip_all_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 断言的 Evidence 指向的 Fragment 不存在 → Warning
#[derive(Debug)]
pub struct EvidenceMismatch;

impl Rule for EvidenceMismatch {
    fn id(&self) -> &str {
        "evidence_mismatch"
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Assertion, Dependency::Fragment]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let kb = match ctx.kb {
            Some(kb) => kb,
            None => return,
        };

        for (id, assertion) in kb.all_assertions() {
            for evidence_id in &assertion.evidence {
                let evidence = match kb.get_evidence(evidence_id) {
                    Some(e) => e,
                    None => {
                        ctx.warning(
                            self.id(),
                            "evidence_mismatch",
                            format!("断言 {} 引用的证据 {} 不存在", id, evidence_id),
                            SourceLocation::assertion(id.clone()),
                        );
                        continue;
                    }
                };

                // 检查 Evidence 指向的 Fragment 是否存在
                if !ctx.ir.document.contains(&evidence.loc) {
                    ctx.warning(
                        self.id(),
                        "evidence_mismatch",
                        format!(
                            "断言 {} 的证据 {} 指向的 Fragment {} 不存在",
                            id, evidence_id, evidence.loc
                        ),
                        SourceLocation::assertion(id.clone()),
                    );
                } else if !evidence.quote.is_empty() {
                    // 片段还在但内容已偏离登记时的摘录：证据"名存实亡"
                    // （片段没删、依据的事实可能已被改写掉）。
                    // 摘录登记时空白已折叠，双侧去空白后比对，换行差异不误报；
                    // 历史摘录带省略号尾巴（正文里不存在该字符），剥离后再比。
                    // 头/中/尾三点采样：任一采样段失配即报（只看开头时
                    // 长片段中后段的改写无感）。
                    if let Some(frag) = ctx.ir.document.get_fragment(&evidence.loc) {
                        let body = strip_all_ws(&frag.text);
                        let excerpt_hit = |excerpt: &str| -> bool {
                            let q = strip_all_ws(excerpt);
                            let q = q.trim_end_matches('…');
                            q.is_empty() || body.contains(q)
                        };
                        let drifted = !excerpt_hit(&evidence.quote)
                            || evidence.quote_mid.as_deref().map(excerpt_hit) == Some(false)
                            || evidence.quote_tail.as_deref().map(excerpt_hit) == Some(false);
                        if drifted {
                            ctx.warning(
                                self.id(),
                                "evidence_mismatch",
                                format!(
                                    "断言 {} 的证据 {} 指向的片段 {} 内容已偏离登记时的摘录，\
                                     正文可能已被改写；核实事实仍成立后可用 refresh_evidence 刷新基线，\
                                     不成立则 retire/delete 断言",
                                    id, evidence_id, evidence.loc
                                ),
                                SourceLocation::assertion(id.clone()),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// 正文含疑似 id/anchors YAML 残块 → Warning。
///
/// 历史写入缺陷可能把模型复读的 `---\nanchors:...---` 块留在正文层
/// （含正文中部），读取与导出时会显示为非正文内容。此处按形态检测：
/// 块内非空行仅为 `id: f_XXXX` / `anchors:` / 列表项，且含 id: 或 anchors:。
#[derive(Debug)]
pub struct FragmentTextResidue;

impl Rule for FragmentTextResidue {
    fn id(&self) -> &str {
        "fragment_text_residue"
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Structure
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Fragment]
    }

    fn run(&self, ctx: &mut RuleContext) {
        for (_id, frag) in ctx.ir.document.all_fragments() {
            if text_has_residue_block(&frag.text) {
                ctx.warning(
                    self.id(),
                    "fragment.text_residue",
                    format!(
                        "片段 {} 的正文含疑似 YAML 残块（id/anchors 块），\
                        请运行修复数据（repair_project）清理",
                        frag.id
                    ),
                    SourceLocation::fragment(frag.id.clone()),
                );
            }
        }
    }
}

/// 检测文本中是否存在 id/anchors 形态的 `---` 围栏块。
fn text_has_residue_block(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].trim() == "---" {
            if let Some(rel) = (i + 1..lines.len()).position(|j| lines[j].trim() == "---") {
                let end = i + 1 + rel;
                if is_residue_block(&lines[i + 1..end]) {
                    return true;
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    false
}

/// 块内非空行仅为 `id: f_XXXX` / `anchors:` / 列表项，且含 id: 或 anchors:。
fn is_residue_block(block: &[&str]) -> bool {
    let mut has_marker = false;
    let mut non_empty = 0usize;
    for line in block {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_empty += 1;
        if t.starts_with("anchors:")
            || (t.starts_with("id:") && t["id:".len()..].trim_start().starts_with("f_"))
        {
            has_marker = true;
        } else if t.starts_with('-') {
            // anchors 条目列表项
        } else if t.starts_with("role:") {
            // 锚点条目的续行（role: appears/mentioned/referenced）
        } else {
            return false;
        }
    }
    non_empty > 0 && has_marker
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 获取 Fragment 所属 Scene 的 story_time
fn scene_time_of_fragment(
    ctx: &RuleContext,
    fragment_id: &story_ir::StableId,
) -> Option<story_ir::StoryTime> {
    // Fragment → Scene → story_time
    let scene_id = ctx.ir.document.scene_of_fragment(fragment_id)?;
    let scene = ctx.ir.document.get_scene(scene_id)?;
    let cal = scene.story_time.as_ref()?;
    Some(story_ir::StoryTime::absolute(cal.clone()))
}

// ============================================================================
// Thread Rules — 叙事线索检查 (RFC-0017 §7.1)
// ============================================================================

/// Thread 确定性检查（合并为一条 Rule，内部检查全部确定性规则）
///
/// 认知倒退检查：同一观察者对同一线索的认知状态沿阅读序应单调不降
/// （unknown → suspected → known），倒退必是录入错误。
#[derive(Debug)]
pub struct KnowledgeRegression;

impl Rule for KnowledgeRegression {
    fn id(&self) -> &str {
        "knowledge_regression"
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Thread]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let Some(threads) = ctx.threads else {
            return;
        };
        let rank = |s: &story_ir::KnowledgeState| -> u8 {
            match s {
                story_ir::KnowledgeState::Unknown => 0,
                story_ir::KnowledgeState::Suspected => 1,
                story_ir::KnowledgeState::Known => 2,
            }
        };
        for (tid, thread) in threads.all() {
            for entry in &thread.knowledge {
                // 按阅读序排序后走一遍，状态降级即告警
                let mut timeline: Vec<&story_ir::StateTimelineEntry> =
                    entry.state_timeline.iter().collect();
                timeline.sort_by_key(|e| {
                    ctx.ir
                        .document
                        .reading_index(&e.scene)
                        .unwrap_or(u32::MAX)
                });
                for pair in timeline.windows(2) {
                    let (prev, next) = (pair[0], pair[1]);
                    if rank(&next.state) < rank(&prev.state) {
                        ctx.warning(
                            self.id(),
                            "knowledge_regression",
                            format!(
                                "线索 {}（{}）观察者 {:?} 的认知在场景 {} 从 {:?} 倒退为 {:?}——认知状态沿阅读序应单调不降，请核对录入",
                                tid, thread.title, entry.observer, next.scene, prev.state, next.state
                            ),
                            SourceLocation::structure(tid.clone()),
                        );
                    }
                }
            }
        }
    }
}

/// 检查项（RFC-0017 §7.1）：
/// - premature_reveal: revealed_at 早于 planted_at → Error
/// - thread_resolve_cycle: resolves 图存在环 → Error
/// - resolve_without_reveal: resolved_at 有但 revealed_at 无 → Warning（scope=pov 暗线豁免）
/// - orphan_thread_ref: 引用不存在的 Entity/Scene/Assertion → Warning
/// - unused_thread: Planned 状态到全书结束 → Warning(main)/Info(minor)
/// - abandoned_thread: abandoned=true → Info
#[derive(Debug)]
pub struct ThreadChecker;

impl Rule for ThreadChecker {
    fn id(&self) -> &str {
        "thread_checks"
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn phase(&self) -> RulePhase {
        RulePhase::Semantic
    }
    fn depends_on(&self) -> &'static [Dependency] {
        &[Dependency::Thread]
    }

    fn run(&self, ctx: &mut RuleContext) {
        let threads = match ctx.threads {
            Some(t) => t,
            None => return,
        };

        // 收集所有 Thread ID（用于循环检测和引用检查）
        let all_thread_ids: std::collections::BTreeSet<String> = threads
            .all()
            .map(|(id, _)| id.as_str().to_string())
            .collect();

        for (_id, thread) in threads.all() {
            let status = thread.status();

            // abandoned_thread → Info
            if status == story_ir::ThreadStatus::Abandoned {
                ctx.warning(
                    self.id(),
                    "abandoned_thread",
                    format!("线索 '{}' 被标记为已放弃", thread.title),
                    SourceLocation::none(),
                );
                continue; // Abandoned 的线索跳过其余检查
            }

            // unused_thread → Warning(main) / Info(minor)
            if status == story_ir::ThreadStatus::Planned {
                let msg = format!("线索 '{}' 仍为 Planned（planted_at 未设置）", thread.title);
                match thread.priority {
                    story_ir::ThreadPriority::Main => {
                        ctx.warning(self.id(), "unused_thread", msg, SourceLocation::none())
                    }
                    story_ir::ThreadPriority::Minor => {} // Info 级，v0.1 暂不报
                }
                continue;
            }

            // premature_reveal → Error（按阅读序拓扑比较，弃 ID 字典序——
            // 字典序在 f_000A 及以后或场景乱序时给出错误结论）
            if let (Some(planted), Some(revealed)) = (&thread.planted_at, &thread.revealed_at) {
                let order = |anchor: &story_ir::StableId| ctx.ir.document.reading_index(anchor);
                let revealed_before_planted = match (order(planted), order(revealed)) {
                    (Some(p), Some(r)) => r < p,
                    // 锚点无法定位到结构树（自由文本等）→ 无法判定，不报
                    _ => false,
                };
                if revealed_before_planted {
                    ctx.error(
                        self.id(),
                        "premature_reveal",
                        format!(
                            "线索 '{}' 的 revealed_at({}) 早于 planted_at({})",
                            thread.title, revealed, planted
                        ),
                        SourceLocation::none(),
                    );
                }
            }

            // resolve_without_reveal → Warning。
            // 暗线（scope = pov）豁免：POV 私人闭环允许 planted 直接走向 resolved，
            // 无需面向读者 revealed（RFC-0017 状态机的明线假设不适用于暗线）
            if thread.resolved_at.is_some()
                && thread.revealed_at.is_none()
                && thread.scope != story_ir::ThreadScope::Pov
            {
                ctx.warning(
                    self.id(),
                    "resolve_without_reveal",
                    format!(
                        "线索 '{}' 已设 resolved_at 但未设 revealed_at，请确认是否为刻意设计",
                        thread.title
                    ),
                    SourceLocation::none(),
                );
            }

            // orphan_thread_ref: resolves 引用的 Thread 不存在
            for resolve_id in &thread.resolves {
                if !all_thread_ids.contains(resolve_id.as_str()) {
                    ctx.warning(
                        self.id(),
                        "orphan_thread_ref",
                        format!(
                            "线索 '{}' 的 resolves 引用了不存在的线索 '{}'",
                            thread.title, resolve_id
                        ),
                        SourceLocation::none(),
                    );
                }
            }

            // orphan_thread_ref: assertions 引用的 Entity 不存在
            for assert_id in &thread.assertions {
                if !ctx.ir.semantic.contains(assert_id) {
                    ctx.warning(
                        self.id(),
                        "orphan_thread_ref",
                        format!(
                            "线索 '{}' 的 assertions 引用了不存在的实体 '{}'",
                            thread.title, assert_id
                        ),
                        SourceLocation::none(),
                    );
                }
            }
        }

        // thread_resolve_cycle → Error（遍历 resolves 图检测环）
        for (id, thread) in threads.all() {
            for resolve_target in &thread.resolves {
                // 检查反向：target 是否也 resolves 当前线程
                if let Some(target) = threads.get(resolve_target) {
                    if target.resolves.iter().any(|r| r == id) {
                        ctx.error(
                            self.id(),
                            "thread_resolve_cycle",
                            format!("线索 '{}' 与 '{}' 构成循环回收", thread.title, target.title),
                            SourceLocation::none(),
                        );
                    }
                }
            }
        }
    }
}

// 压制未使用警告
#[allow(unused_imports)]
use IdPrefix as _IdPrefix;

// ============================================================================
// Tests — 诊断规则单元测试
//
// 这些规则是 Story Studio 的核心价值（"用编译器查一致性"），但此前
// 零测试覆盖。这里覆盖最关键的语义规则：年龄矛盾、断言冲突、死人复活、
// 孤儿断言、区间倒置。每条规则至少一个正例（应触发）+ 一个反例（不应触发）。
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticSink;
    use story_ir::{
        document::{FragmentNode, SceneNode},
        AnchorRole, DocNode, LocationIndex, StoryCalendar, StoryIR, StoryTime, ValidityWindow,
    };
    use story_knowledge::{Assertion, AssertionStatus, KnowledgeBase, ObjectValue, Predicate};

    // ---- 测试辅助函数 ----

    /// 构造一条 age 断言：subject 在 from_year 年 age 岁。
    fn age_assertion(id: &str, subject: &str, age: i64, from_year: i32) -> Assertion {
        Assertion {
            id: StableId::new_unchecked(id),
            subject: StableId::new_unchecked(subject),
            predicate: Predicate::age(),
            object: ObjectValue::Integer(age),
            valid: ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(from_year))),
            ..default_assertion()
        }
    }

    /// 构造一条 alive 断言：subject 从 from_year 起存活状态为 alive。
    fn alive_assertion(id: &str, subject: &str, alive: bool, from_year: i32) -> Assertion {
        Assertion {
            id: StableId::new_unchecked(id),
            subject: StableId::new_unchecked(subject),
            predicate: Predicate::alive(),
            object: ObjectValue::Boolean(alive),
            valid: ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(from_year))),
            ..default_assertion()
        }
    }

    /// 构造一条带指定 object 的通用断言（用于 AssertionContradiction 测试）。
    fn assertion(
        id: &str,
        subject: &str,
        predicate: Predicate,
        object: ObjectValue,
        valid: ValidityWindow,
    ) -> Assertion {
        Assertion {
            id: StableId::new_unchecked(id),
            subject: StableId::new_unchecked(subject),
            predicate,
            object,
            valid,
            ..default_assertion()
        }
    }

    /// Assertion 的默认值（status=Active, evidence=空, extracted_by=Manual）。
    fn default_assertion() -> Assertion {
        Assertion {
            id: StableId::new_unchecked("a_0000"),
            subject: StableId::new_unchecked("c_0000"),
            predicate: Predicate::new("placeholder"),
            object: ObjectValue::Boolean(true),
            valid: ValidityWindow::always(),
            evidence: Vec::new(),
            status: AssertionStatus::Active,
            extracted_by: Default::default(),
        }
    }

    /// 用给定断言构造一个 KnowledgeBase。
    fn kb_of(assertions: Vec<Assertion>) -> KnowledgeBase {
        let mut kb = KnowledgeBase::new();
        for a in assertions {
            kb.insert_assertion(a);
        }
        kb
    }

    /// 跑一条规则，返回产出的诊断 code 列表。
    fn run_rule(
        rule: &dyn Rule,
        ir: &StoryIR,
        loc: Option<&story_ir::LocationIndex>,
        kb: Option<&KnowledgeBase>,
    ) -> Vec<String> {
        let mut sink = DiagnosticSink::new();
        let mut ctx = RuleContext::new(ir, loc, kb, None, Severity::Error, &mut sink);
        rule.run(&mut ctx);
        sink.into_diagnostics()
            .0
            .into_iter()
            .map(|d| d.code.as_str().to_string())
            .collect()
    }

    // ========================================================================
    // AgeInconsistency — 年龄矛盾
    // ========================================================================

    #[test]
    fn age_inconsistency_detects_gap() {
        // 1086 年 17 岁，1088 年 30 岁 —— 时间过 2 年，年龄涨 13 岁，矛盾
        let kb = kb_of(vec![
            age_assertion("a_001", "c_001", 17, 1086),
            age_assertion("a_002", "c_001", 30, 1088),
        ]);
        let ir = StoryIR::new();
        let codes = run_rule(&AgeInconsistency, &ir, None, Some(&kb));
        assert_eq!(codes, vec!["age_inconsistency"]);
    }

    #[test]
    fn age_inconsistency_allows_consistent() {
        // 1086 年 17 岁，1088 年 19 岁 —— 时间过 2 年，年龄涨 2 岁，一致
        let kb = kb_of(vec![
            age_assertion("a_001", "c_001", 17, 1086),
            age_assertion("a_002", "c_001", 19, 1088),
        ]);
        let ir = StoryIR::new();
        let codes = run_rule(&AgeInconsistency, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "一致的年龄不应报错，实际: {codes:?}");
    }

    #[test]
    fn age_inconsistency_ignores_single_point() {
        // 只有一条 age 断言，无法比较，不应报错
        let kb = kb_of(vec![age_assertion("a_001", "c_001", 17, 1086)]);
        let ir = StoryIR::new();
        let codes = run_rule(&AgeInconsistency, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "单条年龄断言不应报错");
    }

    #[test]
    fn age_inconsistency_ignores_inactive() {
        // 两条矛盾的 age，但一条是 Superseded（被取代），应忽略
        let mut a1 = age_assertion("a_001", "c_001", 17, 1086);
        a1.status = AssertionStatus::Superseded;
        let kb = kb_of(vec![a1, age_assertion("a_002", "c_001", 30, 1088)]);
        let ir = StoryIR::new();
        let codes = run_rule(&AgeInconsistency, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "被取代的断言应被忽略");
    }

    // ========================================================================
    // KnowledgeRegression — 认知倒退
    // ========================================================================

    #[test]
    fn knowledge_regression_flags_state_backslide() {
        // 阅读序上 suspected 之后出现 unknown → 告警
        let mut t = story_ir::Thread {
            id: StableId::new_unchecked("t_001"),
            title: "身世线".to_string(),
            description: None,
            goal: None,
            priority: story_ir::ThreadPriority::Minor,
            scope: story_ir::ThreadScope::Reader,
            tags: vec![],
            assertions: vec![],
            planted_at: None,
            revealed_at: None,
            resolved_at: None,
            abandoned: false,
            resolves: vec![],
            resolved_by: vec![],
            knowledge: vec![],
            relationships: vec![],
        };
        t.knowledge.push(story_ir::ThreadKnowledge {
            observer: story_ir::Observer::Reader,
            state_timeline: vec![
                story_ir::StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_01_01"),
                    state: story_ir::KnowledgeState::Suspected,
                },
                story_ir::StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_02_01"),
                    state: story_ir::KnowledgeState::Unknown,
                },
            ],
        });
        let mut threads = story_ir::ThreadCollection::new();
        threads.insert(t);

        // IR 提供阅读序（sc_01_01 早于 sc_02_01）
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_01_01"),
            chapter: StableId::new_unchecked("ch_01"),
            prev: None,
            next: None,
            title: Some("一".into()),
            symbol: None,
            pov: None,
            mood: None,
            function: None,
            summary: None,
            story_time: None,
            story_time_anchor: None,
            story_time_gap: None,
        }));
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_02_01"),
            chapter: StableId::new_unchecked("ch_01"),
            prev: None,
            next: None,
            title: Some("二".into()),
            symbol: None,
            pov: None,
            mood: None,
            function: None,
            summary: None,
            story_time: None,
            story_time_anchor: None,
            story_time_gap: None,
        }));

        let mut sink = DiagnosticSink::new();
        let mut ctx = RuleContext::new(&ir, None, None, Some(&threads), Severity::Error, &mut sink);
        KnowledgeRegression.run(&mut ctx);
        let codes: Vec<String> = sink
            .into_diagnostics()
            .0
            .into_iter()
            .map(|d| d.code.as_str().to_string())
            .collect();
        assert_eq!(codes, vec!["knowledge_regression"]);
    }

    #[test]
    fn knowledge_regression_allows_monotonic_progress() {
        // unknown → suspected → known 单调推进 → 不告警
        let mut t = story_ir::Thread {
            id: StableId::new_unchecked("t_001"),
            title: "身世线".to_string(),
            description: None,
            goal: None,
            priority: story_ir::ThreadPriority::Minor,
            scope: story_ir::ThreadScope::Reader,
            tags: vec![],
            assertions: vec![],
            planted_at: None,
            revealed_at: None,
            resolved_at: None,
            abandoned: false,
            resolves: vec![],
            resolved_by: vec![],
            knowledge: vec![],
            relationships: vec![],
        };
        t.knowledge.push(story_ir::ThreadKnowledge {
            observer: story_ir::Observer::Reader,
            state_timeline: vec![
                story_ir::StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_01_01"),
                    state: story_ir::KnowledgeState::Unknown,
                },
                story_ir::StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_02_01"),
                    state: story_ir::KnowledgeState::Suspected,
                },
                story_ir::StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_03_01"),
                    state: story_ir::KnowledgeState::Known,
                },
            ],
        });
        let mut threads = story_ir::ThreadCollection::new();
        threads.insert(t);
        let ir = StoryIR::new();
        let mut sink = DiagnosticSink::new();
        let mut ctx = RuleContext::new(&ir, None, None, Some(&threads), Severity::Error, &mut sink);
        KnowledgeRegression.run(&mut ctx);
        let codes: Vec<String> = sink
            .into_diagnostics()
            .0
            .into_iter()
            .map(|d| d.code.as_str().to_string())
            .collect();
        assert!(codes.is_empty(), "单调推进不应告警: {codes:?}");
    }

    // ========================================================================
    // EvidenceMismatch — 证据失配 / 摘录漂移
    // ========================================================================

    /// 构造一条引用证据 ev_0001（指向 f_0001）的断言 + 证据登记。
    fn kb_with_evidence(quote: &str) -> KnowledgeBase {
        let mut a = default_assertion();
        a.id = StableId::new_unchecked("a_001");
        a.evidence = vec![StableId::new_unchecked("ev_0001")];
        let mut kb = kb_of(vec![a]);
        kb.insert_evidence(story_knowledge::Evidence {
            id: StableId::new_unchecked("ev_0001"),
            loc: StableId::new_unchecked("f_0001"),
            quote: quote.to_string(),
            quote_mid: None,
            quote_tail: None,
            source: Default::default(),
        });
        kb
    }

    #[test]
    fn evidence_mismatch_flags_drifted_quote() {
        // 摘录登记后片段被改写（正文不再包含摘录）→ 漂移告警
        let kb = kb_with_evidence("林风推门而入，尘土簌簌落下");
        let ir = ir_with_scene_fragment("sc_01", "f_0001", 1090); // 正文="角色出场"
        let codes = run_rule(&EvidenceMismatch, &ir, None, Some(&kb));
        assert_eq!(codes, vec!["evidence_mismatch"]);
    }

    #[test]
    fn evidence_mismatch_allows_matching_quote() {
        // 正文仍包含摘录（换行/空白差异容忍）→ 不告警
        let kb = kb_with_evidence("林风推门而入，尘土簌簌落下");
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_0001"),
            scene: None,
            prev: None,
            next: None,
            text: "前情交代。\n林风推门而入，尘土\n簌簌落下。\n后文收束。".to_string(),
            anchors: Vec::new(),
        }));
        let codes = run_rule(&EvidenceMismatch, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "摘录仍在正文中不应告警: {codes:?}");
    }

    /// 回归：历史摘录带省略号尾巴（80 字截断时代的产物），正文未变时
    /// 剥离省略号后比对，不得误报漂移。
    #[test]
    fn evidence_mismatch_strips_legacy_ellipsis_suffix() {
        let body_text = "夜雨敲在瓦上，檐角滴水连成一线，远处的更声穿过雨幕传来。".repeat(3);
        let quote_80: String = body_text.chars().take(80).collect();
        let kb = kb_with_evidence(&format!("{quote_80}…"));
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_0001"),
            scene: None,
            prev: None,
            next: None,
            text: format!("开头。\n{body_text}\n结尾。"),
            anchors: Vec::new(),
        }));
        let codes = run_rule(&EvidenceMismatch, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "带省略号尾巴的历史摘录不应误报: {codes:?}");
    }

    #[test]
    fn evidence_mismatch_empty_quote_skips_drift_check() {
        // 旧数据 quote 为空：无从而比对，只做存在性检查，不告警
        let kb = kb_with_evidence("");
        let ir = ir_with_scene_fragment("sc_01", "f_0001", 1090);
        let codes = run_rule(&EvidenceMismatch, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "空摘录不应触发漂移告警: {codes:?}");
    }

    // ========================================================================
    // AssertionContradiction — 断言冲突
    // ========================================================================

    #[test]
    fn contradiction_detects_overlapping_single() {
        // 同一角色 alive 断言，时间区间重叠，值不同 → 矛盾
        // alive 是 Single 基数
        let kb = kb_of(vec![
            assertion(
                "a_001",
                "c_001",
                Predicate::alive(),
                ObjectValue::Boolean(true),
                ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1086))),
            ),
            assertion(
                "a_002",
                "c_001",
                Predicate::alive(),
                ObjectValue::Boolean(false),
                ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1086))),
            ),
        ]);
        let ir = StoryIR::new();
        let codes = run_rule(&AssertionContradiction, &ir, None, Some(&kb));
        assert_eq!(codes, vec!["assertion.contradiction"]);
    }

    #[test]
    fn contradiction_allows_non_overlapping() {
        // 同一角色 alive，但时间区间不重叠（一个 1086-1090，一个 1091+），不算矛盾
        let kb = kb_of(vec![
            assertion(
                "a_001",
                "c_001",
                Predicate::alive(),
                ObjectValue::Boolean(true),
                ValidityWindow::new(
                    Some(StoryTime::absolute(StoryCalendar::from_year(1086))),
                    Some(StoryTime::absolute(StoryCalendar::from_year(1090))),
                ),
            ),
            assertion(
                "a_002",
                "c_001",
                Predicate::alive(),
                ObjectValue::Boolean(false),
                ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1091))),
            ),
        ]);
        let ir = StoryIR::new();
        let codes = run_rule(&AssertionContradiction, &ir, None, Some(&kb));
        assert!(
            codes.is_empty(),
            "不重叠的时间区间不算矛盾，实际: {codes:?}"
        );
    }

    #[test]
    fn contradiction_skips_multi_cardinality() {
        // holds 是 Multi 基数，同一时间持有多件物品不算矛盾
        let kb = kb_of(vec![
            assertion(
                "a_001",
                "c_001",
                Predicate::holds(),
                ObjectValue::Reference(StableId::new_unchecked("i_001")),
                ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1086))),
            ),
            assertion(
                "a_002",
                "c_001",
                Predicate::holds(),
                ObjectValue::Reference(StableId::new_unchecked("i_002")),
                ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1086))),
            ),
        ]);
        let ir = StoryIR::new();
        let codes = run_rule(&AssertionContradiction, &ir, None, Some(&kb));
        assert!(
            codes.is_empty(),
            "Multi 基数谓词不应报矛盾，实际: {codes:?}"
        );
    }

    // ========================================================================
    // OrphanAssertion — 孤儿断言
    // ========================================================================

    #[test]
    fn orphan_assertion_detects_missing_object_reference() {
        // located_at 指向已删除的实体 → object 引用悬空告警
        let kb = kb_of(vec![Assertion {
            id: StableId::new_unchecked("a_001"),
            subject: StableId::new_unchecked("c_001"),
            predicate: Predicate::located_at(),
            object: ObjectValue::Reference(StableId::new_unchecked("l_0018")),
            status: AssertionStatus::Active,
            valid: ValidityWindow::always(),
            evidence: vec![],
            extracted_by: Default::default(),
        }]);
        let ir = StoryIR::new(); // 空 IR：subject 与 object 都不存在
        let codes = run_rule(&OrphanAssertion, &ir, None, Some(&kb));
        assert!(codes.contains(&"orphan_assertion".to_string()));
    }

    #[test]
    fn orphan_assertion_detects_missing_subject() {
        // 断言引用的实体不存在于 IR → 警告
        let kb = kb_of(vec![age_assertion("a_001", "c_nonexistent", 17, 1086)]);
        let ir = StoryIR::new(); // 空 IR，没有 c_nonexistent
        let codes = run_rule(&OrphanAssertion, &ir, None, Some(&kb));
        assert_eq!(codes, vec!["orphan_assertion"]);
    }

    #[test]
    fn orphan_assertion_passes_when_subject_exists() {
        // 断言引用的实体存在于 IR → 不报
        use story_ir::{semantic::CharacterNode, SemanticNode};
        let kb = kb_of(vec![age_assertion("a_001", "c_001", 17, 1086)]);
        let mut ir = StoryIR::new();
        ir.semantic.insert(SemanticNode::Character(CharacterNode {
            id: StableId::new_unchecked("c_001"),
            symbol: None,
            display_name: "测试角色".to_string(),
            aliases: vec![],
            name_concealed: false,
            attributes: Default::default(),
        }));
        let codes = run_rule(&OrphanAssertion, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "存在的实体不应报孤儿");
    }

    // ========================================================================
    // DuplicateSymbol — 同名检测
    // ========================================================================

    #[test]
    fn duplicate_symbol_warns_same_kind() {
        // 两个角色同名 → 真冲突，报警
        use story_ir::{semantic::CharacterNode, SemanticNode};
        let mut ir = StoryIR::new();
        for id in ["c_001", "c_002"] {
            ir.semantic.insert(SemanticNode::Character(CharacterNode {
                id: StableId::new_unchecked(id),
                symbol: None,
                display_name: "林风".to_string(),
                aliases: vec![],
                name_concealed: false,
                attributes: Default::default(),
            }));
        }
        let codes = run_rule(&DuplicateSymbol, &ir, None, None);
        assert_eq!(codes, vec!["duplicate_display_name"]);
    }

    #[test]
    fn duplicate_symbol_allows_cross_kind_same_name() {
        // 「星辰学院」既是地点也是机构：合法双面实体，不报警
        use story_ir::{semantic::LocationNode, OrganizationNode, SemanticNode};
        let mut ir = StoryIR::new();
        ir.semantic.insert(SemanticNode::Location(LocationNode {
            id: StableId::new_unchecked("l_001"),
            symbol: None,
            display_name: "星辰学院".to_string(),
            aliases: vec![],
            parent: None,
            kind: None,
            position: None,
        }));
        ir.semantic
            .insert(SemanticNode::Organization(OrganizationNode {
                id: StableId::new_unchecked("o_001"),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                location: None,
                attributes: Default::default(),
            }));
        let codes = run_rule(&DuplicateSymbol, &ir, None, None);
        assert!(
            codes.is_empty(),
            "跨类型同名是合法双面实体，不应报警，实际: {codes:?}"
        );
    }

    #[test]
    fn duplicate_symbol_warns_when_same_kind_exists_alongside_cross_kind() {
        // 两个地点同名 + 一个同名机构 → 地点组仍要报警
        use story_ir::{semantic::LocationNode, OrganizationNode, SemanticNode};
        let mut ir = StoryIR::new();
        for id in ["l_001", "l_002"] {
            ir.semantic.insert(SemanticNode::Location(LocationNode {
                id: StableId::new_unchecked(id),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                parent: None,
                kind: None,
                position: None,
            }));
        }
        ir.semantic
            .insert(SemanticNode::Organization(OrganizationNode {
                id: StableId::new_unchecked("o_001"),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                location: None,
                attributes: Default::default(),
            }));
        let codes = run_rule(&DuplicateSymbol, &ir, None, None);
        assert_eq!(codes, vec!["duplicate_display_name"]);
    }

    // ========================================================================
    // ValidityWindowIllegal — 区间倒置
    // ========================================================================

    #[test]
    fn validity_window_illegal_detects_inverted() {
        // until(1080) 早于 from(1090) → 非法
        let kb = kb_of(vec![assertion(
            "a_001",
            "c_001",
            Predicate::alive(),
            ObjectValue::Boolean(true),
            ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(1090))),
                Some(StoryTime::absolute(StoryCalendar::from_year(1080))),
            ),
        )]);
        let ir = StoryIR::new();
        let codes = run_rule(&ValidityWindowIllegal, &ir, None, Some(&kb));
        assert_eq!(codes, vec!["validity_window_illegal"]);
    }

    #[test]
    fn validity_window_legal_when_ordered() {
        // from(1080) 早于 until(1090) → 合法
        let kb = kb_of(vec![assertion(
            "a_001",
            "c_001",
            Predicate::alive(),
            ObjectValue::Boolean(true),
            ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(1080))),
                Some(StoryTime::absolute(StoryCalendar::from_year(1090))),
            ),
        )]);
        let ir = StoryIR::new();
        let codes = run_rule(&ValidityWindowIllegal, &ir, None, Some(&kb));
        assert!(codes.is_empty(), "合法区间不应报错");
    }

    // ========================================================================
    // DeadCharacterRevived — 死人复活
    // ========================================================================

    /// 构造一个 IR，含一个 Scene（带 story_time）+ 一个 Fragment（属于该 Scene）。
    /// 用于 DeadCharacterRevived 测试。
    fn ir_with_scene_fragment(scene_id: &str, frag_id: &str, year: i32) -> StoryIR {
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked(scene_id),
            chapter: StableId::new_unchecked("ch_01"),
            title: None,
            symbol: None,
            prev: None,
            next: None,
            pov: None,
            story_time_anchor: None,
            story_time_gap: None,
            story_time: Some(StoryCalendar::from_year(year)),
            mood: None,
            function: None,
            summary: None,
        }));
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked(frag_id),
            scene: Some(StableId::new_unchecked(scene_id)),
            prev: None,
            next: None,
            text: "角色出场".to_string(),
            anchors: Vec::new(),
        }));
        ir
    }

    #[test]
    fn dead_character_revived_detects_post_death_appearance() {
        // 角色 1085 年死亡，但在 1090 年的场景里仍出场 → 复活
        let kb = kb_of(vec![alive_assertion("a_001", "c_001", false, 1085)]);
        let ir = ir_with_scene_fragment("sc_01", "f_0001", 1090);
        let mut loc = LocationIndex::new();
        loc.add(
            StableId::new_unchecked("c_001"),
            StableId::new_unchecked("f_0001"),
            AnchorRole::Appears,
        );
        let codes = run_rule(&DeadCharacterRevived, &ir, Some(&loc), Some(&kb));
        assert_eq!(codes, vec!["dead_character_revived"]);
    }

    #[test]
    fn dead_character_revived_allows_pre_death_appearance() {
        // 角色 1085 年死亡，1080 年的出场是合法的（死亡前）
        let kb = kb_of(vec![alive_assertion("a_001", "c_001", false, 1085)]);
        let ir = ir_with_scene_fragment("sc_01", "f_0001", 1080);
        let mut loc = LocationIndex::new();
        loc.add(
            StableId::new_unchecked("c_001"),
            StableId::new_unchecked("f_0001"),
            AnchorRole::Appears,
        );
        let codes = run_rule(&DeadCharacterRevived, &ir, Some(&loc), Some(&kb));
        assert!(codes.is_empty(), "死亡前的出场不应报错，实际: {codes:?}");
    }

    #[test]
    fn dead_character_revived_ignores_mentioned_role() {
        // 角色 1085 年死亡，1090 年只是 Mentioned（提及）而非 Appears → 不算复活
        let kb = kb_of(vec![alive_assertion("a_001", "c_001", false, 1085)]);
        let ir = ir_with_scene_fragment("sc_01", "f_0001", 1090);
        let mut loc = LocationIndex::new();
        loc.add(
            StableId::new_unchecked("c_001"),
            StableId::new_unchecked("f_0001"),
            AnchorRole::Mentioned,
        );
        let codes = run_rule(&DeadCharacterRevived, &ir, Some(&loc), Some(&kb));
        assert!(
            codes.is_empty(),
            "Mentioned（提及）不算出场复活，实际: {codes:?}"
        );
    }

    // ========================================================================
    // FragmentTextResidue — 正文层 YAML 残块
    // ========================================================================

    #[test]
    fn fragment_text_residue_detects_mid_body_block() {
        // 正文中部叠了 anchors 残块（显示名或实体 ID 形态）
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Fragment(FragmentNode::new(
            StableId::new_unchecked("f_0001"),
            "第一段正文。\n\n---\nanchors:\n  - 沈砚之\n---\n\n第二段正文。".into(),
        )));
        let codes = run_rule(&FragmentTextResidue, &ir, None, None);
        assert_eq!(codes, vec!["fragment.text_residue"]);

        // 实体 ID 形态同样命中
        let mut ir2 = StoryIR::new();
        ir2.document.insert(DocNode::Fragment(FragmentNode::new(
            StableId::new_unchecked("f_0001"),
            "正文。\n\n---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n后续。".into(),
        )));
        let codes = run_rule(&FragmentTextResidue, &ir2, None, None);
        assert_eq!(codes, vec!["fragment.text_residue"]);
    }

    #[test]
    fn fragment_text_residue_ignores_prose_divider() {
        // 正文里的分隔线（无围栏块）与含叙事文字的块不告警
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Fragment(FragmentNode::new(
            StableId::new_unchecked("f_0001"),
            "上半天。\n\n---\n\n下半天。\n\n---\n天气转晴，雨停了。\n---\n\n入夜。".into(),
        )));
        let codes = run_rule(&FragmentTextResidue, &ir, None, None);
        assert!(codes.is_empty(), "叙事分隔线不应告警: {codes:?}");
    }

    /// 带 threads 的规则运行（ThreadChecker 需要）
    fn run_rule_with_threads(
        rule: &dyn Rule,
        ir: &StoryIR,
        threads: &story_ir::ThreadCollection,
    ) -> Vec<String> {
        let mut sink = DiagnosticSink::new();
        let mut ctx = RuleContext::new(ir, None, None, Some(threads), Severity::Error, &mut sink);
        rule.run(&mut ctx);
        sink.into_diagnostics()
            .0
            .into_iter()
            .map(|d| d.code.as_str().to_string())
            .collect()
    }

    /// 构建含 书→章→场景 结构的 IR（阅读序即场景声明序）
    fn ir_with_scenes(scene_ids: &[&str]) -> StoryIR {
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Book(story_ir::BookNode {
            id: StableId::new_unchecked("b_1"),
            title: "书".to_string(),
            symbol: None,
        }));
        let chapter = story_ir::ChapterNode {
            id: StableId::new_unchecked("ch_1"),
            title: "章".to_string(),
            symbol: None,
            prev: None,
            next: None,
            story_time_span: None,
        };
        ir.document.insert(DocNode::Chapter(chapter));
        ir.document.set_children(
            StableId::new_unchecked("b_1"),
            vec![StableId::new_unchecked("ch_1")],
        );
        let scene_ids: Vec<StableId> = scene_ids
            .iter()
            .map(|s| StableId::new_unchecked(*s))
            .collect();
        for sid in &scene_ids {
            ir.document.insert(DocNode::Scene(story_ir::SceneNode {
                id: sid.clone(),
                chapter: StableId::new_unchecked("ch_1"),
                title: None,
                symbol: None,
                prev: None,
                next: None,
                pov: None,
                story_time: None,
                story_time_anchor: None,
                story_time_gap: None,
                mood: None,
                function: None,
                summary: None,
            }));
        }
        ir.document
            .set_children(StableId::new_unchecked("ch_1"), scene_ids);
        ir
    }

    fn thread_with(planted: &str, revealed: &str) -> story_ir::Thread {
        story_ir::Thread {
            id: StableId::new_unchecked("t_001"),
            title: "线索".into(),
            description: None,
            goal: None,
            priority: story_ir::ThreadPriority::Main,
            scope: story_ir::ThreadScope::Reader,
            tags: vec![],
            assertions: vec![],
            planted_at: Some(StableId::new_unchecked(planted)),
            revealed_at: Some(StableId::new_unchecked(revealed)),
            resolved_at: None,
            abandoned: false,
            resolves: vec![],
            resolved_by: vec![],
            knowledge: vec![],
            relationships: vec![],
        }
    }

    #[test]
    fn pov_scope_thread_resolves_without_reveal_silently() {
        // 暗线（scope=pov）：planted 直接 resolved 是合法的私人闭环，不告警
        let ir = ir_with_scenes(&["sc_1", "sc_2"]);
        let mut tc = story_ir::ThreadCollection::default();
        let mut t = thread_with("sc_1", "sc_1");
        t.scope = story_ir::ThreadScope::Pov;
        t.revealed_at = None;
        t.resolved_at = Some(StableId::new_unchecked("sc_2"));
        tc.insert(t);
        let codes = run_rule_with_threads(&ThreadChecker, &ir, &tc);
        assert!(
            !codes.contains(&"resolve_without_reveal".to_string()),
            "暗线不应触发 resolve_without_reveal: {codes:?}"
        );

        // 明线（默认 reader）：同样状态应告警
        let mut tc2 = story_ir::ThreadCollection::default();
        let mut t2 = thread_with("sc_1", "sc_1");
        t2.revealed_at = None;
        t2.resolved_at = Some(StableId::new_unchecked("sc_2"));
        tc2.insert(t2);
        let codes2 = run_rule_with_threads(&ThreadChecker, &ir, &tc2);
        assert!(
            codes2.contains(&"resolve_without_reveal".to_string()),
            "明线应触发 resolve_without_reveal: {codes2:?}"
        );
    }

    #[test]
    fn premature_reveal_uses_reading_order_not_lexicographic() {
        // 阅读序 sc_a → sc_b，但字典序 sc_b < sc_a：旧实现会误报/漏报
        let ir = ir_with_scenes(&["sc_b", "sc_a"]); // 阅读序：sc_b 在前

        // 埋在 sc_a（后），揭在 sc_b（前）→ 真提前揭示，应报
        let mut tc = story_ir::ThreadCollection::default();
        tc.insert(thread_with("sc_a", "sc_b"));
        let codes = run_rule_with_threads(&ThreadChecker, &ir, &tc);
        assert!(codes.contains(&"premature_reveal".to_string()), "{codes:?}");

        // 埋在 sc_b（前），揭在 sc_a（后）→ 正常顺序，不报
        let mut tc2 = story_ir::ThreadCollection::default();
        tc2.insert(thread_with("sc_b", "sc_a"));
        let codes2 = run_rule_with_threads(&ThreadChecker, &ir, &tc2);
        assert!(
            !codes2.contains(&"premature_reveal".to_string()),
            "{codes2:?}"
        );
    }
}
