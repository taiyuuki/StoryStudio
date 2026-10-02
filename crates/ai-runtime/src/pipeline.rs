//! Output Pipeline — AI 输出 → Parser → Change → Dry-Run (RFC-0010 §6)
//!
//! 核心流程：AI 输出文本 → Task 解析为 FragmentDraft → Dry-Run 验证 → 返回结果。
//! AI 写小说，系统验逻辑。

use serde::{Deserialize, Serialize};
use story_compiler::{dry_run, RuleRegistry};
use story_ir::{LocationIndex, StoryIR};
use story_knowledge::KnowledgeBase;
use thiserror::Error;

use crate::provider::{AiError, AiProvider, GenerateRequest};
use crate::task::{FragmentDraft, Task, TaskError};

// ============================================================================
// PipelineResult / PipelineError
// ============================================================================

/// Pipeline 执行结果
#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineResult {
    /// AI 生成的 Fragment 草稿（已通过 Dry-Run 验证）
    pub drafts: Vec<FragmentDraft>,
    /// Dry-Run 是否通过
    pub dry_run_passed: bool,
    /// Dry-Run 的诊断（失败时）
    pub diagnostics: Vec<String>,
    /// token 消耗
    pub total_tokens: usize,
}

/// Pipeline 错误
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum PipelineError {
    #[error("AI 调用失败: {0}")]
    Ai(#[from] AiError),

    #[error("输出解析失败: {0}")]
    Parse(#[from] TaskError),
}

// ============================================================================
// OutputPipeline
// ============================================================================

/// 编排 AI 生成 → 解析 → Dry-Run 的管线 (RFC-0010 §6.2)
pub struct OutputPipeline<'a> {
    ir: &'a StoryIR,
    location_index: Option<&'a LocationIndex>,
    kb: Option<&'a KnowledgeBase>,
    registry: &'a RuleRegistry,
}

impl<'a> OutputPipeline<'a> {
    pub fn new(
        ir: &'a StoryIR,
        location_index: Option<&'a LocationIndex>,
        kb: Option<&'a KnowledgeBase>,
        registry: &'a RuleRegistry,
    ) -> Self {
        Self {
            ir,
            location_index,
            kb,
            registry,
        }
    }

    /// 执行无状态任务（RFC-0010 §4.2 + §6）
    ///
    /// 流程：构建上下文 → AI 生成 → 解析 → Dry-Run → 返回
    pub fn run(
        &self,
        task: &dyn Task,
        provider: &dyn AiProvider,
    ) -> Result<PipelineResult, PipelineError> {
        // 1. 构建上下文
        let ctx = task.build_context(self.ir);

        // 2. 调用 AI
        let request = GenerateRequest {
            system: ctx.system,
            messages: vec![crate::provider::Message::user(ctx.user)],
            model: String::new(),
            temperature: ctx.model.temperature,
            max_tokens: ctx.model.max_tokens,
            tools: Vec::new(),
        };

        let response = provider.generate(&request)?;

        // 3. 解析输出
        let drafts = task.parse_output(&response.text)?;

        // 4. 验证当前 IR 状态（不真正 apply draft，只检查现有 IR 的合法性）
        //
        // v0.1 简化：只检查当前 IR 是否通过 Dry-Run。
        // 完整实现（RFC-0010 §6.2）应 clone IR、apply draft、再 Dry-Run。
        let dry_result = dry_run(self.registry, self.ir, self.location_index, self.kb, None);

        let (passed, diagnostics) = match dry_result {
            story_compiler::DryRunResult::Passed => (true, vec![]),
            story_compiler::DryRunResult::Failed(errors) => {
                let diags = errors
                    .into_iter()
                    .map(|e| format!("[{}] {}", e.severity, e.message))
                    .collect();
                (false, diags)
            }
        };

        Ok(PipelineResult {
            drafts,
            dry_run_passed: passed,
            diagnostics,
            total_tokens: response.usage.total(),
        })
    }

    /// 带重试的执行 (RFC-0010 §8)
    ///
    /// **v0.1 占位实现**：当前不进行反馈重试——完整实现需要在重试时
    /// 把上一次的诊断错误注入 `build_context`，该反馈链路尚未实现。
    /// `max_retries` 参数已保留以对齐 RFC 契约，待反馈重试落地后启用。
    /// 现阶段等价于单次 `run`，但保留接口以便调用方提前对接。
    pub fn run_with_retry(
        &self,
        task: &dyn Task,
        provider: &dyn AiProvider,
        _max_retries: usize,
    ) -> Result<PipelineResult, PipelineError> {
        self.run(task, provider)
    }
}

// ============================================================================
// 便捷入口
// ============================================================================

/// 运行无状态任务的便捷入口
pub fn run_stateless(
    task: &dyn Task,
    provider: &dyn AiProvider,
    ir: &StoryIR,
    location_index: Option<&LocationIndex>,
    kb: Option<&KnowledgeBase>,
    registry: &RuleRegistry,
) -> Result<PipelineResult, PipelineError> {
    let pipeline = OutputPipeline::new(ir, location_index, kb, registry);
    pipeline.run(task, provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        AiError, AiProvider, GenerateRequest, GenerateResponse, StopReason, TokenUsage,
    };
    use crate::task::WriteFragmentTask;
    use story_compiler::CompilerConfig;
    use story_ir::*;

    /// Mock Provider 返回固定文本
    #[derive(Debug)]
    struct MockProvider {
        response: String,
    }

    impl AiProvider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        fn generate(&self, _req: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    #[test]
    fn pipeline_write_fragment_success() {
        let ir = StoryIR::new();
        let registry = RuleRegistry::new(CompilerConfig::default());
        let provider = MockProvider {
            response: "林风走进夜色中。".to_string(),
        };

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let result = run_stateless(&task, &provider, &ir, None, None, &registry).unwrap();

        assert!(!result.drafts.is_empty());
        assert!(result.drafts[0].text.contains("林风走进夜色中"));
        assert!(result.dry_run_passed); // 空 IR 应通过 Dry-Run
        assert_eq!(result.total_tokens, 150);
    }

    #[test]
    fn pipeline_parse_frontmatter() {
        let ir = StoryIR::new();
        let registry = RuleRegistry::new(CompilerConfig::default());

        // IR 中有 c_001 角色使 Dry-Run 能通过
        let mut ir = ir;
        ir.semantic.insert(SemanticNode::Character(CharacterNode {
            id: StableId::new_unchecked("c_001"),
            symbol: None,
            display_name: "林风".to_string(),
            aliases: vec![],
            attributes: Default::default(),
        }));

        let provider = MockProvider {
            response: "---\nid: f_999\nanchors:\n  - entity: c_001\n    role: appears\n---\n\n林风站在桥头。".to_string(),
        };

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let result = run_stateless(&task, &provider, &ir, None, None, &registry).unwrap();

        assert_eq!(result.drafts[0].anchors.len(), 1);
        assert_eq!(result.drafts[0].anchors[0].entity.as_str(), "c_001");
    }
}
