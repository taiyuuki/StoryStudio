//! 实体锚点 (RFC-0002 §6.1, RFC-0003 §5.3)
//!
//! Fragment 通过 anchors 标注其涉及的 Semantic 实体出场。
//! anchors 是最高可信度层（RFC-0006 §2.3），由作者显式标注。

use serde::{Deserialize, Serialize};

use crate::id::StableId;

/// Fragment 锚定的实体出场
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityAnchor {
    /// 锚定的实体 StableId
    pub entity: StableId,
    /// 出场角色
    pub role: AnchorRole,
}

/// 实体出场角色 (RFC-0002 §6.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorRole {
    /// 实体在场
    Appears,
    /// 被提及但不在场
    Mentioned,
    /// 间接引用
    Referenced,
}

impl AnchorRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorRole::Appears => "appears",
            AnchorRole::Mentioned => "mentioned",
            AnchorRole::Referenced => "referenced",
        }
    }
}

impl std::str::FromStr for AnchorRole {
    type Err = AnchorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "appears" => Ok(AnchorRole::Appears),
            "mentioned" => Ok(AnchorRole::Mentioned),
            "referenced" => Ok(AnchorRole::Referenced),
            _ => Err(AnchorError::InvalidRole(s.to_string())),
        }
    }
}

impl std::fmt::Display for AnchorRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AnchorError {
    #[error("无效的出场角色: {0}")]
    InvalidRole(String),
}
