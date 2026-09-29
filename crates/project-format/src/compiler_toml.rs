//! config/compiler.toml — Lint 规则配置 (RFC-0003 §5.6)

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// config/compiler.toml 的完整内容
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompilerConfigFile {
    /// rule_id → severity（"error" / "warning" / "hint" / "info" / "off"）
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
    /// 插件规则配置
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginRuleConfig>,
    /// 全局严重级别覆盖
    #[serde(default)]
    pub severity_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginRuleConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
}

fn default_true() -> bool {
    true
}

impl CompilerConfigFile {
    /// 从 TOML 字符串解析
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// 序列化
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example() {
        let toml_str = r#"
[rules]
dead_character_revived = "error"
age_inconsistency = "error"
item_disappeared = "warning"

[plugins.weather_consistency]
enabled = true
severity = "warning"

[severity_overrides]
item_disappeared = "error"
"#;
        let config = CompilerConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(
            config
                .rules
                .get("dead_character_revived")
                .map(|s| s.as_str()),
            Some("error")
        );
        assert_eq!(config.plugins.len(), 1);
        assert_eq!(
            config
                .severity_overrides
                .get("item_disappeared")
                .map(|s| s.as_str()),
            Some("error")
        );
    }
}
