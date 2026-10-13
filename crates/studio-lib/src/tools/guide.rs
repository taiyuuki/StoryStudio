//! Writing Guide Tools — 按作品类型获取专项创作指导
//!
//! 提供按需加载的类型专项创作指导。内置默认指导以 markdown 形式
//! 通过 `include_str!` 编译期嵌入；项目 `guides/` 目录可放置同名
//! `.md` 文件覆盖内置版本，实现系列专属定制。

use story_ai_runtime::{Tool, ToolDef};

use super::{get_str, ToolContext};

// ============================================================================
// 内置默认指导（编译期嵌入）
// ============================================================================

const WUXIA_GUIDE: &str = include_str!("guides/wuxia.md");
const MYSTERY_GUIDE: &str = include_str!("guides/mystery.md");
const ROMANCE_GUIDE: &str = include_str!("guides/romance.md");
const FANTASY_GUIDE: &str = include_str!("guides/fantasy.md");
const URBAN_GUIDE: &str = include_str!("guides/urban.md");
const SCIFI_GUIDE: &str = include_str!("guides/scifi.md");
const HISTORY_GUIDE: &str = include_str!("guides/history.md");
const GAMES_GUIDE: &str = include_str!("guides/games.md");
const MILITARY_GUIDE: &str = include_str!("guides/military.md");
const XIANXIA_GUIDE: &str = include_str!("guides/xianxia.md");
const SUPERNATURAL_GUIDE: &str = include_str!("guides/supernatural.md");
const LIGHTNOVEL_GUIDE: &str = include_str!("guides/lightnovel.md");
const REALISM_GUIDE: &str = include_str!("guides/realism.md");

/// 内置指导（name → 内容）
fn builtin_guide(name: &str) -> Option<&'static str> {
    match name {
        "wuxia" => Some(WUXIA_GUIDE),
        "mystery" => Some(MYSTERY_GUIDE),
        "romance" => Some(ROMANCE_GUIDE),
        "fantasy" => Some(FANTASY_GUIDE),
        "urban" => Some(URBAN_GUIDE),
        "scifi" => Some(SCIFI_GUIDE),
        "history" => Some(HISTORY_GUIDE),
        "games" => Some(GAMES_GUIDE),
        "military" => Some(MILITARY_GUIDE),
        "xianxia" => Some(XIANXIA_GUIDE),
        "supernatural" => Some(SUPERNATURAL_GUIDE),
        "lightnovel" => Some(LIGHTNOVEL_GUIDE),
        "realism" => Some(REALISM_GUIDE),
        _ => None,
    }
}

/// 内置指导的中文展示名（用于聚合输出的标题）
fn guide_display_name(name: &str) -> &str {
    match name {
        "wuxia" => "武侠",
        "mystery" => "悬疑/推理",
        "romance" => "言情",
        "fantasy" => "奇幻/玄幻",
        "urban" => "都市/异能",
        "scifi" => "科幻",
        "history" => "历史",
        "games" => "游戏/电竞",
        "military" => "军事",
        "xianxia" => "仙侠/修真",
        "supernatural" => "灵异/惊悚",
        "lightnovel" => "轻小说",
        "realism" => "现实",
        other => other,
    }
}

