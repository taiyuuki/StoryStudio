//! Entity Linking — 确定性词典匹配 (RFC-0006 §8)
//!
//! 基于 Entity 的 display_name + aliases 构建 Aho-Corasick 自动机，
//! 在 Fragment 正文中做精确匹配，产出 mentions（派生数据）。
//! 完全确定性，不调用 AI。

use std::collections::BTreeMap;

use aho_corasick::AhoCorasick;
use serde::{Deserialize, Serialize};
use story_ir::{SemanticGraph, StableId};

/// 实体词典：词条 → StableId 列表
///
/// 一个词条可能映射到多个 Entity（歧义情况）。
/// BTreeMap 保证确定序列化。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityDictionary {
    /// 词条 → 匹配的实体 ID 列表
    entries: BTreeMap<String, Vec<StableId>>,
    /// 实体类型（id → type_name），用于区分「双面实体」与真歧义
    #[serde(default)]
    kinds: BTreeMap<StableId, String>,
}

/// 从全名派生可独立指代的分段（隐性别名）。
///
/// 仅处理含分隔符（·、-、—、空格）的复合名；每段 ≥2 字、且非数字型
/// （见 [`is_number_like`]）才入典。中文姓名（无分隔符）返回空。
fn derive_name_parts(display_name: &str) -> Vec<String> {
    let separators = ['·', '·', '-', '—', ' '];
    let has_sep = display_name.chars().any(|c| separators.contains(&c));
    if !has_sep {
        return Vec::new();
    }
    display_name
        .split(|c| separators.contains(&c))
        .map(|p| p.trim().to_string())
        .filter(|p| p.chars().count() >= 2)
        .filter(|p| !is_number_like(p))
        .collect()
}

/// 判断词条是否为「数字型」：由数字（ASCII / 汉字数字 / 天干序数）与
/// 常见单位字（号楼层……）及连接符组成，且至少含一个数字。
///
/// 「十七号」「204」「三楼」「丙-17」→ true；「档案楼」「槐安路十七号」→ false。
/// 这类词条作为隐式派生段误报面极大（正文中门牌号、档案编号、层数
/// 随处都是），一律不参与确定性匹配；确有需要时由作者加显式别名。
pub fn is_number_like(text: &str) -> bool {
    const NUMERALS: &str = "0123456789〇零一二两三四五六七八九十百千万亿兆甲乙丙丁戊己庚辛壬癸";
    const UNITS: &str = "号楼层室间座栋单元年月日卷章节回";
    const CONNECTORS: &str = "-－—·.． ";
    let mut has_numeral = false;
    for c in text.chars() {
        if NUMERALS.contains(c) {
            has_numeral = true;
        } else if !UNITS.contains(c) && !CONNECTORS.contains(c) {
            return false;
        }
    }
    has_numeral
}

impl EntityDictionary {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 SemanticGraph 构建词典（RFC-0006 §8.2）
    ///
    /// 将每个 Entity 的 display_name + aliases 入典。
    pub fn from_semantic_graph(graph: &SemanticGraph) -> Self {
        let mut dict = Self::new();

        for entity in graph.all_entities() {
            // 真名遮蔽中的角色不入典：display_name 只是泛称代称（如「中年男人」），
            // 入典会把正文里所有同类泛称误锚到该角色身上。遮蔽期的出场靠
            // 理解锚定（LLM 指代消解）与章节脉络预设名单；reveal 后真名与
            // 代称（已转别名）恢复正常入典。
            if entity.name_concealed() {
                continue;
            }
            dict.kinds
                .insert(entity.id().clone(), entity.type_name().to_string());

            // display_name 入典
            dict.add(entity.display_name(), entity.id().clone());

            // aliases 入典
            for alias in entity.aliases() {
                dict.add(alias, entity.id().clone());
            }

            // 全名分段派生：西式全名（如"洛恩·巴克尔"）按 ·/-/空格 切段，
            // ≥2 字的分段作为隐性别名入典——正文只称呼"洛恩"也能锚定，
            // 不必手工维护别名（中文姓名无分隔符，不受影响）
            for part in derive_name_parts(entity.display_name()) {
                dict.add(&part, entity.id().clone());
            }
        }

        dict
    }

