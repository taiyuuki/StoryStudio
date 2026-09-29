//! Document Graph 节点 (RFC-0002 §6.1)
//!
//! 表达文本的物理/结构组织：Book → Chapter → Scene → Fragment。
//! Fragment 是叶子节点，也是唯一能锚定 Semantic 实体的节点类型。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::anchor::EntityAnchor;
use crate::id::{StableId, Symbol};
use crate::time::StoryCalendar;

// ============================================================================
// DocNode 枚举
// ============================================================================

/// Document Graph 的节点类型联合 (RFC-0002 §6.1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocNode {
    Book(BookNode),
    Chapter(ChapterNode),
    Scene(SceneNode),
    Fragment(FragmentNode),
}

impl DocNode {
    pub fn id(&self) -> &StableId {
        match self {
            DocNode::Book(n) => &n.id,
            DocNode::Chapter(n) => &n.id,
            DocNode::Scene(n) => &n.id,
            DocNode::Fragment(n) => &n.id,
        }
    }
}

// ============================================================================
// Book
// ============================================================================

/// Book 节点（单例，Project 的文档根）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookNode {
    pub id: StableId,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
}

// ============================================================================
// Chapter
// ============================================================================

/// Chapter 节点（结构单元）
///
/// 顺序由父节点的数组表达（RFC-0003 §5.2），prev/next 是内存层派生（§5.3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterNode {
    pub id: StableId,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    /// 内存层链表——不持久化，由 Parser 从数组构建
    #[serde(skip)]
    pub prev: Option<StableId>,
    #[serde(skip)]
    pub next: Option<StableId>,
    /// 可选：章的时间跨度（RFC-0009 §4.3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story_time_span: Option<ChapterTimeSpan>,
}

/// Chapter 的时间跨度
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterTimeSpan {
    pub from: StoryCalendar,
    pub until: StoryCalendar,
}

// ============================================================================
// Scene
// ============================================================================

/// Scene 节点（叙事单元，仅属 Document Graph）
///
/// 包含语义字段（RFC-0007）：POV、mood、function、story_time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneNode {
    pub id: StableId,
    /// 所属 Chapter
    pub chapter: StableId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    /// 内存层链表
    #[serde(skip)]
    pub prev: Option<StableId>,
    #[serde(skip)]
    pub next: Option<StableId>,
    /// 视角角色 (RFC-0007 §3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pov: Option<StableId>,
    /// 此场景的故事时间 (RFC-0009 §4.1)——数值锚（有 year 时）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story_time: Option<StoryCalendar>,
    /// 故事时间历法标签锚（如"灵历3217年秋"），经项目历法解析比较
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub story_time_anchor: Option<String>,
    /// 与阅读序前一场景的时间间隔（模糊描述合法，如"3天""数月"）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub story_time_gap: Option<String>,
    /// 情绪基调 (RFC-0007 §4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mood: Option<String>,
    /// 叙事功能 (RFC-0007 §5)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// 场景摘要（一句话或一段话概括，AI 可生成）(RFC-0007 §6)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ============================================================================
// Fragment
// ============================================================================

/// Fragment 节点（最小编辑单元）
///
/// 只存正文与锚点，不存结构归属（Scene 归属由 structure.toml 决定，RFC-0003 §3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentNode {
    pub id: StableId,
    /// 所属 Scene（由 structure.toml 反向推导，不持久化）
    #[serde(skip)]
    pub scene: Option<StableId>,
    /// 内存层链表
    #[serde(skip)]
    pub prev: Option<StableId>,
    #[serde(skip)]
    pub next: Option<StableId>,
    /// Markdown 正文
    pub text: String,
    /// 锚定的 Semantic 实体（Source of Truth）
    #[serde(default)]
    pub anchors: Vec<EntityAnchor>,
}

impl FragmentNode {
    pub fn new(id: StableId, text: String) -> Self {
        Self {
            id,
            scene: None,
            prev: None,
            next: None,
            text,
            anchors: Vec::new(),
        }
    }
}

// ============================================================================
// DocumentGraph
// ============================================================================

/// Document Graph（RFC-0002 §3.2）
///
/// BTreeMap 保证确定序列化（§9）。prev/next 是内存层派生，不参与序列化。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentGraph {
    /// StableId → DocNode
    nodes: BTreeMap<StableId, DocNode>,
    /// contains 边的集合：(parent, child)
    #[serde(default)]
    edges: BTreeMap<StableId, Vec<StableId>>,
}

impl DocumentGraph {
    pub fn new() -> Self {
        Self::default()
    }

    // —— 节点操作 ——

    pub fn insert(&mut self, node: DocNode) {
        let id = node.id().clone();
        self.nodes.insert(id, node);
    }

