//! Diagnostic — 统一诊断结构 (RFC-0005)
//!
//! 所有检查结果（Compiler Error/Warning、Parser Error、Validator Hint）
//! 都汇聚为同一种 Diagnostic 结构。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use story_ir::StableId;

// ============================================================================
// Severity
// ============================================================================

/// 严重级别 (RFC-0005 §2.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// 必须修复，阻断 Dry-Run 与 Export
    Error,
    /// 建议修复，不阻断
    Warning,
    /// 建议性提示
    Hint,
    /// 信息性说明
    Info,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Hint => "hint",
            Severity::Info => "info",
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "error" => Ok(Severity::Error),
            "warning" => Ok(Severity::Warning),
            "hint" => Ok(Severity::Hint),
            "info" => Ok(Severity::Info),
            "off" => Err("off".to_string()), // "off" 表示禁用，不是合法 Severity
            other => Err(other.to_string()),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================================
// DiagnosticSource
// ============================================================================

/// 诊断的产出者标识 (RFC-0005 §3.2)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiagnosticSource {
    Parser,
    Compiler { rule_id: String },
    SemanticValidator { name: String },
}

// ============================================================================
// DiagnosticCode
// ============================================================================

/// 诊断分类码——稳定标识 (RFC-0005 §3.3)
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ============================================================================
// SourceLocation
// ============================================================================

/// 诊断定位——块级，非行级 (RFC-0005 §4, RFC-0001 §10.1)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub entity: Option<StableId>,
    pub fragment: Option<StableId>,
    pub assertion: Option<StableId>,
    pub structure: Option<StableId>,
}

impl SourceLocation {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn entity(id: StableId) -> Self {
        Self {
            entity: Some(id),
            fragment: None,
            assertion: None,
            structure: None,
        }
    }

    pub fn fragment(id: StableId) -> Self {
        Self {
            entity: None,
            fragment: Some(id),
            assertion: None,
            structure: None,
        }
    }

    pub fn structure(id: StableId) -> Self {
        Self {
            entity: None,
            fragment: None,
            assertion: None,
            structure: Some(id),
        }
    }

    pub fn assertion(id: StableId) -> Self {
        Self {
            entity: None,
            fragment: None,
            assertion: Some(id),
            structure: None,
        }
    }

    /// 可排序键（确定性排序用）
    fn sort_key(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.entity.as_ref().map(|s| s.as_str()).unwrap_or(""),
            self.fragment.as_ref().map(|s| s.as_str()).unwrap_or(""),
            self.structure.as_ref().map(|s| s.as_str()).unwrap_or(""),
            self.assertion.as_ref().map(|s| s.as_str()).unwrap_or(""),
        )
    }
}

impl PartialOrd for SourceLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SourceLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

// ============================================================================
// Lifecycle
// ============================================================================

/// 诊断生命周期（增量编译用）(RFC-0005 §5)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lifecycle {
    /// 本次编译新产出或确认
    #[default]
    Fresh,
    /// 增量编译未重新检查，可能已过时
    Stale,
    /// 问题已修复
    Resolved,
}

// ============================================================================
// Diagnostic
// ============================================================================

/// 一条诊断 (RFC-0005 §3.1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub source: DiagnosticSource,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub loc: SourceLocation,
    #[allow(dead_code)]
    pub lifecycle: Lifecycle,
}

impl Diagnostic {
    pub fn new(
        source: DiagnosticSource,
        severity: Severity,
        code: impl Into<String>,
        message: impl Into<String>,
        loc: SourceLocation,
    ) -> Self {
        Self {
            source,
            severity,
            code: DiagnosticCode::new(code),
            message: message.into(),
            loc,
            lifecycle: Lifecycle::Fresh,
        }
    }

