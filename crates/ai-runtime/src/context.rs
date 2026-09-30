//! Context Builder — 上下文构建 (RFC-0010 §5)
//!
//! 从 IR + KB 中提取相关信息，构建传给 AI 的上下文。
//! 上下文使用 Symbol + Display Name，不用 StableId。

use story_ir::{DocNode, StableId, StoryIR};

/// 取文本末尾至多 `max_chars` 个字符（按字符边界截断）
fn tail_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

// ============================================================================
// ContextBuilder
// ============================================================================

/// 上下文构建器
pub struct ContextBuilder<'a> {
    ir: &'a StoryIR,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(ir: &'a StoryIR) -> Self {
        Self { ir }
    }

    /// 获取 Scene 的前 N 个 Fragment 的正文（提供连贯性）
    ///
    /// 单段截尾 [`FRAGMENT_TAIL_CHARS`] 字：片段理论上可以很长（如整场粘贴），
    /// 不截断会把上下文撑得过大。
    pub fn recent_fragments(&self, scene_id: &StableId, count: usize) -> String {
        const FRAGMENT_TAIL_CHARS: usize = 1200;
        let fragments = self.ir.document.children_of(scene_id);

        let start = fragments.len().saturating_sub(count);
        let recent = &fragments[start..];

        recent
            .iter()
            .filter_map(|fid| self.ir.document.get_fragment(fid).map(|f| f.text.clone()))
            .map(|text| tail_chars(&text, FRAGMENT_TAIL_CHARS))
            .collect::<Vec<_>>()
            .join("\n---\n")
    }

    /// 获取角色的简介
    pub fn character_brief(&self, char_id: &StableId) -> Option<String> {
        let char = self.ir.semantic.get_character(char_id)?;
        let mut brief = char.display_name.clone();
        if !char.aliases.is_empty() {
            brief.push_str(&format!("（{}）", char.aliases.join("、")));
        }
        Some(brief)
    }

    /// 获取 Scene 的所有出场角色简介
    pub fn scene_characters(&self, scene_id: &StableId) -> Vec<String> {
        let fragments = self.ir.document.children_of(scene_id);

        let mut char_ids = std::collections::BTreeSet::new();

        // 从 anchors 收集
        for fid in fragments {
            if let Some(DocNode::Fragment(frag)) = self.ir.document.get(fid) {
                for anchor in &frag.anchors {
                    char_ids.insert(anchor.entity.clone());
                }
            }
        }

        // POV 角色
        if let Some(DocNode::Scene(scene)) = self.ir.document.get(scene_id) {
            if let Some(pov) = &scene.pov {
                char_ids.insert(pov.clone());
            }
        }

        char_ids
            .into_iter()
            .filter_map(|id| self.character_brief(&id))
            .collect()
    }

    /// 构建 WriteFragmentTask 的上下文数据
    pub fn for_write_fragment(
        &self,
        scene_id: &StableId,
    ) -> (String, Option<String>, Option<String>) {
        let recent = self.recent_fragments(scene_id, 3);

        let scene = self.ir.document.get_scene(scene_id);
        let scene_title = scene.and_then(|s| s.title.clone());
        let pov_name = scene
            .and_then(|s| s.pov.as_ref())
            .and_then(|id| self.character_brief(id));

        (recent, pov_name, scene_title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_ir::*;

    fn make_ir_with_content() -> StoryIR {
        let mut ir = StoryIR::new();

        // Character
        ir.semantic.insert(SemanticNode::Character(CharacterNode {
            id: StableId::new_unchecked("c_001"),
            symbol: None,
            display_name: "林风".to_string(),
            aliases: vec!["林少侠".to_string()],
            attributes: Default::default(),
        }));

        // Scene + Fragment
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_01"),
            chapter: StableId::new_unchecked("ch_01"),
            title: Some("渡口".to_string()),
            symbol: None,
            prev: None,
            next: None,
            pov: Some(StableId::new_unchecked("c_001")),
            story_time_anchor: None,
            story_time_gap: None,
            story_time: None,
            mood: None,
            function: None,
            summary: None,
        }));

        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: Some(StableId::new_unchecked("sc_01")),
            prev: None,
            next: None,
            text: "林风推开门。".to_string(),
            anchors: vec![EntityAnchor {
                entity: StableId::new_unchecked("c_001"),
                role: AnchorRole::Appears,
            }],
        }));

        ir.document.set_children(
            StableId::new_unchecked("sc_01"),
            vec![StableId::new_unchecked("f_001")],
        );

        ir
    }

    #[test]
    fn recent_fragments() {
        let ir = make_ir_with_content();
        let builder = ContextBuilder::new(&ir);

        let recent = builder.recent_fragments(&StableId::new_unchecked("sc_01"), 3);
        assert!(recent.contains("林风推开门"));
    }

    #[test]
    fn recent_fragments_caps_each_fragment_tail() {
        let mut ir = StoryIR::new();
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_01"),
            chapter: StableId::new_unchecked("ch_01"),
            title: None,
            symbol: None,
            prev: None,
            next: None,
            pov: None,
            story_time_anchor: None,
            story_time_gap: None,
            story_time: None,
            mood: None,
            function: None,
            summary: None,
        }));
        let long = "长".repeat(3000);
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: Some(StableId::new_unchecked("sc_01")),
            prev: None,
            next: None,
            text: format!("{long}结尾在这里。"),
            anchors: vec![],
        }));
        ir.document.set_children(
            StableId::new_unchecked("sc_01"),
            vec![StableId::new_unchecked("f_001")],
        );

        let recent = ContextBuilder::new(&ir).recent_fragments(&StableId::new_unchecked("sc_01"), 3);
        assert!(recent.contains("结尾在这里"), "截尾应保留末尾: {recent:?}");
        assert!(recent.chars().count() < 1300, "超长片段应被截断: {}", recent.chars().count());
    }

    #[test]
    fn character_brief() {
        let ir = make_ir_with_content();
        let builder = ContextBuilder::new(&ir);

        let brief = builder
            .character_brief(&StableId::new_unchecked("c_001"))
            .unwrap();
        assert!(brief.contains("林风"));
        assert!(brief.contains("林少侠"));
    }

    #[test]
    fn for_write_fragment() {
        let ir = make_ir_with_content();
        let builder = ContextBuilder::new(&ir);

        let (recent, pov, title) = builder.for_write_fragment(&StableId::new_unchecked("sc_01"));
        assert!(!recent.is_empty());
        assert_eq!(pov.as_deref(), Some("林风（林少侠）"));
        assert_eq!(title.as_deref(), Some("渡口"));
    }
}
