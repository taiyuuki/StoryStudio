//! Agent — ReAct 主循环 (Reasoning + Acting)
//!
//! 基于 ReAct 模式的 Agent 框架，自动执行多轮会话：
//!
//! 1. **感知上下文** — 组装 system + 对话历史 + scratchpad 记忆 + 新指令
//! 2. **规划行动** — 复杂指令可先分解为 subtask 计划（plan-first）
//! 3. **调用工具** — LLM 产出 tool_calls → 执行工具 → 收集结果
//! 4. **整合反馈** — 工具结果回传 LLM，产出显式 reasoning（反思）
//! 5. **更新状态** — 更新进度 / 压缩上下文 / 检查完成
//! 6. 循环直到 LLM 返回文本（任务完成）或被取消

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::provider::{
    AiError, AiProvider, GenerateRequest, GenerateResponse, Message, StopReason, StreamEvent,
    TokenUsage, ToolCall, ToolDef,
};

// ============================================================================
// Tool trait
// ============================================================================

/// Agent 可调用的工具
pub trait Tool: Send + Sync {
    /// 工具定义（名称、描述、参数 JSON Schema）
    fn definition(&self) -> ToolDef;

    /// 执行工具，返回结果文本（给 LLM 作为下一轮的上下文）
    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;
}

// ============================================================================
// ToolRegistry
// ============================================================================

/// 工具注册表 — 管理可用工具
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// 获取所有工具定义（给 LLM）
    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// 工具数量
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 按名称查找并执行工具
    ///
    /// 返回 `Some(Ok(result))` 表示工具找到并执行；
    /// 返回 `Some(Err(msg))` 表示工具找到但执行失败（错误信息会反馈给 LLM）；
    /// 返回 `None` 表示工具不存在。
    pub fn execute(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<Result<String, String>> {
        self.tools
            .iter()
            .find(|t| t.definition().name == name)
            .map(|t| t.execute(arguments))
    }
}

// ============================================================================
// AgentConfig
// ============================================================================

/// Agent 配置
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: usize,
    /// 最大工具调用轮次（超过后强制结束）
    pub max_tool_turns: usize,
    /// 上下文窗口 token 预算（超出触发压缩）
    pub max_context_tokens: usize,
    /// 滑动窗口保留最近 N 轮对话原文
    pub keep_recent_turns: usize,
    /// 是否启用 plan-first（复杂指令先分解计划）
    pub enable_planning: bool,
    /// 计划最多子任务数（防止计划爆炸）
    pub max_plan_subtasks: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            temperature: 0.7,
            // 思考型模型的 reasoning token 计入 max_tokens（GLM-5.2 平均 30k+），
            // 8192 会直接导致空输出截断。65536 覆盖绝大多数思考场景。
            max_tokens: 65536,
            max_tool_turns: 40,
            max_context_tokens: 500_000,
            keep_recent_turns: 8,
            enable_planning: true,
            max_plan_subtasks: 10,
        }
    }
}

// ============================================================================
// AgentState / TaskPlan / Scratchpad
// ============================================================================

/// Agent 运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentState {
    /// 等待新指令
    #[default]
    Idle,
    /// 正在分解任务
    Planning,
    /// 执行工具循环中
    Executing,
    /// 工具结果整合中（反思）
    Reflecting,
    /// 本轮指令完成
    Completed,
    /// 被用户取消
    Cancelled,
}

/// 任务计划（plan-first 产物）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskPlan {
    /// 总目标
    pub goal: String,
    /// 分解的子任务
    pub subtasks: Vec<SubTask>,
}

/// 子任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub description: String,
    pub done: bool,
}

/// 上下文记忆（压缩后的早期历史摘要）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scratchpad {
    /// 已完成的事项摘要
    pub completed_actions: Vec<String>,
    /// 发现的关键事实
    pub key_facts: Vec<String>,
}

