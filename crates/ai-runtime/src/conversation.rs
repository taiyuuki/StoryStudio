//! ConversationalTask — 有状态会话 (RFC-0010 §4.3, §10)
//!
//! 适用于续写章节、构思情节等多轮探讨的创作任务。
//! 管理对话历史 + draft 累积 + 回退。
//! draft IR 不进 Git，存于 .novelstudio/sessions/。

use serde::{Deserialize, Serialize};
use story_compiler::{dry_run, DryRunResult, RuleRegistry};
use story_ir::{LocationIndex, StoryIR};
use story_knowledge::KnowledgeBase;

use crate::provider::{AiError, AiProvider, GenerateRequest, Message};
use crate::task::{FragmentDraft, Task, TaskError};

#[cfg(test)]
use crate::provider::Role;

// ============================================================================
// Conversation — 有状态会话
// ============================================================================

/// 一次创作会话
///
/// 维护多轮对话历史与累积的 Fragment 草稿。
/// 每轮对话可以试错——AI 生成的草稿累积在 draft 中，
/// 通过 commit 提交或 rollback 回退。
pub struct Conversation<'a> {
    /// 对话历史（System + 多轮 User/Assistant）
    messages: Vec<Message>,
    /// 累积的 Fragment 草稿（尚未提交到 IR）
    drafts: Vec<FragmentDraft>,
    /// 已消耗的总 token 数
    total_tokens: usize,

    // 引用（只读）
    ir: &'a StoryIR,
    location_index: Option<&'a LocationIndex>,
    kb: Option<&'a KnowledgeBase>,
    registry: &'a RuleRegistry,
}

/// 一次对话轮次的结果
#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// 本轮 AI 生成的草稿
    pub new_drafts: Vec<FragmentDraft>,
    /// 累积的草稿总数
    pub total_drafts: usize,
    /// 本轮 token 消耗
    pub tokens_used: usize,
    /// 累积 token 消耗
    pub total_tokens: usize,
}

/// 提交结果
#[derive(Debug, Serialize, Deserialize)]
pub enum CommitResult {
    /// 所有草稿通过 Dry-Run，可以提交
    Passed { drafts: Vec<FragmentDraft> },
    /// 存在 Error，不可提交
    Failed { errors: Vec<String> },
}

impl<'a> Conversation<'a> {
    /// 创建新会话
    pub fn new(
        ir: &'a StoryIR,
        location_index: Option<&'a LocationIndex>,
        kb: Option<&'a KnowledgeBase>,
        registry: &'a RuleRegistry,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            messages: vec![Message::system(system_prompt)],
            drafts: Vec::new(),
            total_tokens: 0,
            ir,
            location_index,
            kb,
            registry,
        }
    }

    /// 发送用户消息，获取 AI 回复
    pub fn send(
        &mut self,
        user_input: &str,
        task: &dyn Task,
        provider: &dyn AiProvider,
    ) -> Result<ConversationTurn, AiError> {
        // 1. 追加用户消息
        self.messages.push(Message::user(user_input));

        // 2. 调用 AI（带完整对话历史）
        let request = GenerateRequest {
            system: String::new(), // system 已在 messages[0]
            messages: self.messages.clone(),
            model: String::new(),
            temperature: 0.8,
            max_tokens: 4096,
            tools: Vec::new(),
        };

        let response = provider.generate(&request)?;

        // 3. 解析输出
        let new_drafts = task.parse_output(&response.text).unwrap_or_default();

        // 4. 累积草稿
        let drafts_count = new_drafts.len();
        self.drafts.extend(new_drafts);

        // 5. 保存 AI 回复到历史
        self.messages.push(Message::assistant(&response.text));

        // 6. 统计
        let tokens = response.usage.total();
        self.total_tokens += tokens;

        Ok(ConversationTurn {
            new_drafts: self.drafts[self.drafts.len() - drafts_count..].to_vec(),
            total_drafts: self.drafts.len(),
            tokens_used: tokens,
            total_tokens: self.total_tokens,
        })
    }

    /// 对累积的草稿做 Dry-Run 验证
    ///
    /// v0.1 简化：检查当前 IR（草稿尚未 apply 到 IR）。
    /// 完整实现应构建包含草稿的 draft IR 再验证。
    pub fn commit(&self) -> CommitResult {
        let result = dry_run(self.registry, self.ir, self.location_index, self.kb, None);

        match result {
            DryRunResult::Passed => CommitResult::Passed {
                drafts: self.drafts.clone(),
            },
            DryRunResult::Failed(errors) => CommitResult::Failed {
                errors: errors
                    .into_iter()
                    .map(|e| format!("[{}] {}", e.severity, e.message))
                    .collect(),
            },
        }
    }

    /// 回退到指定轮次
    ///
    /// 移除指定轮次之后的所有 AI 回复 + 对应草稿。
    /// round 从 0 开始（0 = 初始状态）。
    pub fn rollback(&mut self, round: usize) {
        // 对话历史：System(0) + 每轮 [User, Assistant]
        // 第 N 轮的 User 在 index 1 + N*2，Assistant 在 1 + N*2 + 1
        let keep_messages = 1 + (round + 1) * 2; // System + 完整轮次
        self.messages.truncate(keep_messages);

        // 草稿回退：需要记录每轮新增了多少草稿
        // v0.1 简化：草稿无法精确按轮次回退（因为没有记录每轮的数量）
        // 完整实现应记录 per-round draft count
        // 当前策略：如果 round=0 则清空所有草稿
        if round == 0 {
            self.drafts.clear();
        }
    }

    /// 获取当前累积的所有草稿
    pub fn drafts(&self) -> &[FragmentDraft] {
        &self.drafts
    }

    /// 获取对话历史
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 累积的 token 消耗
    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }
}

