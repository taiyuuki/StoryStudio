//! AI 编排 — 续写 Fragment / 抽取知识断言

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use story_ai_runtime::{
    AiError, AiProvider, AnthropicProvider, ApiMode, AssertionDraft, CancelToken, ContextBuilder,
    ExtractAssertionsTask, FragmentDraft, GenerateRequest, Message, OpenAiProvider, StreamEvent,
    Task, ThinkingConfig, WriteFragmentTask, stream_generate_with_cancel,
};
use story_ir::{DocNode, SemanticNode, StableId};
use story_parser::parse_project;
use story_project_format::ProjectPaths;

use crate::error::StudioError;

// ============================================================================
// 结果类型
// ============================================================================

/// AI 续写结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    /// AI 生成的 Fragment 草稿
    pub drafts: Vec<FragmentDraft>,
    /// token 消耗
    pub tokens_used: usize,
}

/// AI 抽取结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractResult {
    /// 抽取到的断言草稿
    pub drafts: Vec<AssertionDraft>,
    /// token 消耗
    pub tokens_used: usize,
}

/// 行内补全（幽灵补全）结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteResult {
    /// 补全文本（从光标处续写）
    pub text: String,
    /// token 消耗
    pub tokens_used: usize,
}

/// 按用途分配档案的场景常量（桌面端 / CLI 共用）
pub mod purpose {
    /// 场景续写（write_fragment_streaming）
    pub const WRITE: &str = "write";
    /// 妙笔 Agent 对话（agent_chat_streaming）
    pub const AGENT: &str = "agent";
    /// 逐章成书流水线（run_workflow）
    pub const WORKFLOW: &str = "workflow";
    /// 断言抽取（extract_assertions）
    pub const EXTRACT: &str = "extract";
    /// AI 味检测（ai_check）
    pub const CHECK: &str = "check";
    /// AI 建书（create_ai_book：灵感 → 书名/设定/大纲）
    pub const DESIGN: &str = "design";
    /// 行内补全（complete_fragment_streaming，编辑器幽灵补全）
    pub const COMPLETE: &str = "complete";

    /// 全部合法用途（校验 / UI 列表用）
    pub const ALL: &[&str] = &[WRITE, AGENT, WORKFLOW, EXTRACT, CHECK, DESIGN, COMPLETE];
}

// ============================================================================
// API
// ============================================================================

/// AI 续写 Fragment
///
/// 编排流程：
/// 1. 解析项目
/// 2. 查找目标 Scene
/// 3. 构建上下文（最近 Fragment + POV）
/// 4. 调用 AI Provider
/// 5. 解析输出为 FragmentDraft
pub fn write_fragment(
    dir: &Path,
    scene_id_str: &str,
    provider: &dyn AiProvider,
    instruction: Option<&str>,
) -> Result<WriteResult, StudioError> {
    // 1. 解析项目
    let parse_output = parse_project(dir).map_err(StudioError::from)?;

    // 2. 查找 Scene
    let scene_id = StableId::new_unchecked(scene_id_str);
    if parse_output.ir.document.get_scene(&scene_id).is_none() {
        return Err(StudioError::NotFound(format!(
            "Scene '{scene_id_str}' 不存在"
        )));
    }

    // 3. 构建上下文
    let builder = ContextBuilder::new(&parse_output.ir);
    let (recent, pov, title) = builder.for_write_fragment(&scene_id);

    let user_prompt = match instruction {
        Some(i) => format!("{}\n\n指令：{}", recent, i),
        None => format!("{}\n\n请续写下一个片段。", recent),
    };

    let task = WriteFragmentTask {
        scene_id,
        recent_text: user_prompt,
        pov_name: pov,
        scene_title: title,
    };

    // 4. 调用 AI
    let ctx = task.build_context(&parse_output.ir);
    let request = GenerateRequest {
        system: ctx.system,
        messages: vec![Message::user(ctx.user)],
        model: String::new(),
        temperature: ctx.model.temperature,
        max_tokens: ctx.model.max_tokens,
        tools: Vec::new(),
    };

    let started = std::time::Instant::now();
    let response = match provider.generate(&request) {
        Ok(resp) => {
            crate::ai_ledger::record(
                dir,
                "续写",
                provider.id(), provider.model(),
                resp.usage.total() as u64,
                started.elapsed().as_millis() as u64,
                true,
                None,
            );
            resp
        }
        Err(e) => {
            crate::ai_ledger::record(
                dir,
                "续写",
                provider.id(), provider.model(),
                0,
                started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            );
            return Err(StudioError::Ai(e.to_string()));
        }
    };

    // 5. 解析输出
    let drafts = task
        .parse_output(&response.text)
        .map_err(|e| StudioError::Ai(e.to_string()))?;

    Ok(WriteResult {
        drafts,
        tokens_used: response.usage.total(),
    })
}

/// AI 抽取知识断言
///
/// 编排流程：
/// 1. 解析项目
/// 2. 查找目标 Fragment
/// 3. 收集已知角色 + 构建 ExtractAssertionsTask
/// 4. 调用 AI Provider
/// 5. 解析 YAML 输出为 AssertionDraft
pub fn extract_assertions(
    dir: &Path,
    fragment_id_str: &str,
    provider: &dyn AiProvider,
    predicates: &[String],
) -> Result<ExtractResult, StudioError> {
    // 1. 解析项目
    let parse_output = parse_project(dir).map_err(StudioError::from)?;

    // 2. 查找 Fragment
    let frag_id = StableId::new_unchecked(fragment_id_str);
    let fragment = parse_output
        .ir
        .document
        .get_fragment(&frag_id)
        .ok_or_else(|| StudioError::NotFound(format!("Fragment '{fragment_id_str}' 不存在")))?;

    // 3. 收集已知角色
    let known_characters: Vec<String> = parse_output
        .ir
        .semantic
        .all_entities()
        .filter(|e| matches!(e, SemanticNode::Character(_)))
        .map(|e| e.display_name().to_string())
        .collect();

    let task = ExtractAssertionsTask {
        fragment_text: fragment.text.clone(),
        known_characters,
        predicates: predicates.to_vec(),
    };

    // 4. 调用 AI
    let ctx = task.build_context(&parse_output.ir);
    let request = GenerateRequest {
        system: ctx.system,
        messages: vec![Message::user(ctx.user)],
        model: String::new(),
        temperature: ctx.model.temperature,
        max_tokens: ctx.model.max_tokens,
        tools: Vec::new(),
    };

    let started = std::time::Instant::now();
    let response = match provider.generate(&request) {
        Ok(resp) => {
            crate::ai_ledger::record(
                dir,
                "断言抽取",
                provider.id(), provider.model(),
                resp.usage.total() as u64,
                started.elapsed().as_millis() as u64,
                true,
                None,
            );
            resp
        }
        Err(e) => {
            crate::ai_ledger::record(
                dir,
                "断言抽取",
                provider.id(), provider.model(),
                0,
                started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            );
            return Err(StudioError::Ai(e.to_string()));
        }
    };

    // 5. 解析断言
    let drafts = task
        .parse_assertions(&response.text)
        .map_err(|e| StudioError::Ai(e.to_string()))?;

    Ok(ExtractResult {
        drafts,
        tokens_used: response.usage.total(),
    })
}

