//! Compile Model — 编译模型与依赖调度 (RFC-0004 §4)
//!
//! 核心思想：不要让 CompileEvent 直接决定 Rule 执行。
//! 正确的链路是：CompileEvent → Dependency Resolver → Dirty Set → Rule Scheduler。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use story_ir::{LocationIndex, StoryIR};

use crate::diagnostic::{DiagnosticSink, Diagnostics};
use crate::rule::{Dependency, RuleContext, RulePhase, RuleRegistry};

// ============================================================================
// CompileRequest
// ============================================================================

/// 触发一次编译的请求 (RFC-0004 §4.2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileRequest {
    pub events: Vec<CompileEvent>,
    pub mode: CompileMode,
    pub kind: RequestKind,
}

impl CompileRequest {
    /// 普通全量编译
    pub fn full_build() -> Self {
        Self {
            events: Vec::new(),
            mode: CompileMode::Full,
            kind: RequestKind::Build,
        }
    }

    /// 增量编译
    pub fn incremental(events: Vec<CompileEvent>) -> Self {
        Self {
            events,
            mode: CompileMode::Incremental,
            kind: RequestKind::Build,
        }
    }
}

impl Default for CompileRequest {
    fn default() -> Self {
        Self::full_build()
    }
}

/// 编译模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompileMode {
    Incremental,
    Full,
}

/// 请求类型（DryRun/Build/Rebuild 共享同一个 CompileMode）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestKind {
    Build,
    DryRun { change_summary: String },
    Rebuild,
}

// ============================================================================
// CompileEvent
// ============================================================================

/// 编译事件——描述"什么发生了变化" (RFC-0004 §4.3)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompileEvent {
    FragmentChanged { id: String },
    EntityChanged { id: String },
    StructureChanged { scope: String },
    AssertionChanged { id: String },
}

// ============================================================================
// DirtySet + Dependency Resolver
// ============================================================================

/// 失效的数据维度 + 具体对象集合 (RFC-0004 §4.3)
///
/// 维度（Dependency）决定**哪些规则重跑**；对象 ID 集合回答"这次变更
/// 到底波及哪些片段 / 实体 / 断言"——规则可据此跳过未变对象、续期未检
/// 对象的旧诊断。全量模式下对象集无意义，`full` 置位后一切判脏。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirtySet {
    deps: BTreeSet<Dependency>,
    /// 全量编译：一切皆脏，对象 ID 集不可用
    full: bool,
    fragments: BTreeSet<String>,
    entities: BTreeSet<String>,
    assertions: BTreeSet<String>,
}

impl DirtySet {
    pub fn all() -> Self {
        Self {
            deps: [
                Dependency::Structure,
                Dependency::Fragment,
                Dependency::Entity,
                Dependency::Assertion,
                Dependency::Knowledge,
                Dependency::Thread,
            ]
            .into_iter()
            .collect(),
            full: true,
            fragments: BTreeSet::new(),
            entities: BTreeSet::new(),
            assertions: BTreeSet::new(),
        }
    }

    pub fn contains(&self, dep: Dependency) -> bool {
        self.deps.contains(&dep)
    }

    pub fn insert(&mut self, dep: Dependency) {
        self.deps.insert(dep);
    }

    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn add_fragment_id(&mut self, id: impl Into<String>) {
        self.fragments.insert(id.into());
    }

    pub fn add_entity_id(&mut self, id: impl Into<String>) {
        self.entities.insert(id.into());
    }

    pub fn add_assertion_id(&mut self, id: impl Into<String>) {
        self.assertions.insert(id.into());
    }

    /// 该片段是否脏（全量 = 是）
    pub fn is_fragment_dirty(&self, id: &str) -> bool {
        self.full || self.fragments.contains(id)
    }

    /// 该实体是否脏（全量 = 是）
    pub fn is_entity_dirty(&self, id: &str) -> bool {
        self.full || self.entities.contains(id)
    }

    /// 该断言是否脏（全量 = 是）
    pub fn is_assertion_dirty(&self, id: &str) -> bool {
        self.full || self.assertions.contains(id)
    }

    /// 脏片段 ID（全量时为空——一切皆脏，枚举无意义）
    pub fn fragment_ids(&self) -> impl Iterator<Item = &str> {
        self.fragments.iter().map(|s| s.as_str())
    }

