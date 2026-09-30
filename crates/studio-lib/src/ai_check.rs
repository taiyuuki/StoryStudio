//! AI 写作特征检测（AI 味诊断）
//!
//! 灵感来自 dbs-ai-check：扫描小说正文中的 AI 生成痕迹，输出结构化检测报告。
//! 设计为独立的能力层，**不**纳入 Compiler 的确定性 Diagnostic 体系——
//! AI 检测是概率性的（同一文本两次结果可能不同），与确定性校验严格隔离。
//!
//! 体裁适配：针对「小说正文」从 dbs-ai-check 的 22 条特征中精选 11 条，
//! 并新增 7 条小说高频 AI 味，共 18 条。

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use story_ai_runtime::provider::{GenerateRequest, Message, StopReason};
use story_ai_runtime::AiProvider;
use story_project_format::ProjectPaths;

use crate::error::StudioError;

// ============================================================================
// 数据结构
// ============================================================================

/// 严重度分级（对应 dbs-ai-check 的 🔴 ⚠️ 💡）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AiCheckSeverity {
    /// 🔴 强信号 — 几乎只有 AI 会这样做
    Strong,
    /// ⚠️ 中信号 — AI 高频但人也可能做
    Medium,
    /// 💡 弱信号 — 需要结合上下文判断
    Weak,
}

/// 单处 AI 特征命中
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCheckItem {
    /// 命中处原文引用（让用户一眼看到是哪段话有问题）
    pub quote: String,
    /// 特征编号（如 "A"、"14"、"19"；解析失败时为 "parse_error"）
    pub feature_id: String,
    /// 特征名称（如 "形容词堆砌的场景描写"）
    pub feature_name: String,
    /// 严重度
    pub severity: AiCheckSeverity,
    /// 一两句话说明这段话的问题，具体、直接、不堆术语
    pub explanation: String,
}

/// AI 检测报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCheckReport {
    /// 被检测的 fragment ID；纯文本检测时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fragment_id: Option<String>,
    /// 命中的特征列表（按文本顺序）
    pub items: Vec<AiCheckItem>,
    /// 一两句话概括最突出的问题，不罗列
    pub summary: String,
    /// 本次检测的 token 消耗
    pub tokens_used: usize,
}

impl AiCheckReport {
    /// 命中特征数
    pub fn hit_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.feature_id != "parse_error")
            .count()
    }
}

// ============================================================================
// Prompt
// ============================================================================

// 检测 system prompt（18 条小说适配特征 + 严重度 + 误伤警告 + 输出格式）
// 已文件化：src/prompts/ai_check.md

/// LLM 返回的 JSON 结构（用于反序列化校验）
#[derive(Debug, Deserialize)]
struct LlmReport {
    items: Vec<LlmItem>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize)]
struct LlmItem {
    quote: String,
    feature_id: String,
    feature_name: String,
    severity: String,
    explanation: String,
}

// ============================================================================
// 核心函数
// ============================================================================

