//! 时间系统 (RFC-0009 Timeline)
//!
//! StoryTime 是 Story Studio 的时序坐标系，为 Assertion 的 valid 窗口、
//! SemanticEdge 的时序关系、Compiler 的时序规则提供统一度量。

use serde::{Deserialize, Serialize};

use crate::id::StableId;

// ============================================================================
// StoryCalendar — 故事内历法
// ============================================================================

/// 故事内历法的某年某月某日
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StoryCalendar {
    pub year: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
}

impl StoryCalendar {
    pub fn from_year(year: i32) -> Self {
        Self {
            year,
            month: None,
            day: None,
        }
    }

    pub fn from_year_month(year: i32, month: u8) -> Self {
        Self {
            year,
            month: Some(month),
            day: None,
        }
    }
}

// ============================================================================
// TimeCoord — 时间坐标
// ============================================================================

/// 时间坐标的类型 (RFC-0009 §3.1)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeCoord {
    /// 绝对纪年
    Absolute(StoryCalendar),
    /// 叙事坐标（绑定到 Scene / Fragment）
    Narrative(StableId),
    /// 故事开始前
    BeforeStory,
    /// 故事结束后
    AfterStory,
    /// 未知
    Unknown,
}

// ============================================================================
// TimePrecision
// ============================================================================

/// 时间精度 (RFC-0009 §3.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimePrecision {
    Year,
    Month,
    Day,
    Scene,
    Fragment,
    Unspecified,
}

// ============================================================================
// StoryTime — 统一时间类型
// ============================================================================

/// 故事时间的统一表示 (RFC-0009 §3.1)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StoryTime {
    pub coord: TimeCoord,
    #[serde(default = "default_precision")]
    pub precision: TimePrecision,
}

fn default_precision() -> TimePrecision {
    TimePrecision::Unspecified
}

impl StoryTime {
    pub fn absolute(cal: StoryCalendar) -> Self {
        let precision = if cal.day.is_some() {
            TimePrecision::Day
        } else if cal.month.is_some() {
            TimePrecision::Month
        } else {
            TimePrecision::Year
        };
        Self {
            coord: TimeCoord::Absolute(cal),
            precision,
        }
    }

    pub fn narrative(scene_id: StableId) -> Self {
        Self {
            coord: TimeCoord::Narrative(scene_id),
            precision: TimePrecision::Scene,
        }
    }

    pub fn unknown() -> Self {
        Self {
            coord: TimeCoord::Unknown,
            precision: TimePrecision::Unspecified,
        }
    }

    /// 转为可比较的序数 (RFC-0009 §3.2)
    ///
    /// 无上下文版本：叙事坐标之间不可比（统一为 Narrative(0)）。
    /// 需要场景间排序时用 [`StoryTime::ordinal_in`]。
    pub fn ordinal(&self) -> TimeOrdinal {
        match &self.coord {
            TimeCoord::BeforeStory => TimeOrdinal::BeforeStory,
            TimeCoord::Absolute(cal) => {
                TimeOrdinal::Absolute(cal.year * 12 + cal.month.unwrap_or(1) as i32)
            }
            TimeCoord::Narrative(_) => TimeOrdinal::Narrative(0),
            TimeCoord::AfterStory => TimeOrdinal::AfterStory,
            TimeCoord::Unknown => TimeOrdinal::Unknown,
        }
    }

    /// 带文档上下文的序数：叙事坐标按阅读序拓扑索引赋值
    /// （章→场景→片段展平序号），场景之间因此可比。
    pub fn ordinal_in(&self, doc: &crate::DocumentGraph) -> TimeOrdinal {
        match &self.coord {
            TimeCoord::Narrative(id) => match doc.reading_index(id) {
                Some(idx) => TimeOrdinal::Narrative(idx as i32),
                None => TimeOrdinal::Unknown,
            },
            _ => self.ordinal(),
        }
    }

    /// 判断 self 是否严格早于 other
    pub fn is_before(&self, other: &StoryTime) -> bool {
        self.ordinal() < other.ordinal()
    }
}

