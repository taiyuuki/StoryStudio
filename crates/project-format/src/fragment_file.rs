//! Fragment 文件格式 (RFC-0003 §5.3)
//!
//! 每个 Fragment 一个文件：YAML frontmatter + Markdown 正文。
//! 只存正文与锚点，不存结构归属信息（不含 scene 字段）。
//! id 必须等于文件名。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Fragment 文件的完整内容
#[derive(Debug, Clone)]
pub struct FragmentFile {
    /// YAML frontmatter
    pub frontmatter: FragmentFrontmatter,
    /// Markdown 正文
    pub text: String,
}

/// Fragment 的 YAML frontmatter
///
/// 注意：不含 scene 字段——Scene 归属由 structure.toml 决定（RFC-0003 §3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentFrontmatter {
    /// StableId，必须 == 文件名（不含扩展名）
    pub id: String,
    /// 锚定的 Semantic 实体出场
    #[serde(default)]
    pub anchors: Vec<AnchorEntry>,
}

/// frontmatter 中的锚点条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEntry {
    /// StableId 或 Symbol（Parser 解析）
    pub entity: String,
    /// appears | mentioned | referenced
    pub role: String,
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum FragmentFileError {
    #[error("缺少 frontmatter 分隔符 '---'")]
    NoFrontmatterDelim,

    #[error("frontmatter 解析失败: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("id 不匹配: 文件名期望 {expected}, frontmatter 实际 {actual}")]
    IdMismatch { expected: String, actual: String },

    #[error("无效的 anchor role: {0}（应为 appears/mentioned/referenced）")]
    InvalidRole(String),
}

// ============================================================================
// 解析与序列化
// ============================================================================

impl FragmentFile {
    /// 从字符串解析（frontmatter + 正文）
    pub fn parse(content: &str) -> Result<Self, FragmentFileError> {
        let (yaml, text) = split_frontmatter(content)?;
        let frontmatter: FragmentFrontmatter = serde_yaml::from_str(yaml)?;

        // 校验 anchors 的 role 合法性
        for anchor in &frontmatter.anchors {
            match anchor.role.as_str() {
                "appears" | "mentioned" | "referenced" => {}
                other => return Err(FragmentFileError::InvalidRole(other.to_string())),
            }
        }

        Ok(Self {
            frontmatter,
            text: text.to_string(),
        })
    }

    /// 带文件名校验的解析（RFC-0003 §5.3）
    pub fn parse_with_filename_check(
        content: &str,
        filename_stem: &str,
    ) -> Result<Self, FragmentFileError> {
        let file = Self::parse(content)?;

        if file.frontmatter.id != filename_stem {
            return Err(FragmentFileError::IdMismatch {
                expected: filename_stem.to_string(),
                actual: file.frontmatter.id,
            });
        }

        Ok(file)
    }

    /// 序列化为 Markdown 文件内容（YAML frontmatter + 正文）
    pub fn to_markdown(&self) -> String {
        let yaml = serde_yaml::to_string(&self.frontmatter).unwrap_or_default();
        // serde_yaml 末尾可能有换行，确保格式整洁
        let yaml = yaml.trim_end();
        format!("---\n{yaml}\n---\n\n{}", self.text)
    }

    /// 创建新 Fragment
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            frontmatter: FragmentFrontmatter {
                id: id.into(),
                anchors: Vec::new(),
            },
            text: text.into(),
        }
    }
}

/// 分离 YAML frontmatter 与 Markdown 正文
///
/// 格式：
/// ```text
/// ---
/// id: f_0001
/// anchors:
///   - entity: c_001
///     role: appears
/// ---
///
/// 正文内容...
/// ```
fn split_frontmatter(content: &str) -> Result<(&str, &str), FragmentFileError> {
    // 必须以 --- 开头
    let after_open = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or(FragmentFileError::NoFrontmatterDelim)?;

    // 找结束的 ---（行首）
    let end_pos = after_open
        .find("\n---")
        .ok_or(FragmentFileError::NoFrontmatterDelim)?;

    let yaml = &after_open[..end_pos];
    // 跳过 "\n---" 及其后的换行和空行
    let after_close = &after_open[end_pos + 4..]; // 4 = "\n---".len()
    let text = after_close.strip_prefix('\n').unwrap_or(after_close);
    let text = text.trim_start_matches('\n');

    Ok((yaml, text))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fragment_with_anchors() {
        let content = "---\nid: f_0001\nanchors:\n  - entity: c_8A91D4\n    role: appears\n  - entity: i_A7E902\n    role: mentioned\n---\n\n林风推开门，雨水顺着发梢滴落。\n";
        let file = FragmentFile::parse(content).unwrap();

        assert_eq!(file.frontmatter.id, "f_0001");
        assert_eq!(file.frontmatter.anchors.len(), 2);
        assert_eq!(file.frontmatter.anchors[0].entity, "c_8A91D4");
        assert_eq!(file.frontmatter.anchors[0].role, "appears");
        assert!(file.text.starts_with("林风推开门"));
    }

    #[test]
    fn parse_fragment_no_anchors() {
        let content = "---\nid: f_0002\n---\n\n这是一段没有锚点的正文。\n";
        let file = FragmentFile::parse(content).unwrap();

        assert_eq!(file.frontmatter.id, "f_0002");
        assert!(file.frontmatter.anchors.is_empty());
        assert_eq!(file.text, "这是一段没有锚点的正文。\n");
    }

    #[test]
    fn filename_check_passes() {
        let content = "---\nid: f_0001\n---\n\n正文\n";
        let file = FragmentFile::parse_with_filename_check(content, "f_0001").unwrap();
        assert_eq!(file.frontmatter.id, "f_0001");
    }

    #[test]
    fn filename_check_fails() {
        let content = "---\nid: f_0001\n---\n\n正文\n";
        let result = FragmentFile::parse_with_filename_check(content, "f_0002");
        assert!(matches!(result, Err(FragmentFileError::IdMismatch { .. })));
    }

    #[test]
    fn invalid_role_rejected() {
        let content =
            "---\nid: f_0001\nanchors:\n  - entity: c_001\n    role: exists\n---\n\n正文\n";
        let result = FragmentFile::parse(content);
        assert!(matches!(result, Err(FragmentFileError::InvalidRole(_))));
    }

    #[test]
    fn missing_frontmatter_rejected() {
        let content = "没有 frontmatter 的纯文本";
        let result = FragmentFile::parse(content);
        assert!(matches!(result, Err(FragmentFileError::NoFrontmatterDelim)));
    }

    #[test]
    fn roundtrip() {
        let file = FragmentFile::new("f_0001", "林风推开门。");
        let serialized = file.to_markdown();
        let reparsed = FragmentFile::parse(&serialized).unwrap();
        assert_eq!(reparsed.frontmatter.id, "f_0001");
        assert_eq!(reparsed.text, "林风推开门。");
    }

    #[test]
    fn roundtrip_with_anchors() {
        let mut file = FragmentFile::new("f_0001", "正文内容");
        file.frontmatter.anchors.push(AnchorEntry {
            entity: "c_001".to_string(),
            role: "appears".to_string(),
        });
        let serialized = file.to_markdown();
        let reparsed = FragmentFile::parse(&serialized).unwrap();
        assert_eq!(reparsed.frontmatter.anchors.len(), 1);
        assert_eq!(reparsed.frontmatter.anchors[0].entity, "c_001");
    }
}