    /// 创建 Compiler 产出的 Error
    pub fn compiler_error(
        rule_id: &str,
        code: &str,
        msg: impl Into<String>,
        loc: SourceLocation,
    ) -> Self {
        Self::new(
            DiagnosticSource::Compiler {
                rule_id: rule_id.to_string(),
            },
            Severity::Error,
            code,
            msg,
            loc,
        )
    }

    /// 创建 Compiler 产出的 Warning
    pub fn compiler_warning(
        rule_id: &str,
        code: &str,
        msg: impl Into<String>,
        loc: SourceLocation,
    ) -> Self {
        Self::new(
            DiagnosticSource::Compiler {
                rule_id: rule_id.to_string(),
            },
            Severity::Warning,
            code,
            msg,
            loc,
        )
    }
}

// ============================================================================
// DiagnosticSink + Diagnostics
// ============================================================================

/// 诊断收集器（Sink 模式）(RFC-0004 §3.2, RFC-0005 §8)
#[derive(Debug, Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    pub fn extend(&mut self, diags: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diags);
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// 排序后输出——确定性排序 (RFC-0005 §8.2)
    pub fn into_diagnostics(self) -> Diagnostics {
        let mut diags = self.diagnostics;

        // 排序：severity（Error 优先）→ location → code
        diags.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then(a.loc.cmp(&b.loc))
                .then(a.code.cmp(&b.code))
        });

        // 去重：(source, code, loc) 三元组 (RFC-0005 §8.3)
        let mut seen: BTreeSet<String> = BTreeSet::new();
        diags.retain(|d| {
            let key = format!("{:?}\t{}\t{}", d.source, d.code, d.loc.sort_key());
            seen.insert(key)
        });

        Diagnostics(diags)
    }
}

/// 最终诊断集合
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    /// 是否包含 Error 级诊断
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(|d| d.severity == Severity::Error)
    }

    /// 是否包含 Warning 级诊断
    pub fn has_warnings(&self) -> bool {
        self.0.iter().any(|d| d.severity == Severity::Warning)
    }

    /// 所有 Error 级诊断
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| d.severity == Severity::Error)
    }

    /// 所有 Warning 级诊断
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| d.severity == Severity::Warning)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Hint);
        assert!(Severity::Hint < Severity::Info);
    }

    #[test]
    fn diagnostics_has_errors() {
        let mut sink = DiagnosticSink::new();
        sink.push(Diagnostic::compiler_error(
            "r1",
            "test.error",
            "错误",
            SourceLocation::none(),
        ));
        sink.push(Diagnostic::compiler_warning(
            "r2",
            "test.warn",
            "警告",
            SourceLocation::none(),
        ));

        let diags = sink.into_diagnostics();
        assert!(diags.has_errors());
        assert!(diags.has_warnings());
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn diagnostics_sorted_by_severity() {
        let mut sink = DiagnosticSink::new();
        // 故意反序插入
        sink.push(Diagnostic::compiler_warning(
            "r2",
            "b",
            "警告B",
            SourceLocation::none(),
        ));
        sink.push(Diagnostic::new(
            DiagnosticSource::Parser,
            Severity::Info,
            "c",
            "信息",
            SourceLocation::none(),
        ));
        sink.push(Diagnostic::compiler_error(
            "r1",
            "a",
            "错误A",
            SourceLocation::none(),
        ));

        let diags = sink.into_diagnostics();
        // Error 应在最前
        assert_eq!(diags.0[0].severity, Severity::Error);
        // Info 应在最后
        assert_eq!(diags.0[2].severity, Severity::Info);
    }

    #[test]
    fn diagnostics_dedup() {
        let mut sink = DiagnosticSink::new();
        let loc = SourceLocation::fragment(StableId::new_unchecked("f_001"));
        sink.push(Diagnostic::compiler_error("r1", "dup", "重复", loc.clone()));
        sink.push(Diagnostic::compiler_error("r1", "dup", "重复", loc)); // 完全相同

        let diags = sink.into_diagnostics();
        assert_eq!(diags.len(), 1); // 去重后只有一条
    }
}
