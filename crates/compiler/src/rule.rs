//! Rule Architecture — 统一 Rule 接口 (RFC-0004 §3)
//!
//! 每条检查规则实现 Rule trait，通过 RuleContext 的 report() 产出诊断。
//! RuleRegistry 按 Phase 分组管理，v0.1 仅内置规则。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use story_ir::{LocationIndex, StoryIR};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Severity, SourceLocation};

// ============================================================================
// RulePhase
// ============================================================================

/// 规则执行阶段 (RFC-0004 §3.3)
///
/// 每个阶段的规则全部跑完后才进入下一阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RulePhase {
    /// 阶段 1：结构检查（IR Invariants、引用完整性）
    Structure,
    /// 阶段 2：时序/逻辑检查（Knowledge Base 一致性）
    Semantic,
    /// 阶段 3：风格/约定检查
    Style,
}

// ============================================================================
// Dependency
// ============================================================================

/// 规则依赖的数据维度 (RFC-0004 §3.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Dependency {
    Structure,
    Fragment,
    Entity,
    Assertion,
    Knowledge,
    Thread,
}

// ============================================================================
// Rule trait
// ============================================================================

/// 所有检查规则的统一接口 (RFC-0004 §3.1)
pub trait Rule: std::fmt::Debug {
    /// 规则唯一标识
    fn id(&self) -> &str;

    /// 默认严重级别
    fn default_severity(&self) -> Severity;

    /// 所属阶段
    fn phase(&self) -> RulePhase;

    /// 声明依赖的数据维度
    fn depends_on(&self) -> &'static [Dependency];

    /// 执行检查——通过 ctx.report() 产出诊断
    fn run(&self, ctx: &mut RuleContext);
}

// ============================================================================
// RuleContext
// ============================================================================

/// 规则执行上下文 (RFC-0004 §3.2)
///
/// 一次性定义全，后续新增数据维度只需扩展此结构，Rule trait 签名不变。
pub struct RuleContext<'a> {
    pub ir: &'a StoryIR,
    pub location_index: Option<&'a LocationIndex>,
    pub kb: Option<&'a story_knowledge::KnowledgeBase>,
    pub threads: Option<&'a story_ir::ThreadCollection>,
    pub severity: Severity,
    sink: &'a mut DiagnosticSink,
}

impl<'a> RuleContext<'a> {
    /// 构造（仅 crate 内部使用）
    pub(crate) fn new(
        ir: &'a StoryIR,
        location_index: Option<&'a LocationIndex>,
        kb: Option<&'a story_knowledge::KnowledgeBase>,
        threads: Option<&'a story_ir::ThreadCollection>,
        severity: Severity,
        sink: &'a mut DiagnosticSink,
    ) -> Self {
        Self {
            ir,
            location_index,
            kb,
            threads,
            severity,
            sink,
        }
    }

    /// 产出诊断
    pub fn report(&mut self, diag: Diagnostic) {
        self.sink.push(diag);
    }

    /// 便捷：报 Error
    pub fn error(
        &mut self,
        rule_id: &str,
        code: &str,
        msg: impl Into<String>,
        loc: SourceLocation,
    ) {
        self.report(Diagnostic::compiler_error(rule_id, code, msg, loc));
    }

    /// 便捷：报 Warning
    pub fn warning(
        &mut self,
        rule_id: &str,
        code: &str,
        msg: impl Into<String>,
        loc: SourceLocation,
    ) {
        self.report(Diagnostic::compiler_warning(rule_id, code, msg, loc));
    }
}

// ============================================================================
// CompilerConfig
// ============================================================================

/// 规则配置（来自 config/compiler.toml）(RFC-0004 §3.5)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompilerConfig {
    /// rule_id → severity 覆盖
    pub severities: BTreeMap<String, Severity>,
    /// 被禁用的规则 ID
    pub disabled: BTreeSet<String>,
}

impl CompilerConfig {
    /// 查询某条规则的实际严重级别（None = 被禁用）
    pub fn severity_of(&self, rule: &dyn Rule) -> Option<Severity> {
        if self.disabled.contains(rule.id()) {
            return None;
        }
        Some(
            self.severities
                .get(rule.id())
                .copied()
                .unwrap_or_else(|| rule.default_severity()),
        )
    }
}

// ============================================================================
// RuleRegistry
// ============================================================================

/// 规则注册表 (RFC-0004 §3.4)
#[derive(Debug, Default)]
pub struct RuleRegistry {
    /// 按 phase 分组
    rules_by_phase: BTreeMap<RulePhase, Vec<Box<dyn Rule>>>,
    pub config: CompilerConfig,
}

impl RuleRegistry {
    pub fn new(config: CompilerConfig) -> Self {
        let mut registry = Self {
            rules_by_phase: BTreeMap::new(),
            config,
        };

        // 注册内置规则（RFC-0004 §5）
        // Structure 阶段
        registry.register(Box::new(crate::builtin_rules::IrInvariantChecker));
        registry.register(Box::new(crate::builtin_rules::OrphanReference));
        registry.register(Box::new(crate::builtin_rules::DuplicateSymbol));
        registry.register(Box::new(crate::builtin_rules::AnchorTargetExists));
        registry.register(Box::new(crate::builtin_rules::FragmentTextResidue));
        // Semantic 阶段（需要 KB）
        registry.register(Box::new(crate::builtin_rules::AssertionContradiction));
        registry.register(Box::new(crate::builtin_rules::OrphanAssertion));
        registry.register(Box::new(crate::builtin_rules::DeadCharacterRevived));
        registry.register(Box::new(crate::builtin_rules::AgeInconsistency));
        registry.register(Box::new(crate::builtin_rules::ValidityWindowIllegal));
        registry.register(Box::new(crate::builtin_rules::EvidenceMismatch));
        // Thread 规则（RFC-0017 §7.1）
        registry.register(Box::new(crate::builtin_rules::ThreadChecker));

        registry
    }

    pub fn register(&mut self, rule: Box<dyn Rule>) {
        let phase = rule.phase();
        self.rules_by_phase.entry(phase).or_default().push(rule);
    }

    /// 获取指定阶段的所有规则
    pub fn rules_of(&self, phase: RulePhase) -> impl Iterator<Item = &dyn Rule> {
        self.rules_by_phase
            .get(&phase)
            .into_iter()
            .flat_map(|v| v.iter().map(|b| b.as_ref()))
    }

    /// 所有已注册的规则（按阶段顺序）
    pub fn all_rules(&self) -> impl Iterator<Item = &dyn Rule> {
        [RulePhase::Structure, RulePhase::Semantic, RulePhase::Style]
            .into_iter()
            .flat_map(move |phase| {
                self.rules_by_phase
                    .get(&phase)
                    .into_iter()
                    .flat_map(|v| v.iter().map(|b| b.as_ref()))
            })
    }

    pub fn rule_count(&self) -> usize {
        self.rules_by_phase.values().map(|v| v.len()).sum()
    }
}