// ============================================================================
// 带重试反馈的发送（RFC-0010 §8）
// ============================================================================

/// 带反馈重试的发送——失败时自动将错误反馈注入下一轮
///
/// 流程：发送 → Dry-Run → 失败则带反馈重试 → 最多 max_retries 次
pub fn send_with_retry(
    conversation: &mut Conversation<'_>,
    user_input: &str,
    task: &dyn Task,
    provider: &dyn AiProvider,
    max_retries: usize,
) -> Result<ConversationTurn, TaskError> {
    let mut feedback = String::new();
    let mut attempts = 0;

    loop {
        // 构建本轮输入（含上次的错误反馈）
        let input = if feedback.is_empty() {
            user_input.to_string()
        } else {
            format!("{user_input}\n\n---\n上次尝试的反馈：\n{feedback}")
        };

        let turn = conversation
            .send(&input, task, provider)
            .map_err(|e| TaskError::Parse(format!("AI 调用失败: {e}")))?;

        // Dry-Run 验证
        match conversation.commit() {
            CommitResult::Passed { .. } => return Ok(turn),
            CommitResult::Failed { errors } => {
                attempts += 1;
                feedback = task.build_feedback(&errors);

                if attempts >= max_retries {
                    return Err(TaskError::DryRunFailed { n: errors.len() });
                }
                // 重试：回退上一轮（移除刚加的 assistant 回复 + 对应草稿）
                conversation.rollback(conversation.messages().len() / 2 - 2);
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AiProvider, GenerateRequest, GenerateResponse, StopReason, TokenUsage};
    use story_compiler::CompilerConfig;
    use story_ir::*;

    #[derive(Debug)]
    struct ScriptedProvider {
        responses: Vec<String>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses.into_iter().map(String::from).collect(),
                call_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl AiProvider for ScriptedProvider {
        fn id(&self) -> &str {
            "scripted"
        }
        fn generate(&self, _req: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            let idx = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let text = self.responses.get(idx).cloned().unwrap_or_default();
            Ok(GenerateResponse {
                text,
                usage: TokenUsage {
                    input_tokens: 50,
                    output_tokens: 30,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    use crate::task::WriteFragmentTask;

    fn make_test_env() -> (StoryIR, RuleRegistry) {
        let ir = StoryIR::new();
        let registry = RuleRegistry::new(CompilerConfig::default());
        (ir, registry)
    }

    #[test]
    fn conversation_single_turn() {
        let (ir, registry) = make_test_env();
        let provider = ScriptedProvider::new(vec!["林风走进客栈。"]);

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let mut conv = Conversation::new(&ir, None, None, &registry, "你是小说作者");
        let turn = conv.send("写一段", &task, &provider).unwrap();

        assert_eq!(turn.total_drafts, 1);
        assert!(turn.new_drafts[0].text.contains("林风走进客栈"));
        assert_eq!(conv.total_tokens(), 80); // 50 + 30
    }

    #[test]
    fn conversation_multi_turn() {
        let (ir, registry) = make_test_env();
        let provider = ScriptedProvider::new(vec!["第一段。", "第二段。"]);

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let mut conv = Conversation::new(&ir, None, None, &registry, "你是小说作者");

        let t1 = conv.send("写第一段", &task, &provider).unwrap();
        assert_eq!(t1.total_drafts, 1);

        let t2 = conv.send("再写一段", &task, &provider).unwrap();
        assert_eq!(t2.total_drafts, 2); // 累积两段
    }

    #[test]
    fn conversation_commit_passed() {
        let (ir, registry) = make_test_env();
        let provider = ScriptedProvider::new(vec!["正文"]);

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let mut conv = Conversation::new(&ir, None, None, &registry, "system");
        conv.send("写一段", &task, &provider).unwrap();

        match conv.commit() {
            CommitResult::Passed { drafts } => {
                assert_eq!(drafts.len(), 1);
            }
            CommitResult::Failed { .. } => panic!("空 IR 应通过 Dry-Run"),
        }
    }

    #[test]
    fn conversation_rollback_clears_drafts() {
        let (ir, registry) = make_test_env();
        let provider = ScriptedProvider::new(vec!["第一段。"]);

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let mut conv = Conversation::new(&ir, None, None, &registry, "system");
        conv.send("写一段", &task, &provider).unwrap();
        assert_eq!(conv.drafts().len(), 1);

        conv.rollback(0);
        assert!(conv.drafts().is_empty());
    }

    #[test]
    fn conversation_message_history() {
        let (ir, registry) = make_test_env();
        let provider = ScriptedProvider::new(vec!["AI 回复"]);

        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
            system_template: None,
        };

        let mut conv = Conversation::new(&ir, None, None, &registry, "系统指令");
        conv.send("用户输入", &task, &provider).unwrap();

        let messages = conv.messages();
        // System + User + Assistant = 3
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[2].role, Role::Assistant);
    }
}
