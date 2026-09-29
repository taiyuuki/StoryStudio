//! worldview.toml — 世界观 (RFC-0016 同级设计层)
//!
//! 作者的世界设定文档——基础概念、体系设定、地理秘境、势力。
//! 与大纲一样，不参与 Compiler 检查，是设计层的记录。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// WorldviewFile
// ============================================================================

/// worldview.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldviewFile {
    /// 基础概念（灵气、天道、纪元、种族等世界基本设定）
    #[serde(default, rename = "concepts")]
    pub concepts: Vec<ConceptDef>,
    /// 体系设定（修炼、魔法、科技等力量体系）
    #[serde(default, rename = "systems")]
    pub systems: Vec<SystemDef>,
    /// 地理秘境（大陆、国家、城市、秘境、地形等）
    #[serde(default, rename = "places")]
    pub places: Vec<PlaceDef>,
    /// 势力（宗门、国家、家族、公会、组织等）
    #[serde(default, rename = "factions")]
    pub factions: Vec<FactionDef>,
    /// 时间线（故事内历法的纪元声明）
    #[serde(default, rename = "timeline")]
    pub timeline: TimelineSection,
}

/// [timeline] 故事内历法
///
/// 纪元为有序列表：故事时间锚（如"灵历3217年"）按纪元在列表中的位置
/// 折算为可比较序数。未声明纪元的项目，纯数字年份视为单一纪元。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineSection {
    /// 有序纪元名（如 ["创世", "灵历", "开元"]）
    #[serde(default)]
    pub eras: Vec<String>,
}

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

/// [[places]] 地理秘境
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceDef {
    pub name: String,
    /// 类型：continent（大陆）/ nation（国家）/ city（城市）/ realm（秘境）/ landform（地形）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 上级区域（松散引用，不强制校验）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 特征描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// [[factions]] 势力
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionDef {
    pub name: String,
    /// 势力类型：sect（宗门）/ nation（国家）/ clan（家族）/ guild（公会）/ organization（组织）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 首领/代表人物
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// 势力范围
    #[serde(skip_serializing_if = "Option::is_none")]
    pub territory: Option<String>,
    /// 理念/宗旨
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ideology: Option<String>,
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

impl WorldviewFile {
    pub fn from_toml_str(s: &str) -> Result<Self, WorldviewFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, WorldviewFileError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_worldview() {
        let toml_str = r#"
[[concepts]]
name = "灵气"
category = "世界规则"
summary = "天地间的基础能量"
details = "灵气浓度决定修炼速度"

[[concepts]]
name = "天道"
category = "世界规则"

[[systems]]
name = "修炼体系"
kind = "cultivation"
levels = "炼气→筑基→金丹→元婴→化神"
rules = "每突破一层需要渡劫"
summary = "以灵气为基础的进阶体系"

[[places]]
name = "昆仑山脉"
kind = "landform"
parent = "中州大陆"
features = "终年积雪，有上古遗迹"

[[factions]]
name = "天剑宗"
kind = "sect"
leader = "剑圣李沧海"
territory = "昆仑山东麓"
ideology = "剑道至上"
summary = "正道第一大派"
"#;
        let file = WorldviewFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.concepts.len(), 2);
        assert_eq!(file.concepts[0].name, "灵气");
        assert_eq!(file.concepts[0].category.as_deref(), Some("世界规则"));
        assert_eq!(
            file.concepts[0].details.as_deref(),
            Some("灵气浓度决定修炼速度")
        );
        assert_eq!(file.concepts[1].summary, None);

        assert_eq!(file.systems.len(), 1);
        assert_eq!(file.systems[0].name, "修炼体系");
        assert_eq!(file.systems[0].kind.as_deref(), Some("cultivation"));
        assert!(file.systems[0].levels.as_deref().unwrap().contains("筑基"));

        assert_eq!(file.places.len(), 1);
        assert_eq!(file.places[0].kind.as_deref(), Some("landform"));
        assert_eq!(file.places[0].parent.as_deref(), Some("中州大陆"));

        assert_eq!(file.factions.len(), 1);
        assert_eq!(file.factions[0].leader.as_deref(), Some("剑圣李沧海"));
        assert_eq!(file.factions[0].ideology.as_deref(), Some("剑道至上"));
    }

    #[test]
    fn parse_empty_worldview() {
        let file = WorldviewFile::from_toml_str("").unwrap();
        assert!(file.concepts.is_empty());
        assert!(file.systems.is_empty());
        assert!(file.places.is_empty());
        assert!(file.factions.is_empty());
    }

    #[test]
    fn roundtrip_all_sections() {
        let mut file = WorldviewFile::default();
        file.concepts.push(ConceptDef {
            name: "灵气".into(),
            category: Some("世界规则".into()),
            summary: Some("天地间的基础能量".into()),
            details: None,
        });
        file.systems.push(SystemDef {
            name: "魔法体系".into(),
            kind: Some("magic".into()),
            levels: Some("学徒→法师→大法师→魔导师".into()),
            rules: None,
            summary: Some("元素驱动的力量".into()),
        });
        file.places.push(PlaceDef {
            name: "幽冥秘境".into(),
            kind: Some("realm".into()),
            parent: None,
            features: Some("生者勿入".into()),
            summary: None,
        });
        file.factions.push(FactionDef {
            name: "铁血堂".into(),
            kind: Some("organization".into()),
            leader: None,
            territory: Some("北方荒原".into()),
            ideology: Some("强者生存".into()),
            summary: None,
        });

        let toml_str = file.to_toml_string().unwrap();
        let parsed = WorldviewFile::from_toml_str(&toml_str).unwrap();
        assert_eq!(parsed.concepts.len(), 1);
        assert_eq!(parsed.concepts[0].name, "灵气");
        assert_eq!(parsed.concepts[0].details, None);
        assert_eq!(parsed.systems.len(), 1);
        assert_eq!(parsed.systems[0].kind.as_deref(), Some("magic"));
        assert_eq!(parsed.systems[0].rules, None);
        assert_eq!(parsed.places.len(), 1);
        assert_eq!(parsed.places[0].kind.as_deref(), Some("realm"));
        assert_eq!(parsed.factions.len(), 1);
        assert_eq!(parsed.factions[0].territory.as_deref(), Some("北方荒原"));
    }
}
