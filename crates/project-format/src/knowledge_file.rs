//! Knowledge Base 文件格式 (RFC-0003 §5.5)
//!
//! KB 采用 TOML 集中存储：
//!   knowledge/assertions.toml — 所有断言
//!   knowledge/evidence.toml   — 所有证据
//!
//! 解决 RFC-0015 Open Question #3（KB 存储格式）。

use serde::{Deserialize, Serialize};
use story_ir::{StableId, StoryCalendar, StoryTime, TimeCoord, ValidityWindow};
use story_knowledge::{
    Assertion, AssertionStatus, Evidence, EvidenceSource, ExtractedBy, ObjectValue, Predicate,
};
use thiserror::Error;

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum KnowledgeFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("无效的 object 值: {0}")]
    InvalidObject(String),

    #[error("无效的 StoryTime 格式: {0}")]
    InvalidStoryTime(String),
}

// ============================================================================
// AssertionsFile — assertions.toml
// ============================================================================

/// assertions.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssertionsFile {
    #[serde(default, rename = "assertion")]
    pub assertions: Vec<AssertionDef>,
}

/// 单条断言的 TOML 表示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionDef {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: toml::Value,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub extracted_by: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub valid: Option<ValidityDef>,
}

/// valid 区间的 TOML 表示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidityDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<StoryTimeDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<StoryTimeDef>,
}

/// StoryTime 的 TOML 表示（简化为 year/month/day）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryTimeDef {
    pub year: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
}

impl AssertionsFile {
    pub fn from_toml_str(s: &str) -> Result<Self, KnowledgeFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, KnowledgeFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// 转换为 KnowledgeBase 的 Assertion 列表
    pub fn to_assertions(&self) -> Result<Vec<Assertion>, KnowledgeFileError> {
        self.assertions.iter().map(|d| d.to_assertion()).collect()
    }

    /// 从 Assertion 列表构建
    pub fn from_assertions(assertions: &[Assertion]) -> Self {
        Self {
            assertions: assertions
                .iter()
                .map(AssertionDef::from_assertion)
                .collect(),
        }
    }
}

impl AssertionDef {
    fn to_assertion(&self) -> Result<Assertion, KnowledgeFileError> {
        let object = parse_object(&self.object)?;
        let status = parse_status(&self.status);
        let extracted_by = parse_extracted_by(&self.extracted_by);
        let valid = self
            .valid
            .as_ref()
            .map(|v| v.to_validity_window())
            .unwrap_or_default();

        Ok(Assertion {
            id: StableId::new_unchecked(&self.id),
            subject: StableId::new_unchecked(&self.subject),
            predicate: Predicate::new(&self.predicate),
            object,
            valid,
            evidence: self.evidence.iter().map(StableId::new_unchecked).collect(),
            status,
            extracted_by,
        })
    }

    fn from_assertion(a: &Assertion) -> Self {
        Self {
            id: a.id.as_str().to_string(),
            subject: a.subject.as_str().to_string(),
            predicate: a.predicate.as_str().to_string(),
            object: serialize_object(&a.object),
            status: serialize_status(a.status),
            extracted_by: serialize_extracted_by(&a.extracted_by),
            evidence: a.evidence.iter().map(|s| s.as_str().to_string()).collect(),
            valid: Some(ValidityDef::from_validity(&a.valid)),
        }
    }
}

impl ValidityDef {
    fn to_validity_window(&self) -> ValidityWindow {
        ValidityWindow::new(
            self.from.as_ref().map(|t| t.to_story_time()),
            self.until.as_ref().map(|t| t.to_story_time()),
        )
    }

    fn from_validity(v: &ValidityWindow) -> Self {
        Self {
            from: v.from.as_ref().and_then(StoryTimeDef::from_story_time),
            until: v.until.as_ref().and_then(StoryTimeDef::from_story_time),
        }
    }
}

impl StoryTimeDef {
    fn to_story_time(&self) -> StoryTime {
        StoryTime::absolute(StoryCalendar {
            year: self.year,
            month: self.month,
            day: self.day,
        })
    }

    fn from_story_time(t: &StoryTime) -> Option<Self> {
        match &t.coord {
            TimeCoord::Absolute(cal) => Some(Self {
                year: cal.year,
                month: cal.month,
                day: cal.day,
            }),
            _ => None, // Narrative/BeforeStory/AfterStory/Unknown 暂不序列化
        }
    }
}

// ============================================================================
// EvidenceFile — evidence.toml
// ============================================================================

/// evidence.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvidenceFile {
    #[serde(default, rename = "evidence")]
    pub evidence: Vec<EvidenceDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceDef {
    pub id: String,
    pub loc: String,
    pub quote: String,
    #[serde(default)]
    pub source: String,
}

impl EvidenceFile {
    pub fn from_toml_str(s: &str) -> Result<Self, KnowledgeFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, KnowledgeFileError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn to_evidence(&self) -> Vec<Evidence> {
        self.evidence.iter().map(|d| d.to_evidence()).collect()
    }

    pub fn from_evidence(evidence: &[Evidence]) -> Self {
        Self {
            evidence: evidence.iter().map(EvidenceDef::from_evidence).collect(),
        }
    }
}

impl EvidenceDef {
    fn to_evidence(&self) -> Evidence {
        Evidence {
            id: StableId::new_unchecked(&self.id),
            loc: StableId::new_unchecked(&self.loc),
            quote: self.quote.clone(),
            source: parse_evidence_source(&self.source),
        }
    }

