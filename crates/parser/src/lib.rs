//! Parser — 文件到 Story IR 的确定性转换 (RFC-0006)
//!
//! 四阶段管线：
//!   Phase 1:   Load         — 读文件
//!   Phase 1.5: Entity Scan  — 构建 SymbolTable + SemanticGraph
//!   Phase 2:   Structure    — structure.toml → DocumentGraph（链表）
//!   Phase 3:   Fragment     — frontmatter + anchors
//!   Phase 4:   Entity Link   — Aho-Corasick 词典匹配 → mentions
//!
//! Parser 是纯确定性的。不调用 AI，不做语义推理。

pub mod entity_linking;
pub mod error;
pub mod incremental;
pub mod pipeline;

pub use entity_linking::{is_number_like, EntityDictionary, Mention};
pub use error::{ParseError, ParseErrorKind};
pub use incremental::{detect_dirty, parse_incremental, IncrementalResult};
pub use pipeline::{parse_project, parse_project_with_cache, ParseCache, ParseOutput};

/// parser crate 对应的 RFC 版本
pub const RFC_VERSION: &str = "0.2";
