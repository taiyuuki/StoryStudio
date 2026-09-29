//! chapters.toml — 章节脉络 (Chapter Outline)
//!
//! 介于全局大纲（outline.toml）与章节实现（structure.toml）之间的设计层：
//! 把大纲展开成逐章的叙事脉络——本章目标、节拍、涉及角色/线索、章末钩子。
//! 同时承载卷-章分组映射与每章创作进度。
//!
//! 与 outline.toml 一样属于设计层，不参与 Compiler 检查。
//! chapter_id 与 structure.toml 的 `ch_XX` 对齐；设计阶段可为 None（虚拟章），
//! 真正动笔时用 create_chapter 落实到 structure.toml 并回填此字段。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// ChaptersFile
// ============================================================================

/// chapters.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChaptersFile {
    /// 卷分组（章-卷映射的来源）
    #[serde(default, rename = "volumes")]
    pub volumes: Vec<VolumeGroupDef>,
    /// 章节脉络（顺序数组即章序）
    #[serde(default, rename = "chapters")]
    pub chapters: Vec<ChapterOutlineDef>,
}

/// [[volumes]] 卷分组
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeGroupDef {
    pub name: String,
    /// 卷序号（1-based），章节通过 volume_order 引用
    #[serde(default)]
    pub order: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// [[chapters]] 单章脉络
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterOutlineDef {
    /// 对齐 structure.toml 的章节 ID（ch_XX）。设计阶段可为 None（虚拟章）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter_id: Option<String>,
    pub title: String,
    /// 所属卷序号（1-based）；0 = 未分卷
    #[serde(default)]
    pub volume_order: u32,
    /// 章节梗概
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 本章目标（推进什么剧情/角色弧线）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// 叙事节拍/情节点（有序）
    #[serde(default)]
    pub beats: Vec<String>,
    /// 涉及角色 ID
    #[serde(default)]
    pub characters: Vec<String>,
    /// 涉及线索 ID
    #[serde(default)]
    pub threads: Vec<String>,
    /// 场景规划（预留，通常动笔时由 create_scene 落实）
    #[serde(default)]
    pub scenes: Vec<String>,
    /// 章末钩子（悬念/转折提示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    /// 其他备注
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// 创作进度：planned / writing / completed
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "planned".to_string()
}

/// 合法的章节进度状态
pub const STATUS_PLANNED: &str = "planned";
pub const STATUS_WRITING: &str = "writing";
pub const STATUS_COMPLETED: &str = "completed";

/// 校验 status 是否合法
pub fn is_valid_status(s: &str) -> bool {
    matches!(s, STATUS_PLANNED | STATUS_WRITING | STATUS_COMPLETED)
}

impl Default for ChapterOutlineDef {
    fn default() -> Self {
        Self {
            chapter_id: None,
            title: String::new(),
            volume_order: 0,
            summary: None,
            goal: None,
            beats: Vec::new(),
            characters: Vec::new(),
            threads: Vec::new(),
            scenes: Vec::new(),
            hook: None,
            notes: None,
            status: default_status(),
        }
    }
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum ChaptersFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

impl ChaptersFile {
    pub fn from_toml_str(s: &str) -> Result<Self, ChaptersFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, ChaptersFileError> {
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
    fn parse_full_chapters() {
        let toml_str = r#"
[[volumes]]
name = "第一卷：风起"
order = 1
theme = "离别与成长"
summary = "主角踏上旅途"

[[volumes]]
name = "第二卷：云涌"
order = 2

[[chapters]]
chapter_id = "ch_01"
title = "雨夜"
volume_order = 1
summary = "林风冒雨投宿客栈"
goal = "引出主角身份疑点"
beats = ["登场", "冲突", "悬念"]
characters = ["c_001"]
threads = ["t_001"]
hook = "掌柜提起十年前的旧案"
status = "completed"

[[chapters]]
title = "渡河"
volume_order = 1
goal = "渡河遇险"
status = "planned"
"#;
        let file = ChaptersFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.volumes.len(), 2);
        assert_eq!(file.volumes[0].name, "第一卷：风起");
        assert_eq!(file.volumes[0].order, 1);
        assert_eq!(file.volumes[1].theme, None);

        assert_eq!(file.chapters.len(), 2);
        assert_eq!(file.chapters[0].chapter_id.as_deref(), Some("ch_01"));
        assert_eq!(file.chapters[0].title, "雨夜");
        assert_eq!(file.chapters[0].volume_order, 1);
        assert_eq!(file.chapters[0].beats, vec!["登场", "冲突", "悬念"]);
        assert_eq!(file.chapters[0].characters, vec!["c_001"]);
        assert_eq!(file.chapters[0].threads, vec!["t_001"]);
        assert_eq!(
            file.chapters[0].hook.as_deref(),
            Some("掌柜提起十年前的旧案")
        );
        assert_eq!(file.chapters[0].status, "completed");

        // 虚拟章：chapter_id 为 None
        assert_eq!(file.chapters[1].chapter_id, None);
        assert_eq!(file.chapters[1].beats, Vec::<String>::new());
        assert_eq!(file.chapters[1].status, "planned");
    }

    #[test]
    fn parse_empty_chapters() {
        let file = ChaptersFile::from_toml_str("").unwrap();
        assert!(file.volumes.is_empty());
        assert!(file.chapters.is_empty());
    }

    #[test]
    fn roundtrip_chapters() {
        let mut file = ChaptersFile::default();
        file.volumes.push(VolumeGroupDef {
            name: "上卷".into(),
            order: 1,
            theme: Some("开篇".into()),
            summary: None,
        });
        file.chapters.push(ChapterOutlineDef {
            chapter_id: Some("ch_01".into()),
            title: "序章".into(),
            volume_order: 1,
            summary: Some("故事开端".into()),
            goal: None,
            beats: vec!["节拍一".into()],
            characters: vec!["c_001".into(), "c_002".into()],
            threads: Vec::new(),
            scenes: Vec::new(),
            hook: None,
            notes: Some("注意伏笔".into()),
            status: "writing".into(),
        });

        let toml_str = file.to_toml_string().unwrap();
        let parsed = ChaptersFile::from_toml_str(&toml_str).unwrap();
        assert_eq!(parsed.volumes.len(), 1);
        assert_eq!(parsed.volumes[0].name, "上卷");
        assert_eq!(parsed.chapters.len(), 1);
        assert_eq!(parsed.chapters[0].title, "序章");
        assert_eq!(parsed.chapters[0].chapter_id.as_deref(), Some("ch_01"));
        assert_eq!(parsed.chapters[0].characters, vec!["c_001", "c_002"]);
        assert_eq!(parsed.chapters[0].beats, vec!["节拍一"]);
        assert_eq!(parsed.chapters[0].status, "writing");
    }

    #[test]
    fn default_status_is_planned() {
        let c = ChapterOutlineDef::default();
        assert_eq!(c.status, "planned");
        assert!(is_valid_status(&c.status));
        assert!(is_valid_status("completed"));
        assert!(!is_valid_status("draft"));
    }
}
