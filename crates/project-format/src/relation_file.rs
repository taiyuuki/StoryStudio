//! Relations 文件格式 — 角色之间的关系（独立于知识库）
//!
//! relations.toml 集中存储所有 SemanticEdge（角色关系边）：
//!   [[relation]]
//!   id = "se_0001"
//!   from = "c_8A91D4"
//!   to   = "c_A1B2C3"
//!   relation = "mentor_of"
//!   valid = { from = { year = 1085 } }
//!   evidence = ["f_0007"]
//!
//! valid 区间边界的两种写法（时变关系）：
//!   from = { year = 1085 }            # 绝对历法
//!   from = { scene = "sc_02_01" }      # 叙事锚（场景 ID，按阅读序比较）
//!
//! 与知识库断言（assertions.toml）的区别：
//! - 断言用于正文一致性校验（年龄矛盾、死亡后复活等），带 status/extracted_by
//! - 关系是纯拓扑边，映射到 SemanticGraph，供关系图与实体页消费
//!
//! 关系类型用字符串存取（RelationKind::Custom），预设清单由前端维护：
//! mentor_of / family_of / lover_of / enemy_of / rival_of / subordinate_of / knows

use serde::{Deserialize, Serialize};
use story_ir::{
    EdgeId, RelationKind, SemanticEdge, StableId, StoryCalendar, StoryTime, TimeCoord,
    ValidityWindow,
};
use thiserror::Error;

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum RelationFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

// ============================================================================
// RelationsFile — relations.toml
// ============================================================================

/// relations.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationsFile {
    #[serde(default, rename = "relation")]
    pub relations: Vec<RelationDef>,
}

/// 单条关系的 TOML 表示（镜像 SemanticEdge）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationDef {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid: Option<ValidityDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// valid 区间的 TOML 表示
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidityDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<StoryTimeDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<StoryTimeDef>,
}

/// StoryTime 的 TOML 表示：绝对历法（year/month/day）或叙事锚（scene）。
///
/// scene 优先于历法字段——关系随剧情演变时，用场景 ID 定位生效/失效
/// 边界比绝对纪法更贴合作者的写作语言（"第二章起反目"）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryTimeDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
    /// 叙事锚：场景（或片段）ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
}

impl RelationsFile {
    pub fn from_toml_str(s: &str) -> Result<Self, RelationFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, RelationFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// 转换为 SemanticEdge 列表
    pub fn to_semantic_edges(&self) -> Vec<SemanticEdge> {
        self.relations.iter().filter_map(|d| d.to_edge()).collect()
    }

    /// 从 SemanticEdge 列表构建
    pub fn from_edges(edges: &[SemanticEdge]) -> Self {
        Self {
            relations: edges.iter().map(RelationDef::from_edge).collect(),
        }
    }
}

impl RelationDef {
    fn to_edge(&self) -> Option<SemanticEdge> {
        let id = EdgeId::new(&self.id).ok()?;
        Some(SemanticEdge {
            id,
            from: StableId::new_unchecked(&self.from),
            to: StableId::new_unchecked(&self.to),
            relation: parse_relation(&self.relation),
            valid: self.valid.as_ref().map(|v| v.to_validity_window()),
            evidence: self.evidence.iter().map(StableId::new_unchecked).collect(),
        })
    }

    fn from_edge(e: &SemanticEdge) -> Self {
        Self {
            id: e.id.to_string(),
            from: e.from.as_str().to_string(),
            to: e.to.as_str().to_string(),
            relation: serialize_relation(&e.relation),
            valid: ValidityDef::from_validity(&e.valid),
            evidence: e.evidence.iter().map(|s| s.as_str().to_string()).collect(),
        }
    }
}

impl ValidityDef {
    pub fn to_validity_window(&self) -> ValidityWindow {
        ValidityWindow::new(
            self.from.as_ref().map(|t| t.to_story_time()),
            self.until.as_ref().map(|t| t.to_story_time()),
        )
    }

    fn from_validity(v: &Option<ValidityWindow>) -> Option<Self> {
        let v = v.as_ref()?;
        let from = v.from.as_ref().and_then(StoryTimeDef::from_story_time);
        let until = v.until.as_ref().and_then(StoryTimeDef::from_story_time);
        if from.is_none() && until.is_none() {
            return None;
        }
        Some(Self { from, until })
    }
}

impl StoryTimeDef {
    fn to_story_time(&self) -> StoryTime {
        if let Some(scene) = &self.scene {
            return StoryTime::narrative(StableId::new_unchecked(scene));
        }
        StoryTime::absolute(StoryCalendar {
            year: self.year.unwrap_or(0),
            month: self.month,
            day: self.day,
        })
    }

    fn from_story_time(t: &StoryTime) -> Option<Self> {
        match &t.coord {
            TimeCoord::Absolute(cal) => Some(Self {
                year: Some(cal.year),
                month: cal.month,
                day: cal.day,
                scene: None,
            }),
            TimeCoord::Narrative(id) => Some(Self {
                year: None,
                month: None,
                day: None,
                scene: Some(id.as_str().to_string()),
            }),
            _ => None, // BeforeStory/AfterStory/Unknown 暂不序列化
        }
    }
}