/// AI 续写 Fragment（流式）
///
/// 与 `write_fragment` 相同，但通过 `on_chunk` 回调实时推送生成增量。
/// `on_chunk` 接收 `StreamEvent::Delta` 和 `StreamEvent::Done`。
/// 返回最终的 WriteResult（含解析后的 FragmentDraft）。
pub fn write_fragment_streaming(
    dir: &Path,
    scene_id_str: &str,
    provider: &dyn AiProvider,
    instruction: Option<&str>,
    on_chunk: &(dyn Fn(StreamEvent) + Send + Sync),
) -> Result<WriteResult, StudioError> {
    // 1. 解析项目
    let parse_output = parse_project(dir).map_err(StudioError::from)?;

    // 2. 查找 Scene
    let scene_id = StableId::new_unchecked(scene_id_str);
    if parse_output.ir.document.get_scene(&scene_id).is_none() {
        return Err(StudioError::NotFound(format!(
            "Scene '{scene_id_str}' 不存在"
        )));
    }

    // 3. 构建上下文
    let builder = ContextBuilder::new(&parse_output.ir);
    let (recent, pov, title) = builder.for_write_fragment(&scene_id);

    let user_prompt = match instruction {
        Some(i) => format!("{}\n\n指令：{}", recent, i),
        None => format!("{}\n\n请续写下一个片段。", recent),
    };

    let task = WriteFragmentTask {
        scene_id,
        recent_text: user_prompt,
        pov_name: pov,
        scene_title: title,
    };

    // 4. 构建请求
    let ctx = task.build_context(&parse_output.ir);
    let request = GenerateRequest {
        system: ctx.system,
        messages: vec![Message::user(ctx.user)],
        model: String::new(),
        temperature: ctx.model.temperature,
        max_tokens: ctx.model.max_tokens,
        tools: Vec::new(),
    };

    // 5. 流式调用 AI — 用 Mutex 收集完整文本
    let full_text = std::sync::Mutex::new(String::new());
    let stream_error = std::sync::Mutex::new(None::<String>);
    let stream_usage = std::sync::Mutex::new(None::<story_ai_runtime::TokenUsage>);

    let started = std::time::Instant::now();
    let result = provider.stream_generate(&request, &|event| match event {
        StreamEvent::Delta { ref delta } => {
            if let Ok(mut t) = full_text.lock() {
                t.push_str(delta);
            }
            on_chunk(StreamEvent::Delta {
                delta: delta.clone(),
            });
        }
        StreamEvent::Done { usage, .. } => {
            if let Ok(mut u) = stream_usage.lock() {
                *u = Some(usage);
            }
        }
        StreamEvent::Error { ref message } => {
            if let Ok(mut e) = stream_error.lock() {
                *e = Some(message.clone());
            }
            on_chunk(StreamEvent::Error {
                message: message.clone(),
            });
        }
        // write_fragment_streaming 不使用工具，这些事件不应出现，忽略
        StreamEvent::ToolUseStart { .. } | StreamEvent::ToolUseDelta { .. } => {}
    });

    let tokens_used_ledger = stream_usage
        .lock()
        .unwrap()
        .as_ref()
        .map(|u| u.total())
        .unwrap_or(0);
    match result {
        Ok(()) => crate::ai_ledger::record(
            dir,
            "续写",
            provider.id(), provider.model(),
            tokens_used_ledger as u64,
            started.elapsed().as_millis() as u64,
            true,
            None,
        ),
        Err(ref e) => crate::ai_ledger::record(
            dir,
            "续写",
            provider.id(), provider.model(),
            tokens_used_ledger as u64,
            started.elapsed().as_millis() as u64,
            false,
            Some(&e.to_string()),
        ),
    }

    if let Err(e) = result {
        return Err(StudioError::Ai(e.to_string()));
    }

    if let Ok(err) = stream_error.lock() {
        if let Some(msg) = err.as_ref() {
            return Err(StudioError::Ai(msg.clone()));
        }
    }

    let text = full_text.lock().unwrap().clone();

    // 6. 解析输出
    let drafts = task
        .parse_output(&text)
        .map_err(|e| StudioError::Ai(e.to_string()))?;

    let tokens_used = stream_usage
        .lock()
        .unwrap()
        .as_ref()
        .map(|u| u.total())
        .unwrap_or(0);

    // 7. 发送 Done 事件给调用者
    on_chunk(StreamEvent::Done {
        full_text: text,
        usage: story_ai_runtime::TokenUsage {
            input_tokens: 0,
            output_tokens: tokens_used,
        },
        stop_reason: None,
    });

    Ok(WriteResult {
        drafts,
        tokens_used,
    })
}

// ============================================================================
// 行内补全（幽灵补全）
// ============================================================================

/// 取文本末尾至多 `max_chars` 个字符（按字符边界截断，供上下文截短用）
fn tail_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

/// 组装行内补全的 system / user 提示词（纯函数，便于测试）
fn build_complete_prompt(
    pov: Option<&str>,
    scene_title: Option<&str>,
    prev_fragments: &str,
    before: &str,
    after: Option<&str>,
) -> (String, String) {
    let mut system = String::from(crate::prompts::COMPLETE_FRAGMENT);
    if let Some(pov) = pov {
        system.push_str(&format!("\n本场景视角：{pov}。"));
    }
    if let Some(title) = scene_title {
        system.push_str(&format!("\n场景：{title}。"));
    }

    let mut user = String::new();
    if !prev_fragments.is_empty() {
        user.push_str(&format!("【本场景前文】\n{prev_fragments}\n\n"));
    }
    user.push_str(&format!("【光标前正文】\n{before}\n\n"));
    if let Some(after) = after.filter(|s| !s.trim().is_empty()) {
        user.push_str(&format!(
            "【光标后正文】（你补的文字要与它衔接）\n{}\n\n",
            tail_chars(after, 300)
        ));
    }
    user.push_str("【任务】\n只输出从光标处续写下去的文字。");

    (system, user)
}