/// 类型标签 → 内置指导文件名。
///
/// 支持常见中文写法与英文别名。未命中的标签返回 `None`，
/// 由调用方进一步尝试按原始标签名查找项目自定义指导。
fn normalize_genre(tag: &str) -> Option<&'static str> {
    // 统一处理：去空白、转小写用于英文别名匹配
    let trimmed = tag.trim();
    let lower = trimmed.to_lowercase();
    match trimmed {
        "武侠" => Some("wuxia"),
        "悬疑" | "推理" | "悬疑推理" | "悬疑/推理" => Some("mystery"),
        "言情" => Some("romance"),
        "奇幻" | "玄幻" | "奇幻玄幻" | "奇幻/玄幻" => Some("fantasy"),
        "都市" | "都市异能" | "都市·异能" | "异能" => Some("urban"),
        "科幻" => Some("scifi"),
        "历史" | "历史架空" | "架空历史" | "历史古代" => Some("history"),
        "游戏" | "电竞" | "游戏电竞" | "游戏·电竞" | "竞技" => Some("games"),
        "军事" | "军旅" | "战争" => Some("military"),
        "仙侠" | "修真" | "修仙" => Some("xianxia"),
        "灵异" | "惊悚" | "志怪" => Some("supernatural"),
        "轻小说" => Some("lightnovel"),
        "现实" | "现实向" | "现实题材" | "温暖现实" => Some("realism"),
        _ => match lower.as_str() {
            "wuxia" => Some("wuxia"),
            "mystery" | "detective" => Some("mystery"),
            "romance" => Some("romance"),
            "fantasy" => Some("fantasy"),
            "urban" => Some("urban"),
            "scifi" | "sci-fi" | "science-fiction" => Some("scifi"),
            "history" => Some("history"),
            "games" | "esports" => Some("games"),
            "military" | "war" => Some("military"),
            "xianxia" => Some("xianxia"),
            "supernatural" | "horror" => Some("supernatural"),
            "lightnovel" | "light-novel" => Some("lightnovel"),
            "realism" => Some("realism"),
            _ => None,
        },
    }
}

/// 供建书流水线直接取用：按题材标签解析创作指导文本。
///
/// 优先项目 `guides/` 覆盖 → 内置默认；未知题材返回 None。
pub(crate) fn resolve_guide_text(
    genre: &str,
    project_root: Option<&std::path::Path>,
) -> Option<String> {
    let name = normalize_genre(genre)?;
    if let Some(root) = project_root {
        let custom = root.join("guides").join(format!("{name}.md"));
        if let Ok(text) = std::fs::read_to_string(&custom) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    builtin_guide(name).map(|s| s.to_string())
}

// ============================================================================
// GetWritingGuideTool
// ============================================================================

/// 获取类型专项创作指导。
///
/// LLM 传入 `genres`（类型标签数组，如 `["武侠", "悬疑"]`），工具按标签
/// 逐个查找指导内容（优先项目覆盖 → 内置默认），聚合为一份文本返回。
pub struct GetWritingGuideTool {
    ctx: ToolContext,
}

impl GetWritingGuideTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }

    /// 解析单个标签的指导内容。
    ///
    /// 查找顺序：项目 `guides/{normalized}.md` → 内置默认 → 项目 `guides/{原始标签}.md`。
    /// 返回 `(规范化名或原始标签, 内容)`；均未命中返回 `None`。
    fn resolve_guide(&self, tag: &str) -> Option<(String, String)> {
        let normalized = normalize_genre(tag);

        // 1. 优先项目覆盖（按规范化名）
        if let Some(name) = normalized {
            let path = self.ctx.paths().guide_file(name);
            if let Ok(content) = std::fs::read_to_string(&path) {
                return Some((name.to_string(), content));
            }
        }

        // 2. 内置默认
        if let Some(name) = normalized {
            if let Some(content) = builtin_guide(name) {
                return Some((name.to_string(), content.to_string()));
            }
        }

        // 3. 项目自定义（按原始标签名，支持非内置类型）
        let path = self.ctx.paths().guide_file(tag);
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some((tag.to_string(), content));
        }

        None
    }

    /// 读取 outline.toml 的 story.genre 作为默认类型标签。
    ///
    /// 混合题材（如「武侠悬疑」）先按整串解析；解析不到再按分隔符拆分，
    /// 仍不行则扫描已知题材词的子串命中。
    fn outline_genre_tags(&self) -> Vec<String> {
        let Ok(content) = std::fs::read_to_string(self.ctx.paths().outline_toml()) else {
            return Vec::new();
        };
        let Ok(outline) = story_project_format::OutlineFile::from_toml_str(&content) else {
            return Vec::new();
        };
        let Some(genre) = outline
            .story
            .genre
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
        else {
            return Vec::new();
        };

        // 整串能解析（含 normalize 表里的复合写法，如「奇幻玄幻」）
        if self.resolve_guide(genre).is_some() {
            return vec![genre.to_string()];
        }
        // 按常见分隔符拆分（如「悬疑/推理」「都市·异能」）
        let split: Vec<String> = genre
            .split(['/', '·', '、', '+', '，', ',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if split.len() > 1 {
            return split;
        }
        // 无分隔符的连写混合题材：扫描已知题材词
        split_compound_genre(genre)
    }
}