    /// 添加词条
    ///
    /// 同一词条同一实体去重——派生分段与手工别名撞名时（如别名"碧翠丝"
    /// 恰好等于全名"碧翠丝·艾什沃斯"的派生段），不重复压入，否则
    /// lookup 返回两个相同实体被误判为歧义，该称呼被静默丢弃。
    fn add(&mut self, text: &str, entity: StableId) {
        // 跳过空词条
        if text.trim().is_empty() {
            return;
        }
        let list = self.entries.entry(text.to_string()).or_default();
        if !list.contains(&entity) {
            list.push(entity);
        }
    }

    /// 查询词条匹配的实体
    pub fn lookup(&self, text: &str) -> &[StableId] {
        self.entries.get(text).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 是否有歧义（一词多义）
    pub fn is_ambiguous(&self, text: &str) -> bool {
        self.entries.get(text).is_some_and(|v| v.len() > 1)
    }

    /// 词条命中的实体是否两两类型不同。
    ///
    /// 同名不同类型的「双面实体」（如「星辰学院」既是地点也是机构）
    /// 不是歧义：名称真实指向每一面，各记一条提及。类型未知（旧缓存
    /// 反序列化）时按不互异处理，退回原歧义行为。
    fn all_kinds_distinct(&self, entities: &[StableId]) -> bool {
        let mut kinds: Vec<&str> = entities
            .iter()
            .map(|id| self.kinds.get(id).map(String::as_str).unwrap_or(""))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        entities.len() == kinds.len() && !kinds.first().is_some_and(|k| k.is_empty())
    }

    /// 所有词条
    pub fn patterns(&self) -> impl Iterator<Item = (&String, &[StableId])> {
        self.entries.iter().map(|(k, v)| (k, v.as_slice()))
    }

    /// 词条数量
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 构建 Aho-Corasick 自动机
    ///
    /// 词条按 BTreeMap 的字典序输入，保证自动机结构确定（RFC-0006 §3）。
    pub fn build_automaton(&self) -> Option<AhoCorasick> {
        if self.entries.is_empty() {
            return None;
        }

        let patterns: Vec<&str> = self.entries.keys().map(|s| s.as_str()).collect();
        // AhoCorasick 默认使用 AhoCorasickKind::DFA（确定性），保证确定
        AhoCorasick::new(patterns).ok()
    }
}

/// 一次实体提及（匹配结果）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    /// 匹配到的实体
    pub entity: StableId,
    /// 正文中的起始字符偏移
    pub start: usize,
    /// 正文中的结束字符偏移
    pub end: usize,
    /// 匹配到的文本
    pub matched_text: String,
}

/// 在正文中执行实体链接（RFC-0006 §8.3）
///
/// 返回 (mentions, ambiguities)：
/// - mentions：成功匹配的提及
/// - ambiguities：歧义匹配（一词多义），需产 Warning
pub fn link_in_text(
    text: &str,
    dict: &EntityDictionary,
    automaton: &AhoCorasick,
) -> (Vec<Mention>, Vec<String>) {
    // 收集所有匹配（位置 + 匹配文本）
    let mut candidates: Vec<(usize, usize, &str)> = automaton
        .find_iter(text)
        .map(|m| {
            let matched = &text[m.start()..m.end()];
            (m.start(), m.end(), matched)
        })
        .collect();

    // 按起始位置排序，然后按长度降序（最长匹配优先）
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

    // 解决重叠：最长匹配优先（RFC-0006 §8.4）
    let resolved = resolve_overlaps(&candidates);

    // 转换为 Mention，检测歧义
    let mut mentions = Vec::new();
    let mut ambiguities = Vec::new();

    for (start, end, matched_text) in resolved {
        let entities = dict.lookup(matched_text);

        if entities.len() > 1 && !dict.all_kinds_distinct(entities) {
            // 歧义：同类型内撞名（如两个角色共享「那个人」），记录但不生成 mention
            ambiguities.push(matched_text.to_string());
        } else {
            // 单实体命中；或同名「双面实体」（如「星辰学院」既是地点也是
            // 机构）——名称同时指向每一面，各记一条提及，出场统计两不误
            for entity in entities {
                mentions.push(Mention {
                    entity: entity.clone(),
                    start,
                    end,
                    matched_text: matched_text.to_string(),
                });
            }
        }
    }

    (mentions, ambiguities)
}