/// 检测指定 fragment 的 AI 写作特征
///
/// 流程：读取 fragment（剥 frontmatter 取正文）→ 构造请求 → retry_generate → 解析 JSON
pub fn check_fragment(
    dir: &Path,
    fragment_id: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckReport, StudioError> {
    let paths = ProjectPaths::new(dir);
    let text = read_fragment_body(&paths, fragment_id)?;
    let (items, summary, tokens_used) = check_text_inner(&text, provider)?;
    Ok(AiCheckReport {
        fragment_id: Some(fragment_id.to_string()),
        items,
        summary,
        tokens_used,
    })
}

/// 检测纯文本的 AI 写作特征
pub fn check_text(
    text: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckReport, StudioError> {
    let (items, summary, tokens_used) = check_text_inner(text, provider)?;
    Ok(AiCheckReport {
        fragment_id: None,
        items,
        summary,
        tokens_used,
    })
}

/// 实际检测逻辑，返回 (命中项, 概括, token 消耗)
fn check_text_inner(
    text: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<(Vec<AiCheckItem>, String, usize), StudioError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(StudioError::Ai("待检测文本为空".to_string()));
    }

    let char_count = trimmed.chars().count();
    // 输出空间：检测报告本身不长，但思考模式会占用输出配额（本端点实测
    // 可达 ~10k），下限 16384 保证常规片段（~1500 字）不被截断；
    // 长片段按输入规模放大，上限 32768
    let estimated_output_tokens = (char_count.saturating_mul(3)) + 1024;
    let max_tokens = estimated_output_tokens.clamp(16384, 32768);

    let request = GenerateRequest {
        system: crate::prompts::AI_CHECK.to_string(),
        messages: vec![Message::user(format!("请检测以下小说正文：\n\n{trimmed}"))],
        model: String::new(),
        temperature: 0.3, // 检测要稳定可复现，区别于创作的 0.7
        max_tokens,
        tools: Vec::new(),
    };

    let response = crate::tools::retry_generate(provider, &request)
        .map_err(|e| StudioError::Ai(format!("AI 检测调用失败: {e}")))?;

    // 截断检测：JSON 不完整时不应给出残缺报告
    if matches!(response.stop_reason, StopReason::MaxTokens) {
        return Err(StudioError::Ai(format!(
            "检测结果被截断（达到 max_tokens={max_tokens} 上限）。\
            原文 {char_count} 字过长，建议用 split_fragment 拆分后再检测。"
        )));
    }

    let tokens_used = response.usage.total();
    let items = parse_report(&response.text);
    let summary = extract_summary(&response.text).unwrap_or_else(|| default_summary(&items));

    Ok((items, summary, tokens_used))
}

/// 读取 fragment 文件并剥除 frontmatter，返回纯正文
fn read_fragment_body(paths: &ProjectPaths, fragment_id: &str) -> Result<String, StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::NotFound(format!("无效的 fragment ID: {fragment_id}")))?;
    if !frag_path.exists() {
        return Err(StudioError::NotFound(format!(
            "片段 '{fragment_id}' 不存在"
        )));
    }
    let content = std::fs::read_to_string(&frag_path)
        .map_err(|e| StudioError::Io(format!("读取片段失败: {e}")))?;

    // 复用 ReadFragmentTextTool 的 frontmatter 剥离逻辑
    let body = if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            content[3 + end + 3..]
                .trim_start_matches('\n')
                .trim()
                .to_string()
        } else {
            content.trim().to_string()
        }
    } else {
        content.trim().to_string()
    };

    if body.is_empty() {
        return Err(StudioError::Ai(format!("片段 '{fragment_id}' 正文为空")));
    }
    Ok(body)
}

/// 解析 LLM 返回的报告文本为 AiCheckItem 列表
///
/// 健壮性策略：
/// 1. 先尝试直接 JSON 解析
/// 2. 失败则尝试从 ```json ... ``` 围栏提取
/// 3. 仍失败则把原始文本包成一个 parse_error item 返回（不直接报错）
fn parse_report(raw: &str) -> Vec<AiCheckItem> {
    let cleaned = raw.trim();

    // 尝试 1：直接解析
    if let Some(report) = try_parse_json(cleaned) {
        return report_to_items(report);
    }

    // 尝试 2：提取 ```json ... ``` 围栏
    if let Some(json_str) = extract_json_fence(cleaned) {
        if let Some(report) = try_parse_json(&json_str) {
            return report_to_items(report);
        }
    }

    // 尝试 3：寻找第一个 { 到最后一个 } 的子串
    if let Some(json_str) = extract_json_object(cleaned) {
        if let Some(report) = try_parse_json(&json_str) {
            return report_to_items(report);
        }
    }

    // 尝试 4：LLM 常在字符串值里嵌未转义双引号导致 JSON 破裂，
    // 把字符串内部的 ASCII 双引号替换成中文引号后再试一次。
    let repaired = repair_unescaped_quotes(cleaned);
    if let Some(report) = try_parse_any(&repaired) {
        return report_to_items(report);
    }

    // 尝试 5：宽松提取——逐个对象抢救，只救出能解析的 item（丢 summary 无所谓）
    let loose_items = loose_extract_items(cleaned);
    if !loose_items.is_empty() {
        return loose_items;
    }

    // 全部失败：降级为原始文本展示
    vec![AiCheckItem {
        quote: String::new(),
        feature_id: "parse_error".to_string(),
        feature_name: "报告解析失败".to_string(),
        severity: AiCheckSeverity::Weak,
        explanation: format!("LLM 返回的内容无法解析为 JSON，原始内容：\n\n{raw}"),
    }]
}

fn try_parse_json(s: &str) -> Option<LlmReport> {
    serde_json::from_str::<LlmReport>(s).ok()
}

/// 从 ```json ... ``` 或 ``` ... ``` 围栏中提取内容
fn extract_json_fence(s: &str) -> Option<String> {
    let start_markers = ["```json", "```JSON", "```"];
    for marker in &start_markers {
        if let Some(start_idx) = s.find(marker) {
            let after = &s[start_idx + marker.len()..];
            if let Some(end_idx) = after.find("```") {
                return Some(after[..end_idx].trim().to_string());
            }
        }
    }
    None
}

