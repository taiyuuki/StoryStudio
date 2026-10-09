//! LLM 结构化输出的统一宽松解析。
//!
//! 此前「调 LLM → 期望 JSON」的容错逻辑分散在 ai_check / tools/chapters /
//! tools/entities / book_studio / tools/anchoring 五处，容错梯度与失败
//! 兜底各行其是（有的静默吞错、有的把原始文本倾泻给调用方）。本模块
//! 收敛为同一套梯度，失败一律返回 `Err`（附可行动提示），由上层决定
//! 重试还是把错误回传给 Agent——解析失败不再伪装成"空结果"。
//!
//! 统一容错梯度：剥思考链 → 直解 → 剥代码围栏 → 首尾括号截取 → 修复
//! 字符串内部未转义引号。修复策略保留原字符（转义为 `\"`），不改动内容。

use std::borrow::Cow;
use story_ai_runtime::provider::StopReason;

/// 剥除内联思考链（`<think>...</think>`）。
///
/// Provider 层按结构隔离 reasoning（thinking block / reasoning_content
/// 均不入 text），但部分 OpenAI 兼容端点把思考以内联标签混在 content 里。
/// 这段文本一旦进入下游：JSON 解析必败、纯文本消费方（摘要/改写）被
/// 英文推理污染，因此所有消费入口先过这里。未闭合的 `<think>`（截断）
/// 视为思考占满全文，整体剥除。
pub fn strip_think(raw: &str) -> Cow<'_, str> {
    if !raw.contains("<think>") {
        return Cow::Borrowed(raw);
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "<think>".len()..];
        match after.find("</think>") {
            Some(end) => {
                rest = &after[end + "</think>".len()..];
            }
            None => {
                // 未闭合：思考被 max_tokens 截断、后无正文——仅当思考块位于
                // 开头时按"全文是思考"剥除；出现在中段的孤立 <think> 更像
                // 正文里的字面量，保留原样交给解析层判定
                if out.trim().is_empty() {
                    return Cow::Owned(out);
                }
                out.push_str("<think>");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// 截断检测：`max_tokens` 截断的 JSON 几乎必然残缺，直接报错优于
/// 进入解析兜底（兜底会把残缺文本当解析失败倾泻或静默丢弃）。
pub fn ensure_not_truncated(stop_reason: StopReason, max_tokens: u32) -> Result<(), String> {
    if matches!(stop_reason, StopReason::MaxTokens) {
        Err(format!(
            "输出达到 max_tokens={max_tokens} 上限被截断，未得到完整 JSON；\
             请调大该模型档案的「输出上限」或缩减输入后重试"
        ))
    } else {
        Ok(())
    }
}

/// 宽松解析任意顶层 JSON（对象或数组均可）。
pub fn parse_value(raw: &str) -> Result<serde_json::Value, String> {
    let cleaned = strip_think(raw);
    let cleaned = cleaned.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
        return Ok(v);
    }
    for candidate in candidates(cleaned) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&candidate) {
            return Ok(v);
        }
    }
    // 引号修复后按同梯度再走一遍
    let repaired = repair_unescaped_quotes(cleaned);
    if cleaned != repaired {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repaired) {
            return Ok(v);
        }
        for candidate in candidates(&repaired) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&candidate) {
                return Ok(v);
            }
        }
    }
    Err(NOT_JSON.to_string())
}

/// 宽松解析 JSON 对象。
pub fn parse_object(raw: &str) -> Result<serde_json::Value, String> {
    let v = parse_value(raw).map_err(|_| format!("不是合法的 JSON 对象{HINT}"))?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(format!("顶层不是 JSON 对象（解析得到 {v} 类型的值）{HINT}"))
    }
}

/// 宽松解析 JSON 数组。
pub fn parse_array(raw: &str) -> Result<Vec<serde_json::Value>, String> {
    let v = parse_value(raw).map_err(|_| format!("不是合法的 JSON 数组{HINT}"))?;
    v.as_array()
        .cloned()
        .ok_or_else(|| format!("顶层不是 JSON 数组（解析得到 {v} 类型的值）{HINT}"))
}

const NOT_JSON: &str = "无法解析为 JSON（已尝试剥思考链/剥代码围栏/截取括号/修复引号）";
const HINT: &str = "。请只输出 JSON 本体，字符串内部的引号用「」或 \\\" 转义";

/// 生成解析候选：围栏内容 → 首尾括号截取。
/// 括号按 `{`/`[` 谁先出现在字符串字面量之外排优先级（取更外层）。
fn candidates(cleaned: &str) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(fenced) = extract_json_fence(cleaned) {
        v.push(fenced);
    }
    let brace_first = first_outside_strings(cleaned, '{', '[')
        .map(|c| c == '{')
        .unwrap_or(true);
    let mut push_span = |open: char, close: char| {
        if let (Some(s), Some(e)) = (cleaned.find(open), cleaned.rfind(close)) {
            if e > s {
                v.push(cleaned[s..=e].to_string());
            }
        }
    };
    if brace_first {
        push_span('{', '}');
        push_span('[', ']');
    } else {
        push_span('[', ']');
        push_span('{', '}');
    }
    v
}

/// 返回 `a`/`b` 中先出现在字符串字面量之外的那个字符（用于判断顶层结构）。
fn first_outside_strings(s: &str, a: char, b: char) -> Option<char> {
    let mut in_string = false;
    let mut escape = false;
    for c in s.chars() {
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
            _ if c == a => return Some(a),
            _ if c == b => return Some(b),
            _ => {}
        }
    }
    None
}

