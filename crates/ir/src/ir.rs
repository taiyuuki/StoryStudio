//! Story IR 根结构与派生索引 (RFC-0002 §8)
//!
//! StoryIR 是一个 Project 内完整的图表示。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::anchor::AnchorRole;
use crate::document::DocumentGraph;
use crate::id::{StableId, SymbolTable};
use crate::semantic::SemanticGraph;

// ============================================================================
// IRVersion & Revision
// ============================================================================

/// Schema 版本（RFC-0002 §10）
///
/// 每个 Schema 变更递增。Parser 加载旧版本时走 Migration。
/// v0.1 的 IRVersion = 1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IRVersion(pub u32);

impl Default for IRVersion {
    fn default() -> Self {
        Self(1)
    }
}

/// 修订号（RFC-0002 §8）
///
/// 每次提交自增，用于增量编译的版本依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Revision(pub u64);

impl Revision {
    pub fn next(&self) -> Self {
        Self(self.0 + 1)
    }
}

// ============================================================================
// StoryIR 根
// ============================================================================

/// Story IR 的根——一个 Project 内完整的图表示 (RFC-0002 §8)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoryIR {
    /// Schema 版本
    #[serde(default)]
    pub version: IRVersion,
    /// 修订号（每次提交自增）
    #[serde(default)]
    pub revision: Revision,

    /// Document Graph（文本结构）
    #[serde(default)]
    pub document: DocumentGraph,

    /// Semantic Graph（实体与关系）
    #[serde(default)]
    pub semantic: SemanticGraph,

    /// Symbol → StableId 映射
    #[serde(default)]
    pub symbol_table: SymbolTable,
}

impl StoryIR {
    pub fn new() -> Self {
        Self::default()
    }

    /// 递增修订号（提交时调用）
    pub fn bump_revision(&mut self) {
        self.revision = self.revision.next();
    }
}

// ============================================================================
// LocationIndex — 派生缓存
// ============================================================================

/// Location Index (RFC-0001 §10.1, RFC-0006 §9)
///
/// 实体 → 文本块的映射。合并 anchors（显式）+ mentions（隐式匹配）。
/// 是派生缓存，不进 StoryIR，不进 Git。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocationIndex {
    /// entity → 出场记录列表
    map: BTreeMap<StableId, Vec<Appearance>>,
}

/// 一次实体出场记录
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    /// 出场的 Fragment
    pub fragment: StableId,
    /// 出场角色
    pub role: AnchorRole,
}

impl LocationIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一条出场记录。
    ///
    /// 同一实体在同一片段只记一条：隐式匹配里全名与别名撞在同一处时
    /// 会连发多条，显式锚点与隐式匹配也会对同一片段各记一条——
    /// 重复记录既虚增出场次数，也让 role 展示摇摆。
    /// 后到的 role 不覆盖先到的（显式锚点在 pipeline 中先入，
    /// appears 因此优先于隐式的 mentioned）。
    pub fn add(&mut self, entity: StableId, fragment: StableId, role: AnchorRole) {
        let list = self.map.entry(entity).or_default();
        if list.iter().any(|a| a.fragment == fragment) {
            return;
        }
        list.push(Appearance { fragment, role });
    }

    /// 查询某实体的所有出场
    pub fn appearances_of(&self, entity: &StableId) -> &[Appearance] {
        self.map.get(entity).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 查询某实体在哪些 Fragment 出场
    pub fn fragments_of(&self, entity: &StableId) -> Vec<&StableId> {
        self.map
            .get(entity)
            .map(|v| v.iter().map(|a| &a.fragment).collect())
            .unwrap_or_default()
    }

    /// 查询某实体出场的 Fragment 数量
    pub fn appearance_count(&self, entity: &StableId) -> usize {
        self.map.get(entity).map_or(0, |v| v.len())
    }

    /// 所有有出场记录的实体
    pub fn tracked_entities(&self) -> impl Iterator<Item = &StableId> {
        self.map.keys()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::StableId;

    #[test]
    fn ir_default() {
        let ir = StoryIR::default();
        assert_eq!(ir.version, IRVersion(1));
        assert_eq!(ir.revision, Revision(0));
    }

    #[test]
    fn revision_increment() {
        let mut ir = StoryIR::default();
        ir.bump_revision();
        assert_eq!(ir.revision, Revision(1));
        ir.bump_revision();
        assert_eq!(ir.revision, Revision(2));
    }

    #[test]
    fn location_index() {
        let mut idx = LocationIndex::new();
        let char_id = StableId::new_unchecked("c_001");
        let frag1 = StableId::new_unchecked("f_0001");
        let frag2 = StableId::new_unchecked("f_0002");

        idx.add(char_id.clone(), frag1.clone(), AnchorRole::Appears);
        idx.add(char_id.clone(), frag2.clone(), AnchorRole::Mentioned);

        assert_eq!(idx.appearance_count(&char_id), 2);
        let frags = idx.fragments_of(&char_id);
        assert_eq!(frags.len(), 2);
    }

    #[test]
    fn appearance_deduped_per_fragment() {
        use crate::AnchorRole;
        let mut idx = LocationIndex::new();
        let e = StableId::new_unchecked("c_0001");
        let f = StableId::new_unchecked("f_0001");
        // 全名与别名撞同一处：两条隐式 → 只记一条
        idx.add(e.clone(), f.clone(), AnchorRole::Mentioned);
        idx.add(e.clone(), f.clone(), AnchorRole::Mentioned);
        // 显式锚点后到 → 不覆盖先入的 mentioned（pipeline 中显式先入，
        // 此处验证"后到不覆盖"语义）
        idx.add(e.clone(), f.clone(), AnchorRole::Appears);
        assert_eq!(idx.appearances_of(&e).len(), 1);
        // 不同片段正常累计
        let f2 = StableId::new_unchecked("f_0002");
        idx.add(e.clone(), f2, AnchorRole::Appears);
        assert_eq!(idx.appearances_of(&e).len(), 2);
    }
}