/// 从第一个 { 到最后一个 } 的子串
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

/// 修复 JSON 字符串值里未转义的 ASCII 双引号。
///
/// LLM 常在 summary/explanation 里写 `来"升华"情绪` 这种嵌套引号，破坏 JSON。
/// 策略：状态机扫描，仅在「字符串值内部」把成对的 ASCII 双引号替换成中文引号「」。
/// 键名的引号、结构性的引号保持不动。
fn repair_unescaped_quotes(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escape = false;
    let mut pending_open_quote = true; // 字符串内的下一个裸 " 视作开引号「
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
                pending_open_quote = true;
            }
            i += 1;
            continue;
        }

        // in_string == true
        if escape {
            out.push(c);
            escape = false;
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                escape = true;
            }
            '"' => {
                // 判断这个 " 是字符串的结束引号，还是值内部未转义的引号。
                // 启发式：看后面跳过空白后是不是 JSON 结构字符（, } ] :）或字段名模式。
                if is_structural_close(&chars, i + 1) {
                    // 真正的字符串结束
                    out.push('"');
                    in_string = false;
                } else {
                    // 值内部的裸引号 → 替换成中文引号
                    out.push(if pending_open_quote { '「' } else { '」' });
                    pending_open_quote = !pending_open_quote;
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// 判断 `from` 位置（跳过空白后）是否是 JSON 结构性字符，用于判定前一个 " 是不是字符串结束。
fn is_structural_close(chars: &[char], from: usize) -> bool {
    let mut j = from;
    while j < chars.len()
        && (chars[j] == ' ' || chars[j] == '\n' || chars[j] == '\r' || chars[j] == '\t')
    {
        j += 1;
    }
    match chars.get(j) {
        // 值结束符：逗号、闭合括号
        Some(',') | Some('}') | Some(']') => true,
        // 键值分隔符："key" 后面跟冒号，说明这个 " 是键名结束
        Some(':') => true,
        // 新字段："key": value 前一个字符串结束
        Some('"') => true,
        _ => false,
    }
}

/// 宽松提取：扫描所有花括号匹配的 {...} 子串（含嵌套），逐个尝试解析为单个 item。
/// 能救一个是一个，丢掉 summary 也无所谓。非 item 结构（如整个 report 对象）会自然解析失败被跳过。
fn loose_extract_items(input: &str) -> Vec<AiCheckItem> {
    let chars: Vec<char> = input.chars().collect();
    // 收集所有 {..} 匹配区间（按 { 出现的位置栈式匹配）
    let candidates = collect_brace_spans(&chars);
    let mut items = Vec::new();
    for (s, e) in candidates {
        let obj: String = chars[s..=e].iter().collect();
        // 先原样解析，失败则修复引号后重试
        let parsed = serde_json::from_str::<LlmItem>(&obj)
            .or_else(|_| serde_json::from_str::<LlmItem>(&repair_unescaped_quotes(&obj)));
        if let Ok(item) = parsed {
            items.push(AiCheckItem {
                quote: item.quote,
                feature_id: item.feature_id,
                feature_name: item.feature_name,
                severity: parse_severity(&item.severity),
                explanation: item.explanation,
            });
        }
    }
    items
}

/// 收集所有花括号匹配区间（包括嵌套的内层对象）。
/// 返回 (start, end) 索引对列表，按闭合顺序排列。
fn collect_brace_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_string = false;
    let mut escape = false;

    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push(i),
            '}' => {
                if let Some(start) = stack.pop() {
                    spans.push((start, i));
                }
            }
            _ => {}
        }
    }
    spans
}

fn report_to_items(report: LlmReport) -> Vec<AiCheckItem> {
    report
        .items
        .into_iter()
        .map(|item| AiCheckItem {
            quote: item.quote,
            feature_id: item.feature_id,
            feature_name: item.feature_name,
            severity: parse_severity(&item.severity),
            explanation: item.explanation,
        })
        .collect()
}

fn parse_severity(s: &str) -> AiCheckSeverity {
    match s.to_lowercase().as_str() {
        "strong" => AiCheckSeverity::Strong,
        "medium" => AiCheckSeverity::Medium,
        _ => AiCheckSeverity::Weak,
    }
}

/// 从原始文本中提取 summary（解析失败时用）
fn extract_summary(raw: &str) -> Option<String> {
    let cleaned = raw.trim();
    // 先原样尝试（含围栏/对象提取），失败则修复未转义引号后再试
    if let Some(report) = try_parse_any(cleaned) {
        return Some(report.summary);
    }
    let repaired = repair_unescaped_quotes(cleaned);
    let report = try_parse_any(&repaired)?;
    Some(report.summary)
}