/// AI 行内补全（编辑器幽灵补全，流式）
///
/// 与 [`write_fragment_streaming`] 的区别：不落盘、不拆段 —— 只把「从光标
/// 处续写的下文」流式推给前端渲染成幽灵文字，作者 Tab 采纳后才进入正常
/// 编辑 / 保存链路。因此：
///
/// - `before` / `after` 用编辑器实况文本（当前片段可能还没保存，磁盘上的
///   片段文本不作数）；本片段之前的片段仍从项目解析取（久未变动，够稳）。
/// - `provider` 与 `on_delta` 以 Arc 传入：内部走
///   [`stream_generate_with_cancel`] 的 'static 生成线程。
/// - `cancel_token` 触发时立即返回（取消是正常操作，不记失败台账）。
#[allow(clippy::too_many_arguments)]
pub fn complete_fragment_streaming(
    dir: &Path,
    fragment_id: &str,
    before: &str,
    after: Option<&str>,
    provider: std::sync::Arc<dyn AiProvider>,
    cancel_token: &CancelToken,
    on_delta: &std::sync::Arc<dyn Fn(&str) + Send + Sync>,
    max_tokens: usize,
) -> Result<CompleteResult, StudioError> {
    // 1. 解析项目，定位片段所属场景
    let parse_output = parse_project(dir).map_err(StudioError::from)?;
    let fid = StableId::new_unchecked(fragment_id);
    let scene_id = match parse_output.ir.document.get(&fid) {
        Some(DocNode::Fragment(f)) => f.scene.clone(),
        _ => {
            return Err(StudioError::NotFound(format!(
                "Fragment '{fragment_id}' 不存在"
            )))
        }
    };
    let scene_id = scene_id.ok_or_else(|| {
        StudioError::NotFound(format!("Fragment '{fragment_id}' 不属于任何场景"))
    })?;

    // 2. 构建上下文：本片段之前的最近两段（各取末尾 600 字）+ POV / 场景标题
    let builder = ContextBuilder::new(&parse_output.ir);
    let scene = parse_output.ir.document.get_scene(&scene_id);
    let pov = scene
        .and_then(|s| s.pov.clone())
        .and_then(|id| builder.character_brief(&id));
    let scene_title = scene.and_then(|s| s.title.clone());

    let prev_fragments: Vec<String> = parse_output
        .ir
        .document
        .children_of(&scene_id)
        .iter()
        .take_while(|id| **id != fid)
        .filter_map(|id| match parse_output.ir.document.get(id) {
            Some(DocNode::Fragment(f)) => Some(f.text.clone()),
            _ => None,
        })
        .collect();
    let prev = prev_texts_tail(&prev_fragments, 2, 600);

    // 3. 请求（短输出；思考型模型的 reasoning 计入 max_tokens，见 complete_with_fallback）
    let (system, user) = build_complete_prompt(
        pov.as_deref(),
        scene_title.as_deref(),
        &prev,
        before,
        after,
    );
    let request = GenerateRequest {
        system,
        messages: vec![Message::user(user)],
        model: String::new(),
        temperature: 0.6,
        max_tokens,
        tools: Vec::new(),
    };

    // 4. 可取消的流式生成，增量原样转发
    let provider_id = provider.id().to_string();
    let provider_model = provider.model().to_string();
    let on_delta = std::sync::Arc::clone(on_delta);
    let sink = std::sync::Arc::new(move |event: StreamEvent| {
        if let StreamEvent::Delta { delta } = event {
            on_delta(&delta);
        }
    });

    let started = std::time::Instant::now();
    let response = match stream_generate_with_cancel(provider, &request, cancel_token, sink) {
        Ok(resp) => resp,
        Err(AiError::Cancelled) => return Err(StudioError::Ai("已取消行内补全".to_string())),
        Err(e) => {
            crate::ai_ledger::record(
                dir,
                "行内补全",
                &provider_id, &provider_model,
                0,
                started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            );
            return Err(StudioError::Ai(e.to_string()));
        }
    };
    crate::ai_ledger::record(
        dir,
        "行内补全",
        &provider_id, &provider_model,
        response.usage.total() as u64,
        started.elapsed().as_millis() as u64,
        true,
        None,
    );

    Ok(CompleteResult {
        text: response.text,
        tokens_used: response.usage.total(),
    })
}

/// 行内补全的快慢权衡编排：
///
/// 补全要的是快，所以第一次按档案思考配置跑（缺省即关闭思考），配额
/// 只给 2048——但有一类模型强制开启思考（GLM-5.3、DeepSeek-R1 等），
/// 表现为两种信号：
/// 1. 服务端拒收关闭参数（OpenAI 兼容路径的 minimal 被 400）；
/// 2. Anthropic 兼容路径不发思考参数、服务端默认思考，把小配额吃光，
///    返回成功但正文为空。
/// 出现任一信号时，以 Auto（不发任何思考参数，跟随服务端）重建
/// Provider、放大配额重试**一次**；取消不重试。两次尝试都如实记账。
pub fn complete_with_fallback(
    dir: &Path,
    fragment_id: &str,
    before: &str,
    after: Option<&str>,
    cancel_token: &CancelToken,
    on_delta: &std::sync::Arc<dyn Fn(&str) + Send + Sync>,
) -> Result<CompleteResult, StudioError> {
    const FAST_MAX_TOKENS: usize = 2048;
    const THINKING_MAX_TOKENS: usize = 32768;

    let provider: std::sync::Arc<dyn AiProvider> = std::sync::Arc::from(
        create_provider_for_purpose(purpose::COMPLETE, "anthropic", None)?,
    );
    let first = complete_fragment_streaming(
        dir,
        fragment_id,
        before,
        after,
        provider,
        cancel_token,
        on_delta,
        FAST_MAX_TOKENS,
    );

    if !should_retry(&first) || cancel_token.is_cancelled() {
        return first;
    }

    let provider: std::sync::Arc<dyn AiProvider> = std::sync::Arc::from(
        create_provider_for_purpose_with_thinking(purpose::COMPLETE, ThinkingConfig::Auto, "anthropic")?,
    );
    complete_fragment_streaming(
        dir,
        fragment_id,
        before,
        after,
        provider,
        cancel_token,
        on_delta,
        THINKING_MAX_TOKENS,
    )
}

/// 第一次尝试后是否需要按"强制思考模型"重试
fn should_retry(first: &Result<CompleteResult, StudioError>) -> bool {
    match first {
        Ok(r) => r.text.trim().is_empty(),
        Err(e) => is_thinking_rejection(e),
    }
}