    fn from_evidence(e: &Evidence) -> Self {
        Self {
            id: e.id.as_str().to_string(),
            loc: e.loc.as_str().to_string(),
            quote: e.quote.clone(),
            source: serialize_evidence_source(&e.source),
        }
    }
}

// ============================================================================
// 序列化/反序列化辅助
// ============================================================================

fn parse_object(v: &toml::Value) -> Result<ObjectValue, KnowledgeFileError> {
    match v {
        toml::Value::Boolean(b) => Ok(ObjectValue::Boolean(*b)),
        toml::Value::Integer(i) => Ok(ObjectValue::Integer(*i)),
        toml::Value::String(s) => {
            // 如果字符串像 StableId（含下划线前缀），视为引用
            if StableId::new(s).is_ok() {
                Ok(ObjectValue::Reference(StableId::new_unchecked(s)))
            } else {
                Ok(ObjectValue::Text(s.clone()))
            }
        }
        other => Err(KnowledgeFileError::InvalidObject(other.to_string())),
    }
}

fn serialize_object(v: &ObjectValue) -> toml::Value {
    match v {
        ObjectValue::Boolean(b) => toml::Value::Boolean(*b),
        ObjectValue::Integer(i) => toml::Value::Integer(*i),
        ObjectValue::Text(s) => toml::Value::String(s.clone()),
        ObjectValue::Reference(id) => toml::Value::String(id.as_str().to_string()),
    }
}

fn parse_status(s: &str) -> AssertionStatus {
    match s {
        "superseded" => AssertionStatus::Superseded,
        "contradicted" => AssertionStatus::Contradicted,
        _ => AssertionStatus::Active,
    }
}

fn serialize_status(s: AssertionStatus) -> String {
    match s {
        AssertionStatus::Active => "active".to_string(),
        AssertionStatus::Superseded => "superseded".to_string(),
        AssertionStatus::Contradicted => "contradicted".to_string(),
    }
}

fn parse_extracted_by(s: &str) -> ExtractedBy {
    if s.is_empty() || s == "manual" {
        ExtractedBy::Manual
    } else if let Some(model) = s.strip_prefix("ai:") {
        ExtractedBy::Ai(model.to_string())
    } else {
        ExtractedBy::Manual
    }
}

fn serialize_extracted_by(e: &ExtractedBy) -> String {
    match e {
        ExtractedBy::Manual => "manual".to_string(),
        ExtractedBy::Ai(model) => format!("ai:{model}"),
    }
}

fn parse_evidence_source(s: &str) -> EvidenceSource {
    if s.is_empty() || s == "manual" {
        EvidenceSource::Manual
    } else if let Some(model) = s.strip_prefix("ai:") {
        EvidenceSource::Ai(model.to_string())
    } else {
        EvidenceSource::Manual
    }
}

fn serialize_evidence_source(s: &EvidenceSource) -> String {
    match s {
        EvidenceSource::Manual => "manual".to_string(),
        EvidenceSource::Ai(model) => format!("ai:{model}"),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertions_roundtrip() {
        let toml_str = r#"
[[assertion]]
id = "a_001"
subject = "c_8A91D4"
predicate = "age"
object = 17
status = "active"
extracted_by = "manual"
evidence = ["ev_001"]

[assertion.valid]
from = { year = 1086 }
until = { year = 1088 }

[[assertion]]
id = "a_002"
subject = "c_8A91D4"
predicate = "alive"
object = false
status = "active"
extracted_by = "ai:claude-sonnet"

[assertion.valid]
from = { year = 1092 }
"#;
        let file = AssertionsFile::from_toml_str(toml_str).unwrap();
        let assertions = file.to_assertions().unwrap();

        assert_eq!(assertions.len(), 2);
        assert_eq!(assertions[0].predicate.as_str(), "age");
        assert_eq!(assertions[0].object.as_integer(), Some(17));
        assert!(assertions[0].valid.from.is_some());
        assert!(assertions[0].valid.until.is_some());

        assert_eq!(assertions[1].predicate.as_str(), "alive");
        assert_eq!(assertions[1].object.as_bool(), Some(false));
        assert!(assertions[1].valid.until.is_none()); // 永久

        // Roundtrip
        let file2 = AssertionsFile::from_assertions(&assertions);
        let serialized = file2.to_toml_string().unwrap();
        let reparsed = AssertionsFile::from_toml_str(&serialized).unwrap();
        let re_assertions = reparsed.to_assertions().unwrap();
        assert_eq!(re_assertions.len(), 2);
        assert_eq!(re_assertions[0].object, assertions[0].object);
    }

    #[test]
    fn evidence_roundtrip() {
        let toml_str = r#"
[[evidence]]
id = "ev_001"
loc = "f_0001"
quote = "林风今年十七岁"
source = "manual"
"#;
        let file = EvidenceFile::from_toml_str(toml_str).unwrap();
        let evidence = file.to_evidence();

        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].quote, "林风今年十七岁");
        assert_eq!(evidence[0].loc.as_str(), "f_0001");
    }

    #[test]
    fn object_reference_vs_text() {
        // 像 StableId 的字符串解析为 Reference
        let v = toml::Value::String("i_A7E902".to_string());
        let obj = parse_object(&v).unwrap();
        assert!(matches!(obj, ObjectValue::Reference(_)));

        // 普通字符串解析为 Text
        let v = toml::Value::String("hidden".to_string());
        let obj = parse_object(&v).unwrap();
        assert!(matches!(obj, ObjectValue::Text(_)));
    }
}
