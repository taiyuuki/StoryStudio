//! Project Tools — AI Agent 可调用的项目操作工具
//!
//! 本模块是工具子系统的聚合入口：定义共享的 [`ToolContext`] 与若干内部
//! helper，并通过子模块（entities / structure / knowledge / outline /
//! worldview / agent）按职责域组织 30+ 个 Tool 实现与 Agent 运行入口。
//! 每个工具实现 ai-runtime 的 Tool trait，让 AI Agent 通过 function
//! calling 直接操作项目文件。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use story_ai_runtime::AiProvider;
use story_parser::{detect_dirty, parse_incremental, parse_project, ParseCache, ParseOutput};
use story_project_format::ProjectPaths;

pub struct ToolContext {
    pub project_dir: PathBuf,
    /// 可选的 AI provider，供部分工具在 execute 内部发起一次性 LLM 调用
    /// （如 get_story_recap 的按需摘要生成）。
    ///
    /// `None` 表示无 provider 可用；依赖 provider 的工具需自行降级处理。
    /// 绝大多数工具不使用此字段，`Option` 保证它们零影响。
    pub provider: Option<Arc<dyn AiProvider>>,
    /// 项目解析缓存（Arc 共享：同一工具注册表内的所有 ctx.clone() 复用一份）。
    /// 长篇下妙笔一轮对话几十次工具调用，每次全量重读全书文件是主要耗时。
    parse_state: Arc<Mutex<ParseState>>,
}

/// 解析缓存状态：上次解析结果 + 文件 hash 缓存
struct ParseState {
    cache: ParseCache,
    output: Option<Arc<ParseOutput>>,
    /// 增量机制不追踪、但影响解析结果的文件 hash（threads.toml /
    /// relations.toml）——变化即强制全量重解析，避免读到旧数据
    extra_hashes: [Option<u64>; 2],
}

/// 文件快速 hash（mtime + 大小；不存在 → None），口径与 parser 的 dirty
/// 检测一致：只判断「变没变」，不校验内容
fn quick_hash(path: &std::path::Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Ok(modified) = metadata.modified() {
        if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
            duration.as_nanos().hash(&mut hasher);
        }
    }
    metadata.len().hash(&mut hasher);
    Some(hasher.finish())
}

impl Clone for ToolContext {
    fn clone(&self) -> Self {
        Self {
            project_dir: self.project_dir.clone(),
            provider: self.provider.clone(),
            parse_state: Arc::clone(&self.parse_state),
        }
    }
}