    /// 脏实体 ID
    pub fn entity_ids(&self) -> impl Iterator<Item = &str> {
        self.entities.iter().map(|s| s.as_str())
    }

    /// 脏断言 ID
    pub fn assertion_ids(&self) -> impl Iterator<Item = &str> {
        self.assertions.iter().map(|s| s.as_str())
    }
}

/// Dependency Resolver：将事件传播为 Dirty Set (RFC-0004 §4.3)
pub fn resolve_dirty(events: &[CompileEvent]) -> DirtySet {
    let mut dirty = DirtySet::default();

    for event in events {
        match event {
            CompileEvent::FragmentChanged { id } => {
                dirty.insert(Dependency::Fragment);
                dirty.insert(Dependency::Structure);
                dirty.add_fragment_id(id.clone());
            }
            CompileEvent::EntityChanged { id } => {
                dirty.insert(Dependency::Entity);
                dirty.insert(Dependency::Fragment); // alias 变化影响 Fragment 匹配
                dirty.add_entity_id(id.clone());
            }
            CompileEvent::StructureChanged { .. } => {
                dirty.insert(Dependency::Structure);
            }
            CompileEvent::AssertionChanged { id } => {
                dirty.insert(Dependency::Assertion);
                dirty.insert(Dependency::Knowledge);
                dirty.insert(Dependency::Entity);
                dirty.add_assertion_id(id.clone());
            }
        }
    }

    dirty
}

// ============================================================================
// 编译入口
// ============================================================================