/// 错误信息是否指向"服务端拒绝关闭思考"
fn is_thinking_rejection(err: &StudioError) -> bool {
    let msg = err.to_string().to_lowercase();
    ["思考", "thinking", "reasoning", "effort", "minimal"]
        .iter()
        .any(|k| msg.contains(k))
}

/// 汇总前文片段：取最后 `count` 段，每段截尾 `max_chars` 字
fn prev_texts_tail(texts: &[String], count: usize, max_chars: usize) -> String {
    let start = texts.len().saturating_sub(count);
    texts[start..]
        .iter()
        .map(|t| tail_chars(t, max_chars))
        .collect::<Vec<_>>()
        .join("\n")
}

// ============================================================================
// Provider 创建
// ============================================================================

/// 根据名称和模型创建 AI Provider
///
/// provider_name: "anthropic" / "openai"
/// model_name: None 时使用默认模型
/// api_key: 用户配置的 API Key（优先于环境变量）
/// api_url: 用户配置的 API URL（优先于默认 endpoint）
/// extra_body: 自定义请求体字段（深合并到请求 JSON，覆盖同名基础字段）
/// extra_headers: 自定义 HTTP 头（追加，不覆盖认证头）
/// thinking: 思考强度（跨 Provider 抽象映射）
/// api_mode: OpenAI 的 API 模式（Chat / Responses），仅对 openai 生效，默认 Chat
/// timeout_secs: 单次生成请求超时（秒）；None = 默认 300，深度思考模型可调大
/// max_tokens: 单次生成的输出上限（含思考 token）；None = 各调用点默认值
#[allow(clippy::too_many_arguments)]
pub fn create_provider(
    provider_name: &str,
    model_name: Option<&str>,
    api_key: Option<&str>,
    api_url: Option<&str>,
    api_url_full: bool,
    extra_body: Option<&serde_json::Value>,
    extra_headers: Option<&[(String, String)]>,
    thinking: Option<ThinkingConfig>,
    timeout_secs: Option<u64>,
    api_mode: Option<ApiMode>,
    max_tokens: Option<u64>,
) -> Result<Box<dyn AiProvider>, StudioError> {
    let model_name = model_name.unwrap_or(match provider_name {
        "anthropic" => "claude-sonnet-4-5",
        "openai" => "gpt-4o",
        _ => "claude-sonnet-4-5",
    });

    let api_key_owned = api_key.map(|s| s.to_string());
    let api_url_owned = api_url.map(|s| s.to_string());
    let extra_body_owned = extra_body.cloned();
    let extra_headers_owned = extra_headers
        .map(|pairs| pairs.to_vec())
        .unwrap_or_default();
    let thinking_owned = thinking;
    let timeout_owned = timeout_secs;
    let api_mode_owned = api_mode.unwrap_or_default();
    // 0 视为未配置（前端空输入归一化为 null）
    let max_tokens_owned = max_tokens.filter(|n| *n > 0).map(|n| n as usize);

    match provider_name {
        "anthropic" => Ok(Box::new(AnthropicProvider::new(
            model_name,
            api_key_owned,
            api_url_owned,
            api_url_full,
            extra_body_owned,
            extra_headers_owned,
            thinking_owned,
            timeout_owned,
            max_tokens_owned,
        ))),
        "openai" => Ok(Box::new(OpenAiProvider::new(
            model_name,
            api_key_owned,
            api_url_owned,
            api_url_full,
            extra_body_owned,
            extra_headers_owned,
            thinking_owned,
            api_mode_owned,
            timeout_owned,
            max_tokens_owned,
        ))),
        other => Err(StudioError::Config(format!(
            "未知 Provider: {other}（支持: anthropic, openai）"
        ))),
    }
}

// ============================================================================
// 全局 AI 配置
// ============================================================================

/// 一组可保存、可切换的 AI Provider 配置（档案）
///
/// 用户可在设置页保存多个档案（如「我的 Claude」「公司 OpenAI」「DeepSeek 代理」），
/// 通过 `GlobalAiConfig::active_profile` 指针一键切换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    /// 档案唯一标识（前端用 crypto.randomUUID() 生成）
    pub id: String,
    /// 来源预设 id（如 "deepseek"；仅 UI 展示用，空 = 手动配置）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// 上次连接测试的结果（设置页展示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test: Option<ProfileTestRecord>,
    /// 用户可读的档案名称
    pub name: String,
    /// Provider 类型："anthropic" / "openai"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 默认模型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// API Key（留空则从环境变量读取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// API URL（留空则使用默认 endpoint）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// 为 true 时直接使用 api_url 原样（不追加路径），默认 false（自动拼接）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url_full: Option<bool>,
    /// 自定义请求体字段（深合并到请求 JSON，覆盖同名基础字段）
    ///
    /// 例：`{"top_p": 0.9, "stop": ["\n\n"]}`
    /// 优先级高于 adapter 构造的基础字段，也高于 `thinking` 的自动映射。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    /// 自定义 HTTP 头（追加到请求头，不覆盖认证头 authorization / x-api-key）
    ///
    /// 例：`[("OpenAI-Beta", "assistants=v2"), ("Helicone-Auth", "...")]`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_headers: Option<Vec<(String, String)>>,
    /// 思考强度（跨 Provider 抽象，见 [`ThinkingConfig`]）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// 单次生成请求超时（秒，总时长含流式读取）
    ///
    /// None = 默认 300 秒；深度思考或长文生成较慢的模型可调大。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 单次生成的输出 token 上限（含思考型模型的 reasoning 配额）
    ///
    /// None = 各调用点自己的默认值；思考型模型（GLM/DeepSeek-R1 等）
    /// 的思考 token 计入配额，默认值不够时正文会被截断为空，应在此调大。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// 模型是否支持 100 万 token 长上下文
    ///
    /// Some(true) → Agent 上下文预算放大到 90 万（用满大窗口，减少过早压缩）；
    /// Some(false) → 收缩到 12 万（128K 窗口模型的保守值，避免超出窗口报错）；
    /// None = 未声明，用默认预算 50 万。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<bool>,
    /// OpenAI 的 API 模式：Chat Completions 或 Responses（仅对 provider="openai" 生效）
    ///
    /// 留空则默认 Chat（`/v1/chat/completions`），最大兼容性；
    /// 设为 `responses` 时使用 Responses API（`/v1/responses`，GPT-5 / o 系列原生接口）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_mode: Option<ApiMode>,
}