impl ToolContext {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: dir.into(),
            provider: None,
            parse_state: Arc::new(Mutex::new(ParseState {
                cache: ParseCache::default(),
                output: None,
                extra_hashes: [None; 2],
            })),
        }
    }

    /// 构造带 provider 的 ToolContext（用于需要在工具内调 LLM 的场景）
    pub fn with_provider(dir: impl Into<PathBuf>, provider: Arc<dyn AiProvider>) -> Self {
        Self {
            project_dir: dir.into(),
            provider: Some(provider),
            parse_state: Arc::new(Mutex::new(ParseState {
                cache: ParseCache::default(),
                output: None,
                extra_hashes: [None; 2],
            })),
        }
    }

    pub(super) fn paths(&self) -> ProjectPaths {
        ProjectPaths::new(&self.project_dir)
    }

    /// 解析项目（工具统一入口，带增量缓存）。
    ///
    /// - 首次：全量解析；
    /// - 无文件变化：直接复用上次结果（Arc 零拷贝）；
    /// - Fragment / Entity 变化：增量重解析；
    /// - structure.toml / threads.toml / relations.toml 变化：全量重建
    ///   （关系结构的 SoT，且增量机制不追踪后两者）；
    /// - 增量失败：回退全量，绝不让工具因缓存问题挂掉。
    ///
    /// 返回 Arc 而非所有权值：无变化路径不产生任何克隆。
    pub(crate) fn parse(&self) -> Result<Arc<ParseOutput>, String> {
        let mut state = self
            .parse_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = &self.project_dir;

        // 增量机制不追踪 threads.toml / relations.toml：单独盯住，变了强制全量
        let paths = ProjectPaths::new(dir);
        let extras = [
            quick_hash(&paths.threads_toml()),
            quick_hash(&paths.relations_toml()),
        ];
        let extras_changed = state.output.is_some() && state.extra_hashes != extras;

        let (dirty, _new_cache) = detect_dirty(dir, &state.cache);
        if dirty.is_empty() && !extras_changed {
            if let Some(output) = &state.output {
                return Ok(Arc::clone(output));
            }
        }

        let incremental = if extras_changed {
            None
        } else {
            state.output.as_deref().and_then(|old| {
                // 有脏文件时走增量；structure.toml 变化时 parse_incremental
                // 内部自动切换为全量重建
                parse_incremental(dir, old, &state.cache).ok().map(|res| {
                    Arc::new(ParseOutput {
                        ir: res.ir,
                        location_index: res.location_index,
                        threads: old.threads.clone(),
                        warnings: res.warnings,
                    })
                })
            })
        };

        let output = match incremental {
            Some(output) => output,
            None => {
                // 首次 / 增量失败 / threads·relations 变化 → 全量
                let out = parse_project(dir).map_err(|e| format!("{e:?}"))?;
                Arc::new(out)
            }
        };

        // 以刚解析完的状态刷新 hash 缓存，保留结果供下次复用
        let (_, settled) = detect_dirty(dir, &ParseCache::default());
        state.cache = settled;
        state.extra_hashes = extras;
        state.output = Some(Arc::clone(&output));
        Ok(output)
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

/// 读取批量参数（JSON 数组），兼容字符串与原生数组两种形态。
///
/// 模型原生工具调用常无视 schema 的 "string" 声明直接传数组；若只按
/// 字符串读取会静默返回 None、误入单条分支，报出误导性的"缺少参数"。
/// - 缺省 → `Ok(None)`（走单条分支）
/// - 数组 → 直接使用；字符串 → 宽松解析（剥围栏/截取/修复引号）
/// - 其他类型 → 明确报错，不再静默降级
pub(super) fn get_json_array_opt(
    args: &serde_json::Value,
    key: &str,
) -> Result<Option<Vec<serde_json::Value>>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Array(items)) => Ok(Some(items.clone())),
        Some(serde_json::Value::String(raw)) => {
            self::chapters::parse_json_array_lenient(raw, key).map(Some)
        }
        Some(other) => Err(format!(
            "{key} 需为 JSON 数组（字符串或数组形式），收到: {other}"
        )),
    }
}

pub(super) fn get_u32_opt(args: &serde_json::Value, key: &str) -> Option<u32> {
    match args.get(key) {
        Some(v) => v.as_u64().map(|n| n as u32).or_else(|| {
            v.as_str().and_then(|s| s.trim().parse::<u32>().ok())
        }),
        None => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 工具解析缓存：复用 / 脏检测 / 增量可见性
    #[test]
    fn tool_context_parse_reuses_and_sees_changes() {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "parse-cache-test").unwrap();
        let dir = tmp.path();

        let ch = crate::mutations::create_chapter(&crate::tools::ToolContext::new(dir.to_path_buf()).paths(), "第一章");
        let _ = ch;

        let ctx = ToolContext::new(dir.to_path_buf());

        // 首次：全量解析
        let first = ctx.parse().unwrap();
        assert!(ctx.parse().is_ok());

        // 无变化：零拷贝复用（同一份 Arc）
        let second = ctx.parse().unwrap();
        assert!(Arc::ptr_eq(&first, &second), "无变化时应复用同一份结果");

        // 写入新片段（磁盘变化）→ 下次解析能看到
        let paths = ProjectPaths::new(dir);
        let ch2 = crate::mutations::create_chapter(&paths, "第二章").unwrap();
        let sc2 = crate::mutations::create_scene(&paths, &ch2, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc2, "缓存检测的新段落。").unwrap();

        let third = ctx.parse().unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(
            third.ir.document.get_fragment(&story_ir::StableId::new_unchecked(&fid)).is_some(),
            "磁盘新增片段应被增量解析看到"
        );

        // clone 的 ctx 共享同一份缓存：同样能看到
        let cloned = ctx.clone();
        let fourth = cloned.parse().unwrap();
        assert!(Arc::ptr_eq(&third, &fourth), "ctx.clone() 应共享缓存");

        // threads.toml 变化（增量机制不追踪）→ 强制全量，能读到新线索
        crate::mutations::create_thread(&paths, "新线索", "描述", "main", None, None).unwrap();
        let fifth = ctx.parse().unwrap();
        assert!(
            fifth.threads.all().count() >= 1,
            "threads.toml 变化后应重读线索"
        );
    }
}
