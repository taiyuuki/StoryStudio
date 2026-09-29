//! project.toml — 项目级配置 (RFC-0003 §5.1)

use serde::{Deserialize, Serialize};
use story_ir::IRVersion;

/// project.toml 的完整内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
    #[serde(default)]
    pub ai: AiConfig,
}

/// [project] 段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// IR Schema 版本（RFC-0002 §10），Parser 据此执行迁移
    #[serde(default = "default_ir_version")]
    pub ir_version: IRVersion,
}

fn default_ir_version() -> IRVersion {
    IRVersion(1)
}

/// [knowledge] 段（RFC-0015 §8.1）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnowledgeConfig {
    /// 断言冲突：false=人工裁决, true=自动标 contradicted
    #[serde(default)]
    pub auto_contradict: bool,
}

/// [ai] 段（RFC-0010 §3.3）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extraction_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

impl ProjectConfig {
    /// 创建默认配置
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            project: ProjectSection {
                name: name.into(),
                author: None,
                created: Some(today_string()),
                language: Some("zh-CN".to_string()),
                ir_version: IRVersion(1),
            },
            knowledge: KnowledgeConfig::default(),
            ai: AiConfig::default(),
        }
    }

    /// 从 TOML 字符串解析
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// 序列化为 TOML 字符串
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

fn today_string() -> String {
    // 简单返回日期，不依赖 chrono（避免额外依赖）
    // 格式：YYYY-MM-DD
    // 注：这里用系统时间格式化
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 粗略计算日期（足以作为 created 字段，精确度要求不高）
    let days = secs / 86400;
    let (year, month, day) = days_to_date(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// 将 Unix 天数转为 (年, 月, 日) —— 粗略计算，足够 created 字段用
fn days_to_date(days_since_epoch: i64) -> (i32, u32, u32) {
    // 1970-01-01 = day 0
    let mut year = 1970i32;
    let mut remaining = days_since_epoch;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year as i64 {
            break;
        }
        remaining -= days_in_year as i64;
        year += 1;
    }

    let month_days = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for &md in &month_days {
        if remaining < md as i64 {
            break;
        }
        remaining -= md as i64;
        month += 1;
    }

    let day = remaining as u32 + 1;
    (year, month, day)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_config_roundtrip() {
        let config = ProjectConfig::new("雨夜行");
        let toml_str = config.to_toml_string().unwrap();
        let parsed = ProjectConfig::from_toml_str(&toml_str).unwrap();
        assert_eq!(parsed.project.name, "雨夜行");
        assert_eq!(parsed.project.ir_version, IRVersion(1));
        assert!(!parsed.knowledge.auto_contradict);
    }

    #[test]
    fn project_config_parse_example() {
        let toml_str = r#"
[project]
name = "雨夜行"
author = "Taiyuuki"
created = "2026-07-10"
language = "zh-CN"
ir_version = 1

[knowledge]
auto_contradict = false

[ai]
default_provider = "claude"
extraction_model = "claude-sonnet"
"#;
        let config = ProjectConfig::from_toml_str(toml_str).unwrap();
        assert_eq!(config.project.name, "雨夜行");
        assert_eq!(config.project.author.as_deref(), Some("Taiyuuki"));
        assert_eq!(config.ai.default_provider.as_deref(), Some("claude"));
    }

    #[test]
    fn days_to_date_basic() {
        assert_eq!(days_to_date(0), (1970, 1, 1));
        assert_eq!(days_to_date(31), (1970, 2, 1));
        assert_eq!(days_to_date(365), (1971, 1, 1));
    }
}