impl ProviderProfile {
    /// 构造一个带默认值的新档案（id 由调用方提供）
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            preset: None,
            last_test: None,
            name: name.into(),
            provider: Some("anthropic".to_string()),
            model: None,
            api_key: None,
            api_url: None,
            api_url_full: None,
            extra_body: None,
            extra_headers: None,
            thinking: None,
            timeout_secs: None,
            max_tokens: None,
            long_context: None,
            api_mode: None,
        }
    }
}

/// 全局 AI 配置（存于 ~/.story-studio/config.json，跨项目共享）
///
/// 支持两种形态（向后兼容）：
/// 1. **新形态**：`profiles` 数组 + `active_profile` 指针，支持多档案切换；
///    可选 `purpose_profiles` 给不同用途分配不同档案
/// 2. **旧形态**：单组 `default_provider`/`api_key`/... 字段，读取时自动迁移为默认档案
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalAiConfig {
    /// 所有已保存的 Provider 档案
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<ProviderProfile>>,
    /// 当前激活的档案 id（指向 `profiles` 中某项）——未分配用途的默认档案
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    /// 用途 → 档案 id 分配（如 write / agent / workflow → profile id）
    ///
    /// 允许给不同用途配不同的 AI（例：续写用便宜快速的模型、妙笔对话
    /// 用强模型）。悬空分配（档案已删）自动回退激活档案。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose_profiles: Option<std::collections::BTreeMap<String, String>>,

    // ── 旧字段（保留用于向后兼容读取；新数据写入 profiles 后这些字段不再使用）──
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url_full: Option<bool>,
}

impl GlobalAiConfig {
    /// 取得当前激活的档案（不可变引用）
    ///
    /// 选择逻辑：
    /// 1. 优先按 `active_profile` 指针在 `profiles` 中查找
    /// 2. 若指针无效但 profiles 非空，返回第一个
    /// 3. 否则返回 None（调用方应 fallback 到旧字段）
    pub fn active_profile(&self) -> Option<&ProviderProfile> {
        let profiles = self.profiles.as_ref()?;
        if profiles.is_empty() {
            return None;
        }
        if let Some(id) = self.active_profile.as_deref() {
            if let Some(p) = profiles.iter().find(|p| p.id == id) {
                return Some(p);
            }
        }
        // 指针无效，退回首项
        profiles.first()
    }

    /// 按用途解析生效档案：用途分配优先，悬空分配或未分配回退激活档案
    pub fn profile_for_purpose(&self, purpose: &str) -> Option<&ProviderProfile> {
        if let Some(map) = self.purpose_profiles.as_ref() {
            if let Some(id) = map.get(purpose) {
                if let Some(p) = self
                    .profiles
                    .as_ref()
                    .and_then(|ps| ps.iter().find(|p| &p.id == id))
                {
                    return Some(p);
                }
            }
        }
        self.active_profile()
    }
}

/// 全局配置目录：~/.story-studio/
pub fn global_config_dir() -> Result<PathBuf, StudioError> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| StudioError::Config("无法获取 HOME 目录".to_string()))?;
    Ok(PathBuf::from(home).join(".story-studio"))
}

/// 全局配置文件路径：~/.story-studio/config.json
pub fn global_config_path() -> Result<PathBuf, StudioError> {
    Ok(global_config_dir()?.join("config.json"))
}

/// 读取全局 AI 配置
///
/// 自动执行向后兼容迁移：若 `profiles` 为空但存在旧字段（`default_provider`/`api_key` 等），
/// 则构造一个 id=`default`、name=`默认` 的档案并入 profiles，同时设为激活档案。
pub fn read_global_ai_config() -> Result<GlobalAiConfig, StudioError> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(GlobalAiConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StudioError::Io(format!("读取全局配置失败: {e}")))?;
    let mut config = serde_json::from_str::<GlobalAiConfig>(&content)
        .map_err(|e| StudioError::Config(format!("解析全局配置失败: {e}")))?;

    // ── 向后兼容迁移：旧单组配置 → profiles 默认档案 ──
    let has_profiles = config
        .profiles
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let has_legacy = config.default_provider.is_some()
        || config.api_key.is_some()
        || config.api_url.is_some()
        || config.default_model.is_some()
        || config.api_url_full.is_some();

    if !has_profiles && has_legacy {
        let profile = ProviderProfile {
            id: "default".to_string(),
            preset: None,
            last_test: None,
            name: "默认".to_string(),
            provider: config.default_provider.clone(),
            model: config.default_model.clone(),
            api_key: config.api_key.clone(),
            api_url: config.api_url.clone(),
            api_url_full: config.api_url_full,
            extra_body: None,
            extra_headers: None,
            thinking: None,
            timeout_secs: None,
            max_tokens: None,
            long_context: None,
            api_mode: None,
        };
        config.profiles = Some(vec![profile]);
        config.active_profile = Some("default".to_string());
    }

    Ok(config)
}

/// 写入全局 AI 配置
pub fn write_global_ai_config(config: &GlobalAiConfig) -> Result<(), StudioError> {
    let dir = global_config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| StudioError::Io(format!("创建配置目录失败: {e}")))?;
    let path = global_config_path()?;
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| StudioError::Config(format!("序列化全局配置失败: {e}")))?;
    std::fs::write(&path, json).map_err(|e| StudioError::Io(format!("写入全局配置失败: {e}")))
}

/// 从全局配置创建 AI Provider（合并激活档案的 api_key/api_url）
///
/// 优先级：激活档案 > 旧字段 > 环境变量。
pub fn create_provider_from_global(
    provider_name: &str,
    model_name: Option<&str>,
) -> Result<Box<dyn AiProvider>, StudioError> {
    let config = read_global_ai_config().unwrap_or_default();
    let profile = config.active_profile();
    provider_from_config(&config, profile, provider_name, model_name)
}

/// 按用途创建 AI Provider（合并该用途档案的 api_key/api_url）
///
/// 用途 → 档案的解析见 [`GlobalAiConfig::profile_for_purpose`]：
/// 用途分配优先，悬空分配或未分配回退激活档案；一个档案都没有时
/// 退回旧字段 / 环境变量（与 [`create_provider_from_global`] 一致）。
pub fn create_provider_for_purpose(
    purpose: &str,
    provider_name: &str,
    model_name: Option<&str>,
) -> Result<Box<dyn AiProvider>, StudioError> {
    let config = read_global_ai_config().unwrap_or_default();
    let profile = config.profile_for_purpose(purpose);
    provider_from_config(&config, profile, provider_name, model_name)
}

