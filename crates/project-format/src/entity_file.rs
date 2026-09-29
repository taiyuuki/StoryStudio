//! Entity 文件格式 (RFC-0003 §5.4)
//!
//! 每个 Semantic 实体一个 TOML 文件。
//! 存三重 ID（id/symbol/display_name）+ aliases + 专属属性。
//! 文件名 = StableId（id 必须等于文件名）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Entity 文件的完整内容
///
/// 所有实体类型（Character / Location / Item / Organization）共用此结构。
/// 不同实体类型的专属字段放在 [profile] / [item] / [geography] 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityFile {
    /// StableId（不可变，必须 == 文件名）
    pub id: String,
    /// Symbol（可改，可空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Display Name（正文与 UI 展示用）
    pub display_name: String,
    /// 别名列表
    #[serde(default)]
    pub aliases: Vec<String>,

    /// 角色档案（仅 Character）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileSection>,
    /// 地理信息（仅 Location）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geography: Option<GeographySection>,
    /// 道具信息（仅 Item）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemSection>,
    /// 组织信息（仅 Organization）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<OrgSection>,

    /// 可扩展属性容器（插件可扩展）
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// [profile] 段 — Character 专属 (RFC-0008)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub race: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occupation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birth_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appearance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// 性格（多行自由文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// 习性（口头禅、惯用手、小动作等，多行自由文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub habits: Option<String>,
}

/// [geography] 段 — Location 专属 (RFC-0014)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeographySection {
    /// 父地点（构成地理层级树）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 地理类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 位置描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// [item] 段 — Item 专属 (RFC-0014)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abilities: Option<String>,
    /// 是否为唯一道具（RFC-0014）
    #[serde(default)]
    pub unique: bool,
}

/// [org] 段 — Organization 专属
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgSection {
    /// 所在地（引用 Location 的 StableId）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Error)]
pub enum EntityFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("id 不匹配: 文件名期望 {expected}, 实际 {actual}")]
    IdMismatch { expected: String, actual: String },
}

impl EntityFile {
    /// 从 TOML 字符串解析
    pub fn from_toml_str(s: &str) -> Result<Self, EntityFileError> {
        Ok(toml::from_str(s)?)
    }

    /// 带文件名校验的解析
    pub fn from_toml_with_filename_check(
        s: &str,
        filename_stem: &str,
    ) -> Result<Self, EntityFileError> {
        let entity = Self::from_toml_str(s)?;
        if entity.id != filename_stem {
            return Err(EntityFileError::IdMismatch {
                expected: filename_stem.to_string(),
                actual: entity.id,
            });
        }
        Ok(entity)
    }