/// 执行编译 (RFC-0004 §4.4)
///
/// `previous`：上一次编译的诊断。传入后按 RFC-0005 §5.2 续期——
/// - 未重跑规则的旧诊断 → `Stale` 续期（**不续期就是"增量假绿"**：
///   没检查的规则被静默当作无问题）；
/// - 重跑规则确认已消失的旧诊断 → `Resolved`（墓碑，`has_errors` 不计）。
///
/// 生产方当前每次全量编译，传 `None` 即维持原行为。
pub fn compile(
    registry: &RuleRegistry,
    ir: &StoryIR,
    location_index: Option<&LocationIndex>,
    kb: Option<&story_knowledge::KnowledgeBase>,
    threads: Option<&story_ir::ThreadCollection>,
    request: &CompileRequest,
    previous: Option<&Diagnostics>,
) -> Diagnostics {
    // 1. 解析 Dirty Set
    let dirty = match request.mode {
        CompileMode::Full => DirtySet::all(),
        CompileMode::Incremental => resolve_dirty(&request.events),
    };

    // 2. 构建 Sink
    let mut sink = DiagnosticSink::new();

    // 3. 按阶段执行；记录重跑过的规则（增量续期以规则为粒度）
    let mut executed: BTreeSet<String> = BTreeSet::new();
    for phase in [RulePhase::Structure, RulePhase::Semantic, RulePhase::Style] {
        for rule in registry.rules_of(phase) {
            // 3a. 配置检查
            let severity = match registry.config.severity_of(rule) {
                Some(s) => s,
                None => continue,
            };

            // 3b. Dirty 匹配
            let should_run = rule.depends_on().iter().any(|d| dirty.contains(*d));

            if should_run {
                executed.insert(rule.id().to_string());
                let mut ctx =
                    RuleContext::new(ir, location_index, kb, threads, severity, &mut sink);
                rule.run(&mut ctx);
            }
        }
    }

    let mut diagnostics = sink.into_diagnostics();

    // 4. 增量 / 全量续期（RFC-0005 §5.2）
    if let Some(prev) = previous {
        // 本次实际仍在产出的问题（重跑规则的 Fresh 集合）；
        // SourceLocation 无 Ord，用 Vec + PartialEq（诊断量级下线性扫描足够）
        let fresh_keys: Vec<(String, String, crate::diagnostic::SourceLocation)> = diagnostics
            .0
            .iter()
            .map(|d| {
                (
                    d.code.as_str().to_string(),
                    d.message.clone(),
                    d.loc.clone(),
                )
            })
            .collect();
        for d in &prev.0 {
            let crate::diagnostic::DiagnosticSource::Compiler { rule_id } = &d.source else {
                continue;
            };
            let carried = if executed.contains(rule_id.as_str()) {
                // 规则重跑过：不再产出 = 本次确认已修复（全量与增量命中同语义）
                if fresh_keys.contains(&(
                    d.code.as_str().to_string(),
                    d.message.clone(),
                    d.loc.clone(),
                )) {
                    continue; // Fresh 版本已在结果里
                }
                d.clone()
                    .with_lifecycle(crate::diagnostic::Lifecycle::Resolved)
            } else {
                // 增量未命中：未检查 ≠ 无问题，以 Stale 续期
                d.clone()
                    .with_lifecycle(crate::diagnostic::Lifecycle::Stale)
            };
            diagnostics.0.push(carried);
        }
    }

    // 5. 规则级 severity 覆盖：config 显式指定级别的规则，其全部诊断
    //    统一重映射到该级别（eslint 式语义；规则内部 error/warning 的
    //    自有分级只在未配置覆盖时保留）。续期诊断一并生效。
    if !registry.config.severities.is_empty() {
        for d in diagnostics.0.iter_mut() {
            let crate::diagnostic::DiagnosticSource::Compiler { rule_id } = &d.source else {
                continue;
            };
            if let Some(sev) = registry.config.severities.get(rule_id.as_str()) {
                d.severity = *sev;
            }
        }
    }

    // 6. 确定性排序输出
    diagnostics
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, Lifecycle, Severity, SourceLocation};
    use crate::rule::{CompilerConfig, Rule, RuleContext, RulePhase};

    #[test]
    fn dirty_set_fragment_changed() {
        let dirty = resolve_dirty(&[CompileEvent::FragmentChanged { id: "f_001".into() }]);
        assert!(dirty.contains(Dependency::Fragment));
        assert!(dirty.contains(Dependency::Structure));
        assert!(!dirty.contains(Dependency::Assertion));
        // 对象 ID 进入脏集
        assert!(dirty.is_fragment_dirty("f_001"));
        assert!(!dirty.is_fragment_dirty("f_002"));
        assert!(!dirty.is_entity_dirty("c_001"));
    }

    #[test]
    fn dirty_set_assertion_changed() {
        let dirty = resolve_dirty(&[CompileEvent::AssertionChanged { id: "a_001".into() }]);
        assert!(dirty.contains(Dependency::Assertion));
        assert!(dirty.contains(Dependency::Knowledge));
        assert!(dirty.contains(Dependency::Entity));
        assert!(dirty.is_assertion_dirty("a_001"));
    }

    #[test]
    fn dirty_set_all_marks_everything_dirty() {
        let dirty = DirtySet::all();
        assert!(dirty.contains(Dependency::Structure));
        assert!(dirty.contains(Dependency::Fragment));
        assert!(dirty.contains(Dependency::Entity));
        assert!(dirty.contains(Dependency::Assertion));
        assert!(dirty.contains(Dependency::Knowledge));
        assert!(dirty.is_full());
        // 全量下一切对象皆脏（对象集为空，按 full 判定）
        assert!(dirty.is_fragment_dirty("f_any"));
        assert!(dirty.fragment_ids().next().is_none());
    }

    // ------------------------------------------------------------------
    // 增量续期（RFC-0005 §5.2）：Stale / Resolved 的构造与"假绿"回归
    // ------------------------------------------------------------------

    /// 探针规则：声明单一依赖；`emit` 控制是否产出一条诊断
    #[derive(Debug)]
    struct ProbeRule {
        id: &'static str,
        dep: Dependency,
        emit: bool,
    }

    impl Rule for ProbeRule {
        fn id(&self) -> &str {
            self.id
        }
        fn default_severity(&self) -> Severity {
            Severity::Error
        }
        fn phase(&self) -> RulePhase {
            RulePhase::Style
        }
        fn depends_on(&self) -> &'static [Dependency] {
            // 测试内静态声明（两个变体，按 dep 取用）
            static FRAG: [Dependency; 1] = [Dependency::Fragment];
            static THREAD: [Dependency; 1] = [Dependency::Thread];
            match self.dep {
                Dependency::Fragment => &FRAG,
                _ => &THREAD,
            }
        }
        fn run(&self, ctx: &mut RuleContext) {
            if self.emit {
                ctx.report(Diagnostic::compiler_error(
                    self.id,
                    "probe",
                    format!("probe:{}", self.id),
                    SourceLocation::default(),
                ));
            }
        }
    }

    fn probe_registry(frag_emit: bool) -> RuleRegistry {
        // 空注册表 + 两个探针，避免空 IR 上内置规则的噪音
        let mut reg = RuleRegistry::empty_with_config(CompilerConfig::default());
        reg.register(Box::new(ProbeRule {
            id: "frag_rule",
            dep: Dependency::Fragment,
            emit: frag_emit,
        }));
        reg.register(Box::new(ProbeRule {
            id: "thread_rule",
            dep: Dependency::Thread,
            emit: true,
        }));
        reg
    }

    #[test]
    fn incremental_carries_unchecked_rules_as_stale() {
        let ir = StoryIR::new();
        let reg = probe_registry(true);

        // 全量：两条规则都跑，两条诊断 Fresh
        let full = compile(
            &reg,
            &ir,
            None,
            None,
            None,
            &CompileRequest::full_build(),
            None,
        );
        assert_eq!(full.0.len(), 2);
        assert!(full.0.iter().all(|d| d.lifecycle == Lifecycle::Fresh));
        assert!(full.has_errors());

        // 增量（FragmentChanged）：frag_rule 重跑（Fresh），thread_rule
        // 未跑——旧诊断必须以 Stale 续期。丢掉它就是"增量假绿"
        let inc = compile(
            &reg,
            &ir,
            None,
            None,
            None,
            &CompileRequest::incremental(vec![CompileEvent::FragmentChanged {
                id: "f_001".into(),
            }]),
            Some(&full),
        );
        let fresh = inc
            .0
            .iter()
            .find(|d| d.code.as_str() == "probe" && d.message.contains("frag_rule"))
            .unwrap();
        assert_eq!(fresh.lifecycle, Lifecycle::Fresh);
        let stale = inc
            .0
            .iter()
            .find(|d| d.message.contains("thread_rule"))
            .unwrap();
        assert_eq!(stale.lifecycle, Lifecycle::Stale);
        // Stale 错误仍算错误（未检查 ≠ 已修复）
        assert!(inc.has_errors());
    }

    #[test]
    fn incremental_hit_marks_fixed_diagnostics_resolved() {
        let ir = StoryIR::new();
        let full = compile(
            &probe_registry(true),
            &ir,
            None,
            None,
            None,
            &CompileRequest::full_build(),
            None,
        );

        // frag_rule 重跑但问题已消失（emit=false）→ 旧诊断 Resolved，且
        // 不再阻断编译；thread_rule 未跑 → Stale 仍阻断
        let inc = compile(
            &probe_registry(false),
            &ir,
            None,
            None,
            None,
            &CompileRequest::incremental(vec![CompileEvent::FragmentChanged {
                id: "f_001".into(),
            }]),
            Some(&full),
        );
        let resolved = inc
            .0
            .iter()
            .find(|d| d.message.contains("frag_rule"))
            .unwrap();
        assert_eq!(resolved.lifecycle, Lifecycle::Resolved);
        assert!(inc
            .0
            .iter()
            .any(|d| { d.message.contains("thread_rule") && d.lifecycle == Lifecycle::Stale }));
        assert!(inc.has_errors(), "Stale 的 thread_rule 错误仍在");
    }

    #[test]
    fn full_compile_with_previous_resolves_all_fixed() {
        let ir = StoryIR::new();
        let full = compile(
            &probe_registry(true),
            &ir,
            None,
            None,
            None,
            &CompileRequest::full_build(),
            None,
        );
        // 全量重跑且两条都修好：全部 Resolved，has_errors 复位
        let mut reg = RuleRegistry::empty_with_config(CompilerConfig::default());
        reg.register(Box::new(ProbeRule {
            id: "frag_rule",
            dep: Dependency::Fragment,
            emit: false,
        }));
        reg.register(Box::new(ProbeRule {
            id: "thread_rule",
            dep: Dependency::Thread,
            emit: false,
        }));
        let again = compile(
            &reg,
            &ir,
            None,
            None,
            None,
            &CompileRequest::full_build(),
            Some(&full),
        );
        assert_eq!(again.0.len(), 2);
        assert!(again.0.iter().all(|d| d.lifecycle == Lifecycle::Resolved));
        assert!(!again.has_errors(), "确认修复的错误不再阻断编译");
    }
}
