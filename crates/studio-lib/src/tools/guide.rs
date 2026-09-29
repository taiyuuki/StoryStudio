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

/// 内置指导（name → 内容）
fn builtin_guide(name: &str) -> Option<&'static str> {
    match name {
        "wuxia" => Some(WUXIA_GUIDE),
        "mystery" => Some(MYSTERY_GUIDE),
        "romance" => Some(ROMANCE_GUIDE),
        "fantasy" => Some(FANTASY_GUIDE),
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
        _ => match lower.as_str() {
            "wuxia" => Some("wuxia"),
            "mystery" | "detective" => Some("mystery"),
            "romance" => Some("romance"),
            "fantasy" | "xianxia" => Some("fantasy"),
            _ => None,
        },
    }
}

/// 供建书流水线直接取用：按题材标签解析创作指导文本。
///
/// 优先项目 `guides/` 覆盖 → 内置默认；未知题材返回 None。
pub(crate) fn resolve_guide_text(genre: &str, project_root: Option<&std::path::Path>) -> Option<String> {
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
}

impl Tool for GetWritingGuideTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_writing_guide".into(),
            description: "获取指定作品类型的专项创作指导（武侠/悬疑推理/言情/奇幻玄幻），\
                作为通用创作心法的补充。动笔前、遇到类型相关的叙事决策（如武侠的打斗写法、\
                悬疑的线索铺排）时调用。支持多标签聚合：如 [\"武侠\",\"悬疑\"] 会返回两份指导。\
                项目 guides/ 目录可放置同名 .md 覆盖内置内容。"
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "genres": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "作品类型标签。常用值：武侠、悬疑、推理、言情、奇幻、玄幻。\
                            可从 outline.story.genre 推断；混合类型传多个标签。"
                    }
                },
                "required": ["genres"]
            }),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // 接受数组；也容错接受单个字符串（转成单元素数组）
        let genres: Vec<String> =
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

        if genres.is_empty() {
            return Err("缺少参数: genres（类型标签数组，如 [\"武侠\",\"悬疑\"]）".to_string());
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
        // 英文别名
        assert_eq!(normalize_genre("wuxia"), Some("wuxia"));
        assert_eq!(normalize_genre("Mystery"), Some("mystery"));
        assert_eq!(normalize_genre("xianxia"), Some("fantasy"));
    }

    #[test]
    fn normalize_genre_unknown_returns_none() {
        assert_eq!(normalize_genre("科幻"), None);
        assert_eq!(normalize_genre("历史"), None);
        assert_eq!(normalize_genre("random"), None);
    }

    #[test]
    fn builtin_guides_present() {
        assert!(builtin_guide("wuxia").unwrap().contains("武侠"));
        assert!(builtin_guide("mystery").unwrap().contains("线索公平性"));
        assert!(builtin_guide("romance").unwrap().contains("化学反应"));
        assert!(builtin_guide("fantasy").unwrap().contains("世界观沉浸"));
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

        let args = serde_json::json!({"genres": ["武侠", "科幻"]});
        let result = tool.execute(&args).unwrap();
        // 武侠部分有内容
        assert!(result.contains("江湖氛围"));
        // 科幻缺失被提示
        assert!(result.contains("科幻"));
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
        std::fs::write(
            guides_dir.join("科幻.md"),
            "# 科幻创作指导\n\n硬科幻的设定自洽。",
        )
        .unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);

        let args = serde_json::json!({"genres": ["科幻"]});
        let result = tool.execute(&args).unwrap();
        assert!(result.contains("硬科幻"));
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

        let args = serde_json::json!({"genres": []});
        assert!(tool.execute(&args).is_err());
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = GetWritingGuideTool::new(ctx);
        assert_eq!(tool.definition().name, "get_writing_guide");
    }
}