/// 按用途构建 Provider，并强制覆盖该档案的思考配置。
/// 用于行内补全的降级重试：Auto（不发思考参数，跟随服务端）以兼容
/// 强制思考模型。档案缺省（无 profiles）时无覆盖对象，原样构造。
pub fn create_provider_for_purpose_with_thinking(
    purpose: &str,
    thinking: ThinkingConfig,
    provider_name: &str,
) -> Result<Box<dyn AiProvider>, StudioError> {
    let mut config = read_global_ai_config().unwrap_or_default();
    let Some(profile_id) = config.profile_for_purpose(purpose).map(|p| p.id.clone()) else {
        return create_provider_for_purpose(purpose, provider_name, None);
    };
    if let Some(profiles) = config.profiles.as_mut() {
        if let Some(p) = profiles.iter_mut().find(|p| p.id == profile_id) {
            p.thinking = Some(thinking);
        }
    }
    let profile = config.profile_for_purpose(purpose);
    provider_from_config(&config, profile, provider_name, None)
}

/// 共享的档案 → Provider 构造（档案缺失时走旧字段回退）
fn provider_from_config(
    config: &GlobalAiConfig,
    profile: Option<&ProviderProfile>,
    provider_name: &str,
    model_name: Option<&str>,
) -> Result<Box<dyn AiProvider>, StudioError> {
    let (
        api_key,
        api_url,
        api_url_full,
        default_model,
        extra_body,
        extra_headers,
        thinking,
        timeout_secs,
        api_mode,
        max_tokens,
        resolved_provider,
    ) = match profile {
        Some(p) => (
            p.api_key.as_deref().or(config.api_key.as_deref()),
            p.api_url.as_deref().or(config.api_url.as_deref()),
            p.api_url_full.or(config.api_url_full).unwrap_or(false),
            p.model.as_deref().or(config.default_model.as_deref()),
            p.extra_body.as_ref(),
            p.extra_headers.as_deref(),
            p.thinking.clone(),
            p.timeout_secs,
            p.api_mode.clone(),
            p.max_tokens,
            p.provider.as_deref().unwrap_or(provider_name),
        ),
        None => (
            config.api_key.as_deref(),
            config.api_url.as_deref(),
            config.api_url_full.unwrap_or(false),
            config.default_model.as_deref(),
            None,
            None,
            None,
            None,
            None,
            None,
            provider_name,
        ),
    };

    let model = model_name.or(default_model);
    create_provider(
        resolved_provider,
        model,
        api_key,
        api_url,
        api_url_full,
        extra_body,
        extra_headers,
        thinking,
        timeout_secs,
        api_mode,
        max_tokens,
    )
}

// ============================================================================
// Provider 预设 — 常见服务商的模板（类型 / URL / 默认模型）
// ============================================================================

/// 一个 Provider 预设
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    /// 预设 id（如 "deepseek"）
    pub id: String,
    /// 展示名
    pub name: String,
    /// Provider 类型："anthropic" / "openai"
    pub provider: String,
    /// API URL（None = 官方默认端点）
    pub api_url: Option<String>,
    /// 是否完整 URL 模式
    pub api_url_full: bool,
    /// 建议的默认模型（可空；用户可拉取列表另选）
    pub default_model: Option<String>,
    /// 备注（Key 获取方式等）
    pub note: Option<String>,
}

/// 内置 Provider 预设清单
pub fn provider_presets() -> Vec<ProviderPreset> {
    [
        ("openai", "OpenAI 官方", "openai", None, false, Some("gpt-4o"), Some("platform.openai.com 生成 Key")),
        ("anthropic", "Anthropic 官方", "anthropic", None, false, Some("claude-sonnet-4-5"), Some("console.anthropic.com 生成 Key")),
        ("deepseek", "DeepSeek", "openai", Some("https://api.deepseek.com"), false, Some("deepseek-chat"), Some("platform.deepseek.com 生成 Key")),
        ("zhipu", "智谱 GLM", "openai", Some("https://open.bigmodel.cn/api/paas/v4/chat/completions"), true, Some("glm-4.5"), Some("open.bigmodel.cn 生成 Key；思考模式已自动适配智谱原生参数")),
        ("zhipu-coding", "智谱 GLM Coding Plan", "anthropic", Some("https://open.bigmodel.cn/api/anthropic"), false, None, Some("Claude Code 同款接入点，走 Coding Plan 套餐额度不扣余额；模型点「拉取」选择")),
        ("moonshot", "Kimi（月之暗面）", "openai", Some("https://api.moonshot.cn"), false, None, Some("platform.moonshot.cn 生成 Key")),
        ("qwen", "通义千问（百炼）", "openai", Some("https://dashscope.aliyuncs.com/compatible-mode"), false, Some("qwen-plus"), Some("bailian.console.aliyun.com 生成 Key；开启思考自动发 enable_thinking 参数")),
        ("openrouter", "OpenRouter", "openai", Some("https://openrouter.ai/api/v1/chat/completions"), true, None, Some("openrouter.ai 聚合平台，一个 Key 用多家模型")),
        ("siliconflow", "硅基流动", "openai", Some("https://api.siliconflow.cn"), false, None, Some("siliconflow.cn 生成 Key")),
        ("gemini", "Google Gemini", "openai", Some("https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"), true, None, Some("aistudio.google.com 生成 Key")),
        ("ollama", "Ollama（本地）", "openai", Some("http://localhost:11434"), false, None, Some("本机运行 Ollama 即可，无需 Key")),
    ]
    .into_iter()
    .map(|(id, name, provider, api_url, api_url_full, default_model, note)| ProviderPreset {
        id: id.to_string(),
        name: name.to_string(),
        provider: provider.to_string(),
        api_url: api_url.map(|s| s.to_string()),
        api_url_full,
        default_model: default_model.map(|s| s.to_string()),
        note: note.map(|s| s.to_string()),
    })
    .collect()
}

/// 按 id 取预设
pub fn provider_preset(id: &str) -> Option<ProviderPreset> {
    provider_presets().into_iter().find(|p| p.id == id)
}

// ============================================================================
// 连接测试 — 先试零消耗的模型列表，失败再退最小生成调用
// ============================================================================

/// 一次连接测试的结果（持久化在档案上，设置页展示「上次测试」）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTestRecord {
    pub ok: bool,
    /// 测试时间（毫秒时间戳）
    pub at: u64,
    /// 耗时（毫秒）
    pub latency_ms: u64,
    /// 结果说明（成功方式 / 失败原因）
    pub detail: String,
}