/// 从 ```json ... ``` 或 ``` ... ``` 围栏中提取内容。
pub fn extract_json_fence(s: &str) -> Option<String> {
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

/// 从第一个 `{` 到最后一个 `}` 的子串。
pub fn extract_json_object(s: &str) -> Option<String> {
    span(s, '{', '}')
}

/// 从第一个 `[` 到最后一个 `]` 的子串。
pub fn extract_json_array(s: &str) -> Option<String> {
    span(s, '[', ']')
}

fn span(s: &str, open: char, close: char) -> Option<String> {
    let start = s.find(open)?;
    let end = s.rfind(close)?;
    if end > start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

/// 修复 JSON 字符串值里未转义的 ASCII 双引号。
///
/// LLM 常在 summary/quote 里写 `来"升华"情绪` 这种嵌套引号破坏 JSON。
/// 状态机扫描，仅在「字符串值内部」的裸 `"` 转义为 `\"`；键名与结构性
/// 引号保持不动（判定依据：后面跳过空白是结构字符或字段名模式）。
pub fn repair_unescaped_quotes(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
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
                if is_structural_close(&chars, i + 1) {
                    out.push('"');
                    in_string = false;
                } else {
                    out.push_str("\\\"");
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// 判断 `from` 位置（跳过空白后）是否是 JSON 结构性字符，
/// 用于判定前一个 `"` 是不是字符串结束。
///
/// 注意 `"` 后紧跟 `"` 不算结构性（合法 JSON 中值结束符后必是
/// `, } ] :`）——那是值内部的裸引号，应转义。
fn is_structural_close(chars: &[char], from: usize) -> bool {
    let mut j = from;
    while j < chars.len()
        && (chars[j] == ' ' || chars[j] == '\n' || chars[j] == '\r' || chars[j] == '\t')
    {
        j += 1;
    }
    matches!(chars.get(j), Some(',') | Some('}') | Some(']') | Some(':'))
}

/// 收集所有花括号匹配区间（包括嵌套的内层对象），按闭合顺序排列。
/// 供「逐对象抢救」使用：整体解析失败时把能解析的子对象逐个救出。
pub fn collect_brace_spans(chars: &[char]) -> Vec<(usize, usize)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_variants() {
        assert_eq!(strip_think("<think>reasoning</think>{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_think("{\"a\":1}"), "{\"a\":1}");
        // 前后混正文 + 多段思考
        assert_eq!(
            strip_think("前缀<think>x</think>中段<think>y</think>后缀"),
            "前缀中段后缀"
        );
        // 未闭合且位于开头（思考占满全文、被截断）→ 整体剥除
        assert_eq!(strip_think("<think>only reasoning"), "");
        // 未闭合且出现在中段 → 更像正文里的字面量，保留原样
        assert_eq!(strip_think("{\"a\":1}<think>unfinished"), "{\"a\":1}<think>unfinished");
        // 正文中出现 <think> 字面量概率极低，且仅成对出现时才剥
        assert_eq!(strip_think("正文提到 <think> 标签"), "正文提到 <think> 标签");
    }

    #[test]
    fn parse_object_ladder() {
        // 直解
        assert_eq!(parse_object(r#"{"a":1}"#).unwrap()["a"], 1);
        // 围栏
        assert_eq!(
            parse_object("```json\n{\"a\": 2}\n```").unwrap()["a"],
            2
        );
        // 前后缀散文包裹
        assert_eq!(
            parse_object("好的，结果如下：{\"a\": 3} 以上。").unwrap()["a"],
            3
        );
        // 思考链污染
        assert_eq!(
            parse_object("<think>let me think</think>\n{\"a\": 4}").unwrap()["a"],
            4
        );
        // 字符串内未转义引号
        assert_eq!(
            parse_object(r#"{"summary":"他"低声"说"}"#).unwrap()["summary"],
            "他\"低声\"说"
        );
        // 顶层是数组 → Err
        assert!(parse_object("[1,2]").is_err());
        // 纯散文 → Err
        assert!(parse_object("这不是 JSON").is_err());
    }

    #[test]
    fn parse_array_ladder() {
        assert_eq!(parse_array(r#"[1,2]"#).unwrap().len(), 2);
        assert_eq!(parse_array("```json\n[\"a\"]\n```").unwrap()[0], "a");
        // 数组对象混合：散文 + 前缀说明
        assert_eq!(
            parse_array("结果：[{\"x\":1},{\"x\":2}] 完毕").unwrap().len(),
            2
        );
        // 思考链 + 围栏
        assert_eq!(
            parse_array("<think>hmm</think>[3]").unwrap().len(),
            1
        );
        assert!(parse_array("{\"a\":1}").is_err());
    }

    #[test]
    fn repair_preserves_structure() {
        let input = r#"{"key":"value","nested":"a"b"c"}"#;
        let repaired = repair_unescaped_quotes(input);
        assert!(
            repaired.contains(r#""key":"value""#),
            "键值对应完整保留: {repaired}"
        );
        assert!(
            repaired.contains(r#""nested":"a\"b\"c""#),
            "值内部引号应转义: {repaired}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&repaired).is_ok());
    }

    #[test]
    fn truncated_response_errors() {
        assert!(ensure_not_truncated(StopReason::MaxTokens, 1024).is_err());
        assert!(ensure_not_truncated(StopReason::EndTurn, 1024).is_ok());
    }
}
