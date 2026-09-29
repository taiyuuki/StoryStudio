//! 三重 ID 系统 (RFC-0002 §4)
//!
//! | 层级          | 示例         | 可修改 | 用途                     |
//! |--------------|-------------|--------|--------------------------|
//! | StableId     | c_8A91D4   | 否     | Graph 边、存储、内部引用    |
//! | Symbol       | hero        | 是     | Parser 引用解析、Prompt    |
//! | Display Name | 林风         | 是     | 正文、UI 展示              |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// StableId
// ============================================================================

/// 实体的永久唯一标识。一经分配，终身不变。
///
/// 格式：`前缀_Token`，如 `c_8A91D4`、`f_0001`、`se_0033`
///
/// `#[serde(transparent)]` 确保 StableId 在 JSON 中序列化为纯字符串（如 `"c_001"`），
/// 而非 `{"inner": "c_001"}`。这对作为 BTreeMap key 使用至关重要。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StableId {
    /// 序列化/显示时的完整字符串（如 "c_8A91D4"）
    inner: String,
}

impl StableId {
    /// 从字符串构造，校验格式合法性
    pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
        let inner = s.into();
        let prefix = Self::parse_prefix(&inner)?;
        let _ = prefix; // 校验通过
        Ok(Self { inner })
    }

    /// 仅用于内部构造（已保证合法时）
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self { inner: s.into() }
    }

    /// 字符串表示
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// 解析前缀
    pub fn prefix(&self) -> IdPrefix {
        Self::parse_prefix(&self.inner).unwrap_or(IdPrefix::Unknown)
    }

    fn parse_prefix(s: &str) -> Result<IdPrefix, IdError> {
        if !s.contains('_') {
            return Err(IdError::NoUnderscore(s.to_string()));
        }
        let prefix_str = s.split('_').next().unwrap(); // 安全：已确认含 '_'
        IdPrefix::from_str(prefix_str)
    }

    /// 是否匹配指定前缀
    pub fn has_prefix(&self, prefix: IdPrefix) -> bool {
        self.prefix() == prefix
    }
}

impl std::fmt::Display for StableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.inner)
    }
}

impl std::str::FromStr for StableId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

// ============================================================================
// IdPrefix
// ============================================================================

/// StableId 的类型前缀 (RFC-0002 §4.2)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IdPrefix {
    Character,    // c_
    Fragment,     // f_
    Location,     // l_
    Item,         // i_
    Organization, // o_
    Event,        // e_
    Book,         // b_
    Chapter,      // ch_
    Scene,        // sc_
    SemanticEdge, // se_
    Unknown,
}

impl IdPrefix {
    /// 前缀字符串（不含下划线）
    pub fn as_str(&self) -> &'static str {
        match self {
            IdPrefix::Character => "c",
            IdPrefix::Fragment => "f",
            IdPrefix::Location => "l",
            IdPrefix::Item => "i",
            IdPrefix::Organization => "o",
            IdPrefix::Event => "e",
            IdPrefix::Book => "b",
            IdPrefix::Chapter => "ch",
            IdPrefix::Scene => "sc",
            IdPrefix::SemanticEdge => "se",
            IdPrefix::Unknown => "?",
        }
    }

    /// 从前缀字符串解析
    ///
    /// 注：刻意不实现 `std::str::FromStr`，避免在泛型上下文中被误约束；
    /// 作为 inherent 方法保留以维持现有的调用风格。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<IdPrefix, IdError> {
        Ok(match s {
            "c" => IdPrefix::Character,
            "f" => IdPrefix::Fragment,
            "l" => IdPrefix::Location,
            "i" => IdPrefix::Item,
            "o" => IdPrefix::Organization,
            "e" => IdPrefix::Event,
            "b" => IdPrefix::Book,
            "ch" => IdPrefix::Chapter,
            "sc" => IdPrefix::Scene,
            "se" => IdPrefix::SemanticEdge,
            _ => return Err(IdError::UnknownPrefix(s.to_string())),
        })
    }

    /// 对应的默认目录（RFC-0003 §7 Path Conventions）
    pub fn default_dir(&self) -> Option<&'static str> {
        match self {
            IdPrefix::Fragment => Some("fragments"),
            IdPrefix::Character => Some("entities/characters"),
            IdPrefix::Location => Some("entities/locations"),
            IdPrefix::Item => Some("entities/items"),
            IdPrefix::Organization => Some("entities/organizations"),
            IdPrefix::Event => Some("entities/events"),
            _ => None,
        }
    }
}