/// 测试一个档案的连通性。
///
/// 不修改配置：优先 `GET /models`（不消耗 token）；该服务不提供列表或
/// 返回为空时，回退到一次最小生成调用（思考模型需要预算余量，正文为
/// 空但调用成功也算通过）。
pub fn test_profile(profile: &ProviderProfile) -> ProfileTestRecord {
    let started = std::time::Instant::now();
    let provider_name = profile.provider.as_deref().unwrap_or("anthropic");
    let now = crate::journal::now_ms();
    let fail = |detail: String| ProfileTestRecord {
        ok: false,
        at: now,
        latency_ms: started.elapsed().as_millis() as u64,
        detail,
    };
    let ok = |detail: String| ProfileTestRecord {
        ok: true,
        at: now,
        latency_ms: started.elapsed().as_millis() as u64,
        detail,
    };

    // 1) 零消耗路径：模型列表
    match story_ai_runtime::providers::list_models(
        provider_name,
        profile.api_url.as_deref(),
        profile.api_url_full.unwrap_or(false),
        profile.api_key.as_deref(),
    ) {
        Ok(models) if !models.is_empty() => {
            return ok(format!("连接成功，可用模型 {} 个（未消耗 token）", models.len()));
        }
        _ => {}
    }

    // 2) 回退：最小生成调用（智谱等无 /models 的服务）
    let provider = match create_provider(
        provider_name,
        profile.model.as_deref(),
        profile.api_key.as_deref(),
        profile.api_url.as_deref(),
        profile.api_url_full.unwrap_or(false),
        profile.extra_body.as_ref(),
        profile.extra_headers.as_deref(),
        profile.thinking.clone(),
        profile.timeout_secs,
        profile.api_mode.clone(),
        // 连通性测试只要两个字，不套用档案的输出上限（大配额反而拖长思考）
        None,
    ) {
        Ok(p) => p,
        Err(e) => return fail(format!("配置无法使用: {e}")),
    };

    let request = GenerateRequest {
        system: "你是连通性测试。".to_string(),
        messages: vec![Message::user("只回复两个字：正常")],
        model: String::new(),
        temperature: 0.0,
        // 思考模式下正文为空也属成功；留出思考预算避免截断误判
        max_tokens: 1024,
        tools: Vec::new(),
    };
    match provider.generate(&request) {
        Ok(resp) => {
            let text = resp.text.trim();
            if text.is_empty() {
                ok("调用成功（该模型返回了空正文，连接与鉴权正常）".to_string())
            } else {
                let snippet: String = text.chars().take(24).collect();
                ok(format!("生成测试通过：{snippet}"))
            }
        }
        Err(e) => fail(e.to_string()),
    }
}

// ============================================================================
// 模型列表拉取（设置页用；参数取自编辑中的档案草稿，不必先保存）
// ============================================================================

pub fn list_models(
    provider_name: &str,
    api_url: Option<&str>,
    api_url_full: bool,
    api_key: Option<&str>,
) -> Result<Vec<String>, StudioError> {
    story_ai_runtime::providers::list_models(provider_name, api_url, api_url_full, api_key)
        .map_err(|e| StudioError::Ai(format!("拉取模型列表失败: {e}")))
}

// ============================================================================
// save_drafts 的便捷入口（需要 ProjectPaths）
// ============================================================================

