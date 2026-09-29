//! Assertion — 可验证的命题 (RFC-0015 §4)
//!
//! 形如"主体 在 时间T 具有 属性P = 值V"。
//! 每条断言带 ValidityWindow（时间区间）和 Evidence（文本证据）。

use serde::{Deserialize, Serialize};

use story_ir::{StableId, ValidityWindow};

// ============================================================================
// Assertion
// ============================================================================

/// 一条可被验证的命题
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assertion {
    /// 断言的 StableId（前缀 a_）
    pub id: StableId,

    /// 断言的主体（通常是 Character / Item 的 StableId）
    pub subject: StableId,

    /// 谓词（如 alive / age / holds / located_at）
    pub predicate: Predicate,

    /// 断言的值
    pub object: ObjectValue,

    /// 有效时间区间
    #[serde(default)]
    pub valid: ValidityWindow,

    /// 支撑此断言的证据 ID 列表
    #[serde(default)]
    pub evidence: Vec<StableId>,

    /// 状态
    #[serde(default)]
    pub status: AssertionStatus,

    /// 产出方式
    #[serde(default)]
    pub extracted_by: ExtractedBy,
}

// ============================================================================
// Predicate
// ============================================================================

/// 断言的谓词类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Predicate(String);

impl Predicate {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    // 常用谓词
    pub fn alive() -> Self {
        Self::new("alive")
    }
    pub fn age() -> Self {
        Self::new("age")
    }
    pub fn holds() -> Self {
        Self::new("holds")
    }
    pub fn located_at() -> Self {
        Self::new("located_at")
    }
    pub fn status() -> Self {
        Self::new("status")
    }
}

impl std::fmt::Display for Predicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ============================================================================
// ObjectValue
// ============================================================================

/// 断言的值（支持多种类型）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ObjectValue {
    /// 布尔值（如 alive = true）
    Boolean(bool),
    /// 整数（如 age = 17）
    Integer(i64),
    /// 字符串（如 status = "hidden"）
    Text(String),
    /// 引用另一个实体（如 holds = i_umbrella）
    Reference(StableId),
}

impl ObjectValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ObjectValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            ObjectValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ObjectValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<bool> for ObjectValue {
    fn from(b: bool) -> Self {
        ObjectValue::Boolean(b)
    }
}

impl From<i64> for ObjectValue {
    fn from(i: i64) -> Self {
        ObjectValue::Integer(i)
    }
}

impl From<String> for ObjectValue {
    fn from(s: String) -> Self {
        ObjectValue::Text(s)
    }
}

// ============================================================================
// AssertionStatus
// ============================================================================

/// 断言状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AssertionStatus {
    /// 活跃（当前有效）
    #[default]
    Active,
    /// 已被后续断言取代
    Superseded,
    /// 与其他断言矛盾（待人工裁决）
    Contradicted,
}

// ============================================================================
// ExtractedBy
// ============================================================================

/// 断言的产出方式
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExtractedBy {
    /// 人工手动标注
    #[default]
    Manual,
    /// AI 抽取（标注模型）
    Ai(String),
}

impl ExtractedBy {
    pub fn is_ai(&self) -> bool {
        matches!(self, ExtractedBy::Ai(_))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ir::{StoryCalendar, StoryTime, ValidityWindow};

    #[test]
    fn assertion_alive() {
        let a = Assertion {
            id: StableId::new_unchecked("a_001"),
            subject: StableId::new_unchecked("c_001"),
            predicate: Predicate::alive(),
            object: ObjectValue::Boolean(true),
            valid: ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(1069))),
                None,
            ),
            evidence: vec![StableId::new_unchecked("ev_001")],
            status: AssertionStatus::Active,
            extracted_by: ExtractedBy::Manual,
        };

        assert_eq!(a.subject.as_str(), "c_001");
        assert_eq!(a.predicate.as_str(), "alive");
        assert_eq!(a.object.as_bool(), Some(true));
        assert!(a.valid.from.is_some());
        assert!(a.valid.until.is_none()); // 永久有效
    }

    #[test]
    fn assertion_age() {
        let a = Assertion {
            id: StableId::new_unchecked("a_002"),
            subject: StableId::new_unchecked("c_001"),
            predicate: Predicate::age(),
            object: ObjectValue::Integer(17),
            valid: ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(1086))),
                Some(StoryTime::absolute(StoryCalendar::from_year(1088))),
            ),
            evidence: vec![],
            status: AssertionStatus::Active,
            extracted_by: ExtractedBy::Ai("claude-sonnet".to_string()),
        };

        assert_eq!(a.object.as_integer(), Some(17));
        assert!(a.extracted_by.is_ai());
    }
}