// ============================================================================
// 序列化/反序列化辅助
// ============================================================================

/// relation 字符串 → RelationKind。
/// 已知枚举值映射到对应变体；其余落入 Custom(String)。
fn parse_relation(s: &str) -> RelationKind {
    match s {
        "appears_in" => RelationKind::AppearsIn,
        "knows" => RelationKind::Knows,
        "related_to" => RelationKind::RelatedTo,
        "holds" => RelationKind::Holds,
        "located_at" => RelationKind::LocatedAt,
        "member_of" => RelationKind::MemberOf,
        other => RelationKind::Custom(other.to_string()),
    }
}

/// RelationKind → relation 字符串。
fn serialize_relation(r: &RelationKind) -> String {
    r.as_str().to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relations_roundtrip() {
        let toml_str = r#"
[[relation]]
id = "se_0001"
from = "c_8A91D4"
to = "c_A1B2C3"
relation = "mentor_of"
evidence = ["f_0007"]

[relation.valid]
from = { year = 1085 }

[[relation]]
id = "se_0002"
from = "c_8A91D4"
to = "c_3F4E5D"
relation = "knows"
"#;
        let file = RelationsFile::from_toml_str(toml_str).unwrap();
        let edges = file.to_semantic_edges();

        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].id.to_string(), "se_0001");
        assert_eq!(edges[0].from.as_str(), "c_8A91D4");
        assert_eq!(edges[0].to.as_str(), "c_A1B2C3");
        assert!(matches!(&edges[0].relation, RelationKind::Custom(s) if s == "mentor_of"));
        assert!(edges[0].valid.is_some());
        assert_eq!(edges[0].evidence.len(), 1);

        assert!(matches!(&edges[1].relation, RelationKind::Knows));
        assert!(edges[1].valid.is_none());

        // Roundtrip
        let file2 = RelationsFile::from_edges(&edges);
        let serialized = file2.to_toml_string().unwrap();
        let reparsed = RelationsFile::from_toml_str(&serialized).unwrap();
        let re_edges = reparsed.to_semantic_edges();
        assert_eq!(re_edges.len(), 2);
        assert_eq!(re_edges[0].from, edges[0].from);
        assert_eq!(re_edges[0].relation, edges[0].relation);
    }

    #[test]
    fn empty_file() {
        let file = RelationsFile::from_toml_str("").unwrap();
        assert!(file.relations.is_empty());
    }

    #[test]
    fn custom_relation_roundtrip() {
        let toml_str = r#"
[[relation]]
id = "se_0003"
from = "c_001"
to = "c_002"
relation = "sworn_brother_of"
"#;
        let file = RelationsFile::from_toml_str(toml_str).unwrap();
        let edges = file.to_semantic_edges();
        assert!(matches!(&edges[0].relation, RelationKind::Custom(s) if s == "sworn_brother_of"));

        // 序列化后应还原为原字符串
        let serialized = RelationsFile::from_edges(&edges).to_toml_string().unwrap();
        assert!(serialized.contains("sworn_brother_of"));
    }

    #[test]
    fn invalid_edge_id_prefix_skipped() {
        // EdgeId 必须是 se_ 前缀；非法前缀的条目被 to_edge 跳过
        let toml_str = r#"
[[relation]]
id = "x_0001"
from = "c_001"
to = "c_002"
relation = "knows"
"#;
        let file = RelationsFile::from_toml_str(toml_str).unwrap();
        let edges = file.to_semantic_edges();
        assert!(edges.is_empty());
    }

    #[test]
    fn scene_anchored_validity_roundtrip() {
        // 时变关系：生效边界用场景锚（叙事坐标），序列化为 scene 字段
        let toml_str = r#"
[[relation]]
id = "se_0001"
from = "c_001"
to = "c_002"
relation = "enemy_of"

[relation.valid]
from = { scene = "sc_01_02" }
until = { scene = "sc_03_01" }
"#;
        let file = RelationsFile::from_toml_str(toml_str).unwrap();
        let edges = file.to_semantic_edges();
        let valid = edges[0].valid.as_ref().expect("valid 应解析");
        assert!(matches!(
            &valid.from.as_ref().expect("from 应存在").coord,
            TimeCoord::Narrative(id) if id.as_str() == "sc_01_02"
        ));
        assert!(matches!(
            &valid.until.as_ref().expect("until 应存在").coord,
            TimeCoord::Narrative(id) if id.as_str() == "sc_03_01"
        ));

        // Roundtrip：scene 锚不丢
        let serialized = RelationsFile::from_edges(&edges)
            .to_toml_string()
            .unwrap();
        assert!(serialized.contains("scene = \"sc_01_02\""), "{serialized}");
        assert!(serialized.contains("scene = \"sc_03_01\""), "{serialized}");
        let reparsed = RelationsFile::from_toml_str(&serialized).unwrap();
        assert_eq!(reparsed.to_semantic_edges()[0].valid, edges[0].valid);
    }
}
