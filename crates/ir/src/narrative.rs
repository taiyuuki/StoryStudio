//! Narrative Layer — 叙事模型 (RFC-0017)
//!
//! Thread 是独立的叙事对象，管理故事节奏与信息流。
//! status 由系统从锚点 + abandoned 推导，不手工维护。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::StableId;

// ============================================================================
// ThreadStatus（推导状态，只读）
// ============================================================================

/// 推导出的 Thread 状态（§4.3）
///
/// 不持久化——每次从锚点 + abandoned 计算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadStatus {
    /// 计划中：planted_at 为 null
    Planned,
    /// 已埋下：planted_at 不为 null
    Planted,
    /// 已揭晓：revealed_at 不为 null
    Revealed,
    /// 已回收：resolved_at 不为 null
    Resolved,
    /// 已放弃：abandoned = true
    Abandoned,
}

impl ThreadStatus {
    /// 推导逻辑（纯函数，§4.3）
    pub fn derive(thread: &Thread) -> ThreadStatus {
        if thread.abandoned {
            return ThreadStatus::Abandoned;
        }
        if thread.resolved_at.is_some() {
            return ThreadStatus::Resolved;
        }
        if thread.revealed_at.is_some() {
            return ThreadStatus::Revealed;
        }
        if thread.planted_at.is_some() {
            return ThreadStatus::Planted;
        }
        ThreadStatus::Planned
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ThreadStatus::Planned => "planned",
            ThreadStatus::Planted => "planted",
            ThreadStatus::Revealed => "revealed",
            ThreadStatus::Resolved => "resolved",
            ThreadStatus::Abandoned => "abandoned",
        }
    }
}

// ============================================================================
// ThreadPriority
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ThreadPriority {
    Main,
    #[default]
    Minor,
}

impl ThreadPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThreadPriority::Main => "main",
            ThreadPriority::Minor => "minor",
        }
    }

    /// 从字符串解析，未识别值回落到 `Minor`（不返回 Result，故不实现 `FromStr`）。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "main" => ThreadPriority::Main,
            _ => ThreadPriority::Minor,
        }
    }
}

// ============================================================================
// Observer / KnowledgeState
// ============================================================================

/// 观察者类型（§4.2）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Observer {
    Reader,
    Author,
    Omniscient,
    Character { entity: StableId },
    Organization { entity: StableId },
}

/// 知晓程度（§4.2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnowledgeState {
    Unknown,
    Suspected,
    Known,
}

impl KnowledgeState {
    /// 从字符串解析，未识别值回落到 `Unknown`（不返回 Result，故不实现 `FromStr`）。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "suspected" => KnowledgeState::Suspected,
            "known" => KnowledgeState::Known,
            _ => KnowledgeState::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            KnowledgeState::Unknown => "unknown",
            KnowledgeState::Suspected => "suspected",
            KnowledgeState::Known => "known",
        }
    }
}

/// 知识可见性条目（§4.2）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadKnowledge {
    pub observer: Observer,
    /// 状态时间线（按场景排序）
    pub state_timeline: Vec<StateTimelineEntry>,
}

/// 状态时间线条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTimelineEntry {
    pub scene: StableId,
    pub state: KnowledgeState,
}

// ============================================================================
// Relationship
// ============================================================================

/// 叙事对象关系（§4.3）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRelationship {
    pub entities: Vec<StableId>,
    pub relation: Option<String>,
    pub description: Option<String>,
}

// ============================================================================
// Thread
// ============================================================================

/// 一条叙事线索（§4.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: StableId,
    pub title: String,
    pub description: Option<String>,
    pub goal: Option<String>,
    pub priority: ThreadPriority,
    pub tags: Vec<String>,
    pub assertions: Vec<StableId>,
    pub planted_at: Option<StableId>,
    pub revealed_at: Option<StableId>,
    pub resolved_at: Option<StableId>,
    pub abandoned: bool,
    pub resolves: Vec<StableId>,
    pub resolved_by: Vec<StableId>,
    pub knowledge: Vec<ThreadKnowledge>,
    pub relationships: Vec<ThreadRelationship>,
}

impl Thread {
    /// 推导当前状态（§4.3）
    pub fn status(&self) -> ThreadStatus {
        ThreadStatus::derive(self)
    }

