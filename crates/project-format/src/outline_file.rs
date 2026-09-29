//! outline.toml — 故事大纲 (RFC-0016)
//!
//! 作者的故事设计文档——大纲、结构节拍、角色弧线。
//! 不参与 Compiler 检查，是设计层的记录。

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// OutlineFile
// ============================================================================

/// outline.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutlineFile {
    #[serde(default)]
    pub story: StoryMeta,
    #[serde(default, rename = "volumes")]
    pub volumes: Vec<VolumeDef>,
    #[serde(default, rename = "arcs")]
    pub arcs: Vec<ArcDef>,
    #[serde(default, rename = "storylines")]
    pub storylines: Vec<StorylineDef>,
    #[serde(default, rename = "conflicts")]
    pub conflicts: Vec<ConflictDef>,
}

/// [story] 段
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoryMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premise: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_span: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// 目标字数（中文）— 让 AI 了解创作规模，用于规划篇幅和节奏
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_words: Option<u64>,
    /// 单章目标字数 — 逐章成书与续写的篇幅基准
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_words: Option<u64>,
    /// 叙事视角（如：第三人称限知）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pov: Option<String>,
    /// 文风基调（如：热血 / 轻松 / 暗黑 / 细腻）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// [[arcs]] 角色弧线
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcDef {
    pub character: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stages: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// [[storylines]] 主线/支线
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorylineDef {
    /// 关联的线索 ID（可选——故事线不强制绑定线索）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// [[volumes]] 卷结构 — 长篇小说的宏观分卷
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// [[conflicts]] 冲突关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictDef {
    pub name: String,
    /// 冲突类型：character（角色间）/ faction（势力间）/ internal（角色内心）/ cosmic（天命/世界级）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 冲突双方或多方描述（自由文本，如 "主角 vs 反派"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parties: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 冲突在哪一幕/卷达到高潮
    #[serde(skip_serializing_if = "Option::is_none")]
    pub climax: Option<String>,
}

// ============================================================================
// 错误
// ============================================================================

#[derive(Debug, Error)]
pub enum OutlineFileError {
    #[error("TOML 解析失败: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

impl OutlineFile {
    pub fn from_toml_str(s: &str) -> Result<Self, OutlineFileError> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml_string(&self) -> Result<String, OutlineFileError> {
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
    fn parse_full_outline() {
        let toml_str = r#"
[story]
title = "雨夜行"
genre = "武侠悬疑"
premise = "一个剑客寻找过去"
theme = "身份认同"
setting = "江南小镇"
time_span = "三个月"
audience = "青年"
target_words = 800000

[[arcs]]
character = "c_001"
name = "林风的成长"
stages = "迷茫→觉醒"

[[storylines]]
thread = "t_001"
name = "主线"
"#;
        let file = OutlineFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.story.title.as_deref(), Some("雨夜行"));
        assert_eq!(file.story.setting.as_deref(), Some("江南小镇"));
        assert_eq!(file.story.time_span.as_deref(), Some("三个月"));
        assert_eq!(file.story.audience.as_deref(), Some("青年"));
        assert_eq!(file.story.target_words, Some(800000));
        assert_eq!(file.arcs.len(), 1);
        assert_eq!(file.arcs[0].character, "c_001");
        assert_eq!(file.storylines.len(), 1);
        assert_eq!(file.storylines[0].thread.as_deref(), Some("t_001"));
        // 新字段默认为空
        assert!(file.volumes.is_empty());
        assert!(file.conflicts.is_empty());
    }

    #[test]
    fn parse_empty_outline() {
        let file = OutlineFile::from_toml_str("").unwrap();
        assert!(file.arcs.is_empty());
        assert!(file.volumes.is_empty());
        assert!(file.conflicts.is_empty());
    }

    #[test]
    fn parse_volumes_and_conflicts() {
        let toml_str = r#"
[[volumes]]
name = "第一卷：风起"
summary = "主角踏上旅途"
theme = "离别与成长"

[[volumes]]
name = "第二卷：云涌"

[[conflicts]]
name = "宿命之争"
kind = "character"
parties = "林风 vs 暗影"
summary = "贯穿全书的核心对抗"
climax = "第三幕"

[[conflicts]]
name = "朝廷与江湖"
kind = "faction"
"#;
        let file = OutlineFile::from_toml_str(toml_str).unwrap();

        assert_eq!(file.volumes.len(), 2);
        assert_eq!(file.volumes[0].name, "第一卷：风起");
        assert_eq!(file.volumes[1].summary, None);

        assert_eq!(file.conflicts.len(), 2);
        assert_eq!(file.conflicts[0].name, "宿命之争");
        assert_eq!(file.conflicts[0].kind.as_deref(), Some("character"));
        assert_eq!(file.conflicts[0].climax.as_deref(), Some("第三幕"));
        assert_eq!(file.conflicts[1].kind.as_deref(), Some("faction"));
    }

    #[test]
    fn roundtrip_volumes_and_conflicts() {
        let mut file = OutlineFile::default();
        file.volumes.push(VolumeDef {
            name: "上卷".into(),
            summary: Some("开篇".into()),
            theme: None,
        });
        file.conflicts.push(ConflictDef {
            name: "核心冲突".into(),
            kind: Some("internal".into()),
            parties: Some("主角内心".into()),
            summary: None,
            climax: None,
        });

        let toml_str = file.to_toml_string().unwrap();
        let parsed = OutlineFile::from_toml_str(&toml_str).unwrap();
        assert_eq!(parsed.volumes.len(), 1);
        assert_eq!(parsed.volumes[0].name, "上卷");
        assert_eq!(parsed.conflicts.len(), 1);
        assert_eq!(parsed.conflicts[0].kind.as_deref(), Some("internal"));
    }
}
