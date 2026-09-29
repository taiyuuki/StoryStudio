//! Compiler — 确定性一致性检查 (RFC-0004) + Diagnostics (RFC-0005)
//!
//! Compiler 是 Parser 的对称镜像：
//!   Parser 管"能否解析"，Compiler 管"是否合理"。
//!   两者都纯确定、都只读、都不碰 AI。
//!
//! 规则统一注册、调度器按依赖执行：
//!   CompileEvent → Dependency Resolver → Dirty Set → Rule Scheduler → Rules

pub mod builtin_rules;
pub mod compile;
pub mod diagnostic;
pub mod dry_run;
pub mod rule;

pub use compile::{
    compile, resolve_dirty, CompileEvent, CompileMode, CompileRequest, DirtySet, RequestKind,
};
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticSink, DiagnosticSource, Diagnostics, Lifecycle, Severity,
    SourceLocation,
};
pub use dry_run::{dry_run, DryRunResult};
pub use rule::{CompilerConfig, Dependency, Rule, RuleContext, RulePhase, RuleRegistry};

/// compiler crate 对应的 RFC 版本
pub const RFC_VERSION: &str = "0.2";
