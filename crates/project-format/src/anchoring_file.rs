//! anchoring.toml — 未锚定指代的处置台账
//!
//! 理解锚定每次会产出「名册外指代」（如"摆的人""写的人"），本文件
//! 把它们与处置决定（建档 / 合并到已有实体 / 忽略）一起落盘，
//! 让"决定不建档"也有据可查，下次锚定不再重复提示。
//!
//! 记录按 (fragment_id, quote) 唯一；正文改稿后不再包含的 quote
//! 由锚定流程自动清理（见 studio-lib mutations::sync_unresolved_refs）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// AnchoringFile
// ============================================================================

/// anchoring.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnchoringFile {
    #[serde(default, rename = "ref")]
    pub refs: Vec<UnresolvedRefDef>,
}

/// 单条未锚定指代记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedRefDef {
    /// 指代出现的片段
    pub fragment_id: String,
    /// 指代原文（如"摆的人"）
    pub quote: String,
    /// 锚定时模型给的提示（如"疑似新角色"），每次重锚定刷新
    #[serde(default)]
    pub hint: String,
    /// 处置状态：pending 待处置 | ignored 已忽略 | resolved 已归拢
    #[serde(default = "default_status")]
    pub status: String,
    /// resolved 时归拢到的实体 ID（建档或合并的目标）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

fn default_status() -> String {
    "pending".to_string()
}

impl UnresolvedRefDef {
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum AnchoringFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

impl AnchoringFile {
    pub fn from_toml_str(s: &str) -> Result<Self, AnchoringFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, AnchoringFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// 查找 (fragment_id, quote) 对应的记录下标
    pub fn index_of(&self, fragment_id: &str, quote: &str) -> Option<usize> {
        self.refs
            .iter()
            .position(|r| r.fragment_id == fragment_id && r.quote == quote)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full() {
        let toml_str = r#"
[[ref]]
fragment_id = "f_0001"
quote = "摆的人"
hint = "疑似凶手匿名指代"
status = "resolved"
target = "c_0009"

[[ref]]
fragment_id = "f_0001"
quote = "穿灰袍的女人"
hint = "疑似新角色"
"#;
        let file = AnchoringFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.refs.len(), 2);
        assert_eq!(file.refs[0].quote, "摆的人");
        assert_eq!(file.refs[0].status, "resolved");
        assert_eq!(file.refs[0].target.as_deref(), Some("c_0009"));
        // status 缺省为 pending
        assert_eq!(file.refs[1].status, "pending");
        assert!(file.refs[1].target.is_none());
        assert!(file.refs[1].is_pending());
    }

    #[test]
    fn roundtrip() {
        let file = AnchoringFile {
            refs: vec![UnresolvedRefDef {
                fragment_id: "f_0002".into(),
                quote: "写的人".into(),
                hint: "疑似凶手".into(),
                status: "ignored".into(),
                target: None,
            }],
        };
        let serialized = file.to_toml_string().unwrap();
        let reparsed = AnchoringFile::from_toml_str(&serialized).unwrap();
        assert_eq!(reparsed.refs.len(), 1);
        assert_eq!(reparsed.refs[0].quote, "写的人");
        assert_eq!(reparsed.refs[0].status, "ignored");
        // target 为 None 时不应序列化
        assert!(!serialized.contains("target"));
    }

    #[test]
    fn empty_file() {
        let file = AnchoringFile::from_toml_str("").unwrap();
        assert!(file.refs.is_empty());
    }

    #[test]
    fn index_of_finds_pair() {
        let file = AnchoringFile {
            refs: vec![UnresolvedRefDef {
                fragment_id: "f_0001".into(),
                quote: "摆的人".into(),
                hint: String::new(),
                status: "pending".into(),
                target: None,
            }],
        };
        assert_eq!(file.index_of("f_0001", "摆的人"), Some(0));
        assert_eq!(file.index_of("f_0001", "写的人"), None);
        assert_eq!(file.index_of("f_0002", "摆的人"), None);
    }
}