    pub fn get(&self, id: &StableId) -> Option<&DocNode> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: &StableId) -> Option<&mut DocNode> {
        self.nodes.get_mut(id)
    }

    pub fn contains(&self, id: &StableId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn remove(&mut self, id: &StableId) -> Option<DocNode> {
        self.nodes.remove(id)
    }

    pub fn all_nodes(&self) -> impl Iterator<Item = (&StableId, &DocNode)> {
        self.nodes.iter()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    // —— 类型化访问 ——

    pub fn get_book(&self, id: &StableId) -> Option<&BookNode> {
        match self.nodes.get(id) {
            Some(DocNode::Book(n)) => Some(n),
            _ => None,
        }
    }

    pub fn get_chapter(&self, id: &StableId) -> Option<&ChapterNode> {
        match self.nodes.get(id) {
            Some(DocNode::Chapter(n)) => Some(n),
            _ => None,
        }
    }

    pub fn get_scene(&self, id: &StableId) -> Option<&SceneNode> {
        match self.nodes.get(id) {
            Some(DocNode::Scene(n)) => Some(n),
            _ => None,
        }
    }

    pub fn get_fragment(&self, id: &StableId) -> Option<&FragmentNode> {
        match self.nodes.get(id) {
            Some(DocNode::Fragment(n)) => Some(n),
            _ => None,
        }
    }

    pub fn get_fragment_mut(&mut self, id: &StableId) -> Option<&mut FragmentNode> {
        match self.nodes.get_mut(id) {
            Some(DocNode::Fragment(n)) => Some(n),
            _ => None,
        }
    }

    // —— contains 边 ——

    /// 设置 parent contains child 关系
    pub fn set_children(&mut self, parent: StableId, children: Vec<StableId>) {
        self.edges.insert(parent, children);
    }

    pub fn children_of(&self, parent: &StableId) -> &[StableId] {
        self.edges.get(parent).map(|v| v.as_slice()).unwrap_or(&[])
    }

    // —— 反向索引（派生）——

    /// 查找某 Scene 所属的 Chapter（从 edges 反向查找）
    pub fn chapter_of_scene(&self, scene_id: &StableId) -> Option<&StableId> {
        for (parent_id, children) in &self.edges {
            if children.contains(scene_id)
                && self
                    .nodes
                    .get(parent_id)
                    .is_some_and(|n| matches!(n, DocNode::Chapter(_)))
            {
                return Some(parent_id);
            }
        }
        None
    }

    /// 查找某 Fragment 所属的 Scene
    pub fn scene_of_fragment(&self, fragment_id: &StableId) -> Option<&StableId> {
        // 优先用 FragmentNode.scene（内存层已构建）
        if let Some(DocNode::Fragment(frag)) = self.nodes.get(fragment_id) {
            if let Some(ref scene) = frag.scene {
                return Some(scene);
            }
        }
        // 回退到 edges 反向查找
        for (parent_id, children) in &self.edges {
            if children.contains(fragment_id)
                && self
                    .nodes
                    .get(parent_id)
                    .is_some_and(|n| matches!(n, DocNode::Scene(_)))
            {
                return Some(parent_id);
            }
        }
        None
    }

    /// 获取所有 Fragment
    pub fn all_fragments(&self) -> impl Iterator<Item = (&StableId, &FragmentNode)> {
        self.nodes.iter().filter_map(|(id, node)| match node {
            DocNode::Fragment(n) => Some((id, n)),
            _ => None,
        })
    }

    /// 获取某 Chapter 下的所有 Scene
    pub fn scenes_of_chapter(&self, chapter_id: &StableId) -> &[StableId] {
        self.children_of(chapter_id)
    }

    /// 阅读序拓扑索引：按 章→场景→片段 的既定顺序展平，
    /// 返回节点（场景或片段）的全局序号。
    ///
    /// 这是叙事时序的默认度量——绝大多数小说的故事编年序即阅读序；
    /// 倒叙/并行需要显式标注（第三期）。无法定位返回 None。
    pub fn reading_index(&self, id: &StableId) -> Option<u32> {
        let book_id = self
            .nodes
            .iter()
            .find_map(|(bid, n)| matches!(n, DocNode::Book(_)).then_some(bid))?;
        let mut idx = 0u32;
        for ch_id in self.children_of(book_id) {
            for sc_id in self.scenes_of_chapter(ch_id) {
                if sc_id == id {
                    return Some(idx);
                }
                idx += 1;
                for fr_id in self.fragments_of_scene(sc_id) {
                    if fr_id == id {
                        return Some(idx);
                    }
                    idx += 1;
                }
            }
        }
        None
    }

    /// 获取某 Scene 下的所有 Fragment
    pub fn fragments_of_scene(&self, scene_id: &StableId) -> &[StableId] {
        self.children_of(scene_id)
    }
}