    /// 查询某观察者在某场景的知识状态
    pub fn knowledge_state_at(&self, observer: &Observer, scene: &StableId) -> KnowledgeState {
        let entry = self.knowledge.iter().find(|k| &k.observer == observer);
        let Some(entry) = entry else {
            return KnowledgeState::Unknown;
        };

        // 默认 Unknown，遍历 timeline 找到最后一个 <= scene 的状态
        let mut state = KnowledgeState::Unknown;
        for timeline_entry in &entry.state_timeline {
            // 简化：按字符串比较 scene ID（完整实现应用 RFC-0009 的时序排序）
            if timeline_entry.scene.as_str() <= scene.as_str() {
                state = timeline_entry.state;
            }
        }
        state
    }
}

// ============================================================================
// ThreadCollection
// ============================================================================

/// 线索集合（从 threads.toml 加载）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadCollection {
    threads: BTreeMap<StableId, Thread>,
}

impl ThreadCollection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, thread: Thread) {
        self.threads.insert(thread.id.clone(), thread);
    }

    pub fn get(&self, id: &StableId) -> Option<&Thread> {
        self.threads.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = (&StableId, &Thread)> {
        self.threads.iter()
    }

    pub fn len(&self) -> usize {
        self.threads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_thread(id: &str) -> Thread {
        Thread {
            id: StableId::new_unchecked(id),
            title: "测试".to_string(),
            description: None,
            goal: None,
            priority: ThreadPriority::Minor,
            tags: vec![],
            assertions: vec![],
            planted_at: None,
            revealed_at: None,
            resolved_at: None,
            abandoned: false,
            resolves: vec![],
            resolved_by: vec![],
            knowledge: vec![],
            relationships: vec![],
        }
    }

    #[test]
    fn status_planned() {
        let t = make_thread("t_001");
        assert_eq!(t.status(), ThreadStatus::Planned);
    }

    #[test]
    fn status_planted() {
        let mut t = make_thread("t_001");
        t.planted_at = Some(StableId::new_unchecked("sc_01"));
        assert_eq!(t.status(), ThreadStatus::Planted);
    }

    #[test]
    fn status_revealed() {
        let mut t = make_thread("t_001");
        t.planted_at = Some(StableId::new_unchecked("sc_01"));
        t.revealed_at = Some(StableId::new_unchecked("sc_05"));
        assert_eq!(t.status(), ThreadStatus::Revealed);
    }

    #[test]
    fn status_resolved() {
        let mut t = make_thread("t_001");
        t.planted_at = Some(StableId::new_unchecked("sc_01"));
        t.resolved_at = Some(StableId::new_unchecked("sc_10"));
        assert_eq!(t.status(), ThreadStatus::Resolved);
    }

    #[test]
    fn status_abandoned_overrides() {
        let mut t = make_thread("t_001");
        t.planted_at = Some(StableId::new_unchecked("sc_01"));
        t.resolved_at = Some(StableId::new_unchecked("sc_10"));
        t.abandoned = true;
        assert_eq!(t.status(), ThreadStatus::Abandoned);
    }

    #[test]
    fn knowledge_state_query() {
        let mut t = make_thread("t_001");
        t.knowledge.push(ThreadKnowledge {
            observer: Observer::Reader,
            state_timeline: vec![
                StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_01"),
                    state: KnowledgeState::Suspected,
                },
                StateTimelineEntry {
                    scene: StableId::new_unchecked("sc_05"),
                    state: KnowledgeState::Known,
                },
            ],
        });

        // sc_01 之前 = Unknown
        assert_eq!(
            t.knowledge_state_at(&Observer::Reader, &StableId::new_unchecked("sc_00")),
            KnowledgeState::Unknown
        );
        // sc_01 = Suspected
        assert_eq!(
            t.knowledge_state_at(&Observer::Reader, &StableId::new_unchecked("sc_01")),
            KnowledgeState::Suspected
        );
        // sc_05 = Known
        assert_eq!(
            t.knowledge_state_at(&Observer::Reader, &StableId::new_unchecked("sc_05")),
            KnowledgeState::Known
        );
        // 未声明的观察者 = Unknown
        assert_eq!(
            t.knowledge_state_at(
                &Observer::Character {
                    entity: StableId::new_unchecked("c_001")
                },
                &StableId::new_unchecked("sc_05")
            ),
            KnowledgeState::Unknown
        );
    }
}