    /// 序列化为 TOML 字符串
    pub fn to_toml_string(&self) -> Result<String, EntityFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// 创建 Character Entity
    pub fn new_character(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            symbol: None,
            display_name: display_name.into(),
            aliases: Vec::new(),
            profile: Some(ProfileSection::default()),
            geography: None,
            item: None,
            org: None,
            attributes: BTreeMap::new(),
        }
    }

    /// 创建 Location Entity
    pub fn new_location(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            symbol: None,
            display_name: display_name.into(),
            aliases: Vec::new(),
            profile: None,
            geography: Some(GeographySection::default()),
            item: None,
            org: None,
            attributes: BTreeMap::new(),
        }
    }

    /// 创建 Item Entity
    pub fn new_item(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            symbol: None,
            display_name: display_name.into(),
            aliases: Vec::new(),
            profile: None,
            geography: None,
            item: Some(ItemSection::default()),
            org: None,
            attributes: BTreeMap::new(),
        }
    }

    /// 创建 Organization Entity
    pub fn new_organization(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            symbol: None,
            display_name: display_name.into(),
            aliases: Vec::new(),
            profile: None,
            geography: None,
            item: None,
            org: Some(OrgSection::default()),
            attributes: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_roundtrip() {
        let toml_str = r#"
id = "c_8A91D4"
symbol = "hero"
display_name = "林风"
aliases = ["林少侠", "风哥"]

[profile]
race = "human"
gender = "male"
occupation = "剑客"
birth_year = 1069
appearance = "身长七尺，眉目清朗"
personality = "外冷内热，重诺言，寡言但果决"
habits = "口头禅『不过如此』；左撇子；紧张时会摩挲剑柄"

[attributes]
weapon = "i_A7E902"
"#;
        let entity = EntityFile::from_toml_str(toml_str).unwrap();
        assert_eq!(entity.id, "c_8A91D4");
        assert_eq!(entity.display_name, "林风");
        assert_eq!(entity.aliases, vec!["林少侠", "风哥"]);
        assert_eq!(
            entity.profile.as_ref().unwrap().race.as_deref(),
            Some("human")
        );
        assert_eq!(entity.profile.as_ref().unwrap().birth_year, Some(1069));
        assert_eq!(
            entity.profile.as_ref().unwrap().personality.as_deref(),
            Some("外冷内热，重诺言，寡言但果决")
        );
        assert_eq!(
            entity.profile.as_ref().unwrap().habits.as_deref(),
            Some("口头禅『不过如此』；左撇子；紧张时会摩挲剑柄")
        );
        assert_eq!(
            entity.attributes.get("weapon").map(|s| s.as_str()),
            Some("i_A7E902")
        );

        // Roundtrip
        let serialized = entity.to_toml_string().unwrap();
        let reparsed = EntityFile::from_toml_str(&serialized).unwrap();
        assert_eq!(entity.id, reparsed.id);
        assert_eq!(entity.aliases, reparsed.aliases);
    }

    #[test]
    fn location_roundtrip() {
        let toml_str = r#"
id = "l_3F2B1C"
display_name = "青江渡口"
aliases = ["渡口"]

[geography]
parent = "l_1A2B3C"
kind = "settlement"
position = "青江下游"
"#;
        let entity = EntityFile::from_toml_str(toml_str).unwrap();
        assert_eq!(entity.id, "l_3F2B1C");
        let geo = entity.geography.unwrap();
        assert_eq!(geo.parent.as_deref(), Some("l_1A2B3C"));
        assert_eq!(geo.kind.as_deref(), Some("settlement"));
    }

    #[test]
    fn item_roundtrip() {
        let toml_str = r#"
id = "i_A7E902"
display_name = "寒霜剑"

[item]
kind = "weapon"
grade = "legendary"
description = "通体泛着淡蓝光芒的长剑"
unique = true
"#;
        let entity = EntityFile::from_toml_str(toml_str).unwrap();
        let item = entity.item.as_ref().unwrap();
        assert_eq!(item.kind.as_deref(), Some("weapon"));
        assert_eq!(item.grade.as_deref(), Some("legendary"));
        assert!(item.unique);

        // Roundtrip
        let serialized = entity.to_toml_string().unwrap();
        let reparsed = EntityFile::from_toml_str(&serialized).unwrap();
        assert!(reparsed.item.unwrap().unique);
    }

    #[test]
    fn item_unique_defaults_false() {
        // 无 [item] 段时，item 为 None；有 [item] 段但未设 unique 时默认 false
        let toml_no_item = r#"
id = "i_0001"
display_name = "铁剑"
"#;
        let entity = EntityFile::from_toml_str(toml_no_item).unwrap();
        assert!(entity.item.is_none());

        let toml_with_item = r#"
id = "i_0001"
display_name = "铁剑"

[item]
kind = "weapon"
"#;
        let entity2 = EntityFile::from_toml_str(toml_with_item).unwrap();
        assert!(!entity2.item.unwrap().unique);
    }

    #[test]
    fn organization_roundtrip() {
        let toml_str = r#"
id = "o_4D5E6F"
symbol = "smuggle"
display_name = "江南走私团"
aliases = ["水路帮"]

[org]
location = "l_3F2B1C"
"#;
        let entity = EntityFile::from_toml_str(toml_str).unwrap();
        assert_eq!(entity.id, "o_4D5E6F");
        assert_eq!(entity.display_name, "江南走私团");
        assert_eq!(
            entity.org.as_ref().unwrap().location.as_deref(),
            Some("l_3F2B1C")
        );

        // Roundtrip
        let serialized = entity.to_toml_string().unwrap();
        let reparsed = EntityFile::from_toml_str(&serialized).unwrap();
        assert_eq!(reparsed.org.unwrap().location.as_deref(), Some("l_3F2B1C"));
    }

    #[test]
    fn new_organization_has_empty_org_section() {
        let entity = EntityFile::new_organization("o_0001", "天机阁");
        assert_eq!(entity.id, "o_0001");
        assert!(entity.org.is_some());
        assert!(entity.org.as_ref().unwrap().location.is_none());
        assert!(entity.profile.is_none());
        assert!(entity.geography.is_none());
        assert!(entity.item.is_none());
    }

    #[test]
    fn filename_check() {
        let toml_str = "id = \"c_001\"\ndisplay_name = \"测试\"";
        assert!(EntityFile::from_toml_with_filename_check(toml_str, "c_001").is_ok());
        assert!(EntityFile::from_toml_with_filename_check(toml_str, "c_002").is_err());
    }

    #[test]
    fn new_character_helper() {
        let entity = EntityFile::new_character("c_001", "林风");
        assert_eq!(entity.id, "c_001");
        assert_eq!(entity.display_name, "林风");
        assert!(entity.profile.is_some());
        assert!(entity.geography.is_none());
        assert!(entity.item.is_none());
    }
}