impl std::fmt::Display for StoryTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.coord {
            TimeCoord::Absolute(cal) => {
                write!(f, "{}", cal.year)?;
                if let Some(m) = cal.month {
                    write!(f, "-{m:02}")?;
                    if let Some(d) = cal.day {
                        write!(f, "-{d:02}")?;
                    }
                }
                Ok(())
            }
            TimeCoord::Narrative(id) => write!(f, "scene:{id}"),
            TimeCoord::BeforeStory => write!(f, "before-story"),
            TimeCoord::AfterStory => write!(f, "after-story"),
            TimeCoord::Unknown => write!(f, "unknown"),
        }
    }
}

/// 可比较的时间序数（内部使用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeOrdinal {
    BeforeStory,
    Absolute(i32),
    /// 叙事坐标：携带阅读序拓扑索引（无上下文时为 0，彼此不可比）
    Narrative(i32),
    AfterStory,
    Unknown,
}

// ============================================================================
// ValidityWindow — 有效区间
// ============================================================================

/// 断言 / 边的有效区间 (RFC-0009 §6)
///
/// - from = None 表示"自故事开始"
/// - until = None 表示"直到故事结束 / 永久"
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ValidityWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<StoryTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<StoryTime>,
}

impl ValidityWindow {
    pub fn new(from: Option<StoryTime>, until: Option<StoryTime>) -> Self {
        Self { from, until }
    }

    /// 全时段有效
    pub fn always() -> Self {
        Self {
            from: None,
            until: None,
        }
    }

    /// 从某时间点开始，永久有效
    pub fn from(start: StoryTime) -> Self {
        Self {
            from: Some(start),
            until: None,
        }
    }

    /// 判断某时间点是否在此窗口内 (RFC-0009 §6.1)
    pub fn contains(&self, time: &StoryTime) -> bool {
        let after_from = self.from.as_ref().is_none_or(|f| !time.is_before(f));
        let before_until = self.until.as_ref().is_none_or(|u| time.is_before(u));
        after_from && before_until
    }

    /// 判断两个窗口是否重叠 (RFC-0009 §6.1)
    pub fn overlaps(&self, other: &ValidityWindow) -> bool {
        // 标准 interval overlap: not (self.until <= other.from || other.until <= self.from)
        let self_ends_before_other = self.until.as_ref().is_some_and(|u| {
            other
                .from
                .as_ref()
                .is_some_and(|f| u.is_before(f) || u == f)
        });
        let other_ends_before_self = other
            .until
            .as_ref()
            .is_some_and(|u| self.from.as_ref().is_some_and(|f| u.is_before(f) || u == f));
        !(self_ends_before_other || other_ends_before_self)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_time_absolute_ordinal() {
        let t1 = StoryTime::absolute(StoryCalendar::from_year(1086));
        let t2 = StoryTime::absolute(StoryCalendar::from_year(1088));
        assert!(t1.is_before(&t2));
        assert!(!t2.is_before(&t1));
    }

    #[test]
    fn validity_window_contains() {
        let window = ValidityWindow::new(
            Some(StoryTime::absolute(StoryCalendar::from_year(1086))),
            Some(StoryTime::absolute(StoryCalendar::from_year(1088))),
        );
        let inside = StoryTime::absolute(StoryCalendar::from_year_month(1087, 6));
        let before = StoryTime::absolute(StoryCalendar::from_year(1085));
        let after = StoryTime::absolute(StoryCalendar::from_year(1089));

        assert!(window.contains(&inside));
        assert!(!window.contains(&before));
        assert!(!window.contains(&after));
    }

    #[test]
    fn validity_window_overlaps() {
        let w1 = ValidityWindow::new(
            Some(StoryTime::absolute(StoryCalendar::from_year(1086))),
            Some(StoryTime::absolute(StoryCalendar::from_year(1088))),
        );
        let w2 = ValidityWindow::new(
            Some(StoryTime::absolute(StoryCalendar::from_year(1087))),
            Some(StoryTime::absolute(StoryCalendar::from_year(1090))),
        );
        let w3 = ValidityWindow::new(
            Some(StoryTime::absolute(StoryCalendar::from_year(1090))),
            Some(StoryTime::absolute(StoryCalendar::from_year(1092))),
        );

        assert!(w1.overlaps(&w2)); // 重叠
        assert!(!w1.overlaps(&w3)); // 不重叠
    }

    #[test]
    fn validity_window_always_contains_everything() {
        let window = ValidityWindow::always();
        let any_time = StoryTime::absolute(StoryCalendar::from_year(9999));
        assert!(window.contains(&any_time));
    }
}