/// 在无分隔符的复合题材串（如「武侠悬疑」）中扫描已知题材词。
/// 按内置指导的固定顺序输出，保证结果稳定。
fn split_compound_genre(genre: &str) -> Vec<String> {
    const KNOWN_TAGS: &[&str] = &[
        "武侠",
        "悬疑",
        "推理",
        "言情",
        "奇幻",
        "玄幻",
        "都市",
        "异能",
        "科幻",
        "历史",
        "游戏",
        "电竞",
        "军事",
        "仙侠",
        "修真",
        "灵异",
        "惊悚",
        "轻小说",
        "现实",
    ];
    let hits: Vec<&str> = KNOWN_TAGS
        .iter()
        .copied()
        .filter(|t| genre.contains(t))
        .collect();
    // 至少命中两个才算「复合」；只命中一个时整串本来就是未知题材，交给上层报 missing
    if hits.len() >= 2 {
        hits.into_iter().map(str::to_string).collect()
    } else {
        vec![genre.to_string()]
    }
}

impl Tool for GetWritingGuideTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_writing_guide".into(),
            description: "获取指定作品类型的专项创作指导，作为通用创作心法的补充。\
                内置类型：武侠/悬疑推理/言情/奇幻玄幻/都市异能/科幻/历史/游戏电竞/军事/仙侠修真/\
                灵异惊悚/轻小说/现实题材。动笔前、遇到类型相关的叙事决策（如武侠的打斗写法、\
                悬疑的线索铺排、都市文的打脸节奏）时调用。支持多标签聚合：\
                如 [\"武侠\",\"悬疑\"] 会返回两份指导。省略 genres 时自动使用 outline.toml \
                的题材（story.genre）。项目 guides/ 目录可放置同名 .md 覆盖内置内容。"
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "genres": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "作品类型标签。常用值：武侠、悬疑、言情、奇幻、玄幻、仙侠、\
                            都市、异能、科幻、历史、游戏、电竞、军事、灵异、轻小说、现实。\
                            混合类型传多个标签；省略时使用 outline.story.genre。"
                    }
                }
            }),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // 接受数组；也容错接受单个字符串（转成单元素数组）
        let mut genres: Vec<String> =
            if let Some(arr) = arguments.get("genres").and_then(|v| v.as_array()) {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            } else if let Some(s) = arguments.get("genres").and_then(|v| v.as_str()) {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    Vec::new()
                } else {
                    vec![trimmed]
                }
            } else {
                // 兜底：尝试用 get_str 读（兼容极简调用）
                match get_str(arguments, "genres") {
                    Ok(s) => vec![s],
                    Err(_) => Vec::new(),
                }
            };

        // 缺参回退：outline.toml 的题材（story.genre）
        let mut used_outline_fallback = false;
        if genres.is_empty() {
            genres = self.outline_genre_tags();
            used_outline_fallback = !genres.is_empty();
        }

        if genres.is_empty() {
            return Err(
                "缺少参数: genres（类型标签数组，如 [\"武侠\",\"悬疑\"]；项目未设置 outline.toml 题材，无法自动回退）"
                    .to_string(),
            );
        }

        let mut sections: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        // 按解析出的指导文件名去重（"武侠" 与 "wuxia" 解析到同一份指导）
        let mut resolved_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for tag in &genres {
            match self.resolve_guide(tag) {
                Some((name, content)) => {
                    if resolved_names.insert(name.clone()) {
                        let display = guide_display_name(&name);
                        sections.push(format!("# {display} 创作指导\n\n{content}"));
                    }
                }
                None => missing.push(tag.clone()),
            }
        }

        let mut out = String::new();
        if used_outline_fallback {
            out.push_str(&format!(
                "（未传 genres，已按 outline.toml 题材回退：{}）\n\n",
                genres.join("、")
            ));
        }
        if !sections.is_empty() {
            out.push_str(&sections.join("\n\n---\n\n"));
        }
        if !missing.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!(
                "（以下类型暂无内置指导，可在项目 guides/ 目录放置对应 .md 文件自定义：{}）",
                missing.join("、")
            ));
        }

        if out.is_empty() {
            Err("未能获取任何创作指导".to_string())
        } else {
            Ok(out)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_genre_known_tags() {
        assert_eq!(normalize_genre("武侠"), Some("wuxia"));
        assert_eq!(normalize_genre("悬疑"), Some("mystery"));
        assert_eq!(normalize_genre("推理"), Some("mystery"));
        assert_eq!(normalize_genre("悬疑推理"), Some("mystery"));
        assert_eq!(normalize_genre("言情"), Some("romance"));
        assert_eq!(normalize_genre("奇幻"), Some("fantasy"));
        assert_eq!(normalize_genre("玄幻"), Some("fantasy"));
        // 第二批内置类型
        assert_eq!(normalize_genre("都市"), Some("urban"));
        assert_eq!(normalize_genre("都市异能"), Some("urban"));
        assert_eq!(normalize_genre("异能"), Some("urban"));
        assert_eq!(normalize_genre("科幻"), Some("scifi"));
        assert_eq!(normalize_genre("历史"), Some("history"));
        assert_eq!(normalize_genre("架空历史"), Some("history"));
        assert_eq!(normalize_genre("游戏"), Some("games"));
        assert_eq!(normalize_genre("电竞"), Some("games"));
        assert_eq!(normalize_genre("竞技"), Some("games"));
        assert_eq!(normalize_genre("军事"), Some("military"));
        assert_eq!(normalize_genre("仙侠"), Some("xianxia"));
        assert_eq!(normalize_genre("修真"), Some("xianxia"));
        assert_eq!(normalize_genre("灵异"), Some("supernatural"));
        assert_eq!(normalize_genre("轻小说"), Some("lightnovel"));
        assert_eq!(normalize_genre("现实"), Some("realism"));
        assert_eq!(normalize_genre("现实题材"), Some("realism"));
        // 英文别名
        assert_eq!(normalize_genre("wuxia"), Some("wuxia"));
        assert_eq!(normalize_genre("Mystery"), Some("mystery"));
        assert_eq!(normalize_genre("xianxia"), Some("xianxia"));
        assert_eq!(normalize_genre("Sci-Fi"), Some("scifi"));
        assert_eq!(normalize_genre("esports"), Some("games"));
    }

    #[test]
    fn normalize_genre_unknown_returns_none() {
        assert_eq!(normalize_genre("克苏鲁"), None);
        assert_eq!(normalize_genre("random"), None);
    }

    #[test]
    fn builtin_guides_present() {
        assert!(builtin_guide("wuxia").unwrap().contains("武侠"));
        assert!(builtin_guide("mystery").unwrap().contains("线索公平性"));
        assert!(builtin_guide("romance").unwrap().contains("化学反应"));
        assert!(builtin_guide("fantasy").unwrap().contains("世界观沉浸"));
        // 第二批内置指南各有实质内容
        assert!(builtin_guide("urban").unwrap().contains("打脸"));
        assert!(builtin_guide("scifi").unwrap().contains("设定"));
        assert!(builtin_guide("history").unwrap().contains("时代质感"));
        assert!(builtin_guide("games").unwrap().contains("比赛"));
        assert!(builtin_guide("military").unwrap().contains("代价"));
        assert!(builtin_guide("xianxia").unwrap().contains("境界"));
        assert!(builtin_guide("supernatural").unwrap().contains("规则"));
        assert!(builtin_guide("lightnovel").unwrap().contains("吐槽"));
        assert!(builtin_guide("realism").unwrap().contains("行业"));
        assert!(builtin_guide("nonexistent").is_none());
    }

    #[test]
    fn tool_returns_guide_for_single_genre() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["武侠"]});
        let result = tool.execute(&args).unwrap();
        assert!(result.contains("武侠"));
        assert!(result.contains("江湖氛围"));
    }

    #[test]
    fn tool_aggregates_multiple_genres() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["武侠", "悬疑"]});
        let result = tool.execute(&args).unwrap();
        assert!(result.contains("武侠"));
        assert!(result.contains("悬疑"));
        assert!(result.contains("江湖氛围"));
        assert!(result.contains("线索公平性"));
    }

    #[test]
    fn tool_reports_missing_genres() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["武侠", "克苏鲁"]});
        let result = tool.execute(&args).unwrap();
        // 武侠部分有内容
        assert!(result.contains("江湖氛围"));
        // 克苏鲁缺失被提示
        assert!(result.contains("克苏鲁"));
        assert!(result.contains("暂无内置指导"));
    }

    #[test]
    fn tool_accepts_single_string_genre() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        // 容错：genres 传成字符串而非数组
        let args = serde_json::json!({"genres": "武侠"});
        let result = tool.execute(&args).unwrap();
        assert!(result.contains("江湖氛围"));
    }

    #[test]
    fn tool_reads_project_override() {
        let dir = tempfile::tempdir().unwrap();
        let guides_dir = dir.path().join("guides");
        std::fs::create_dir_all(&guides_dir).unwrap();
        // 项目放置武侠覆盖文件
        std::fs::write(
            guides_dir.join("wuxia.md"),
            "# 自定义武侠指导\n\n这是项目专属内容。",
        )
        .unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["武侠"]});
        let result = tool.execute(&args).unwrap();
        // 应读到项目覆盖而非内置
        assert!(result.contains("项目专属内容"));
        assert!(!result.contains("江湖氛围"));
    }

    #[test]
    fn tool_reads_custom_genre_from_project() {
        let dir = tempfile::tempdir().unwrap();
        let guides_dir = dir.path().join("guides");
        std::fs::create_dir_all(&guides_dir).unwrap();
        // 克苏鲁无内置指导：验证按原始标签读取项目自定义文件
        std::fs::write(
            guides_dir.join("克苏鲁.md"),
            "# 克苏鲁创作指导\n\n不可名状的恐惧。",
        )
        .unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["克苏鲁"]});
        let result = tool.execute(&args).unwrap();
        assert!(result.contains("不可名状"));
    }

    #[test]
    fn tool_deduplicates_genres() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["武侠", "武侠", "wuxia"]});
        let result = tool.execute(&args).unwrap();
        // 武侠只应出现一次（标题只一个）
        assert_eq!(result.matches("武侠 创作指导").count(), 1);
    }

    #[test]
    fn tool_errors_on_empty_genres() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        // 无 outline.toml 也无 genres：仍报错
        let args = serde_json::json!({"genres": []});
        assert!(tool.execute(&args).is_err());
        assert!(tool.execute(&serde_json::json!({})).is_err());
    }

    #[test]
    fn tool_falls_back_to_outline_genre() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("outline.toml"),
            "[story]\ngenre = \"武侠\"\n",
        )
        .unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(
            result.contains("江湖氛围"),
            "应回退到 outline 题材: {result}"
        );
        assert!(
            result.contains("已按 outline.toml 题材回退"),
            "got: {result}"
        );
    }

    #[test]
    fn tool_falls_back_to_compound_outline_genre() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("outline.toml"),
            "[story]\ngenre = \"武侠悬疑\"\n",
        )
        .unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        // 无分隔符的复合题材：拆分为武侠 + 悬疑两份指导
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("江湖氛围"), "got: {result}");
        assert!(result.contains("线索公平性"), "got: {result}");
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);
        assert_eq!(tool.definition().name, "get_writing_guide");
    }
}
