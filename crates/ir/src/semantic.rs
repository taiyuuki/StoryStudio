//! Semantic Graph 节点与边 (RFC-0002 §6.2, §7.2)
//!
//! 表达故事世界中的实体与关系：Character / Location / Item / Organization / Event / Concept。
//! SemanticEdge 是一级公民，拥有自己的 EdgeId。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{EdgeId, StableId, Symbol};
use crate::time::ValidityWindow;

// ============================================================================
// SemanticNode 枚举
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SemanticNode {
    Character(CharacterNode),
    Location(LocationNode),
    Item(ItemNode),
    Organization(OrganizationNode),
    Event(EventNode),
    Concept(ConceptNode),
}

impl SemanticNode {
    pub fn id(&self) -> &StableId {
        match self {
            SemanticNode::Character(n) => &n.id,
            SemanticNode::Location(n) => &n.id,
            SemanticNode::Item(n) => &n.id,
            SemanticNode::Organization(n) => &n.id,
            SemanticNode::Event(n) => &n.id,
            SemanticNode::Concept(n) => &n.id,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            SemanticNode::Character(n) => &n.display_name,
            SemanticNode::Location(n) => &n.display_name,
            SemanticNode::Item(n) => &n.display_name,
            SemanticNode::Organization(n) => &n.display_name,
            SemanticNode::Event(n) => &n.display_name,
            SemanticNode::Concept(n) => &n.display_name,
        }
    }

    pub fn symbol(&self) -> Option<&Symbol> {
        match self {
            SemanticNode::Character(n) => n.symbol.as_ref(),
            SemanticNode::Location(n) => n.symbol.as_ref(),
            SemanticNode::Item(n) => n.symbol.as_ref(),
            SemanticNode::Organization(n) => n.symbol.as_ref(),
            SemanticNode::Event(n) => n.symbol.as_ref(),
            SemanticNode::Concept(n) => n.symbol.as_ref(),
        }
    }

    pub fn aliases(&self) -> &[String] {
        match self {
            SemanticNode::Character(n) => &n.aliases,
            SemanticNode::Location(n) => &n.aliases,
            SemanticNode::Item(n) => &n.aliases,
            SemanticNode::Organization(n) => &n.aliases,
            SemanticNode::Event(n) => &n.aliases,
            SemanticNode::Concept(n) => &n.aliases,
        }
    }

    /// 实体类型字符串（用于错误消息）
    pub fn type_name(&self) -> &'static str {
        match self {
            SemanticNode::Character(_) => "character",
            SemanticNode::Location(_) => "location",
            SemanticNode::Item(_) => "item",
            SemanticNode::Organization(_) => "organization",
            SemanticNode::Event(_) => "event",
            SemanticNode::Concept(_) => "concept",
        }
    }
}

// ============================================================================
// Entity 通用 trait（用于类型化访问）
// ============================================================================

/// 所有 Semantic 实体共享的字段访问
pub trait Entity {
    fn id(&self) -> &StableId;
    fn symbol(&self) -> Option<&Symbol>;
    fn display_name(&self) -> &str;
    fn aliases(&self) -> &[String];
}

// ============================================================================
// Character (RFC-0008)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub attributes: EntityAttributes,
}

// ============================================================================
// Location (RFC-0014)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// 地理层级：父地点
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<StableId>,
    /// 地理类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 位置描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

// ============================================================================
// Item (RFC-0014)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// 道具类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 品质
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    /// 是否为唯一道具（神器/专属道具 vs 普通道具）(RFC-0014)
    #[serde(default)]
    pub unique: bool,
}

// ============================================================================
// Organization / Event / Concept（v0.1 骨架）
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// 所在地（引用 Location 的 StableId）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<StableId>,
    #[serde(default)]
    pub attributes: EntityAttributes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub attributes: EntityAttributes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptNode {
    pub id: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub attributes: EntityAttributes,
}

// ============================================================================
// EntityAttributes
// ============================================================================

/// 可扩展属性容器（BTreeMap 保证确定序列化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityAttributes {
    #[serde(flatten)]
    pub custom: BTreeMap<String, AttributeValue>,
}

/// 属性值（支持多种类型）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Integer(i64),
    Boolean(bool),
    /// 引用另一个实体的 StableId
    Reference(StableId),
}

// ============================================================================
// SemanticEdge
// ============================================================================

/// Semantic Graph 的边（一级公民，RFC-0002 §7.2）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticEdge {
    pub id: EdgeId,
    pub from: StableId,
    pub to: StableId,
    pub relation: RelationKind,
    /// 时序区间（None = 全时段）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid: Option<ValidityWindow>,
    /// 支撑该边的 Fragment 锚点
    #[serde(default)]
    pub evidence: Vec<StableId>,
}

/// 关系类型（开放集合，可由 Plugin 扩展）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationKind {
    // 角色关系
    AppearsIn,
    Knows,
    RelatedTo,
    // 物品关系
    Holds,
    LocatedAt,
    // 组织关系
    MemberOf,
    // 扩展
    Custom(String),
}

impl RelationKind {
    pub fn as_str(&self) -> &str {
        match self {
            RelationKind::AppearsIn => "appears_in",
            RelationKind::Knows => "knows",
            RelationKind::RelatedTo => "related_to",
            RelationKind::Holds => "holds",
            RelationKind::LocatedAt => "located_at",
            RelationKind::MemberOf => "member_of",
            RelationKind::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for RelationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================================
// SemanticGraph
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticGraph {
    nodes: BTreeMap<StableId, SemanticNode>,
    edges: BTreeMap<EdgeId, SemanticEdge>,
}

impl SemanticGraph {
    pub fn new() -> Self {
        Self::default()
    }

    // —— 节点 ——

    pub fn insert(&mut self, node: SemanticNode) {
        let id = node.id().clone();
        self.nodes.insert(id, node);
    }

    pub fn get(&self, id: &StableId) -> Option<&SemanticNode> {
        self.nodes.get(id)
    }

    pub fn get_character(&self, id: &StableId) -> Option<&CharacterNode> {
        match self.nodes.get(id) {
            Some(SemanticNode::Character(n)) => Some(n),
            _ => None,
        }
    }

    pub fn contains(&self, id: &StableId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn all_nodes(&self) -> impl Iterator<Item = (&StableId, &SemanticNode)> {
        self.nodes.iter()
    }

    pub fn all_entities(&self) -> impl Iterator<Item = &SemanticNode> {
        self.nodes.values()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    // —— 边 ——

    pub fn insert_edge(&mut self, edge: SemanticEdge) {
        self.edges.insert(edge.id.clone(), edge);
    }

    pub fn get_edge(&self, id: &EdgeId) -> Option<&SemanticEdge> {
        self.edges.get(id)
    }

    pub fn all_edges(&self) -> impl Iterator<Item = (&EdgeId, &SemanticEdge)> {
        self.edges.iter()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// 查找从某实体出发的所有边
    pub fn edges_from<'a>(
        &'a self,
        entity: &'a StableId,
    ) -> impl Iterator<Item = &'a SemanticEdge> + 'a {
        self.edges.values().filter(move |e| &e.from == entity)
    }

    /// 查找指向某实体的所有边
    pub fn edges_to<'a>(
        &'a self,
        entity: &'a StableId,
    ) -> impl Iterator<Item = &'a SemanticEdge> + 'a {
        self.edges.values().filter(move |e| &e.to == entity)
    }
}
