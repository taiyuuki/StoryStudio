//! Dry-Run — 试运行 (RFC-0004 §6)
//!
//! DryRun 是一种 CompileRequest（RequestKind::DryRun），
//! 在 draft IR 上试运行，不修改真实 IR。
//! "必要但不充分"——只校验结构与时序层（Error 级），不保证语义质量。

use serde::{Deserialize, Serialize};
use story_ir::{LocationIndex, StoryIR};

use crate::compile::{compile, CompileMode, CompileRequest, RequestKind};
use crate::diagnostic::Severity;
use crate::rule::RuleRegistry;

/// Dry-Run 结果
#[derive(Debug, Serialize, Deserialize)]
pub enum DryRunResult {
    /// 无 Error，可以提交
    Passed,
    /// 存在 Error，不可提交
    Failed(Vec<crate::diagnostic::Diagnostic>),
}

/// 在 IR 上执行 Dry-Run
///
/// v0.1 的简化实现：直接对给定 IR 跑全量编译，检查是否有 Error。
/// 完整实现（RFC-0004 §6.2）应先 clone IR、apply Change、再编译。
pub fn dry_run(
    registry: &RuleRegistry,
    ir: &StoryIR,
    location_index: Option<&LocationIndex>,
    kb: Option<&story_knowledge::KnowledgeBase>,
    threads: Option<&story_ir::ThreadCollection>,
) -> DryRunResult {
    let request = CompileRequest {
        events: Vec::new(),
        mode: CompileMode::Full,
        kind: RequestKind::DryRun {
            change_summary: String::new(),
        },
    };

    let diags = compile(registry, ir, location_index, kb, threads, &request, None);

    let errors: Vec<_> = diags
        .0
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();

    if errors.is_empty() {
        DryRunResult::Passed
    } else {
        DryRunResult::Failed(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::CompilerConfig;
    use story_ir::*;

    #[test]
    fn dry_run_passes_clean_ir() {
        let registry = RuleRegistry::new(CompilerConfig::default());
        let ir = StoryIR::new();

        let result = dry_run(&registry, &ir, None, None, None);
        assert!(matches!(result, DryRunResult::Passed));
    }

    #[test]
    fn dry_run_fails_on_dangling_anchor() {
        let registry = RuleRegistry::new(CompilerConfig::default());
        let mut ir = StoryIR::new();

        // 插入一个 anchor 引用不存在实体的 Fragment
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: None,
            prev: None,
            next: None,
            text: "测试".to_string(),
            anchors: vec![EntityAnchor {
                entity: StableId::new_unchecked("c_nonexistent"),
                role: AnchorRole::Appears,
            }],
        }));

        let result = dry_run(&registry, &ir, None, None, None);
        assert!(matches!(result, DryRunResult::Failed(_)));
    }
}