// ============================================================================
// EdgeId
// ============================================================================

/// SemanticEdge 的 StableId（边是一级公民，RFC-0002 §7.2）
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdgeId(pub StableId);

impl EdgeId {
    pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
        let id = StableId::new(s)?;
        if !id.has_prefix(IdPrefix::SemanticEdge) {
            return Err(IdError::WrongPrefix {
                expected: IdPrefix::SemanticEdge,
                actual: id.prefix(),
            });
        }
        Ok(Self(id))
    }
}

impl std::fmt::Display for EdgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// ============================================================================
// Symbol
// ============================================================================

/// 可读的实体别名，用于 Parser 引用解析与 Prompt 上下文（RFC-0002 §4）
///
/// 可被作者重命名（RenameSymbol），不影响 StableId。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Symbol(String);

impl Symbol {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Symbol {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

// ============================================================================
// SymbolTable
// ============================================================================

/// Symbol → StableId 映射表 (RFC-0002 §4.3)
///
/// 由 Parser 在 Phase 1.5 构建（RFC-0006 §4）。
/// BTreeMap 保证确定序列化。
///
/// `#[serde(transparent)]` 使其序列化为直接的 `{ "hero": "c_001" }` 映射，
/// 而非 `{"map": {...}}` 包装。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolTable {
    map: BTreeMap<Symbol, StableId>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, symbol: Symbol, id: StableId) {
        self.map.insert(symbol, id);
    }

    pub fn get(&self, symbol: &Symbol) -> Option<&StableId> {
        self.map.get(symbol)
    }

    pub fn resolve(&self, symbol_str: &str) -> Option<&StableId> {
        self.map
            .iter()
            .find(|(s, _)| s.as_str() == symbol_str)
            .map(|(_, id)| id)
    }

    pub fn contains(&self, symbol_str: &str) -> bool {
        self.resolve(symbol_str).is_some()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Symbol, &StableId)> {
        self.map.iter()
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum IdError {
    #[error("ID 缺少下划线分隔符: {0}")]
    NoUnderscore(String),

    #[error("未知的前缀: {0}")]
    UnknownPrefix(String),

    #[error("前缀不匹配: 期望 {expected:?}, 实际 {actual:?}")]
    WrongPrefix {
        expected: IdPrefix,
        actual: IdPrefix,
    },
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_parse_character() {
        let id = StableId::new("c_8A91D4").unwrap();
        assert_eq!(id.prefix(), IdPrefix::Character);
        assert_eq!(id.as_str(), "c_8A91D4");
    }

    #[test]
    fn stable_id_parse_fragment() {
        let id = StableId::new("f_0001").unwrap();
        assert_eq!(id.prefix(), IdPrefix::Fragment);
    }

    #[test]
    fn stable_id_parse_semantic_edge() {
        let id = StableId::new("se_0033").unwrap();
        assert_eq!(id.prefix(), IdPrefix::SemanticEdge);
    }

    #[test]
    fn stable_id_rejects_unknown_prefix() {
        assert!(StableId::new("x_1234").is_err());
    }

    #[test]
    fn stable_id_rejects_no_underscore() {
        assert!(StableId::new("c8A91D4").is_err());
    }

    #[test]
    fn stable_id_ordering() {
        // BTreeMap 要求 Ord，测试排序正确性
        let a = StableId::new("c_0001").unwrap();
        let b = StableId::new("c_0002").unwrap();
        let c = StableId::new("f_0001").unwrap();
        assert!(a < b);
        assert!(b < c); // 'c' < 'f'
    }

    #[test]
    fn edge_id_requires_se_prefix() {
        assert!(EdgeId::new("se_0001").is_ok());
        assert!(EdgeId::new("c_0001").is_err()); // 错误前缀
    }

    #[test]
    fn symbol_table_resolve() {
        let mut table = SymbolTable::new();
        let symbol = Symbol::new("hero");
        let id = StableId::new("c_8A91D4").unwrap();
        table.insert(symbol, id.clone());

        assert_eq!(table.resolve("hero"), Some(&id));
        assert!(table.contains("hero"));
        assert!(!table.contains("villain"));
    }

    #[test]
    fn id_prefix_default_dir() {
        assert_eq!(IdPrefix::Fragment.default_dir(), Some("fragments"));
        assert_eq!(
            IdPrefix::Character.default_dir(),
            Some("entities/characters")
        );
        assert_eq!(IdPrefix::Book.default_dir(), None);
    }
}