/// 解决重叠匹配：保留最长匹配，移除被包含的短匹配
fn resolve_overlaps<'a>(candidates: &'a [(usize, usize, &'a str)]) -> Vec<(usize, usize, &'a str)> {
    let mut result = Vec::new();
    let mut last_end = 0usize;

    for &(start, end, text) in candidates {
        // 如果当前匹配的起始位置 >= 上一个保留匹配的结束位置，不重叠
        if start >= last_end {
            result.push((start, end, text));
            last_end = end;
        }
        // 否则被上一个（更长的）匹配包含，跳过
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_ir::{CharacterNode, SemanticGraph};

    fn make_graph() -> SemanticGraph {
        let mut graph = SemanticGraph::new();

        let mut lin = CharacterNode {
            id: StableId::new_unchecked("c_001"),
            symbol: None,
            display_name: "林风".to_string(),
            aliases: vec!["林少侠".to_string(), "风哥".to_string()],
            name_concealed: false,
            attributes: Default::default(),
        };
        graph.insert(story_ir::SemanticNode::Character(lin.clone()));

        let zhou = CharacterNode {
            id: StableId::new_unchecked("c_002"),
            symbol: None,
            display_name: "周明".to_string(),
            aliases: vec![],
            name_concealed: false,
            attributes: Default::default(),
        };
        graph.insert(story_ir::SemanticNode::Character(zhou));

        // 添加一个共享别名制造歧义
        lin.aliases = vec![
            "林少侠".to_string(),
            "风哥".to_string(),
            "那个人".to_string(),
        ];
        graph.insert(story_ir::SemanticNode::Character(lin));

        let other = CharacterNode {
            id: StableId::new_unchecked("c_003"),
            symbol: None,
            display_name: "路人".to_string(),
            aliases: vec!["那个人".to_string()], // 与林风共享"那个人"
            name_concealed: false,
            attributes: Default::default(),
        };
        graph.insert(story_ir::SemanticNode::Character(other.clone()));

        graph
    }

    #[test]
    fn dictionary_from_graph() {
        let graph = make_graph();
        let dict = EntityDictionary::from_semantic_graph(&graph);

        assert_eq!(dict.lookup("林风"), &[StableId::new_unchecked("c_001")]);
        assert_eq!(dict.lookup("林少侠"), &[StableId::new_unchecked("c_001")]);
        assert!(dict.is_ambiguous("那个人")); // 两个实体共享
    }

    #[test]
    fn link_basic() {
        let graph = make_graph();
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let automaton = dict.build_automaton().unwrap();

        let text = "林风推开门，周明站在门外。";
        let (mentions, ambiguities) = link_in_text(text, &dict, &automaton);

        assert_eq!(mentions.len(), 2); // 林风 + 周明
        assert!(mentions.iter().any(|m| m.matched_text == "林风"));
        assert!(mentions.iter().any(|m| m.matched_text == "周明"));
        assert!(ambiguities.is_empty());
    }

    #[test]
    fn link_longest_match() {
        // "林少侠"（3字）和"林"不该都匹配——但我们没有单独的"林"词条
        // 测试"林少侠"和"风哥"都出现的情况
        let graph = make_graph();
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let automaton = dict.build_automaton().unwrap();

        let text = "林少侠走了过来。";
        let (mentions, _) = link_in_text(text, &dict, &automaton);
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].matched_text, "林少侠");
        assert_eq!(mentions[0].entity, StableId::new_unchecked("c_001"));
    }

    #[test]
    fn link_ambiguous() {
        let graph = make_graph();
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let automaton = dict.build_automaton().unwrap();

        let text = "那个人走了。"; // "那个人"是歧义词
        let (mentions, ambiguities) = link_in_text(text, &dict, &automaton);

        assert!(mentions.is_empty()); // 歧义不生成 mention
        assert_eq!(ambiguities, vec!["那个人".to_string()]);
    }

    /// 「星辰学院」既是地点也是机构：同名不同类型是合法的双面实体，
    /// 正文提及各记一条，不报歧义
    #[test]
    fn link_cross_kind_same_name_dual_facet() {
        let mut graph = SemanticGraph::new();
        graph.insert(story_ir::SemanticNode::Location(story_ir::LocationNode {
            id: StableId::new_unchecked("l_001"),
            symbol: None,
            display_name: "星辰学院".to_string(),
            aliases: vec![],
            parent: None,
            kind: None,
            position: None,
        }));
        graph.insert(story_ir::SemanticNode::Organization(
            story_ir::OrganizationNode {
                id: StableId::new_unchecked("o_001"),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                location: None,
                attributes: Default::default(),
            },
        ));
        let dict = EntityDictionary::from_semantic_graph(&graph);
        assert!(dict.all_kinds_distinct(dict.lookup("星辰学院")));

        let automaton = dict.build_automaton().unwrap();
        let (mentions, ambiguities) = link_in_text("他走进了星辰学院。", &dict, &automaton);
        assert_eq!(mentions.len(), 2, "双面实体应各记一条提及");
        assert!(mentions
            .iter()
            .any(|m| m.entity == StableId::new_unchecked("l_001")));
        assert!(mentions
            .iter()
            .any(|m| m.entity == StableId::new_unchecked("o_001")));
        assert!(ambiguities.is_empty());
    }

    /// 两个地点同名（外加同名机构）→ 同类型内撞名才是真歧义，提及丢弃
    #[test]
    fn link_same_kind_duplicate_still_ambiguous() {
        let mut graph = SemanticGraph::new();
        for id in ["l_001", "l_002"] {
            graph.insert(story_ir::SemanticNode::Location(story_ir::LocationNode {
                id: StableId::new_unchecked(id),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                parent: None,
                kind: None,
                position: None,
            }));
        }
        graph.insert(story_ir::SemanticNode::Organization(
            story_ir::OrganizationNode {
                id: StableId::new_unchecked("o_001"),
                symbol: None,
                display_name: "星辰学院".to_string(),
                aliases: vec![],
                location: None,
                attributes: Default::default(),
            },
        ));
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let automaton = dict.build_automaton().unwrap();

        let (mentions, ambiguities) = link_in_text("他走进了星辰学院。", &dict, &automaton);
        assert!(mentions.is_empty(), "同类型撞名不应生成提及");
        assert_eq!(ambiguities, vec!["星辰学院".to_string()]);
    }

    #[test]
    fn link_no_match() {
        let graph = make_graph();
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let automaton = dict.build_automaton().unwrap();

        let text = "天空下着雨。";
        let (mentions, ambiguities) = link_in_text(text, &dict, &automaton);
        assert!(mentions.is_empty());
        assert!(ambiguities.is_empty());
    }

    #[test]
    fn resolve_overlaps_keeps_longest() {
        // 模拟重叠：(0,2,"林风") 和 (0,3,"林少侠")
        // 排序后 (0,3) 在前（更长），(0,2) 被跳过
        let candidates = vec![(0, 3, "林少侠"), (0, 2, "林风")];
        let resolved = resolve_overlaps(&candidates);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].2, "林少侠");
    }

    #[test]
    fn derives_parts_from_western_names() {
        assert_eq!(
            derive_name_parts("洛恩·巴克尔"),
            vec!["洛恩".to_string(), "巴克尔".to_string()]
        );
    }

    #[test]
    fn number_like_derived_parts_skipped() {
        // 「十七号·档案楼」：数字段不入典，只留「档案楼」——
        // 否则正文里门牌号 / 档案编号里的"十七号"会全文误锚
        assert_eq!(
            derive_name_parts("十七号·档案楼"),
            vec!["档案楼".to_string()]
        );
        assert!(derive_name_parts("A·204").is_empty());
        // 含其他文字的段不受影响
        assert_eq!(derive_name_parts("槐安路十七号·档案楼").len(), 2);
    }

    #[test]
    fn is_number_like_classification() {
        for s in ["十七号", "204", "三楼", "丙-17", "十二", "305室"] {
            assert!(is_number_like(s), "{s} 应为数字型");
        }
        for s in ["档案楼", "槐安路十七号", "顾岩", "十三姨"] {
            assert!(!is_number_like(s), "{s} 不应为数字型");
        }
        assert!(!is_number_like(""));
    }

    #[test]
    fn number_like_derived_part_not_in_dictionary() {
        let mut graph = SemanticGraph::new();
        graph.insert(story_ir::SemanticNode::Location(story_ir::LocationNode {
            id: StableId::new_unchecked("l_001"),
            symbol: None,
            display_name: "十七号·档案楼".to_string(),
            aliases: vec![],
            parent: None,
            kind: None,
            position: None,
        }));
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let auto = dict.build_automaton().unwrap();
        // 正文里的裸"十七号"（如医生口中的红档编号）不再误锚
        let (mentions, _) = link_in_text("医生说：什么十七号。", &dict, &auto);
        assert!(mentions.is_empty(), "{mentions:?}");
        // 全名与"档案楼"段照常命中
        let (mentions, _) = link_in_text("他推开档案楼的门。", &dict, &auto);
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].entity.as_str(), "l_001");
    }

    /// 真名遮蔽中的角色：display_name 是泛称代称，不入锚定词典——
    /// 否则正文里任何「中年男人」都会被误锚到该角色
    #[test]
    fn concealed_character_not_in_dictionary() {
        let mut graph = SemanticGraph::new();
        graph.insert(story_ir::SemanticNode::Character(story_ir::CharacterNode {
            id: StableId::new_unchecked("c_007"),
            symbol: Some(story_ir::Symbol::new("shen")),
            display_name: "中年男人".to_string(),
            aliases: vec!["灰袍男人".to_string()],
            name_concealed: true,
            attributes: Default::default(),
        }));
        let dict = EntityDictionary::from_semantic_graph(&graph);
        assert!(dict.lookup("中年男人").is_empty());
        assert!(dict.lookup("灰袍男人").is_empty());
        // 揭露后（name_concealed = false）真名与代称别名正常入典
        let mut graph2 = SemanticGraph::new();
        graph2.insert(story_ir::SemanticNode::Character(story_ir::CharacterNode {
            id: StableId::new_unchecked("c_007"),
            symbol: None,
            display_name: "沈孤鸿".to_string(),
            aliases: vec!["中年男人".to_string()],
            name_concealed: false,
            attributes: Default::default(),
        }));
        let dict2 = EntityDictionary::from_semantic_graph(&graph2);
        assert_eq!(
            dict2.lookup("中年男人"),
            &[StableId::new_unchecked("c_007")]
        );
    }

    #[test]
    fn chinese_names_derive_nothing() {
        assert!(derive_name_parts("沈砚之").is_empty());
    }

    #[test]
    fn single_char_parts_skipped() {
        assert_eq!(derive_name_parts("A·巴克尔"), vec!["巴克尔".to_string()]);
    }

    #[test]
    fn prefix_alias_same_as_derived_part_not_ambiguous() {
        // 名"碧翠丝·艾什沃斯" + 别名"碧翠丝"（= 派生段）：
        // 曾因词条重复压入被判歧义而丢弃，正文只喊短名时完全认不到
        let mut graph = SemanticGraph::new();
        graph.insert(story_ir::SemanticNode::Character(story_ir::CharacterNode {
            id: StableId::new_unchecked("c_004"),
            symbol: None,
            display_name: "碧翠丝·艾什沃斯".to_string(),
            aliases: vec!["碧翠丝".to_string()],
            name_concealed: false,
            attributes: Default::default(),
        }));
        let dict = EntityDictionary::from_semantic_graph(&graph);
        let auto = dict.build_automaton().unwrap();
        let (mentions, ambiguities) = link_in_text("碧翠丝推开门，屋里没人。", &dict, &auto);
        assert!(
            !mentions.is_empty() && mentions[0].entity.as_str() == "c_004",
            "前缀别名应命中: {mentions:?}"
        );
        assert!(
            ambiguities.is_empty(),
            "同实体重复词条不应判歧义: {ambiguities:?}"
        );
    }
}