/// 将 AI 生成的草稿保存到项目（封装 fragment 模块的 save_drafts）
pub fn save_write_result(
    dir: &Path,
    scene_id: &str,
    result: &WriteResult,
) -> Result<Vec<String>, StudioError> {
    let paths = ProjectPaths::new(dir);
    crate::fragment::save_drafts(&paths, scene_id, &result.drafts)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod purpose_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn profile(id: &str, name: &str, provider: &str, model: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            preset: None,
            last_test: None,
            name: name.to_string(),
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            api_key: None,
            api_url: None,
            api_url_full: None,
            extra_body: None,
            extra_headers: None,
            thinking: None,
            timeout_secs: None,
            max_tokens: None,
            long_context: None,
            api_mode: None,
        }
    }

    fn sample_config() -> GlobalAiConfig {
        GlobalAiConfig {
            profiles: Some(vec![
                profile("p1", "快模型", "openai", "gpt-4o-mini"),
                profile("p2", "强模型", "anthropic", "claude-sonnet-4-5"),
            ]),
            active_profile: Some("p1".to_string()),
            purpose_profiles: None,
            default_provider: None,
            default_model: None,
            api_key: None,
            api_url: None,
            api_url_full: None,
        }
    }

    #[test]
    fn unassigned_purpose_falls_back_to_active() {
        let config = sample_config();
        assert_eq!(config.profile_for_purpose(purpose::WRITE).unwrap().id, "p1");
        assert_eq!(config.profile_for_purpose("任何未知用途").unwrap().id, "p1");
    }

    #[test]
    fn assigned_purpose_uses_that_profile() {
        let mut config = sample_config();
        let mut map = BTreeMap::new();
        map.insert(purpose::AGENT.to_string(), "p2".to_string());
        map.insert(purpose::WORKFLOW.to_string(), "p2".to_string());
        config.purpose_profiles = Some(map);

        assert_eq!(config.profile_for_purpose(purpose::AGENT).unwrap().id, "p2");
        assert_eq!(config.profile_for_purpose(purpose::WORKFLOW).unwrap().id, "p2");
        // 未分配的用途仍走激活档案
        assert_eq!(config.profile_for_purpose(purpose::WRITE).unwrap().id, "p1");
    }

    #[test]
    fn dangling_assignment_falls_back_to_active() {
        let mut config = sample_config();
        let mut map = BTreeMap::new();
        map.insert(purpose::WRITE.to_string(), "已删除的档案".to_string());
        config.purpose_profiles = Some(map);

        assert_eq!(
            config.profile_for_purpose(purpose::WRITE).unwrap().id,
            "p1",
            "悬空分配应回退激活档案"
        );
    }

    #[test]
    fn legacy_config_without_purpose_field_parses() {
        let json = r#"{
            "profiles": [{"id": "a", "name": "默认", "provider": "anthropic"}],
            "active_profile": "a"
        }"#;
        let config: GlobalAiConfig = serde_json::from_str(json).unwrap();
        assert!(config.purpose_profiles.is_none());
        assert_eq!(config.profile_for_purpose(purpose::WRITE).unwrap().id, "a");
    }

    #[test]
    fn profile_max_tokens_parses_and_defaults_to_none() {
        // 旧配置没有 max_tokens 字段 → None（各调用点默认值生效）
        let old = r#"{"profiles": [{"id": "a", "name": "默认"}], "active_profile": "a"}"#;
        let config: GlobalAiConfig = serde_json::from_str(old).unwrap();
        assert!(config.active_profile().unwrap().max_tokens.is_none());

        // 新配置带 max_tokens → 透传
        let new = r#"{
            "profiles": [{"id": "a", "name": "GLM", "max_tokens": 65536}],
            "active_profile": "a"
        }"#;
        let config: GlobalAiConfig = serde_json::from_str(new).unwrap();
        assert_eq!(config.active_profile().unwrap().max_tokens, Some(65536));

        // 序列化时 None 跳过、Some 保留
        let json = serde_json::to_string(config.active_profile().unwrap()).unwrap();
        assert!(json.contains("\"max_tokens\":65536"));

        // long_context 同样三态：缺省 None / true / false 透传
        let lc = r#"{
            "profiles": [{"id": "a", "name": "长上下文", "long_context": true}],
            "active_profile": "a"
        }"#;
        let config: GlobalAiConfig = serde_json::from_str(lc).unwrap();
        assert_eq!(config.active_profile().unwrap().long_context, Some(true));
    }

    #[test]
    fn create_provider_for_purpose_uses_assigned_profile() {
        let mut config = sample_config();
        let mut map = BTreeMap::new();
        map.insert(purpose::AGENT.to_string(), "p2".to_string());
        config.purpose_profiles = Some(map);

        // p2 是 anthropic 档案：解析出的 provider 应是 anthropic 而非默认
        let provider =
            provider_from_config(&config, config.profile_for_purpose(purpose::AGENT), "openai", None)
                .unwrap();
        assert_eq!(provider.id(), "anthropic");

        // 未分配用途走 p1（openai）
        let provider =
            provider_from_config(&config, config.profile_for_purpose(purpose::WRITE), "openai", None)
                .unwrap();
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn no_profiles_at_all_falls_back_to_legacy() {
        let config = GlobalAiConfig::default();
        assert!(config.profile_for_purpose(purpose::WRITE).is_none());
        // provider_from_config 的 None 分支不 panic，构造走旧字段路径
        let provider = provider_from_config(&config, None, "anthropic", None).unwrap();
        assert_eq!(provider.id(), "anthropic");
    }

    #[test]
    fn tail_chars_keeps_suffix_on_char_boundary() {
        assert_eq!(tail_chars("林风推开门", 3), "推开门");
        assert_eq!(tail_chars("短", 10), "短");
        // 多字节字符不截半
        let tailed = tail_chars("风雨如晦鸡鸣不已", 4);
        assert_eq!(tailed, "鸡鸣不已");
    }

    #[test]
    fn prev_texts_tail_takes_last_n() {
        let texts: Vec<String> = vec!["一".to_string(), "二".to_string(), "三".to_string()];
        assert_eq!(prev_texts_tail(&texts, 2, 100), "二\n三");
        assert_eq!(prev_texts_tail(&texts, 5, 100), "一\n二\n三");
        assert_eq!(prev_texts_tail(&[], 2, 100), "");
    }

    #[test]
    fn build_complete_prompt_sections() {
        let (system, user) = build_complete_prompt(
            Some("林风（林少侠）"),
            Some("渡口"),
            "前一段的正文。",
            "他推开门，",
            None,
        );
        assert!(system.contains("行内补全"));
        assert!(system.contains("林风"));
        assert!(system.contains("渡口"));
        assert!(user.contains("【本场景前文】"));
        assert!(user.contains("前一段的正文"));
        assert!(user.contains("【光标前正文】\n他推开门，"));
        // 没有光标后文本时不输出该段
        assert!(!user.contains("【光标后正文】"));
        assert!(user.contains("只输出从光标处续写"));
    }

    #[test]
    fn build_complete_prompt_includes_after_when_present() {
        let (_, user) = build_complete_prompt(None, None, "", "开头", Some("，后面还有半句。"));
        assert!(user.contains("【光标后正文】"));
        assert!(user.contains("，后面还有半句。"));
        // 空白 after 视为没有
        let (_, user) = build_complete_prompt(None, None, "", "开头", Some("   "));
        assert!(!user.contains("【光标后正文】"));
    }

    #[test]
    fn thinking_rejection_detected_by_message() {
        for msg in [
            "API 错误 (400): 该模型不支持关闭 thinking",
            "API 错误 (400): reasoning_effort 参数非法",
            "API 错误 (400): 不支持 minimal effort",
        ] {
            assert!(is_thinking_rejection(&StudioError::Ai(msg.to_string())), "{msg}");
        }
        // 无关错误不触发重试
        assert!(!is_thinking_rejection(&StudioError::Ai("网络错误: 连接超时".to_string())));
        assert!(!is_thinking_rejection(&StudioError::NotFound("f_x 不存在".to_string())));
    }

    #[test]
    fn should_retry_on_empty_text_or_thinking_rejection() {
        // 空正文（思考吃光配额）→ 重试
        let empty = Ok(CompleteResult { text: "  ".to_string(), tokens_used: 10 });
        assert!(should_retry(&empty));
        // 有正文 → 不重试
        let ok = Ok(CompleteResult { text: "他抬起头。".to_string(), tokens_used: 10 });
        assert!(!should_retry(&ok));
        // 思考拒收错误 → 重试
        let rejected = Err(StudioError::Ai("API 错误 (400): thinking 已禁用".to_string()));
        assert!(should_retry(&rejected));
        // 其他错误 → 不重试
        let other = Err(StudioError::Ai("认证失败: 缺少 API Key".to_string()));
        assert!(!should_retry(&other));
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    #[test]
    fn presets_registry_sane() {
        let presets = provider_presets();
        assert!(presets.len() >= 8);
        // id 唯一
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), presets.len());
        for p in &presets {
            assert!(
                p.provider == "openai" || p.provider == "anthropic",
                "{} 的 provider 非法",
                p.id
            );
            if let Some(url) = &p.api_url {
                assert!(url.starts_with("http://") || url.starts_with("https://"), "{} 的 URL 非法", p.id);
            }
        }
        assert!(provider_preset("deepseek").is_some());
        assert!(provider_preset("不存在").is_none());
    }

    #[test]
    fn profile_preset_field_roundtrip_and_legacy() {
        // 新字段默认可省（旧配置兼容）
        let legacy = r#"{"id":"a","name":"n","provider":"openai"}"#;
        let p: ProviderProfile = serde_json::from_str(legacy).unwrap();
        assert!(p.preset.is_none());
        // 序列化回填
        let json = serde_json::to_value(ProviderProfile::new("a", "n")).unwrap();
        assert!(json.get("preset").is_none(), "None 不序列化");
    }
}
