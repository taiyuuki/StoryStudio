//! threads.toml — 叙事线索 (RFC-0017)
//!
//! Thread 是独立的叙事对象，管理故事节奏与信息流。
//! status 由系统从 planted_at/revealed_at/resolved_at/abandoned 推导，不手工维护。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// ThreadsFile
// ============================================================================

/// threads.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsFile {
    #[serde(default, rename = "thread")]
    pub threads: Vec<ThreadDef>,
}

/// 单条叙事线索的 TOML 表示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDef {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 叙事目标（给 AI 的上下文）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// 优先级：main | minor（默认 minor）
    #[serde(default = "default_priority")]
    pub priority: String,
    /// 可见性范围：reader（默认，面向读者的明线）| pov（暗线：POV 私人闭环，
    /// 允许 planted 直接 resolved，不触发「已回收未揭示」告警）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 计划埋设卷号（1-based，对应 chapters.toml 的 volume_order）。
    /// 未设置视为全书长线；已到卷仍未埋设时诊断升级为「本卷该埋未埋」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_volume: Option<u32>,
    /// 开放标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 关联的 Assertion（可选）
    #[serde(default)]
    pub assertions: Vec<String>,
    /// 叙事锚点
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revealed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    /// 唯一手工状态标记
    #[serde(default)]
    pub abandoned: bool,
    /// 回收网络
    #[serde(default)]
    pub resolves: Vec<String>,
    #[serde(default)]
    pub resolved_by: Vec<String>,
    /// 知识可见性
    #[serde(default)]
    pub knowledge: Vec<KnowledgeDef>,
    /// 叙事对象关系
    #[serde(default)]
    pub relationships: Vec<RelationshipDef>,
}

fn default_priority() -> String {
    "minor".to_string()
}

/// 知识可见性条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDef {
    /// 观察者：reader / author / omniscient / { character = "..." } / { organization = "..." }
    pub observer: ObserverDef,
    /// 状态时间线
    #[serde(default)]
    pub state_timeline: Vec<StateTimelineEntry>,
}

/// 观察者的 TOML 表示
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ObserverDef {
    /// reader / author / omniscient
    Builtin(String),
    /// { character = "c_001" }
    Character { character: String },
    /// { organization = "o_001" }
    Organization { organization: String },
}

/// 状态时间线条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTimelineEntry {
    pub scene: String,
    pub state: String,
}

/// 叙事对象关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipDef {
    pub entities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum ThreadsFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

impl ThreadsFile {
    pub fn from_toml_str(s: &str) -> Result<Self, ThreadsFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, ThreadsFileError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_thread() {
        let toml_str = r#"
[[thread]]
id = "t_001"
title = "林风的皇子身份"
description = "林风实际上是前朝皇子。"
goal = "第三幕揭晓身份"
priority = "main"
tags = ["foreshadowing", "identity"]
assertions = ["a_00152"]
planted_at = "sc_01_02"
abandoned = false

[[thread.knowledge]]
observer = "reader"
state_timeline = [
    { scene = "sc_01_02", state = "suspected" },
    { scene = "sc_03_01", state = "known" },
]

[[thread.knowledge]]
observer = { character = "c_001" }
state_timeline = [
    { scene = "sc_05_03", state = "known" },
]

[[thread.relationships]]
entities = ["c_001", "c_002"]
relation = "rivals"
"#;
        let file = ThreadsFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.threads.len(), 1);
        let t = &file.threads[0];
        assert_eq!(t.id, "t_001");
        assert_eq!(t.priority, "main");
        assert_eq!(t.tags, vec!["foreshadowing", "identity"]);
        assert_eq!(t.assertions, vec!["a_00152"]);
        assert_eq!(t.planted_at.as_deref(), Some("sc_01_02"));
        assert!(!t.abandoned);

        // knowledge
        assert_eq!(t.knowledge.len(), 2);
        assert!(matches!(&t.knowledge[0].observer, ObserverDef::Builtin(s) if s == "reader"));
        assert_eq!(t.knowledge[0].state_timeline.len(), 2);
        assert!(
            matches!(&t.knowledge[1].observer, ObserverDef::Character { character } if character == "c_001")
        );

