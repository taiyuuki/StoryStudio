//! Project Tools — AI Agent 可调用的项目操作工具
//!
//! 本模块是工具子系统的聚合入口：定义共享的 [`ToolContext`] 与若干内部
//! helper，并通过子模块（entities / structure / knowledge / outline /
//! worldview / agent）按职责域组织 30+ 个 Tool 实现与 Agent 运行入口。
//! 每个工具实现 ai-runtime 的 Tool trait，让 AI Agent 通过 function
//! calling 直接操作项目文件。

use std::path::PathBuf;
use std::sync::Arc;

use story_ai_runtime::AiProvider;
use story_project_format::ProjectPaths;

pub struct ToolContext {
    pub project_dir: PathBuf,
    /// 可选的 AI provider，供部分工具在 execute 内部发起一次性 LLM 调用
    /// （如 get_story_recap 的按需摘要生成）。
    ///
    /// `None` 表示无 provider 可用；依赖 provider 的工具需自行降级处理。
    /// 绝大多数工具不使用此字段，`Option` 保证它们零影响。
    pub provider: Option<Arc<dyn AiProvider>>,
}

impl Clone for ToolContext {
    fn clone(&self) -> Self {
        Self {
            project_dir: self.project_dir.clone(),
            provider: self.provider.clone(),
        }
    }
}

impl ToolContext {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: dir.into(),
            provider: None,
        }
    }

    /// 构造带 provider 的 ToolContext（用于需要在工具内调 LLM 的场景）
    pub fn with_provider(dir: impl Into<PathBuf>, provider: Arc<dyn AiProvider>) -> Self {
        Self {
            project_dir: dir.into(),
            provider: Some(provider),
        }
    }

    pub(super) fn paths(&self) -> ProjectPaths {
        ProjectPaths::new(&self.project_dir)
    }
}

pub(super) fn object_schema(properties: Vec<(&str, &str, &str, bool)>) -> serde_json::Value {
    let mut props = serde_json::Map::new();
    let mut required: Vec<serde_json::Value> = Vec::new();
    for (name, typ, desc, req) in &properties {
        props.insert(
            name.to_string(),
            serde_json::json!({"type": typ, "description": desc}),
        );
        if *req {
            required.push(serde_json::Value::String(name.to_string()));
        }
    }
    serde_json::json!({"type": "object", "properties": props, "required": required})
}

pub(super) fn get_str(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("缺少参数: {key}"))
}

pub(super) fn get_str_opt(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

pub(super) fn format_brief(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        // 按字符边界截断，避免在 UTF-8 多字节字符中间切断（会 panic）
        let truncated: String = text.chars().take(max_len).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}

/// 实体档案摘要：名称（ID，职业，种族/类型…）——只拼非空字段，
/// 用于写前把档案要点推到创作者眼前。
pub(super) fn entity_profile_brief(id: &str, e: &story_project_format::EntityFile) -> String {
    let mut parts: Vec<String> = vec![id.to_string()];
    if let Some(p) = &e.profile {
        if let Some(o) = &p.occupation {
            parts.push(o.clone());
        }
        if let Some(r) = &p.race {
            parts.push(r.clone());
        }
    }
    if let Some(g) = &e.geography {
        if let Some(k) = &g.kind {
            parts.push(k.clone());
        }
    }
    format!("{}（{}）", e.display_name, parts.join("，"))
}

/// 扫描正文提及的实体（display_name / aliases 出现即命中），
/// 返回档案摘要列表（最多 max 条）——与 LocationIndex 隐式匹配同思路，
/// 但不依赖完整 parse，写入回执场景够用。
pub(super) fn mentioned_entity_briefs(
    paths: &ProjectPaths,
    text: &str,
    max: usize,
) -> Vec<String> {
    let mut briefs = Vec::new();
    for dir in [
        paths.characters_dir(),
        paths.locations_dir(),
        paths.items_dir(),
        paths.organizations_dir(),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(".toml") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(entity) = story_project_format::EntityFile::from_toml_str(&content) else {
                continue;
            };
            let id = fname.trim_end_matches(".toml").to_string();
            let hit = text.contains(entity.display_name.as_str())
                || entity
                    .aliases
                    .iter()
                    .any(|a| !a.is_empty() && text.contains(a.as_str()));
            if hit {
                briefs.push(entity_profile_brief(&id, &entity));
                if briefs.len() >= max {
                    return briefs;
                }
            }
        }
    }
    briefs
}

/// 索引越界错误信息（删除/修改通用，outline/worldview 共享）
pub(super) fn out_of_range(label: &str, index: usize, len: usize) -> String {
    if len == 0 {
        format!("{label}列表为空，无法操作 index={index}")
    } else {
        format!("{label}序号 {index} 超出范围（有效范围 1-{len}）")
    }
}

/// 校验索引有效性，无效则返回 Err（outline/worldview 共享）
pub(super) fn check_index(label: &str, index: usize, len: usize) -> Result<(), String> {
    if index > len {
        Err(out_of_range(label, index, len))
    } else {
        Ok(())
    }
}

/// 带重试的 LLM 调用 —— 供工具内部（如 rewrite_fragment、get_story_recap）使用。
///
/// 对速率限制（429）、服务端错误（5xx）、网络错误自动重试，指数退避（1s, 2s, 4s...），
/// 最多 5 次。这解决了智谱等 API 的速率限制导致工具内 LLM 调用直接失败的问题。
pub(super) fn retry_generate(
    provider: &Arc<dyn AiProvider>,
    request: &story_ai_runtime::provider::GenerateRequest,
) -> Result<story_ai_runtime::provider::GenerateResponse, String> {
    const MAX_RETRIES: usize = 5;
    let mut last_error = String::new();
    for attempt in 0..=MAX_RETRIES {
        match provider.generate(request) {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                let err_str = format!("{e}");
                last_error = err_str.clone();
                // 判断是否可重试
                let retryable = is_retryable_error(&e);
                if !retryable || attempt == MAX_RETRIES {
                    return Err(err_str);
                }
                let backoff = std::cmp::min(1000 * 2u64.pow(attempt as u32), 10_000);
                eprintln!(
                    "[tool] LLM 调用失败（可重试），{}ms 后重试 (attempt {}/{}): {}",
                    backoff,
                    attempt + 1,
                    MAX_RETRIES,
                    err_str
                );
                std::thread::sleep(std::time::Duration::from_millis(backoff));
            }
        }
    }
    Err(last_error)
}

/// 判断 AiError 是否可重试（速率限制、网络错误、服务端临时错误）
fn is_retryable_error(e: &story_ai_runtime::provider::AiError) -> bool {
    use story_ai_runtime::provider::AiError;
    match e {
        AiError::Network(_) => true,
        AiError::Api { status, message } => {
            if *status == 429 || (500..=599).contains(status) {
                return true;
            }
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

mod entities;
pub use anchoring::*;
pub use entities::*;

mod structure;
pub use structure::*;

mod knowledge;
pub use knowledge::*;

mod outline;
pub use outline::*;

mod anchoring;
mod chapters;
pub use chapters::*;

mod worldview;
pub use worldview::*;

mod query;
pub use query::*;

pub(crate) mod guide;
pub use guide::*;

mod ai_check;
pub use ai_check::*;

mod agent;
pub use agent::*;
