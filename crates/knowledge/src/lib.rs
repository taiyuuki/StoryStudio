//! Knowledge Layer — Assertion + Evidence (RFC-0015)
//!
//! Knowledge Layer 位于 Story IR 与 Compiler 之间，表示故事中
//! 可被推理和验证的知识（断言 + 证据）。
//!
//! 数据契约在本 crate 定义；执行编排（Extraction 管线）由 AI Runtime 负责。
//! Compiler 单向消费 KnowledgeBase 执行一致性检查。

pub mod assertion;
pub mod evidence;
pub mod knowledge_base;

pub use assertion::{Assertion, AssertionStatus, ExtractedBy, ObjectValue, Predicate};
pub use evidence::{Evidence, EvidenceSource};
pub use knowledge_base::KnowledgeBase;

/// crate 对应的 RFC 版本
pub const RFC_VERSION: &str = "0.2";