impl Scratchpad {
    /// 渲染为注入 system prompt 的文本块
    fn render(&self) -> String {
        let mut parts = Vec::new();
        if !self.completed_actions.is_empty() {
            parts.push(format!(
                "已完成的事项：\n{}",
                self.completed_actions
                    .iter()
                    .map(|a| format!("- {a}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        if !self.key_facts.is_empty() {
            parts.push(format!(
                "关键事实：\n{}",
                self.key_facts
                    .iter()
                    .map(|f| format!("- {f}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        parts.join("\n\n")
    }

    fn is_empty(&self) -> bool {
        self.completed_actions.is_empty() && self.key_facts.is_empty()
    }
}

// ============================================================================
// AgentStep / AgentTrace
// ============================================================================

/// Agent 运行中的每个步骤（给前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStep {
    /// 用户消息
    UserMessage { content: String },
    /// AI 文本回复
    AiText { content: String },
    /// AI 请求调用工具
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// 工具执行结果
    ToolResult { tool_name: String, content: String },
    /// 错误
    Error { message: String },
    /// 任务计划分解
    Planning { plan: TaskPlan },
    /// 子任务进度更新
    PlanProgress {
        subtask_index: usize,
        total: usize,
        description: String,
    },
    /// 显式推理（工具结果后的反思）
    Reasoning { thought: String },
    /// 上下文压缩通知
    ContextCompacted {
        summary: String,
        tokens_saved: usize,
    },
    /// Agent 向用户提问（等待用户回答后继续）
    ///
    /// `prompt_id` 唯一标识本次提问，供前端去重/标记已回答；
    /// `question` 为问题文本；`options` 为 Agent 自拟的可选项（可为空，表示纯开放式）。
    AskUser {
        prompt_id: String,
        question: String,
        options: Vec<String>,
    },
    /// LLM 调用网络失败，正在重试（瞬态状态，前端应替换显示而非累积）
    ///
    /// `attempt` 为当前重试序号（1-based），`max_retries` 为最大重试次数，
    /// `reason` 为失败原因摘要。重试成功后由后续正常 step 自然覆盖。
    Retrying {
        attempt: usize,
        max_retries: usize,
        reason: String,
    },
}

/// Agent 运行完整跟踪
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentTrace {
    pub steps: Vec<AgentStep>,
    pub total_tokens: usize,
    pub tool_call_count: usize,
}

// ============================================================================
// 回调类型
// ============================================================================

/// 步骤回调 — 每产生一个 AgentStep 即推送
type StepCallback = Arc<dyn Fn(AgentStep) + Send + Sync>;

/// 文本增量回调 — 流式输出时推送
type DeltaCallback = Arc<dyn Fn(String) + Send + Sync>;

// ============================================================================
// Agent
// ============================================================================

/// Agent 主循环 — 基于 ReAct 模式管理多轮对话与工具调用。
///
/// 持有 owned 的 provider 和 tool registry（Arc），
/// 因此可存入 SessionStore 跨调用保持状态。
pub struct Agent {
    config: AgentConfig,
    provider: Arc<dyn AiProvider>,
    tools: Arc<ToolRegistry>,
    system_prompt: String,
    /// 对话历史（System + 多轮 User/Assistant/Tool）
    messages: Vec<Message>,
    /// 当前运行状态
    state: AgentState,
    /// 任务计划（plan-first 产物，单次指令内有效）
    ///
    /// 保留供未来显式 plan-first 阶段使用；当前让 LLM 在首轮自然处理规划。
    #[allow(dead_code)]
    plan: Option<TaskPlan>,
    /// 压缩后的上下文记忆
    scratchpad: Scratchpad,
    /// 当前指令的 trace（运行时累积）
    trace: AgentTrace,
    /// 取消令牌
    cancel_token: CancelToken,
}

impl Agent {
    /// 创建 Agent（owned provider + tools）
    pub fn new(
        system_prompt: impl Into<String>,
        config: AgentConfig,
        provider: Arc<dyn AiProvider>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            config,
            provider,
            tools,
            messages: Vec::new(),
            state: AgentState::Idle,
            plan: None,
            scratchpad: Scratchpad::default(),
            trace: AgentTrace::default(),
            cancel_token: CancelToken::new(),
        }
    }

    /// 获取当前 trace
    pub fn trace(&self) -> &AgentTrace {
        &self.trace
    }

    /// 获取对话历史
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 获取当前状态
    pub fn state(&self) -> AgentState {
        self.state
    }

    /// 获取取消令牌（克隆后可用于异步取消）
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel_token.clone()
    }

    /// 是否有对话历史（跨调用记忆）
    pub fn has_history(&self) -> bool {
        self.messages.len() > 1 || !self.scratchpad.is_empty()
    }

    /// 请求取消当前运行
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    // ========================================================================
    // 主循环
    // ========================================================================

    /// 运行一轮 Agent 交互（ReAct 主循环）
    ///
    /// 接收用户消息，通过 LLM + 工具调用循环，返回最终文本回复。
    /// 整个过程的步骤记录在 trace 中，并通过 on_step 回调实时推送。
    ///
    /// - `on_step`：每个 AgentStep 产生时的回调（含工具调用、推理等）
    /// - `on_delta`：流式文本增量回调（仅最终文本回复时生效）
    pub fn chat(
        &mut self,
        user_input: &str,
        on_step: Option<StepCallback>,
        on_delta: Option<DeltaCallback>,
    ) -> Result<AgentTrace, AiError> {
        // 重置取消标志（上次 chat 可能残留取消状态，避免新一轮立即被判定为已取消）
        self.cancel_token.reset();

        // 重置本轮 trace 与状态
        self.trace = AgentTrace {
            steps: vec![AgentStep::UserMessage {
                content: user_input.to_string(),
            }],
            total_tokens: 0,
            tool_call_count: 0,
        };
        self.emit_step(
            AgentStep::UserMessage {
                content: user_input.to_string(),
            },
            &on_step,
        );
        self.state = AgentState::Idle;

        // 追加用户消息到历史
        self.messages.push(Message::user(user_input));

        // 计划阶段（可选）—— 交给 LLM 在首轮自然处理；
        // 若需要显式 plan-first，在此处插入规划逻辑（见 plan_first 辅助方法）。

        // ReAct 主循环
        let mut tools_disabled = false;
        // 重复检测：记录上一轮的工具调用签名（name + 参数），用于检测死循环。
        // ReAct 在无 frequency_penalty 时，若工具结果不变，模型会确定性地产出相同
        // ToolUse。连续重复 N 次时打印醒目警告，便于定位根因。
        let mut last_tool_signature: Option<String> = None;
        let mut repeat_count: usize = 0;
        // 追踪本次对话是否发生过工具调用（用于判断"空输出"是否为异常截断）
        let mut had_tool_calls = false;
        self.state = AgentState::Executing;

        for _turn in 0..self.config.max_tool_turns {
            // —— 取消检查 ——
            if self.cancel_token.is_cancelled() {
                self.state = AgentState::Cancelled;
                let msg = "用户取消了执行";
                self.trace.steps.push(AgentStep::Error {
                    message: msg.to_string(),
                });
                self.emit_step(
                    AgentStep::Error {
                        message: msg.to_string(),
                    },
                    &on_step,
                );
                return Ok(self.trace.clone());
            }

            // —— 压缩检查（工具循环中持续管理上下文）——
            self.compact_if_needed(&on_step)?;

            let checkpoint = self.messages.len();
            let response = match self.call_llm(!tools_disabled, on_delta.clone(), &on_step) {
                Ok(r) => r,
                Err(AiError::Cancelled) => {
                    // generate 被用户取消（来自 generate_with_cancel）→ 立即退出
                    self.state = AgentState::Cancelled;
                    let msg = "用户取消了执行";
                    self.trace.steps.push(AgentStep::Error {
                        message: msg.to_string(),
                    });
                    self.emit_step(
                        AgentStep::Error {
                            message: msg.to_string(),
                        },
                        &on_step,
                    );
                    return Ok(self.trace.clone());
                }
                Err(e) => {
                    if !tools_disabled {
                        // 区分错误类型决定是否回退到无工具模式：
                        //
                        // - "provider 不支持工具"（错误信息明确提及 tools/functions 不支持）
                        //   → 回退到无工具重试，让用户至少得到文本回复。
                        // - 其他 API 错误（消息格式 400、限流 429、服务端 5xx）
                        //   → **不回退**。禁用工具重试会让 Agent 失去工具能力、
                        //   静默停止（表现为"（Agent 已执行操作）"），且掩盖真正的
                        //   错误。应直接抛出，让用户看到明确的错误信息。
                        // - 网络错误（超时、连接失败）→ **不回退**。回退到无工具模式会让
                        //   本来计划用工具的模型严重退化（典型：无限重复"先获取上下文"
                        //   直到 max_tokens 耗尽）。
                        let supports_tools_fallback = matches!(
                            &e,
                            AiError::Api { status: 400..=499, message }
                                if message_indicates_no_tools(message)
                        );
                        if supports_tools_fallback {
                            tracing::warn!(
                                target: "agent::call_llm",
                                error = %e,
                                "LLM call rejected: provider likely does not support tools, retrying without tools"
                            );
                            tools_disabled = true;
                            self.messages.truncate(checkpoint);
                            self.call_llm(false, on_delta.clone(), &on_step)?
                        } else {
                            return Err(e);
                        }
                    } else {
                        return Err(e);
                    }
                }
            };
            self.trace.total_tokens += response.usage.total();

            // —— generate 返回后立即检查取消 ——
            // 即使在 generate 期间无法中断，返回后第一时间退出，避免进入工具执行造成更多延迟
            if self.cancel_token.is_cancelled() {
                self.state = AgentState::Cancelled;
                let msg = "用户取消了执行";
                self.trace.steps.push(AgentStep::Error {
                    message: msg.to_string(),
                });
                self.emit_step(
                    AgentStep::Error {
                        message: msg.to_string(),
                    },
                    &on_step,
                );
                return Ok(self.trace.clone());
            }

            // 以 tool_calls 是否非空作为"本轮要调用工具"的主判据。
            // stop_reason 在兼容 OpenAI 的网关/代理上经常丢失或非标准（会被默认成
            // EndTurn），若仅依赖 stop_reason == ToolUse，模型明明要调用工具
            // 却因 stop_reason 丢失而误判为"最终回复"，导致 Agent 提前终止、
            // 本轮工具调用未执行。tool_calls 非空是更强的信号。
            let has_tool_calls = !response.tool_calls.is_empty();
            tracing::info!(
                target: "agent::loop",
                turn = _turn,
                stop_reason = ?response.stop_reason,
                tool_call_count = response.tool_calls.len(),
                text_len = response.text.len(),
                has_tool_calls,
                "LLM 响应"
            );
            if has_tool_calls {
                self.state = AgentState::Executing;
                had_tool_calls = true;

                // 记录 assistant 的推理文本（若有）
                if !response.text.trim().is_empty() {
                    let thought = response.text.clone();
                    self.trace.steps.push(AgentStep::Reasoning {
                        thought: thought.clone(),
                    });
                    self.emit_step(AgentStep::Reasoning { thought }, &on_step);
                }

                // assistant 消息（含 tool_calls）先加入历史——
                // 必须在 tool 结果之前，否则 API 会拒绝或模型困惑
                self.messages.push(Message::assistant_with_tools(
                    response.text.clone(),
                    response.tool_calls.clone(),
                ));

                // 先构造本轮工具调用签名（仅 name + arguments），用于执行后的重复检测
                let current_call_signature = response
                    .tool_calls
                    .iter()
                    .map(|tc| format!("{}({})", tc.name, tc.arguments))
                    .collect::<Vec<_>>()
                    .join(";");

                // 收集本轮所有工具执行结果，用于和签名一起做重复检测
                let mut result_parts: Vec<String> = Vec::new();

                // 执行工具调用
                for tc in &response.tool_calls {
                    self.trace.tool_call_count += 1;
                    self.trace.steps.push(AgentStep::ToolCall {
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    });
                    self.emit_step(
                        AgentStep::ToolCall {
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                        &on_step,
                    );

                    let result = match self.tools.execute(&tc.name, &tc.arguments) {
                        Some(Ok(raw_output)) => {
                            // 全局结果守卫：超长结果截断并引导模型改用过滤参数，
                            // 防止一次大范围查询（全书结构/全部出场记录等）撑爆上下文
                            let output = clamp_tool_result(&raw_output);
                            tracing::debug!(
                                target: "agent::tool",
                                tool = %tc.name,
                                result_len = output.len(),
                                "tool executed successfully"
                            );
                            self.trace.steps.push(AgentStep::ToolResult {
                                tool_name: tc.name.clone(),
                                content: output.clone(),
                            });
                            self.emit_step(
                                AgentStep::ToolResult {
                                    tool_name: tc.name.clone(),
                                    content: output.clone(),
                                },
                                &on_step,
                            );
                            result_parts.push(format!(
                                "{}→{}",
                                tc.name,
                                truncate_chars(&output, 200)
                            ));
                            output
                        }
                        Some(Err(err)) => {
                            tracing::warn!(
                                target: "agent::tool",
                                tool = %tc.name,
                                error = %err,
                                "tool execution failed"
                            );
                            // 工具执行失败 —— 错误信息反馈给 LLM（而非丢弃）
                            let msg = format!("工具 {} 执行失败: {}", tc.name, err);
                            self.trace.steps.push(AgentStep::Error {
                                message: msg.clone(),
                            });
                            self.emit_step(
                                AgentStep::Error {
                                    message: msg.clone(),
                                },
                                &on_step,
                            );
                            result_parts.push(format!(
                                "{}→ERR:{}",
                                tc.name,
                                truncate_chars(&msg, 200)
                            ));
                            msg
                        }
                        None => {
                            tracing::warn!(
                                target: "agent::tool",
                                tool = %tc.name,
                                "unknown tool requested"
                            );
                            let err = format!("未知工具: {}", tc.name);
                            self.trace.steps.push(AgentStep::Error {
                                message: err.clone(),
                            });
                            self.emit_step(
                                AgentStep::Error {
                                    message: err.clone(),
                                },
                                &on_step,
                            );
                            err
                        }
                    };

                    // 工具结果加入对话历史（assistant 消息已先于循环 push）
                    self.messages.push(Message::tool(tc.id.clone(), result));
                }

                // —— 重复检测：签名 = 调用 + 结果 ——
                // 只有"相同调用 + 相同结果"才算死循环。
                // 删除类工具（如 delete_chapter_outline index=1）虽然调用签名相同，
                // 但每次结果不同（列表变短），不应判为死循环。
                let current_signature = format!(
                    "{} | results: {}",
                    current_call_signature,
                    result_parts.join(" | ")
                );
                let is_repeating = last_tool_signature.as_deref() == Some(&current_signature);
                if is_repeating {
                    repeat_count += 1;
                    tracing::warn!(
                        target: "agent::loop",
                        turn = _turn,
                        repeat_count,
                        signature = %current_signature.chars().take(200).collect::<String>(),
                        "⚠ 检测到重复工具调用（调用+结果均相同）—— 模型可能在死循环"
                    );
                    if repeat_count >= 4 {
                        tracing::error!(
                            target: "agent::loop",
                            repeat_count,
                            "重复工具调用已达 {} 次，主动中断循环",
                            repeat_count
                        );
                        self.state = AgentState::Completed;
                        let msg = format!(
                            "检测到工具调用重复 {} 次（可能陷入死循环），已自动停止。重复调用: {}",
                            repeat_count,
                            current_call_signature.chars().take(300).collect::<String>()
                        );
                        self.trace.steps.push(AgentStep::Error {
                            message: msg.clone(),
                        });
                        self.emit_step(
                            AgentStep::Error {
                                message: msg.clone(),
                            },
                            &on_step,
                        );
                        return Ok(self.trace.clone());
                    }
                } else {
                    repeat_count = 0;
                }
                last_tool_signature = Some(current_signature);

                // 工具循环继续 → 进入反思态，下一轮 LLM 会基于工具结果推理
                self.state = AgentState::Reflecting;
            } else {
                // 无工具调用 — 最终回复（EndTurn / MaxTokens / Stop，或 stop_reason 缺失）
                let text = response.text.clone();

                // —— 单轮退化检测 ——
                // 模型偶尔会在单次响应里退化成重复短句（典型表现：把计划陈述
                // "先获取上下文。"重复到 max_tokens 耗尽）。检测到这种退化时，
                // 不把垃圾文本当最终答案，而是报告错误，避免污染对话历史。
                if response.stop_reason == StopReason::MaxTokens
                    || (response.usage.output_tokens > 0
                        && response.usage.output_tokens >= self.config.max_tokens * 9 / 10)
                {
                    if let Some(repeated) = detect_repetition(&text) {
                        tracing::warn!(
                            target: "agent::loop",
                            turn = _turn,
                            output_tokens = response.usage.output_tokens,
                            max_tokens = self.config.max_tokens,
                            repeated_phrase = %repeated.chars().take(80).collect::<String>(),
                            "⚠ 检测到模型输出退化（重复短句），丢弃该响应"
                        );
                        self.state = AgentState::Completed;
                        let msg = format!(
                            "模型输出异常（检测到重复退化，疑似 token 上限截断）。\
                                 重复内容：{}…。可能是网络不稳定或上下文过长导致，请重试。",
                            repeated.chars().take(60).collect::<String>()
                        );
                        self.trace.steps.push(AgentStep::Error {
                            message: msg.clone(),
                        });
                        self.emit_step(
                            AgentStep::Error {
                                message: msg.clone(),
                            },
                            &on_step,
                        );
                        return Ok(self.trace.clone());
                    }
                }

                // —— 空输出截断检测 ——
                // 模型既无文本也无工具调用（典型场景：开启 extended thinking 时
                // budget 挤满 max_tokens，可见输出被截断为空；或网络短流）。
                // 若此前已发生工具调用，说明任务进行中被截断，绝非正常完成。
                // 此前静默 return 会造成"未生成总结"的无信息提示，这里改为明确报错。
                if text.is_empty() && had_tool_calls {
                    tracing::warn!(
                        target: "agent::loop",
                        turn = _turn,
                        stop_reason = ?response.stop_reason,
                        output_tokens = response.usage.output_tokens,
                        max_tokens = self.config.max_tokens,
                        "⚠ 模型输出为空（疑似 max_tokens 截断），任务未完成"
                    );
                    self.state = AgentState::Completed;
                    let reason = match response.stop_reason {
                        StopReason::MaxTokens => "输出达到 token 上限被截断".to_string(),
                        _ => "模型返回了空响应".to_string(),
                    };
                    let msg = format!(
                        "{reason}，任务可能尚未完成。\
                         可能是上下文过长或思考预算挤占了输出空间，\
                         请精简问题或重试。"
                    );
                    self.trace.steps.push(AgentStep::Error {
                        message: msg.clone(),
                    });
                    self.emit_step(
                        AgentStep::Error {
                            message: msg.clone(),
                        },
                        &on_step,
                    );
                    return Ok(self.trace.clone());
                }

                if !text.is_empty() {
                    self.trace.steps.push(AgentStep::AiText {
                        content: text.clone(),
                    });
                    self.emit_step(
                        AgentStep::AiText {
                            content: text.clone(),
                        },
                        &on_step,
                    );
                    self.messages.push(Message::assistant(text));
                }
                self.state = AgentState::Completed;
                return Ok(self.trace.clone());
            }
        }

        // 达到最大轮次 — 这不是错误，而是工具预算耗尽。
        // 给出明确提示，让用户知道可以接着对话让 Agent 继续。
        self.state = AgentState::Completed;
        let msg = format!(
            "已达到本回合工具调用上限（{} 次），任务可能尚未完成。\
             若需继续，请回复“继续”，我将接着处理剩余部分。",
            self.config.max_tool_turns
        );
        // 记入对话历史：这样用户回复"继续"时，模型知道上次是因上限而停，
        // 能基于已有的工具调用结果继续推进，而不是从头开始。
        self.messages.push(Message::assistant(msg.clone()));
        self.trace.steps.push(AgentStep::AiText {
            content: msg.clone(),
        });
        self.emit_step(
            AgentStep::AiText {
                content: msg.clone(),
            },
            &on_step,
        );
        Ok(self.trace.clone())
    }

    // ========================================================================
    // LLM 调用
    // ========================================================================

    /// 调用 LLM（带完整对话历史和工具列表）
    /// 有工具时走同步 generate，无工具时走流式 stream_generate
    fn call_llm(
        &self,
        with_tools: bool,
        on_delta: Option<DeltaCallback>,
        on_step: &Option<StepCallback>,
    ) -> Result<GenerateResponse, AiError> {
        let tools = if with_tools {
            self.tools.definitions()
        } else {
            Vec::new()
        };
        let has_tools = !tools.is_empty();

        let request = self.build_request(tools);

        // 诊断日志：输出每轮的消息序列
        tracing::debug!(
            target: "agent::call_llm",
            turn = self.trace.tool_call_count,
            has_tools,
            msg_count = self.messages.len(),
            msg_roles = ?self.messages.iter().map(|m| {
                let role = format!("{:?}", m.role);
                if !m.tool_calls.is_empty() {
                    format!("{}(+{}tc)", role, m.tool_calls.len())
                } else if m.tool_call_id.is_some() {
                    format!("{}(result)", role)
                } else {
                    role
                }
            }).collect::<Vec<_>>(),
            "call_llm"
        );

        // 统一走流式路径（有工具时也流式），避免非流式长输出（如写正文）时
        // 连接长时间无数据被中间网络设备掐断。stream_with_cancel 已支持 tool_call
        // 增量解析。仅当 on_delta=None（非交互测试场景）时回退到同步 generate。
        //
        // 网络重试：最多 MAX_LLM_RETRIES 次，指数退避；API 错误和取消不重试。
        const MAX_LLM_RETRIES: usize = 10;

        for attempt in 0..=MAX_LLM_RETRIES {
            let result = if let Some(ref delta_cb) = on_delta {
                self.stream_with_cancel(request.clone(), Arc::clone(delta_cb))
            } else {
                self.generate_with_cancel(request.clone())
            };
            match result {
                Ok(r) => return Ok(r),
                Err(ref e) if attempt < MAX_LLM_RETRIES && is_retryable(e) => {
                    let backoff = backoff_ms(attempt);
                    let reason = error_display(e);
                    tracing::warn!(
                        target: "agent::call_llm",
                        attempt = attempt + 1,
                        max_retries = MAX_LLM_RETRIES,
                        backoff_ms = backoff,
                        error = %reason,
                        "LLM 调用失败（可重试），等待后重试"
                    );
                    self.emit_step(
                        AgentStep::Retrying {
                            attempt: attempt + 1,
                            max_retries: MAX_LLM_RETRIES,
                            reason,
                        },
                        on_step,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(backoff));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        // 不可达：循环要么 return Ok，要么 return Err
        unreachable!("retry loop should have returned")
    }

    /// 流式 generate 的可取消包装。
    ///
    /// 与 `generate_with_cancel` 对称：把 `stream_generate` 放到独立线程，
    /// 主线程轮询 cancel_token。流式生成可能持续很久（长文本输出），
    /// 不包装则用户取消时无法中断，表现为"停不下来"。
    ///
    /// delta 回调跨线程传递：用 `Arc<dyn Fn>` 包装，线程内调用。
    fn stream_with_cancel(
        &self,
        request: GenerateRequest,
        on_delta: DeltaCallback,
    ) -> Result<GenerateResponse, AiError> {
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::time::Duration;

        let provider = Arc::clone(&self.provider);
        // delta 回调需要跨线程共享 —— DeltaCallback 本身是 Arc<dyn Fn>，可安全 move
        let on_delta_for_thread = on_delta;
        let (tx, rx) = mpsc::channel::<Result<GenerateResponse, AiError>>();

        std::thread::spawn(move || {
            let full_text = std::sync::Mutex::new(String::new());
            let stream_error = std::sync::Mutex::new(None::<AiError>);
            let stream_usage = std::sync::Mutex::new(TokenUsage::default());
            // 流式 tool_call 累积：index → (id, name, arguments_json_buffer, thought_signature)
            // Anthropic/OpenAI 的 tool_call 跨多个 SSE 事件到达（Start + 若干 Delta），
            // 需按 index 聚合后拼出完整 ToolCall。Gemini 的 thought_signature 可能
            // 随 Start 帧或后续 Delta 帧到达，统一累积到第 4 位。
            #[allow(clippy::type_complexity)]
            let tool_calls_acc: std::sync::Mutex<
                std::collections::BTreeMap<usize, (String, String, String, Option<String>)>,
            > = std::sync::Mutex::new(std::collections::BTreeMap::new());
            let final_stop_reason = std::sync::Mutex::new(None::<StopReason>);

            let result = provider.stream_generate(&request, &|event| match event {
                StreamEvent::Delta { delta } => {
                    on_delta_for_thread(delta.clone());
                    if let Ok(mut t) = full_text.lock() {
                        t.push_str(&delta);
                    }
                }
                StreamEvent::Done {
                    usage, stop_reason, ..
                } => {
                    if let Ok(mut u) = stream_usage.lock() {
                        *u = usage;
                    }
                    if let Some(sr) = stop_reason {
                        if let Ok(mut r) = final_stop_reason.lock() {
                            *r = Some(sr);
                        }
                    }
                }
                StreamEvent::ToolUseStart {
                    index,
                    id,
                    name,
                    signature,
                } => {
                    if let Ok(mut acc) = tool_calls_acc.lock() {
                        let entry = acc.entry(index).or_default();
                        entry.0 = id;
                        entry.1 = name;
                        if let Some(sig) = signature {
                            entry.3 = Some(sig);
                        }
                    }
                }
                StreamEvent::ToolUseDelta {
                    index,
                    partial_json,
                    signature,
                } => {
                    if let Ok(mut acc) = tool_calls_acc.lock() {
                        let entry = acc.entry(index).or_default();
                        entry.2.push_str(&partial_json);
                        if let Some(sig) = signature {
                            entry.3 = Some(sig);
                        }
                    }
                }
                StreamEvent::Error { message } => {
                    if let Ok(mut e) = stream_error.lock() {
                        *e = Some(AiError::Network(message));
                    }
                }
            });

            let response = result.and_then(|_| {
                if let Some(err) = stream_error.lock().unwrap().take() {
                    return Err(err);
                }
                let text = full_text.lock().unwrap().clone();
                let usage = stream_usage.lock().unwrap().clone();
                // 聚合 tool_calls（BTreeMap 按 index 排序）。
                // 过滤空 name 条目：签名-only 帧在 Start 之前到达时可能留下占位
                let tool_calls: Vec<ToolCall> = tool_calls_acc
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|(_, name, _, _)| !name.is_empty())
                    .map(|(id, name, args_json, signature)| ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args_json).unwrap_or(serde_json::json!({})),
                        thought_signature: signature.clone(),
                    })
                    .collect();
                let stop_reason = final_stop_reason
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or(StopReason::EndTurn);
                Ok(GenerateResponse {
                    text,
                    usage,
                    stop_reason,
                    tool_calls,
                })
            });
            let _ = tx.send(response);
        });

        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {
                    if self.cancel_token.is_cancelled() {
                        tracing::info!(
                            target: "agent::call_llm",
                            "stream_generate 已被用户取消，放弃等待（后台线程继续完成）"
                        );
                        return Err(AiError::Cancelled);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(AiError::Network(
                        "stream_generate 线程异常退出（channel 断开）".to_string(),
                    ));
                }
            }
        }
    }

    /// 同步 generate 的可取消包装。
    ///
    /// `provider.generate` 是阻塞 HTTP 调用（ureq），无法中途中断。
    /// 这里把 generate 放到独立线程，主线程轮询 cancel_token：
    /// - generate 正常完成 → 返回结果
    /// - generate 进行中收到取消 → 主线程立即返回 `AiError::Cancelled`，
    ///   后台线程继续跑完（结果丢弃），避免阻塞主循环
    ///
    /// 轮询间隔 100ms，cancel 响应延迟 ≤ 100ms（不含线程退出时间）。
    fn generate_with_cancel(&self, request: GenerateRequest) -> Result<GenerateResponse, AiError> {
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::time::Duration;

        let provider = Arc::clone(&self.provider);
        let (tx, rx) = mpsc::channel::<Result<GenerateResponse, AiError>>();

        std::thread::spawn(move || {
            let result = provider.generate(&request);
            // 发送结果（若主线程已因取消放弃接收，这里会失败，忽略）
            let _ = tx.send(result);
        });

        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {
                    if self.cancel_token.is_cancelled() {
                        tracing::info!(
                            target: "agent::call_llm",
                            "generate 已被用户取消，放弃等待（后台线程继续完成）"
                        );
                        return Err(AiError::Cancelled);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(AiError::Network(
                        "generate 线程异常退出（channel 断开）".to_string(),
                    ));
                }
            }
        }
    }

    /// 构建 GenerateRequest — 将 scratchpad 注入 system prompt
    fn build_request(&self, tools: Vec<ToolDef>) -> GenerateRequest {
        // 组装 system prompt：base system + scratchpad 记忆
        let system = if self.scratchpad.is_empty() {
            self.system_prompt.clone()
        } else {
            format!(
                "{}\n\n--- 记忆 ---\n{}",
                self.system_prompt,
                self.scratchpad.render()
            )
        };

        GenerateRequest {
            system,
            messages: self.messages.clone(),
            model: self.config.model.clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            tools,
        }
    }

    // ========================================================================
    // 上下文压缩
    // ========================================================================

    /// 当上下文超出预算时，压缩早期消息到 scratchpad
    fn compact_if_needed(&mut self, on_step: &Option<StepCallback>) -> Result<(), AiError> {
        let estimated = self.estimate_tokens();
        let threshold = (self.config.max_context_tokens as f64 * 0.7) as usize;

        if estimated <= threshold {
            return Ok(());
        }

        // 保留最近 keep_recent_turns 轮（每轮 ~2 条消息：assistant + user/tool）
        let keep_msgs = self.config.keep_recent_turns * 2;
        // system prompt 在 request 层注入，不在 messages 中 → 从 index 0 开始可压缩
        // 但保留第一条用户消息（任务原始意图不丢）
        let first_removable = 1; // 跳过第一条 user 消息（任务意图）
        let available_to_compact = self.messages.len().saturating_sub(first_removable);
        let compact_count = available_to_compact.saturating_sub(keep_msgs);

        if compact_count == 0 {
            return Ok(()); // 没有足够的历史可压缩
        }

        // 关键：drain 范围不能切断"工具调用 ↔ 工具结果"的配对。
        // Anthropic API 要求每条含 tool_use 的 assistant 消息后面紧跟包含所有对应
        // tool_result 的 user 消息；若 drain 把 assistant(tool_use) 切掉但留下
        // 孤立的 tool_result（或反之），下一轮请求会返回 400，被误判为"不支持工具"
        // 而禁用工具重试 → Agent 失忆停止。
        // 因此把 compact_count 收敛到最近的安全边界：
        // 从后往前扫，找到一个"既不落在 Tool 消息序列中间、也不把某个 assistant
        // (tool_use) 与其后续 tool_result 分开"的位置。
        let safe_end = find_safe_drain_boundary(&self.messages, first_removable + compact_count);
        let compact_count = safe_end.saturating_sub(first_removable);
        if compact_count == 0 {
            return Ok(()); // 找不到安全边界，本轮放弃压缩
        }

        // 先把要压缩的消息转成 owned 文本（避免借用冲突）
        let transcript: String = self
            .messages
            .iter()
            .skip(first_removable)
            .take(compact_count)
            .map(format_message_for_compaction)
            .collect::<Vec<_>>()
            .join("\n");

        // 构建压缩请求
        let compacted_summary = self.summarize_transcript(&transcript)?;

        // 更新 scratchpad
        self.scratchpad
            .completed_actions
            .push(compacted_summary.summary.clone());

        let tokens_saved = estimated - self.estimate_tokens_after_removal(compact_count);

        // 从 messages 中移除已压缩的消息
        self.messages
            .drain(first_removable..first_removable + compact_count);

        // 记录压缩事件
        let step = AgentStep::ContextCompacted {
            summary: compacted_summary.summary,
            tokens_saved,
        };
        self.trace.steps.push(step.clone());
        self.emit_step(step, on_step);

        Ok(())
    }

    /// 调用 LLM 将 transcript 文本压缩为摘要
    fn summarize_transcript(&mut self, transcript: &str) -> Result<CompactSummary, AiError> {
        let system = "你是上下文压缩器。将以下对话历史压缩为简洁的摘要，\
                      保留：已执行的操作、发现的关键事实、待完成的任务。\
                      用要点输出，不超过 200 字。"
            .to_string();

        let request = GenerateRequest {
            system,
            messages: vec![Message::user(format!("请压缩以下对话：\n\n{transcript}"))],
            model: self.config.model.clone(),
            temperature: 0.2,
            max_tokens: 512,
            tools: Vec::new(),
        };

        let response = self.provider.generate(&request)?;
        self.trace.total_tokens += response.usage.total();

        Ok(CompactSummary {
            summary: response.text.trim().to_string(),
        })
    }

    /// 估算当前 messages 的 token 数（粗略：字符数 / 3）
    fn estimate_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.content.chars().count() / 3 + 4)
            .sum()
    }

    /// 估算移除 N 条消息后的 token 数
    fn estimate_tokens_after_removal(&self, count: usize) -> usize {
        self.estimate_tokens().saturating_sub(count * 20)
    }

    // ========================================================================
    // 辅助
    // ========================================================================

    /// 推送步骤到回调
    fn emit_step(&self, step: AgentStep, on_step: &Option<StepCallback>) {
        if let Some(cb) = on_step {
            cb(step);
        }
    }
}

// ============================================================================
// CompactSummary（内部用）
// ============================================================================

struct CompactSummary {
    summary: String,
}

/// 找到一个安全的 drain 结束位置，避免破坏 tool_use ↔ tool_result 配对。
///
/// Anthropic API 要求：每条含 tool_use 的 assistant 消息之后，必须紧跟一条
/// user 消息包含所有对应的 tool_result。若 drain 把 assistant(tool_use) 切掉
/// 但留下孤立的 tool_result（或反之），下一轮请求会 400。
///
/// 从期望位置 `desired` 开始往前找，返回第一个满足以下条件的位置 `end`：
/// - `messages[end-1]` 不是 `Role::Tool`（不能切在 tool_result 序列中间）
/// - `messages[end]` 不是含 tool_calls 的 assistant 之后紧跟的 tool result 开头
///
/// 简化策略：end 必须落在"tool_result 序列已完整结束"的边界，即 end 处的消息
/// 不是 Tool 角色，且 end-1 处的消息也不是 Tool 角色（整个 tool_result 批次被
/// 完整包含或完整排除）。
/// 按字符边界安全截断字符串，避免在 UTF-8 多字节字符中间切断（会 panic）。
/// 用于工具结果摘要等需要限制长度的场景。
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s.char_indices().nth(max_chars).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

/// 单个工具结果的字符上限：超出即截断（保留前半），并附上引导文字
/// 让模型改用更小范围的调用方式（过滤参数 / 分批获取），而不是再次全量请求。
/// 长篇小说后期，结构树 / 章节脉络 / 出场记录这类"索引型"工具的结果随书
/// 线性膨胀，一次几十万字的结果会直接撑爆单次请求——这是所有工具共用的
/// 最后一道闸。
const MAX_TOOL_RESULT_CHARS: usize = 20_000;

/// 工具结果输出闸：超出上限即截断并附引导文字。
/// Agent 循环与 MCP 服务共用，保证两条调用路径的输出体积一致。
pub fn clamp_tool_result(raw: &str) -> String {
    let total = raw.chars().count();
    if total <= MAX_TOOL_RESULT_CHARS {
        return raw.to_string();
    }
    format!(
        "{}\n\n⚠️ 工具结果过长（共 {total} 字），已截断只保留前 {MAX_TOOL_RESULT_CHARS} 字。\
         这通常意味着查询范围太大：请改用工具的过滤 / 范围参数（如 volume_order、status、\
         scope、chapter_id、section 等）缩小到需要的部分，或分批获取；不要原样重复这次调用。",
        truncate_chars(raw, MAX_TOOL_RESULT_CHARS)
    )
}

fn find_safe_drain_boundary(messages: &[Message], desired: usize) -> usize {
    use crate::provider::Role;
    if messages.is_empty() || desired == 0 {
        return 0;
    }
    let upper = messages.len();
    if desired >= upper {
        // 整个范围：只有当末尾不在 Tool 序列中间时才安全
        return if messages[upper - 1].role == Role::Tool {
            // 末尾是 Tool，往前退到 Tool 序列开始之前（即上一个非 Tool 消息 +1）
            find_safe_drain_boundary(messages, upper.saturating_sub(1))
        } else {
            upper
        };
    }
    // 从 desired 往前退，直到 messages[end-1] 不是 Tool
    let mut end = desired;
    while end > 0 && messages[end - 1].role == Role::Tool {
        end -= 1;
    }
    // 进一步检查：end 处若是 Tool（说明 desired 落在 Tool 序列内部，继续退）
    while end > 0 && messages[end].role == Role::Tool {
        end -= 1;
    }
    end
}

/// 将单条消息格式化为压缩用的文本行
fn format_message_for_compaction(m: &Message) -> String {
    match m.role {
        crate::provider::Role::User => format!("[用户] {}", m.content),
        crate::provider::Role::Assistant => {
            if m.content.is_empty() && !m.tool_calls.is_empty() {
                format!(
                    "[助手] 调用工具: {}",
                    m.tool_calls
                        .iter()
                        .map(|tc| tc.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!("[助手] {}", m.content)
            }
        }
        crate::provider::Role::Tool => format!("[工具结果] {}", m.content),
        crate::provider::Role::System => format!("[系统] {}", m.content),
    }
}

/// 判断 API 错误信息是否表明 provider 不支持工具调用。
///
/// 仅当错误信息明确提及 tools/functions 不被支持时，才认为 provider 不支持工具，
/// 可以安全回退到无工具模式。其他 4xx 错误（消息格式、认证、限流）不应触发回退，
/// 因为禁用工具会让 Agent 失去执行能力而静默停止。
fn message_indicates_no_tools(message: &str) -> bool {
    let lower = message.to_lowercase();
    // 常见不支持工具的错误特征（覆盖 OpenAI / Anthropic / 各类兼容端点）
    const MARKERS: &[&str] = &[
        "does not support tools",
        "tool use is not supported",
        "tools are not supported",
        "function calling is not supported",
        "functions are not supported",
        "no support for tool",
        "no support for function",
        "unrecognized request argument: tools",
        "unknown argument: tools",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// 网络重试的指数退避间隔（毫秒）。
///
/// attempt 0 → 1000ms, 1 → 2000ms, 2 → 4000ms, ...
/// 上限 10 秒，避免重试间隔过长。
fn backoff_ms(attempt: usize) -> u64 {
    const BASE: u64 = 1000;
    const CAP: u64 = 10_000;
    std::cmp::min(BASE * 2u64.pow(attempt as u32), CAP)
}

/// 判断错误是否可重试（速率限制、临时网络问题、服务端错误）。
///
/// 以下情况视为可恢复，应等待后重试：
/// - `AiError::Network`：网络超时、连接中断等
/// - `AiError::Api` 状态码 429（速率限制）、500/502/503/504（服务端临时错误）
/// - `AiError::Api` 消息含 quota/rate_limit/exceeded 等关键词
fn is_retryable(e: &AiError) -> bool {
    match e {
        AiError::Network(_) => true,
        AiError::Api { status, message } => {
            // 429 = 速率限制；5xx = 服务端临时错误
            if *status == 429 || (500..=599).contains(status) {
                return true;
            }
            // 消息关键词匹配（某些 API 用非标准状态码返回速率限制）
            let lower = message.to_lowercase();
            lower.contains("quota")
                || lower.contains("rate limit")
                || lower.contains("rate_limit")
                || lower.contains("exceeded")
                || lower.contains("too many requests")
                || lower.contains("overloaded")
        }
        _ => false,
    }
}

/// 获取错误的简短描述（用于日志和 Retrying 事件）
fn error_display(e: &AiError) -> String {
    match e {
        AiError::Network(msg) => msg.clone(),
        AiError::Api { status, message } => format!("HTTP {status}: {message}"),
        AiError::Auth => "认证失败".to_string(),
        AiError::Unavailable(msg) => msg.clone(),
        AiError::Parse(msg) => format!("解析失败: {msg}"),
        AiError::Cancelled => "用户取消".to_string(),
    }
}

/// 检测文本是否为短句的高频重复（模型退化标志）。
///
/// 模型在 token 上限截断或上下文混乱时，可能输出如
/// "先获取上下文。先获取上下文。先获取上下文。…" 的退化文本。
///
/// 判定标准：文本中存在一个短句（≤ 30 字符），其连续重复占据文本至少 60%。
/// 命中则返回该重复短语，否则返回 `None`。
///
/// 这是保守检测：只标记明显的退化，正常文本（即使有排比/反复修辞）不会误判。
fn detect_repetition(text: &str) -> Option<String> {
    let trimmed = text.trim();
    // 太短的文本不检测
    if trimmed.chars().count() < 60 {
        return None;
    }

    // 尝试不同的候选短语长度（按标点切分后的短语，或固定字符窗口）。
    // 最常见的退化模式是"短句+句号"重复，所以先按中文句号/问号/感叹号切分。
    let sentences: Vec<&str> = trimmed
        .split(['。', '？', '！', '.', '?', '!'])
        .filter(|s| !s.trim().is_empty())
        .collect();

    if sentences.len() < 5 {
        // 句子太少，用固定窗口检测（无标点退化的兜底）
        return detect_repetition_window(trimmed);
    }

    // 统计每个句子出现的频率
    let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for s in &sentences {
        *freq.entry(*s).or_insert(0) += 1;
    }

    // 找最高频的句子
    let (most_common, count) = freq.iter().max_by_key(|(_, c)| **c)?;
    let ratio = *count as f64 / sentences.len() as f64;

    // 命中条件：高频句占比 ≥ 60%，且重复 ≥ 5 次
    if ratio >= 0.6 && *count >= 5 {
        return Some(most_common.trim().to_string());
    }

    detect_repetition_window(trimmed)
}

/// 固定字符窗口的退化检测（兜底：退化文本无标点时）。
///
/// 取文本前 N 个字符作为候选，检测它在文本里重复多少次。
fn detect_repetition_window(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 60 {
        return None;
    }

    // 尝试候选窗口长度 2-30
    for window_len in 2..=30 {
        if window_len * 4 > chars.len() {
            break;
        }
        let candidate: String = chars[..window_len].iter().collect();
        let candidate_str = candidate.as_str();
        // 统计非重叠出现次数
        let mut count = 0;
        let mut i = 0;
        while i + window_len <= chars.len() {
            let slice: String = chars[i..i + window_len].iter().collect();
            if slice == candidate_str {
                count += 1;
                i += window_len;
            } else {
                i += 1;
            }
        }
        // 重复 ≥ 5 次且覆盖文本 ≥ 60%
        let coverage = (count * window_len) as f64 / chars.len() as f64;
        if count >= 5 && coverage >= 0.6 {
            return Some(candidate_str.trim().to_string());
        }
    }
    None
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        AiProvider, GenerateRequest, GenerateResponse, Role, StopReason, TokenUsage, ToolCall,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn clamp_tool_result_truncates_and_guides() {
        // 不超限：原样返回
        assert_eq!(clamp_tool_result("短结果"), "短结果");

        // 超限：截断 + 引导改用过滤参数，长度受控
        let huge = "字".repeat(MAX_TOOL_RESULT_CHARS + 5000);
        let clamped = clamp_tool_result(&huge);
        assert!(clamped.contains("已截断"), "应包含截断说明");
        assert!(
            clamped.contains("缩小到需要的部分"),
            "应包含改用过滤参数的引导"
        );
        assert!(clamped.chars().count() < MAX_TOOL_RESULT_CHARS + 500);
        assert!(clamped.starts_with(&"字".repeat(100)), "保留原文前段");
    }

    /// Mock provider that returns "no tools needed" text
    #[derive(Debug)]
    struct MockAgentProvider {
        text: String,
    }

    impl AiProvider for MockAgentProvider {
        fn id(&self) -> &str {
            "mock-agent"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.text.clone(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    /// Scripted provider — 按顺序返回预设响应（模拟多轮工具调用）
    #[derive(Debug)]
    struct ScriptedProvider {
        responses: Vec<GenerateResponse>,
        call_count: AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<GenerateResponse>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl AiProvider for ScriptedProvider {
        fn id(&self) -> &str {
            "scripted"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or(GenerateResponse {
                    text: String::new(),
                    usage: TokenUsage::default(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: Vec::new(),
                }))
        }
    }

    /// A simple echo tool for testing
    struct EchoTool;

    impl Tool for EchoTool {
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "echo".to_string(),
                description: "Echo back the input".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "Message to echo" }
                    },
                    "required": ["message"]
                }),
            }
        }

        fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
            let msg = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("nothing");
            Ok(format!("Echo: {msg}"))
        }
    }

    /// 计数器工具：每次调用返回递增数字，模拟"相同参数但结果每次不同"的场景
    /// （如 delete_chapter_outline(index=1) 每次删不同的条目）
    struct CounterTool;
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    impl Tool for CounterTool {
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "delete_item".to_string(),
                description: "Delete item by index".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "index": { "type": "string", "description": "Item index" }
                    },
                    "required": ["index"]
                }),
            }
        }

        fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            Ok(format!("已删除条目 #{}", n))
        }
    }

    fn make_agent(provider: Arc<dyn AiProvider>, tools: Arc<ToolRegistry>) -> Agent {
        Agent::new("You are helpful", AgentConfig::default(), provider, tools)
    }

    #[test]
    fn agent_runs_without_tools() {
        let provider = Arc::new(MockAgentProvider {
            text: "你好！".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        let trace = agent.chat("Hello", None, None).unwrap();
        assert_eq!(trace.steps.len(), 2); // UserMessage + AiText
        assert_eq!(trace.tool_call_count, 0);
    }

    #[test]
    fn agent_trace_contains_user_message() {
        let provider = Arc::new(MockAgentProvider {
            text: "OK".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        let trace = agent.chat("test", None, None).unwrap();
        assert!(matches!(trace.steps[0], AgentStep::UserMessage { .. }));
    }

    #[test]
    fn agent_tool_call_loop() {
        // 第一轮：工具调用；第二轮：文本完成
        let responses = vec![
            GenerateResponse {
                text: "我来查一下".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: "tc_1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({"message": "hello"}),
                    thought_signature: None,
                }],
            },
            GenerateResponse {
                text: "完成了".to_string(),
                usage: TokenUsage {
                    input_tokens: 15,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            },
        ];
        let provider = Arc::new(ScriptedProvider::new(responses)) as Arc<dyn AiProvider>;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        let trace = agent.chat("echo hello", None, None).unwrap();

        // UserMessage + Reasoning + ToolCall + ToolResult + AiText
        assert!(trace.steps.len() >= 5);
        assert_eq!(trace.tool_call_count, 1);
        assert_eq!(agent.state(), AgentState::Completed);

        // 验证消息历史顺序正确：
        // [0] User, [1] Assistant(含 tool_calls), [2] Tool(结果), [3] Assistant(最终文本)
        let msgs = agent.messages();
        assert_eq!(msgs.len(), 4, "应有 4 条消息");
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert!(
            !msgs[1].tool_calls.is_empty(),
            "assistant 消息应包含 tool_calls"
        );
        assert_eq!(msgs[2].role, Role::Tool, "工具结果应在 assistant 之后");
        assert!(msgs[2].tool_call_id.is_some(), "tool 消息应有 tool_call_id");
        assert_eq!(msgs[3].role, Role::Assistant);
        assert!(msgs[3].tool_calls.is_empty(), "最终回复不应有 tool_calls");
    }

    /// 验证修复：工具调用后模型返回空文本 + MaxTokens 截断时，
    /// Agent 应报错（Error 步骤）而非静默完成。
    ///
    /// 这复现了用户反馈的"任务未完成就停止、未生成总结"问题：
    /// 开启 extended thinking 时 budget 挤占 max_tokens，可见输出被截断为空。
    #[test]
    fn agent_reports_error_on_empty_output_after_tool_calls() {
        let responses = vec![
            // 第一轮：正常工具调用
            GenerateResponse {
                text: "我来查一下".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: "tc_1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({"message": "hello"}),
                    thought_signature: None,
                }],
            },
            // 第二轮：空文本 + MaxTokens 截断（thinking 挤满配额的典型表现）
            GenerateResponse {
                text: String::new(),
                usage: TokenUsage {
                    input_tokens: 15,
                    output_tokens: 0,
                },
                stop_reason: StopReason::MaxTokens,
                tool_calls: vec![],
            },
        ];
        let provider = Arc::new(ScriptedProvider::new(responses)) as Arc<dyn AiProvider>;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        let trace = agent.chat("echo hello", None, None).unwrap();

        // 最后一个步骤应是 Error（而非静默完成的 AiText）
        let last = trace.steps.last().expect("应至少有一个步骤");
        assert!(
            matches!(last, AgentStep::Error { .. }),
            "空输出截断应报错，实际最后一步: {last:?}"
        );
        if let AgentStep::Error { message } = last {
            assert!(
                message.contains("截断") || message.contains("空响应"),
                "错误信息应说明截断原因: {message}"
            );
        }
        assert_eq!(agent.state(), AgentState::Completed);
    }

    #[test]
    fn agent_same_call_different_result_not_loop() {
        // 验证修复：AI 连续调用 delete_item(index=1) 删除条目，每次结果不同（模拟删除列表），
        // 不应被误判为死循环。之前用"签名=调用参数"会误判，现在改为"签名=调用+结果"。
        COUNTER.store(0, Ordering::SeqCst);

        // 构造 6 轮响应：前 5 轮都调 delete_item(index=1)，第 6 轮文本完成
        let mut responses = Vec::new();
        for i in 0..5 {
            responses.push(GenerateResponse {
                text: format!("正在删除第 {} 个", i + 1),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: format!("tc_{i}"),
                    name: "delete_item".to_string(),
                    arguments: serde_json::json!({"index": "1"}), // 相同参数
                    thought_signature: None,
                }],
            });
        }
        responses.push(GenerateResponse {
            text: "全部删除完成".to_string(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
        });

        let provider = Arc::new(ScriptedProvider::new(responses)) as Arc<dyn AiProvider>;
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CounterTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        let trace = agent.chat("删除全部", None, None).unwrap();

        // 不应被循环检测中断（因为每次结果不同）
        assert_eq!(
            agent.state(),
            AgentState::Completed,
            "应正常完成，不应被循环检测中断"
        );
        // 应执行了全部 5 次工具调用
        assert_eq!(
            trace.tool_call_count, 5,
            "应执行 5 次 delete_item，实际 {}",
            trace.tool_call_count
        );
        // 不应有循环中断错误
        let has_loop_error = trace
            .steps
            .iter()
            .any(|s| matches!(s, AgentStep::Error { message } if message.contains("死循环")));
        assert!(
            !has_loop_error,
            "不应触发死循环检测（相同调用但结果每次不同）"
        );
    }

    /// CaptureProvider — 记录每次调用收到的 messages，用于验证工具结果传到了下一轮
    #[derive(Debug)]
    struct CaptureProvider {
        /// 按顺序返回的响应
        responses: Vec<GenerateResponse>,
        call_count: AtomicUsize,
        /// 每次调用收到的 request（克隆）
        captured: std::sync::Mutex<Vec<GenerateRequest>>,
    }

    impl CaptureProvider {
        fn new(responses: Vec<GenerateResponse>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
                captured: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl AiProvider for CaptureProvider {
        fn id(&self) -> &str {
            "capture"
        }
        fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            self.captured.lock().unwrap().push(request.clone());
            let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or(GenerateResponse {
                    text: String::new(),
                    usage: TokenUsage::default(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: Vec::new(),
                }))
        }
    }

    #[test]
    fn agent_tool_result_reaches_next_turn() {
        // 验证：工具执行的结果（含魔法世界观大纲）确实传到了第二轮 LLM 调用
        let responses = vec![
            // 第一轮：请求工具
            GenerateResponse {
                text: "让我查看大纲".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: "tc_1".to_string(),
                    name: "outline_reader".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                }],
            },
            // 第二轮：基于工具结果回复
            GenerateResponse {
                text: "根据魔法大纲创建角色".to_string(),
                usage: TokenUsage {
                    input_tokens: 20,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            },
        ];

        let provider = Arc::new(CaptureProvider::new(responses)) as Arc<dyn AiProvider>;

        // 工具返回魔法大纲内容
        struct OutlineTool;
        impl Tool for OutlineTool {
            fn definition(&self) -> ToolDef {
                ToolDef {
                    name: "outline_reader".to_string(),
                    description: "read outline".to_string(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                }
            }
            fn execute(&self, _: &serde_json::Value) -> Result<String, String> {
                Ok("## 故事概要\n标题: 魔法纪元\n类型: 奇幻\n前提: 魔法世界的冒险".to_string())
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(OutlineTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        agent.chat("根据大纲创建角色", None, None).unwrap();

        // 直接检查 agent 的最终消息历史
        let msgs = agent.messages();
        // User + Assistant(tool_call) + Tool(result) + Assistant(final)
        assert_eq!(msgs.len(), 4);
        // 工具结果消息的内容应包含魔法大纲
        let tool_msg = msgs
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("应有一条 Tool 消息");
        assert!(
            tool_msg.content.contains("魔法纪元"),
            "工具结果应包含魔法大纲内容，实际: {}",
            tool_msg.content
        );
        assert!(tool_msg.content.contains("奇幻"), "工具结果应包含奇幻类型");
    }

    #[test]
    fn agent_tool_error_is_not_swallowed() {
        // 验证：工具执行失败时，错误信息反馈给 LLM（而非被 .ok() 丢弃变成"未知工具"）
        let responses = vec![
            GenerateResponse {
                text: "让我查看大纲".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: "tc_1".to_string(),
                    name: "failing_tool".to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                }],
            },
            GenerateResponse {
                text: "工具出错了，我换个方式".to_string(),
                usage: TokenUsage {
                    input_tokens: 20,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            },
        ];
        let provider = Arc::new(ScriptedProvider::new(responses)) as Arc<dyn AiProvider>;

        // 工具总是返回错误
        struct FailingTool;
        impl Tool for FailingTool {
            fn definition(&self) -> ToolDef {
                ToolDef {
                    name: "failing_tool".to_string(),
                    description: "always fails".to_string(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                }
            }
            fn execute(&self, _: &serde_json::Value) -> Result<String, String> {
                Err("大纲文件解析失败: 格式错误".to_string())
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FailingTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        agent.chat("查看大纲", None, None).unwrap();

        // 检查 Tool 消息的内容应包含原始错误，而不是"未知工具"
        let tool_msg = agent
            .messages()
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("应有一条 Tool 消息");
        assert!(
            tool_msg.content.contains("大纲文件解析失败"),
            "工具错误应反馈给 LLM，实际内容: {}",
            tool_msg.content
        );
        assert!(
            !tool_msg.content.contains("未知工具"),
            "不应被当作未知工具处理"
        );
    }

    #[test]
    fn agent_cancellation() {
        // chat 开始时会 reset cancel_token，确保上一轮的取消标志不影响新一轮。
        // 这里验证：先 cancel，再 chat，应正常运行（状态为 Completed 而非 Cancelled）。
        let provider = Arc::new(MockAgentProvider {
            text: "你好".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        // 在运行前取消（模拟上一轮残留的取消标志）
        agent.cancel();
        assert!(agent.cancel_token().is_cancelled());

        // 新一轮 chat 应重置取消标志并正常运行
        let trace = agent.chat("hello", None, None).unwrap();
        assert_eq!(agent.state(), AgentState::Completed);
        // 不应有取消错误
        assert!(trace
            .steps
            .iter()
            .all(|s| !matches!(s, AgentStep::Error { .. })));
    }

    /// 慢速 provider：generate 阻塞指定时长，模拟长时间 LLM 调用
    #[derive(Debug)]
    struct SlowGenProvider {
        delay: Duration,
    }

    impl AiProvider for SlowGenProvider {
        fn id(&self) -> &str {
            "slow-gen"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            thread::sleep(self.delay);
            Ok(GenerateResponse {
                text: "慢回复".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    /// 验证 generate_with_cancel 能在 generate 阻塞期间响应取消。
    ///
    /// 关键场景：provider.generate 阻塞 3 秒，200ms 后触发取消，
    /// Agent 应在 ~300ms 内退出（而非等满 3 秒）。
    #[test]
    fn agent_cancel_interrupts_long_generate() {
        let provider = Arc::new(SlowGenProvider {
            delay: Duration::from_secs(3),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);
        let cancel_token = agent.cancel_token();

        // 另一线程延迟 200ms 后取消（此时 generate 已在进行中）
        let token_clone = cancel_token.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            token_clone.cancel();
        });

        let start = Instant::now();
        let trace = agent.chat("hello", None, None).unwrap();
        let elapsed = start.elapsed();

        // 应被取消（而非正常完成）
        assert_eq!(
            agent.state(),
            AgentState::Cancelled,
            "Agent 应在 generate 阻塞期间被取消"
        );
        // cancel 响应延迟应远小于 3 秒（generate_with_cancel 轮询 100ms）
        assert!(
            elapsed < Duration::from_secs(1),
            "应在 1 秒内响应取消，实际 {:?}",
            elapsed
        );
        assert!(
            trace
                .steps
                .iter()
                .any(|s| matches!(s, AgentStep::Error { .. })),
            "trace 应含取消错误"
        );
    }

    /// 慢速流式 provider：stream_generate 阻塞指定时长，模拟长时间流式输出。
    ///
    /// 用于验证 stream_with_cancel 能在流式阻塞期间响应取消。
    #[derive(Debug)]
    struct SlowStreamProvider {
        delay: Duration,
    }

    impl AiProvider for SlowStreamProvider {
        fn id(&self) -> &str {
            "slow-stream"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            // 不应被调用（流式路径走 stream_generate）
            Err(AiError::Network("不应走同步 generate".to_string()))
        }
        fn stream_generate(
            &self,
            _request: &GenerateRequest,
            _on_event: &(dyn Fn(StreamEvent) + Send + Sync),
        ) -> Result<(), AiError> {
            thread::sleep(self.delay);
            Ok(())
        }
    }

    /// 验证 stream_with_cancel 能在 stream_generate 阻塞期间响应取消。
    ///
    /// 关键场景：provider.stream_generate 阻塞 3 秒，200ms 后触发取消，
    /// Agent 应在 ~300ms 内退出（而非等满 3 秒）。
    /// 这复现了"网络超时后流式路径停不下来"的 bug：修复前 stream_generate
    /// 无 cancel 包装，用户取消无法中断。
    #[test]
    fn agent_cancel_interrupts_long_stream() {
        let provider = Arc::new(SlowStreamProvider {
            delay: Duration::from_secs(3),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);
        let cancel_token = agent.cancel_token();

        // 传入 on_delta 触发流式路径（无工具 + on_delta=Some → stream_with_cancel）
        let on_delta: DeltaCallback = Arc::new(|_delta: String| {});
        let on_delta_handle = Arc::clone(&on_delta);

        let token_clone = cancel_token.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            token_clone.cancel();
        });

        let start = Instant::now();
        let trace = agent.chat("hello", None, Some(on_delta_handle)).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(
            agent.state(),
            AgentState::Cancelled,
            "Agent 应在 stream_generate 阻塞期间被取消"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "流式取消应在 1 秒内响应，实际 {:?}",
            elapsed
        );
        assert!(
            trace
                .steps
                .iter()
                .any(|s| matches!(s, AgentStep::Error { .. })),
            "trace 应含取消错误"
        );
    }

    #[test]
    fn agent_multi_turn_memory() {
        // 同一个 agent 连续 chat 两次，第二次应能看到第一次的历史
        let provider = Arc::new(MockAgentProvider {
            text: "回复".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        // 第一次
        agent.chat("第一句", None, None).unwrap();
        // system-prompt 不在 messages，只有 user + assistant = 2
        assert_eq!(agent.messages().len(), 2);

        // 第二次
        agent.chat("第二句", None, None).unwrap();
        // 第一次的 user+assistant + 第二次的 user+assistant
        assert_eq!(agent.messages().len(), 4);
        assert!(agent.has_history());
    }

    #[test]
    fn agent_step_callback_fires() {
        let provider = Arc::new(MockAgentProvider {
            text: "hi".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        let on_step: StepCallback = Arc::new(move |_step| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        });

        agent.chat("test", Some(on_step), None).unwrap();
        // UserMessage + AiText = 2 个步骤回调
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn scratchpad_render() {
        let mut sp = Scratchpad::default();
        sp.completed_actions.push("创建了角色 c_0001".to_string());
        sp.key_facts.push("林风是主角".to_string());
        let rendered = sp.render();
        assert!(rendered.contains("创建了角色"));
        assert!(rendered.contains("林风是主角"));
        assert!(!sp.is_empty());

        let empty = Scratchpad::default();
        assert!(empty.is_empty());
    }

    #[test]
    fn agent_state_transitions() {
        let provider = Arc::new(MockAgentProvider {
            text: "done".to_string(),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        assert_eq!(agent.state(), AgentState::Idle);
        agent.chat("hi", None, None).unwrap();
        assert_eq!(agent.state(), AgentState::Completed);
    }

    /// 验证重复工具调用检测：当模型连续多次返回完全相同的 tool_call 时，
    /// Agent 应在第 4 次重复时主动中断循环（而非跑满 max_tool_turns）。
    ///
    /// 这模拟了 ReAct 死循环场景：工具结果不变 → 模型确定性地产出相同 ToolUse。
    #[test]
    fn agent_breaks_on_repeated_identical_tool_calls() {
        // 每次都返回完全相同的 tool_call（模拟模型死循环）
        let looping_response = GenerateResponse {
            text: String::new(),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
            tool_calls: vec![ToolCall {
                id: "tc_loop".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"message": "same"}),
                thought_signature: None,
            }],
        };
        // 返回足够多次，确保不靠 max_tool_turns 兜底
        let responses: Vec<GenerateResponse> = (0..30).map(|_| looping_response.clone()).collect();
        let provider = Arc::new(ScriptedProvider::new(responses)) as Arc<dyn AiProvider>;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        let trace = agent.chat("loop test", None, None).unwrap();

        // 应因重复检测中断，而非正常完成 —— trace 里应包含"死循环"错误
        let has_loop_error = trace
            .steps
            .iter()
            .any(|s| matches!(s, AgentStep::Error { message } if message.contains("死循环")));
        assert!(
            has_loop_error,
            "重复工具调用应触发死循环中断，但 trace 中未找到相关错误。steps: {:?}",
            trace
                .steps
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
        );
    }

    // —— detect_repetition 单元测试 ——

    #[test]
    fn detect_repetition_catches_degenerate_output() {
        // 复现实际 bug：模型把"先获取上下文。"重复到 max_tokens
        let text = "先获取上下文。".repeat(200);
        assert_eq!(
            detect_repetition(&text),
            Some("先获取上下文".to_string()),
            "应检测到退化重复"
        );
    }

    #[test]
    fn detect_repetition_catches_no_punctuation() {
        // 无标点的退化（兜底窗口检测），足够长以触发检测（≥ 60 字符）
        let text = "继续".repeat(40); // 80 字符
        assert!(detect_repetition(&text).is_some(), "无标点重复也应检测到");
    }

    #[test]
    fn detect_repetition_ignores_normal_text() {
        // 正常文本（即使有排比）不应误判
        let normal = "林风走进客栈，环顾四周。掌柜的抬头看了他一眼，又低下头去。\
                      酒客们各自喧哗，没人注意这个风尘仆仆的年轻人。\
                      他在角落坐下，要了一壶茶。窗外雨声渐密。\
                      这是一座普通的小镇，至少表面上看是这样。";
        assert!(
            detect_repetition(normal).is_none(),
            "正常文本不应被误判为退化"
        );
    }

    #[test]
    fn detect_repetition_ignores_short_text() {
        assert!(detect_repetition("先获取上下文。先获取上下文。").is_none());
        assert!(detect_repetition("短文本").is_none());
    }

    #[test]
    fn detect_repetition_ignores_repeated_short_answer() {
        // 即使重复但总量少的（如模型短回复"好的。好的。"）不应误判
        assert!(detect_repetition("好的。好的。好的。").is_none());
    }

    #[test]
    fn detect_repetition_normal_long_text() {
        // 长篇正常文本（不同句子）不应误判
        let text = "第一句话是这样。第二句完全不同。第三句继续变化。\
                    第四句又不一样了。第五句也很独特。第六句继续写下去。\
                    第七句保持多样性。第八句不重复。第九句新的内容。第十句结束。";
        assert!(detect_repetition(text).is_none());
    }

    // —— 网络重试测试 ——

    /// 按失败次数后成功的 provider —— 前 N 次返回网络错误，之后成功
    #[derive(Debug)]
    struct FailThenSucceedProvider {
        fail_count: AtomicUsize,
        target_fails: usize,
    }

    impl AiProvider for FailThenSucceedProvider {
        fn id(&self) -> &str {
            "fail-then-succeed"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            let n = self.fail_count.fetch_add(1, Ordering::Relaxed);
            if n < self.target_fails {
                Err(AiError::Network(format!("模拟网络失败 #{}", n + 1)))
            } else {
                Ok(GenerateResponse {
                    text: "重试后成功".to_string(),
                    usage: TokenUsage {
                        input_tokens: 5,
                        output_tokens: 5,
                    },
                    stop_reason: StopReason::EndTurn,
                    tool_calls: Vec::new(),
                })
            }
        }
    }

    /// 网络错误后自动重试，前 2 次失败后第 3 次成功 → chat 应成功完成
    #[test]
    fn agent_retries_on_network_error_then_succeeds() {
        let provider = Arc::new(FailThenSucceedProvider {
            fail_count: AtomicUsize::new(0),
            target_fails: 2,
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        // 用 on_step 回调捕获事件（前端通过此通道收到 Retrying）
        let captured: Arc<std::sync::Mutex<Vec<AgentStep>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        let on_step: StepCallback = Arc::new(move |step| {
            captured_clone.lock().unwrap().push(step);
        });

        let trace = agent.chat("hello", Some(on_step), None).unwrap();
        assert_eq!(agent.state(), AgentState::Completed);
        // 最终应有 AI 文本回复
        assert!(
            trace.steps.iter().any(
                |s| matches!(s, AgentStep::AiText { content } if content.contains("重试后成功"))
            ),
            "重试成功后应有正常回复，trace: {:?}",
            trace
                .steps
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
        );

        // 回调捕获的事件里应有 Retrying（前 2 次失败 → 2 个 Retrying）
        let events = captured.lock().unwrap();
        let retry_events: Vec<&AgentStep> = events
            .iter()
            .filter(|s| matches!(s, AgentStep::Retrying { .. }))
            .collect();
        assert_eq!(
            retry_events.len(),
            2,
            "应收到 2 个 Retrying 事件，实际收到 {}: {:?}",
            retry_events.len(),
            events.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>()
        );
        // 验证字段
        if let Some(AgentStep::Retrying {
            attempt,
            max_retries,
            ..
        }) = retry_events.first()
        {
            assert!(
                *attempt >= 1 && *attempt <= *max_retries,
                "attempt 应在 1..=max"
            );
            assert_eq!(*max_retries, 10, "max_retries 应为 MAX_LLM_RETRIES (10)");
        }
    }

    /// 永远网络失败的 provider → 超过重试次数后应返回错误
    #[test]
    fn agent_gives_up_after_max_retries() {
        let provider = Arc::new(FailThenSucceedProvider {
            fail_count: AtomicUsize::new(0),
            target_fails: 100, // 永远失败
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        let result = agent.chat("hello", None, None);
        assert!(result.is_err(), "超过重试次数应返回错误");
        let err = result.unwrap_err();
        assert!(
            matches!(err, AiError::Network(ref m) if m.contains("模拟网络失败")),
            "应是网络错误，实际: {err:?}"
        );
    }

    /// API 错误（非网络错误）不应重试 —— 直接返回
    #[test]
    fn agent_does_not_retry_api_error() {
        use std::sync::atomic::AtomicUsize;

        #[derive(Debug)]
        struct ApiErrorProvider {
            call_count: AtomicUsize,
        }
        impl AiProvider for ApiErrorProvider {
            fn id(&self) -> &str {
                "api-error"
            }
            fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
                self.call_count.fetch_add(1, Ordering::Relaxed);
                Err(AiError::Api {
                    status: 400,
                    message: "bad request".to_string(),
                })
            }
        }

        let provider = Arc::new(ApiErrorProvider {
            call_count: AtomicUsize::new(0),
        }) as Arc<dyn AiProvider>;
        let tools = Arc::new(ToolRegistry::new());
        let mut agent = make_agent(provider, tools);

        let result = agent.chat("hello", None, None);
        // API 错误会被 agent 当作"工具不支持"回退到无工具重试，
        // 无工具时再失败则返回错误。无论哪种，都不应无限重试。
        assert!(result.is_err() || agent.state() == AgentState::Completed);
    }

    #[test]
    fn backoff_ms_exponential() {
        assert_eq!(backoff_ms(0), 1000);
        assert_eq!(backoff_ms(1), 2000);
        assert_eq!(backoff_ms(2), 4000);
        assert_eq!(backoff_ms(3), 8000);
        // 上限 10 秒
        assert_eq!(backoff_ms(4), 10_000);
        assert_eq!(backoff_ms(10), 10_000);
    }

    // —— 流式 tool_call 端到端测试 ——

    /// 脚本化流式 provider：按预设脚本依次发送 StreamEvent。
    ///
    /// 模拟 Anthropic SSE 的事件序列：text delta → tool_use start →
    /// input_json_delta(分片) → done(stop_reason=tool_use)。
    #[derive(Debug)]
    struct ScriptedStreamProvider {
        /// 每次调用 stream_generate 时发送的脚本（多轮对话时轮换）
        scripts: Vec<Vec<StreamEvent>>,
        call_count: AtomicUsize,
    }

    impl AiProvider for ScriptedStreamProvider {
        fn id(&self) -> &str {
            "scripted-stream"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Err(AiError::Network("测试应走 stream_generate".to_string()))
        }
        fn stream_generate(
            &self,
            _request: &GenerateRequest,
            on_event: &(dyn Fn(StreamEvent) + Send + Sync),
        ) -> Result<(), AiError> {
            let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
            let script = self
                .scripts
                .get(idx)
                .expect("脚本用尽：ScriptedStreamProvider 的调用次数超过预设脚本数");
            for event in script {
                on_event(event.clone());
            }
            Ok(())
        }
    }

    /// 端到端验证：流式路径能正确解析 tool_call 并驱动工具循环。
    ///
    /// 第一轮：流式返回 tool_use（write_fragment_text + 分片参数）→ Agent 执行工具
    /// 第二轮：流式返回纯文本 → Agent 完成
    #[test]
    fn agent_streaming_tool_call_loop() {
        let scripts = vec![
            // 第一轮：先输出 reasoning 文本，再调工具
            vec![
                StreamEvent::Delta {
                    delta: "让我写正文。".to_string(),
                },
                StreamEvent::ToolUseStart {
                    index: 0,
                    id: "toolu_001".to_string(),
                    name: "echo".to_string(),
                    signature: None,
                },
                StreamEvent::ToolUseDelta {
                    index: 0,
                    partial_json: r#"{"message":"#.to_string(),
                    signature: None,
                },
                StreamEvent::ToolUseDelta {
                    index: 0,
                    partial_json: r#""流式工具调用"}"#.to_string(),
                    // Gemini 兼容端点：签名可延后到参数分片帧才到达
                    signature: Some("CrICAdHtim".to_string()),
                },
                StreamEvent::Done {
                    full_text: String::new(),
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 20,
                    },
                    stop_reason: Some(StopReason::ToolUse),
                },
            ],
            // 第二轮：基于工具结果回复，纯文本
            vec![
                StreamEvent::Delta {
                    delta: "正文已完成。".to_string(),
                },
                StreamEvent::Done {
                    full_text: String::new(),
                    usage: TokenUsage {
                        input_tokens: 15,
                        output_tokens: 10,
                    },
                    stop_reason: Some(StopReason::EndTurn),
                },
            ],
        ];

        let provider = Arc::new(ScriptedStreamProvider {
            scripts,
            call_count: AtomicUsize::new(0),
        }) as Arc<dyn AiProvider>;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let tools = Arc::new(registry);

        let mut agent = make_agent(provider, tools);
        // 传入 on_delta 触发流式路径
        let on_delta: DeltaCallback = Arc::new(|_d: String| {});
        let trace = agent
            .chat("echo 流式工具调用", None, Some(on_delta))
            .unwrap();

        // 应完成工具循环：UserMessage + Reasoning + ToolCall + ToolResult + AiText
        assert_eq!(agent.state(), AgentState::Completed);
        assert_eq!(trace.tool_call_count, 1, "应执行 1 次工具调用");

        // 验证消息历史顺序（流式路径也应有正确的 tool_call 结构）
        let msgs = agent.messages();
        // [0] User, [1] Assistant(reasoning + tool_calls), [2] Tool(result), [3] Assistant(final)
        assert_eq!(msgs.len(), 4, "应有 4 条消息");
        assert_eq!(msgs[1].role, Role::Assistant);
        assert!(
            !msgs[1].tool_calls.is_empty(),
            "assistant 消息应包含 tool_calls（流式解析得到）"
        );
        // tool_call 的参数应正确拼接
        let tc = &msgs[1].tool_calls[0];
        assert_eq!(tc.name, "echo");
        assert_eq!(tc.id, "toolu_001");
        assert_eq!(
            tc.arguments,
            serde_json::json!({"message": "流式工具调用"}),
            "分片 JSON 应正确拼接"
        );
        assert_eq!(
            tc.thought_signature.as_deref(),
            Some("CrICAdHtim"),
            "签名应从 Delta 帧累积进 ToolCall，供下轮请求回放"
        );
        // 工具结果应包含 echo 的回复
        assert_eq!(msgs[2].role, Role::Tool);
        assert!(msgs[2].content.contains("流式工具调用"));
        // 最终回复
        assert_eq!(msgs[3].role, Role::Assistant);
        assert!(msgs[3].content.contains("正文已完成"));
    }
}