        // relationships
        assert_eq!(t.relationships.len(), 1);
        assert_eq!(t.relationships[0].entities, vec!["c_001", "c_002"]);
    }

    #[test]
    fn parse_minimal_thread() {
        let toml_str = r#"
[[thread]]
id = "t_002"
title = "感情线"
"#;
        let file = ThreadsFile::from_toml_str(toml_str).unwrap();
        let t = &file.threads[0];
        assert_eq!(t.id, "t_002");
        assert_eq!(t.priority, "minor"); // 默认值
        assert!(t.due_volume.is_none()); // 默认全书长线
        assert!(t.assertions.is_empty());
        assert!(t.knowledge.is_empty());
        assert!(!t.abandoned);
    }

    #[test]
    fn parse_due_volume() {
        let toml_str = r#"
[[thread]]
id = "t_010"
title = "卷级线索"
priority = "main"
due_volume = 2
"#;
        let file = ThreadsFile::from_toml_str(toml_str).unwrap();
        assert_eq!(file.threads[0].due_volume, Some(2));

        // 序列化往返保持
        let serialized = file.to_toml_string().unwrap();
        assert!(serialized.contains("due_volume = 2"), "{serialized}");
        let reparsed = ThreadsFile::from_toml_str(&serialized).unwrap();
        assert_eq!(reparsed.threads[0].due_volume, Some(2));

        // 未设置的线索序列化时不应带 due_volume 字段
        let bare =
            ThreadsFile::from_toml_str("[[thread]]\nid = \"t_011\"\ntitle = \"x\"\n").unwrap();
        let s = bare.to_toml_string().unwrap();
        assert!(!s.contains("due_volume"), "{s}");
    }

    #[test]
    fn roundtrip() {
        let toml_str = r#"
[[thread]]
id = "t_001"
title = "测试"
priority = "main"
tags = ["mystery"]
planted_at = "sc_01"
abandoned = false

[[thread.knowledge]]
observer = "reader"
state_timeline = [{ scene = "sc_01", state = "known" }]
"#;
        let file = ThreadsFile::from_toml_str(toml_str).unwrap();
        let serialized = file.to_toml_string().unwrap();
        let reparsed = ThreadsFile::from_toml_str(&serialized).unwrap();
        assert_eq!(reparsed.threads.len(), 1);
        assert_eq!(reparsed.threads[0].id, "t_001");
    }

    #[test]
    fn empty_file() {
        let file = ThreadsFile::from_toml_str("").unwrap();
        assert!(file.threads.is_empty());
    }

    /// 验证 ObserverDef untagged enum 的三种形态在序列化后保持可被前端识别的形式：
    /// `"reader"` / `{ character = "c_0001" }` / `{ organization = "o_0001" }`。
    /// 这是前后端 schema 契约的关键——若序列化形态漂移，前端会读不出 observer。
    #[test]
    fn observer_forms_roundtrip() {
        let toml_str = r#"
[[thread]]
id = "t_005"
title = "主线"

[[thread.knowledge]]
observer = "reader"
state_timeline = [{ scene = "sc_01_01", state = "suspected" }]

[[thread.knowledge]]
observer = { character = "c_0001" }
state_timeline = [{ scene = "sc_01_01", state = "suspected" }]

[[thread.knowledge]]
observer = { organization = "o_0001" }
state_timeline = [{ scene = "sc_02_01", state = "known" }]
"#;
        let file = ThreadsFile::from_toml_str(toml_str).unwrap();
        let serialized = file.to_toml_string().unwrap();

        // 三种 observer 形态都必须在序列化结果中以可识别的字符串出现。
        assert!(
            serialized.contains("observer = \"reader\""),
            "builtin observer 形态丢失: {serialized}"
        );
        assert!(
            serialized.contains("character = \"c_0001\""),
            "character observer 形态丢失: {serialized}"
        );
        assert!(
            serialized.contains("organization = \"o_0001\""),
            "organization observer 形态丢失: {serialized}"
        );

        // 往返后 observer 类型仍正确。
        let reparsed = ThreadsFile::from_toml_str(&serialized).unwrap();
        let k = &reparsed.threads[0].knowledge;
        assert_eq!(k.len(), 3);
        assert!(matches!(&k[0].observer, ObserverDef::Builtin(s) if s == "reader"));
        assert!(
            matches!(&k[1].observer, ObserverDef::Character { character } if character == "c_0001")
        );
        assert!(
            matches!(&k[2].observer, ObserverDef::Organization { organization } if organization == "o_0001")
        );
    }
}
