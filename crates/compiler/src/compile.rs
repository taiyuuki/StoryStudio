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

/// 失效的数据维度集合 (RFC-0004 §4.3)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirtySet(pub BTreeSet<Dependency>);

impl DirtySet {
    pub fn all() -> Self {
        Self(
            [
                Dependency::Structure,
                Dependency::Fragment,
                Dependency::Entity,
                Dependency::Assertion,
                Dependency::Knowledge,
                Dependency::Thread,
            ]
            .into_iter()
            .collect(),
        )
    }

    pub fn contains(&self, dep: Dependency) -> bool {
        self.0.contains(&dep)
    }

    pub fn insert(&mut self, dep: Dependency) {
        self.0.insert(dep);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Dependency Resolver：将事件传播为 Dirty Set (RFC-0004 §4.3)
pub fn resolve_dirty(events: &[CompileEvent]) -> DirtySet {
    let mut dirty = BTreeSet::new();

    for event in events {
        match event {
            CompileEvent::FragmentChanged { .. } => {
                dirty.insert(Dependency::Fragment);
                dirty.insert(Dependency::Structure);
            }
            CompileEvent::EntityChanged { .. } => {
                dirty.insert(Dependency::Entity);
                dirty.insert(Dependency::Fragment); // alias 变化影响 Fragment 匹配
            }
            CompileEvent::StructureChanged { .. } => {
                dirty.insert(Dependency::Structure);
            }
            CompileEvent::AssertionChanged { .. } => {
                dirty.insert(Dependency::Assertion);
                dirty.insert(Dependency::Knowledge);
                dirty.insert(Dependency::Entity);
            }
        }
    }

    DirtySet(dirty)
}

// ============================================================================
// 编译入口
// ============================================================================

/// 执行编译 (RFC-0004 §4.4)
pub fn compile(
    registry: &RuleRegistry,
    ir: &StoryIR,
    location_index: Option<&LocationIndex>,
    kb: Option<&story_knowledge::KnowledgeBase>,
    threads: Option<&story_ir::ThreadCollection>,
    request: &CompileRequest,
) -> Diagnostics {
    // 1. 解析 Dirty Set
    let dirty = match request.mode {
        CompileMode::Full => DirtySet::all(),
        CompileMode::Incremental => resolve_dirty(&request.events),
    };

    // 2. 构建 Sink
    let mut sink = DiagnosticSink::new();

    // 3. 按阶段执行
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
                let mut ctx =
                    RuleContext::new(ir, location_index, kb, threads, severity, &mut sink);
                rule.run(&mut ctx);
            }
        }
    }

    // 4. 确定性排序输出
    sink.into_diagnostics()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_set_fragment_changed() {
        let dirty = resolve_dirty(&[CompileEvent::FragmentChanged { id: "f_001".into() }]);
        assert!(dirty.contains(Dependency::Fragment));
        assert!(dirty.contains(Dependency::Structure));
        assert!(!dirty.contains(Dependency::Assertion));
    }

    #[test]
    fn dirty_set_assertion_changed() {
        let dirty = resolve_dirty(&[CompileEvent::AssertionChanged { id: "a_001".into() }]);
        assert!(dirty.contains(Dependency::Assertion));
        assert!(dirty.contains(Dependency::Knowledge));
        assert!(dirty.contains(Dependency::Entity));
    }

    #[test]
    fn dirty_set_all() {
        let dirty = DirtySet::all();
        assert!(dirty.contains(Dependency::Structure));
        assert!(dirty.contains(Dependency::Fragment));
        assert!(dirty.contains(Dependency::Entity));
        assert!(dirty.contains(Dependency::Assertion));
        assert!(dirty.contains(Dependency::Knowledge));
    }
}
