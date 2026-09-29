//! Parser 错误类型 (RFC-0006 §10)
//!
//! 错误映射到 Diagnostics 级别：
//!   格式错误 / id不匹配 / 引用断裂 / 重复id → Error
//!   空Scene / 歧义匹配 / 无锚点 / 孤儿Fragment → Warning

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 解析错误（可批量收集，非立即返回）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub loc: SourceLocation,
}

/// 错误定位
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: Option<PathBuf>,
    pub entity: Option<String>,
    pub fragment: Option<String>,
}

impl SourceLocation {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(path.into()),
            entity: None,
            fragment: None,
        }
    }

    pub fn fragment(frag_id: impl Into<String>) -> Self {
        Self {
            file: None,
            entity: None,
            fragment: Some(frag_id.into()),
        }
    }

    pub fn entity(entity_id: impl Into<String>) -> Self {
        Self {
            file: None,
            entity: Some(entity_id.into()),
            fragment: None,
        }
    }
}

impl ParseError {
    pub fn new(kind: ParseErrorKind, loc: SourceLocation) -> Self {
        Self { kind, loc }
    }

    /// 错误码（稳定的分类标识，RFC-0005 §3.3）
    pub fn code(&self) -> &str {
        self.kind.code()
    }
}

/// 错误分类
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ParseErrorKind {
    // —— Error 级 ——
    #[error("文件读取失败: {file}: {reason}")]
    FileRead { file: String, reason: String },

    #[error("TOML 解析失败: {file}: {reason}")]
    TomlParse { file: String, reason: String },

    #[error("YAML frontmatter 解析失败: {reason}")]
    YamlParse { reason: String },

    #[error("id 不匹配: 文件名 {expected}, frontmatter.id {actual}")]
    IdMismatch { expected: String, actual: String },

    #[error("引用断裂: {reference} 指向不存在的 {target_type} '{target_id}'")]
    DanglingReference {
        reference: String,
        target_type: String, // "Fragment" / "Entity" / "Scene" / "Chapter"
        target_id: String,
    },

    #[error("重复的 StableId: {id}")]
    DuplicateId { id: String },

    #[error("重复的 Symbol: {symbol}")]
    DuplicateSymbol { symbol: String },

    // —— Warning 级 ——
    #[error("空 Scene: {scene_id} 的 fragments 列表为空")]
    EmptyScene { scene_id: String },

    #[error("歧义匹配: 正文中的 '{text}' 匹配到多个实体")]
    AmbiguousMatch { text: String, entities: Vec<String> },

    #[error("孤儿 Fragment: {fragment_id} 未被 structure.toml 引用")]
    OrphanFragment { fragment_id: String },

    #[error("无效的 anchor role: {role}")]
    InvalidAnchorRole { role: String },
}

impl ParseErrorKind {
    /// 错误码
    pub fn code(&self) -> &'static str {
        match self {
            ParseErrorKind::FileRead { .. } => "parse.file_read",
            ParseErrorKind::TomlParse { .. } => "parse.toml_syntax",
            ParseErrorKind::YamlParse { .. } => "parse.yaml_syntax",
            ParseErrorKind::IdMismatch { .. } => "parse.id_mismatch",
            ParseErrorKind::DanglingReference { .. } => "parse.dangling_reference",
            ParseErrorKind::DuplicateId { .. } => "parse.duplicate_id",
            ParseErrorKind::DuplicateSymbol { .. } => "parse.duplicate_symbol",
            ParseErrorKind::EmptyScene { .. } => "parse.empty_scene",
            ParseErrorKind::AmbiguousMatch { .. } => "parse.ambiguous_match",
            ParseErrorKind::OrphanFragment { .. } => "parse.orphan_fragment",
            ParseErrorKind::InvalidAnchorRole { .. } => "parse.invalid_anchor_role",
        }
    }

    /// 是否为 Error 级（阻断）
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            ParseErrorKind::FileRead { .. }
                | ParseErrorKind::TomlParse { .. }
                | ParseErrorKind::YamlParse { .. }
                | ParseErrorKind::IdMismatch { .. }
                | ParseErrorKind::DanglingReference { .. }
                | ParseErrorKind::DuplicateId { .. }
                | ParseErrorKind::DuplicateSymbol { .. }
        )
    }

    /// 是否为 Warning 级（非阻断）
    pub fn is_warning(&self) -> bool {
        !self.is_error()
    }
}

/// 解析结果：IR + 错误列表（错误不中断解析，尽量收集）
#[derive(Debug, Serialize, Deserialize)]
pub struct ParseResult<T> {
    pub value: Option<T>,
    pub errors: Vec<ParseError>,
}

impl<T> ParseResult<T> {
    pub fn ok(value: T) -> Self {
        Self {
            value: Some(value),
            errors: Vec::new(),
        }
    }

    pub fn with_errors(value: T, errors: Vec<ParseError>) -> Self {
        Self {
            value: Some(value),
            errors,
        }
    }

    pub fn fail(errors: Vec<ParseError>) -> Self {
        Self {
            value: None,
            errors,
        }
    }

    pub fn has_errors(&self) -> bool {
        self.errors.iter().any(|e| e.kind.is_error())
    }

    pub fn has_warnings(&self) -> bool {
        self.errors.iter().any(|e| e.kind.is_warning())
    }
}
