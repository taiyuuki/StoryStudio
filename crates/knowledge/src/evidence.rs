//! Evidence — 证据 (RFC-0015 §4)
//!
//! 一条断言的文本依据，指向 Fragment（复用 Location Index 定位），摘录原文。

use serde::{Deserialize, Serialize};

use story_ir::StableId;

// ============================================================================
// Evidence
// ============================================================================

/// 一条证据——断言的文本依据
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// 证据的 StableId（前缀 ev_）
    pub id: StableId,

    /// 源文本位置（Fragment 的 StableId）
    pub loc: StableId,

    /// 原文摘录（头段采样）
    pub quote: String,

    /// 中段采样摘录（漂移比对锚点之二；None = 旧数据，仅头段比对）
    #[serde(default)]
    pub quote_mid: Option<String>,

    /// 尾段采样摘录（漂移比对锚点之三；None = 旧数据，仅头段比对）
    #[serde(default)]
    pub quote_tail: Option<String>,

    /// 来源
    #[serde(default)]
    pub source: EvidenceSource,
}

// ============================================================================
// EvidenceSource
// ============================================================================

/// 证据来源
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EvidenceSource {
    /// 人工标注
    #[default]
    Manual,
    /// AI 提取
    Ai(String),
}

impl Evidence {
    pub fn new(id: StableId, loc: StableId, quote: impl Into<String>) -> Self {
        Self {
            id,
            loc,
            quote: quote.into(),
            quote_mid: None,
            quote_tail: None,
            source: EvidenceSource::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_basic() {
        let e = Evidence::new(
            StableId::new_unchecked("ev_001"),
            StableId::new_unchecked("f_0001"),
            "林风今年十七岁",
        );

        assert_eq!(e.loc.as_str(), "f_0001");
        assert_eq!(e.quote, "林风今年十七岁");
        assert_eq!(e.source, EvidenceSource::Manual);
    }
}
