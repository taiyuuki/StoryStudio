//! KnowledgeBase — 断言与证据的集合 (RFC-0015 §4, §8)
//!
//! Source of Truth，提交 Git。
//! Compiler 单向消费此结构执行一致性检查。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use story_ir::StableId;

use crate::assertion::{Assertion, AssertionStatus};
use crate::evidence::Evidence;

// ============================================================================
// KnowledgeBase
// ============================================================================

/// 一个 Project 内全部已提交断言 + 证据的集合
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeBase {
    /// 断言（按 StableId 索引，BTreeMap 保证确定序）
    assertions: BTreeMap<StableId, Assertion>,
    /// 证据
    evidence: BTreeMap<StableId, Evidence>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self::default()
    }

    // —— 断言操作 ——

    pub fn insert_assertion(&mut self, assertion: Assertion) {
        self.assertions.insert(assertion.id.clone(), assertion);
    }

    pub fn get_assertion(&self, id: &StableId) -> Option<&Assertion> {
        self.assertions.get(id)
    }

    pub fn all_assertions(&self) -> impl Iterator<Item = (&StableId, &Assertion)> {
        self.assertions.iter()
    }

    pub fn assertion_count(&self) -> usize {
        self.assertions.len()
    }

    // —— 证据操作 ——

    pub fn insert_evidence(&mut self, evidence: Evidence) {
        self.evidence.insert(evidence.id.clone(), evidence);
    }

    pub fn get_evidence(&self, id: &StableId) -> Option<&Evidence> {
        self.evidence.get(id)
    }

    pub fn all_evidence(&self) -> impl Iterator<Item = (&StableId, &Evidence)> {
        self.evidence.iter()
    }

    pub fn evidence_count(&self) -> usize {
        self.evidence.len()
    }

    // —— 查询接口（供 Compiler 使用）——

    /// 查询某主体的所有 active 断言
    pub fn active_assertions_of(&self, subject: &StableId) -> Vec<&Assertion> {
        self.assertions
            .values()
            .filter(|a| &a.subject == subject && a.status == AssertionStatus::Active)
            .collect()
    }

    /// 查询某主体 + 某谓词的所有 active 断言
    pub fn find_assertions(
        &self,
        subject: &StableId,
        predicate: &crate::assertion::Predicate,
    ) -> Vec<&Assertion> {
        self.assertions
            .values()
            .filter(|a| {
                &a.subject == subject
                    && &a.predicate == predicate
                    && a.status == AssertionStatus::Active
            })
            .collect()
    }

    /// 查询某主体在某时间点的某属性值
    ///
    /// 返回所有 valid 区间包含该时间点的 active 断言
    pub fn query(
        &self,
        subject: &StableId,
        predicate: &crate::assertion::Predicate,
        time: &story_ir::StoryTime,
    ) -> Vec<&Assertion> {
        self.find_assertions(subject, predicate)
            .into_iter()
            .filter(|a| a.valid.contains(time))
            .collect()
    }

    /// 获取某断言的所有证据
    pub fn evidence_for(&self, assertion: &Assertion) -> Vec<&Evidence> {
        assertion
            .evidence
            .iter()
            .filter_map(|eid| self.evidence.get(eid))
            .collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertion::{ObjectValue, Predicate};
    use crate::evidence::Evidence;
    use story_ir::{StoryCalendar, StoryTime, ValidityWindow};

    fn make_alive_assertion(id: &str, subject: &str, alive: bool, from_year: i32) -> Assertion {
        Assertion {
            id: StableId::new_unchecked(id),
            subject: StableId::new_unchecked(subject),
            predicate: Predicate::alive(),
            object: ObjectValue::Boolean(alive),
            valid: ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(from_year))),
                None,
            ),
            evidence: vec![],
            status: AssertionStatus::Active,
            extracted_by: Default::default(),
        }
    }

    #[test]
    fn kb_basic_operations() {
        let mut kb = KnowledgeBase::new();

        let a1 = make_alive_assertion("a_001", "c_001", true, 1069);
        kb.insert_assertion(a1.clone());

        assert_eq!(kb.assertion_count(), 1);
        assert_eq!(
            kb.get_assertion(&StableId::new_unchecked("a_001")),
            Some(&a1)
        );
    }

    #[test]
    fn kb_query_alive() {
        let mut kb = KnowledgeBase::new();

        // 林风 1069-1092 活着
        let a1 = Assertion {
            id: StableId::new_unchecked("a_001"),
            subject: StableId::new_unchecked("c_001"),
            predicate: Predicate::alive(),
            object: ObjectValue::Boolean(true),
            valid: ValidityWindow::new(
                Some(StoryTime::absolute(StoryCalendar::from_year(1069))),
                Some(StoryTime::absolute(StoryCalendar::from_year(1092))),
            ),
            evidence: vec![],
            status: AssertionStatus::Active,
            extracted_by: Default::default(),
        };
        // 林风 1092 死亡
        let a2 = Assertion {
            id: StableId::new_unchecked("a_002"),
            subject: StableId::new_unchecked("c_001"),
            predicate: Predicate::alive(),
            object: ObjectValue::Boolean(false),
            valid: ValidityWindow::from(StoryTime::absolute(StoryCalendar::from_year(1092))),
            evidence: vec![],
            status: AssertionStatus::Active,
            extracted_by: Default::default(),
        };

        kb.insert_assertion(a1);
        kb.insert_assertion(a2);

        let char_id = StableId::new_unchecked("c_001");
        let pred = Predicate::alive();

        // 1085 年：活着
        let t1 = StoryTime::absolute(StoryCalendar::from_year(1085));
        let results = kb.query(&char_id, &pred, &t1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object.as_bool(), Some(true));

        // 1095 年：死了
        let t2 = StoryTime::absolute(StoryCalendar::from_year(1095));
        let results = kb.query(&char_id, &pred, &t2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object.as_bool(), Some(false));
    }

    #[test]
    fn kb_evidence_linking() {
        let mut kb = KnowledgeBase::new();

        let evidence = Evidence::new(
            StableId::new_unchecked("ev_001"),
            StableId::new_unchecked("f_0001"),
            "林风今年十七岁",
        );
        kb.insert_evidence(evidence);

        let mut assertion = make_alive_assertion("a_001", "c_001", true, 1069);
        assertion.evidence = vec![StableId::new_unchecked("ev_001")];
        kb.insert_assertion(assertion.clone());

        let linked = kb.evidence_for(&assertion);
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].quote, "林风今年十七岁");
    }
}