/// 依次尝试：直接解析 → 围栏提取 → 对象提取
fn try_parse_any(s: &str) -> Option<LlmReport> {
    try_parse_json(s)
        .or_else(|| extract_json_fence(s).and_then(|j| try_parse_json(&j)))
        .or_else(|| extract_json_object(s).and_then(|j| try_parse_json(&j)))
}

fn default_summary(items: &[AiCheckItem]) -> String {
    let real: Vec<_> = items
        .iter()
        .filter(|i| i.feature_id != "parse_error")
        .collect();
    if real.is_empty() {
        return "未发现明显 AI 特征".to_string();
    }
    let strong = real
        .iter()
        .filter(|i| i.severity == AiCheckSeverity::Strong)
        .count();
    if strong > 0 {
        format!("最突出的问题有 {strong} 处强信号，建议优先处理")
    } else {
        format!("共命中 {} 处特征，多为中弱信号", real.len())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户真实案例：LLM 在 summary 里嵌了未转义双引号 `来"升华"情绪`，
    /// 破坏了整个 JSON 结构。修复后应能正确提取出 2 个 item。
    #[test]
    fn test_parse_unescaped_quotes_in_summary() {
        let raw = r#"```json
{
  "items": [
    {
      "quote": "画师做的事情，就是把模糊的东西固定住。",
      "feature_id": "13",
      "feature_name": "每个段落都有收束金句",
      "severity": "Medium",
      "explanation": "在画遗像的情节末尾突然拔高，用一句工整的格言式总结收尾。"
    },
    {
      "quote": "他来了，画了两个月的画，赚了一点钱，现在要走了。",
      "feature_id": "3",
      "feature_name": "匀速排比",
      "severity": "Medium",
      "explanation": "三个结构对称、长度相近的分句机械地罗列主角的经历。"
    }
  ],
  "summary": "整体口语化、毛边感足，AI 味很淡。仅在两处段落收尾暴露了 AI 习惯：喜欢用格言式金句或匀速排比来"升华"情绪。"
}
```"#;

        let items = parse_report(raw);

        // 不应出现 parse_error
        assert!(
            !items.iter().any(|i| i.feature_id == "parse_error"),
            "应成功解析，不应降级为 parse_error；实际：{items:?}"
        );
        // 应提取出 2 个 item
        assert_eq!(
            items.len(),
            2,
            "应解析出 2 个 item；实际 {} 个",
            items.len()
        );
        assert_eq!(items[0].feature_id, "13");
        assert_eq!(items[0].severity, AiCheckSeverity::Medium);
        assert_eq!(items[1].feature_id, "3");
        assert_eq!(
            items[1].quote,
            "他来了，画了两个月的画，赚了一点钱，现在要走了。"
        );
    }

    /// summary 也应被修复后提取出来
    #[test]
    fn test_extract_summary_with_unescaped_quotes() {
        let raw = r#"{"items":[],"summary":"喜欢来"升华"情绪。"}"#;
        let summary = extract_summary(raw);
        assert!(summary.is_some(), "summary 应被修复提取");
        assert!(summary.unwrap().contains("升华"));
    }

    /// 正常 JSON 应不受影响
    #[test]
    fn test_parse_normal_json() {
        let raw = r#"{"items":[{"quote":"x","feature_id":"A","feature_name":"test","severity":"Strong","explanation":"y"}],"summary":"ok"}"#;
        let items = parse_report(raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature_id, "A");
        assert_eq!(items[0].severity, AiCheckSeverity::Strong);
    }

    /// repair_unescaped_quotes 不应破坏键名和结构引号
    #[test]
    fn test_repair_preserves_structure() {
        let input = r#"{"key":"value","nested":"a"b"c"}"#;
        let repaired = repair_unescaped_quotes(input);
        // 结构性引号和键名引号应保持为 ASCII "
        assert!(
            repaired.contains(r#""key":"value""#),
            "键值对应完整保留: {repaired}"
        );
        // 值内部的引号应被替换成中文引号
        assert!(repaired.contains('「'), "内部引号应替换为「: {repaired}");
        assert!(repaired.contains('」'), "内部引号应替换为」: {repaired}");
    }

    /// 完全无法解析的内容应降级为 parse_error
    #[test]
    fn test_parse_garbage_fallback() {
        let items = parse_report("这不是 JSON，也没有花括号");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature_id, "parse_error");
    }
}
