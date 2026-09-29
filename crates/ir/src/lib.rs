//! Story IR — Typed Graph Model (RFC-0002)
//!
//! Story IR 是 Story Studio 的核心数据结构，由两张图组成：
//! - Document Graph：文本结构（Book → Chapter → Scene → Fragment）
//! - Semantic Graph：实体与关系（Character / Location / Item / ...）
//!
//! 所有集合使用 BTreeMap / BTreeSet 以保证确定性序列化（§9）。

pub mod anchor;
pub mod text;
pub mod document;
pub mod id;
pub mod ir;
pub mod narrative;
pub mod semantic;
pub mod time;

pub use anchor::{AnchorRole, EntityAnchor};
pub use text::{count_words, is_cjk};
pub use document::{BookNode, ChapterNode, DocNode, DocumentGraph, FragmentNode, SceneNode};
pub use id::{EdgeId, IdPrefix, StableId, Symbol, SymbolTable};
pub use ir::{Appearance, IRVersion, LocationIndex, Revision, StoryIR};
pub use narrative::{
    KnowledgeState, Observer, StateTimelineEntry, Thread, ThreadCollection, ThreadKnowledge,
    ThreadPriority, ThreadRelationship, ThreadStatus,
};
pub use semantic::{
    AttributeValue, CharacterNode, ConceptNode, Entity, EntityAttributes, EventNode, ItemNode,
    LocationNode, OrganizationNode, RelationKind, SemanticEdge, SemanticGraph, SemanticNode,
};
pub use time::{StoryCalendar, StoryTime, TimeCoord, TimePrecision, ValidityWindow};
