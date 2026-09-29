//! structure.toml — 文档结构树 (RFC-0003 §5.2)
//!
//! 这是所有序列关系的唯一 Source of Truth。
//! Book → Chapter → Scene → Fragment 的层级与顺序都在这里。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// structure.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructureFile {
    pub book: BookDef,
    /// chapter_id → Chapter 定义
    #[serde(default)]
    pub chapter: BTreeMap<String, ChapterDef>,
    /// scene_id → Scene 定义
    #[serde(default)]
    pub scene: BTreeMap<String, SceneDef>,
}

/// [book] 段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookDef {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Chapter 顺序（数组 = 顺序，这是 Chapter 序列的唯一来源）
    #[serde(default)]
    pub chapters: Vec<String>,
}

impl Default for BookDef {
    fn default() -> Self {
        Self {
            id: "b_0001".to_string(),
            title: String::new(),
            symbol: None,
            chapters: Vec::new(),
        }
    }
}

/// [chapter.xxx] 段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterDef {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Scene 顺序（唯一来源）
    #[serde(default)]
    pub scenes: Vec<String>,
}

/// [scene.xxx] 段
///
/// 注意：fragments 数组是 Fragment 顺序的唯一来源（RFC-0003 §3）。
/// Fragment 文件不存 scene 字段——归属由此列表决定。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 视角角色的 StableId (RFC-0007 §3)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pov: Option<String>,
    /// Fragment 顺序（唯一来源）
    #[serde(default)]
    pub fragments: Vec<String>,
    /// 故事时间 (RFC-0009 §4.1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story_time: Option<StoryTimeDef>,
    /// 情绪基调 (RFC-0007 §4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mood: Option<String>,
    /// 叙事功能 (RFC-0007 §5)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// 场景摘要（AI 可生成）(RFC-0007 §6)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// story_time 的 TOML 表示
///
/// 三种表达（可组合 anchor+gap）：
/// - 数值锚：year（可带 month/day），如 1023-9-15
/// - 历法标签锚：anchor 自由文本（如"灵历3217年秋"），经项目历法
///   （worldview.toml [timeline].eras）解析为可比较序数，解析不了仍可展示
/// - gap：与阅读序前一场景的时间间隔（如"3天""十年"），模糊描述合法
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoryTimeDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
}

impl StructureFile {
    /// 从 TOML 字符串解析
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// 序列化为 TOML 字符串
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// 创建空结构（只有 Book，无 Chapter）
    pub fn new(book_title: impl Into<String>) -> Self {
        Self {
            book: BookDef {
                id: "b_0001".to_string(),
                title: book_title.into(),
                symbol: None,
                chapters: Vec::new(),
            },
            chapter: BTreeMap::new(),
            scene: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_roundtrip() {
        let toml_str = r#"
[book]
id = "b_0001"
title = "雨夜行"
symbol = "rain_night"
chapters = ["ch_01", "ch_02"]

[chapter.ch_01]
title = "雨夜"
scenes = ["sc_01_01", "sc_01_02"]

[chapter.ch_02]
title = "渡河"
scenes = ["sc_02_01"]

[scene.sc_01_01]
title = "推门"
pov = "c_8A91D4"
fragments = ["f_0001", "f_0002"]
summary = "林风冒雨推开客栈大门，与掌柜交涉住宿"

[scene.sc_01_02]
title = "河岸"
pov = "c_8A91D4"
fragments = ["f_0003", "f_0004", "f_0005"]

[scene.sc_02_01]
pov = "c_9B3E01"
fragments = ["f_0006"]
"#;
        let structure = StructureFile::from_toml_str(toml_str).unwrap();

        assert_eq!(structure.book.title, "雨夜行");
        assert_eq!(structure.book.chapters, vec!["ch_01", "ch_02"]);
        assert_eq!(
            structure.chapter["ch_01"].scenes,
            vec!["sc_01_01", "sc_01_02"]
        );
        assert_eq!(
            structure.scene["sc_01_02"].fragments,
            vec!["f_0003", "f_0004", "f_0005"]
        );
        assert_eq!(structure.scene["sc_01_01"].pov.as_deref(), Some("c_8A91D4"));
        assert_eq!(
            structure.scene["sc_01_01"].summary.as_deref(),
            Some("林风冒雨推开客栈大门，与掌柜交涉住宿")
        );

        // 序列化后重新解析，验证确定性
        let serialized = structure.to_toml_string().unwrap();
        let reparsed = StructureFile::from_toml_str(&serialized).unwrap();
        assert_eq!(structure.book.chapters, reparsed.book.chapters);
    }

    #[test]
    fn empty_structure() {
        let structure = StructureFile::new("新书");
        assert_eq!(structure.book.title, "新书");
        assert!(structure.book.chapters.is_empty());
        assert!(structure.chapter.is_empty());
    }
}
