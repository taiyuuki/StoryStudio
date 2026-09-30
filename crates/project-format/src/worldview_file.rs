//! worldview/ — 世界观 (RFC-0016 同级设计层)
//!
//! 作者的世界设定文档。地理与势力已统一为实体
//!（`entities/locations/`、`entities/organizations/`），
//! 本目录只保留没有实体对应的部分，一节一文件：
//!
//! | 文件 | 内容 |
//! |------|------|
//! | `concepts.toml` | 基础概念（灵气、天道、纪元、种族等世界基本设定） |
//! | `systems.toml` | 体系设定（修炼、魔法、科技等力量体系） |
//! | `timeline.toml` | 故事内历法（有序纪元表） |
//!
//! 与大纲一样，不参与 Compiler 检查，是设计层的记录。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// 分节文件
// ============================================================================

/// worldview/concepts.toml — 基础概念
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptsFile {
    /// [[concepts]] 基础概念列表
    #[serde(default)]
    pub concepts: Vec<ConceptDef>,
}

/// worldview/systems.toml — 体系设定
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemsFile {
    /// [[systems]] 体系设定列表
    #[serde(default)]
    pub systems: Vec<SystemDef>,
}

/// worldview/timeline.toml — 故事内历法
///
/// 纪元为有序列表：故事时间锚（如"灵历3217年"）按纪元在列表中的位置
/// 折算为可比较序数。未声明纪元的项目，纯数字年份视为单一纪元。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineFile {
    /// 有序纪元名（如 ["创世", "灵历", "开元"]）
    #[serde(default)]
    pub eras: Vec<String>,
}

/// 世界观分节文件的 TOML 解析/序列化能力（五个文件共用同一套约定）
pub trait WorldviewSectionFile: Default + Sized {
    fn from_toml_str(s: &str) -> Result<Self, WorldviewFileError>;
    fn to_toml_string(&self) -> Result<String, WorldviewFileError>;
}

macro_rules! impl_section_file {
    ($t:ty) => {
        impl WorldviewSectionFile for $t {
            fn from_toml_str(s: &str) -> Result<Self, WorldviewFileError> {
                Ok(toml::from_str(s)?)
            }

            fn to_toml_string(&self) -> Result<String, WorldviewFileError> {
                Ok(toml::to_string_pretty(self)?)
            }
        }
    };
}

impl_section_file!(ConceptsFile);
impl_section_file!(SystemsFile);
impl_section_file!(TimelineFile);

// ============================================================================
// 条目定义
// ============================================================================

/// [[concepts]] 基础概念
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptDef {
    pub name: String,
    /// 概念分类（如「世界规则」「纪元」「种族」「神灵」）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 详细说明（展开描述）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// [[systems]] 体系设定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemDef {
    pub name: String,
    /// 体系类型：cultivation（修炼）/ magic（魔法）/ technology（科技）/ power（异能）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 等级划分（如「炼气→筑基→金丹→元婴→化神」）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<String>,
    /// 规则与代价
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum WorldviewFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_concepts_file() {
        let toml_str = r#"
[[concepts]]
name = "灵气"
category = "世界规则"
summary = "天地间的基础能量"
details = "灵气浓度决定修炼速度"

[[concepts]]
name = "天道"
category = "世界规则"
"#;
        let file = ConceptsFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.concepts.len(), 2);
        assert_eq!(file.concepts[0].name, "灵气");
        assert_eq!(file.concepts[0].category.as_deref(), Some("世界规则"));
        assert_eq!(
            file.concepts[0].details.as_deref(),
            Some("灵气浓度决定修炼速度")
        );
        assert_eq!(file.concepts[1].summary, None);
    }

    #[test]
    fn parse_systems_file() {
        let toml_str = r#"
[[systems]]
name = "修炼体系"
kind = "cultivation"
levels = "炼气→筑基→金丹→元婴→化神"
rules = "每突破一层需要渡劫"
summary = "以灵气为基础的进阶体系"
"#;
        let file = SystemsFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.systems.len(), 1);
        assert_eq!(file.systems[0].name, "修炼体系");
        assert_eq!(file.systems[0].kind.as_deref(), Some("cultivation"));
        assert!(file.systems[0].levels.as_deref().unwrap().contains("筑基"));
    }

    #[test]
    fn parse_timeline_file() {
        let toml_str = r#"eras = ["创世", "灵历", "开元"]"#;
        let file = TimelineFile::from_toml_str(toml_str).unwrap();
        assert_eq!(file.eras, vec!["创世", "灵历", "开元"]);
    }

    #[test]
    fn empty_input_yields_default_for_every_section() {
        assert!(ConceptsFile::from_toml_str("").unwrap().concepts.is_empty());
        assert!(SystemsFile::from_toml_str("").unwrap().systems.is_empty());
        assert!(TimelineFile::from_toml_str("").unwrap().eras.is_empty());
    }

    #[test]
    fn roundtrip_all_sections() {
        let concepts = ConceptsFile {
            concepts: vec![ConceptDef {
                name: "灵气".into(),
                category: Some("世界规则".into()),
                summary: Some("天地间的基础能量".into()),
                details: None,
            }],
        };
        let parsed = ConceptsFile::from_toml_str(&concepts.to_toml_string().unwrap()).unwrap();
        assert_eq!(parsed.concepts.len(), 1);
        assert_eq!(parsed.concepts[0].name, "灵气");
        assert_eq!(parsed.concepts[0].details, None);

        let systems = SystemsFile {
            systems: vec![SystemDef {
                name: "魔法体系".into(),
                kind: Some("magic".into()),
                levels: Some("学徒→法师→大法师→魔导师".into()),
                rules: None,
                summary: Some("元素驱动的力量".into()),
            }],
        };
        let parsed = SystemsFile::from_toml_str(&systems.to_toml_string().unwrap()).unwrap();
        assert_eq!(parsed.systems.len(), 1);
        assert_eq!(parsed.systems[0].kind.as_deref(), Some("magic"));
        assert_eq!(parsed.systems[0].rules, None);

        let timeline = TimelineFile {
            eras: vec!["创世".into(), "灵历".into()],
        };
        let parsed = TimelineFile::from_toml_str(&timeline.to_toml_string().unwrap()).unwrap();
        assert_eq!(parsed.eras, vec!["创世", "灵历"]);
    }
}
