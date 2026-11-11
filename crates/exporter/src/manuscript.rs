//! Manuscript Assembly — 正文拼接 (RFC-0013 §4)
//!
//! 按 Document Graph 的链表顺序遍历 Fragment，将正文无缝拼接。
//! Fragment 边界不产生视觉分隔——渲染时是连续的叙事。

use serde::{Deserialize, Serialize};
use story_ir::{CharacterNode, DocNode, SemanticNode, StableId, StoryIR};

// ============================================================================
// Manuscript 数据结构
// ============================================================================

/// 拼接后的完整书稿
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manuscript {
    pub book_title: String,
    pub chapters: Vec<ChapterText>,
}

/// 一章的文本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterText {
    pub id: StableId,
    pub title: String,
    /// 章节编号（从 1 开始，导出时计算）
    pub number: usize,
    pub scenes: Vec<SceneText>,
    /// 本章总字数
    pub word_count: usize,
}

/// 一个场景的文本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneText {
    pub id: StableId,
    pub title: Option<String>,
    /// 所有 Fragment 拼接后的正文
    pub body: String,
}

// ============================================================================
// 拼接
// ============================================================================

/// 从 IR 收集正文（按链表顺序）(RFC-0013 §4.1)
pub fn collect_manuscript(ir: &StoryIR) -> Manuscript {
    let book_title = ir
        .document
        .all_nodes()
        .find_map(|(_, node)| match node {
            DocNode::Book(b) => Some(b.title.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "无题".to_string());

    let mut chapters = Vec::new();

    // 找到 Book 节点获取 Chapter 列表
    for (_, node) in ir.document.all_nodes() {
        if let DocNode::Book(book) = node {
            let chapter_ids = ir.document.children_of(&book.id);

            for (ch_idx, ch_id) in chapter_ids.iter().enumerate() {
                let chapter = match ir.document.get_chapter(ch_id) {
                    Some(c) => c,
                    None => continue,
                };

                let scene_ids = ir.document.children_of(ch_id);

                let mut scenes = Vec::new();
                let mut ch_word_count = 0usize;

                for sc_id in scene_ids {
                    let scene = match ir.document.get_scene(sc_id) {
                        Some(s) => s,
                        None => continue,
                    };

                    let fragment_ids = ir.document.children_of(sc_id);

                    // 拼接该 Scene 内所有 Fragment 的正文
                    let mut bodies = Vec::new();
                    for frag_id in fragment_ids {
                        if let Some(DocNode::Fragment(frag)) = ir.document.get(frag_id) {
                            let text = strip_anchor_comments(&frag.text);
                            if !text.trim().is_empty() {
                                bodies.push(text);
                            }
                        }
                    }

                    let body = bodies.join("\n\n");
                    ch_word_count += count_words(&body);

                    scenes.push(SceneText {
                        id: sc_id.clone(),
                        title: scene.title.clone(),
                        body,
                    });
                }

                chapters.push(ChapterText {
                    id: ch_id.clone(),
                    title: chapter.title.clone(),
                    number: ch_idx + 1,
                    scenes,
                    word_count: ch_word_count,
                });
            }
        }
    }

    Manuscript {
        book_title,
        chapters,
    }
}

// ============================================================================
// 锚点剥离
// ============================================================================

/// 剥离正文中的锚点注释（RFC-0013 §5）
///
/// 移除所有 `<!-- @... -->` 格式的 HTML 注释。
/// 这些是内部标记，成品不应包含。
pub fn strip_anchor_comments(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    loop {
        if let Some(start) = remaining.find("<!--") {
            // 保留注释前的内容
            result.push_str(&remaining[..start]);

            if let Some(end) = remaining[start..].find("-->") {
                // 跳过整个注释
                remaining = &remaining[start + end + 3..];
            } else {
                // 没有结束标记，保留剩余内容
                result.push_str(&remaining[start..]);
                break;
            }
        } else {
            result.push_str(remaining);
            break;
        }
    }

    // 清理多余空行（注释移除后可能留下连续空行）
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    result.trim().to_string()
}

// ============================================================================
// 辅助
// ============================================================================

/// 统计字数（中文字符数 + 英文单词数）
fn count_words(text: &str) -> usize {
    // 全工作区统一口径：CJK 每字 1、ASCII 词 1、标点不计（ir::count_words）
    story_ir::count_words(text) as usize
}

/// 格式化章节标题文本（Markdown / TXT 共用）。
///
/// - `chapter_numbering = true`：`第N章 标题`（或仅 `第N章`，标题为空时）
/// - `chapter_numbering = false`（默认）：`标题`（或 `第N章`，标题为空时 fallback）
pub(crate) fn format_chapter_heading(
    chapter: &ChapterText,
    options: &crate::ExportOptions,
) -> String {
    if options.chapter_numbering {
        if chapter.title.is_empty() {
            format!("第{}章", chapter.number)
        } else {
            format!("第{}章 {}", chapter.number, chapter.title)
        }
    } else if chapter.title.is_empty() {
        // 标题为空时 fallback 到编号，保证至少有内容
        format!("第{}章", chapter.number)
    } else {
        chapter.title.clone()
    }
}

/// 生成章节文件名（Markdown / TXT 共用）。
///
/// 序号补零（至少两位），标题过滤文件系统非法字符。
/// 文件名始终带序号前缀以确保排序正确，不受 `chapter_numbering` 影响。
pub(crate) fn make_chapter_filename(number: usize, title: &str, ext: &str) -> String {
    let sanitized = sanitize_filename(title);
    if sanitized.is_empty() {
        format!("第{:02}章.{ext}", number)
    } else {
        format!("第{:02}章_{}.{ext}", number, sanitized)
    }
}

/// 过滤文件名中的非法字符（跨平台：Windows 限制最严格）
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// 收集全部角色实体，按 display_name 排序（附录生成共用，保证各格式顺序一致）
pub(crate) fn sorted_characters(ir: &StoryIR) -> Vec<&CharacterNode> {
    let mut characters: Vec<&CharacterNode> = ir
        .semantic
        .all_entities()
        .filter_map(|e| match e {
            SemanticNode::Character(c) => Some(c),
            _ => None,
        })
        .collect();
    characters.sort_by_key(|c| c.display_name.clone());
    characters
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ir::*;

    #[test]
    fn strip_comments_basic() {
        let input = "<!-- @entity hero: appears -->\n林风推开门。";
        let result = strip_anchor_comments(input);
        assert_eq!(result, "林风推开门。");
    }

    #[test]
    fn strip_multiple_comments() {
        let input = "<!-- @fragment f_001 -->\n正文一\n<!-- @entity c_001 -->\n正文二";
        let result = strip_anchor_comments(input);
        // 注释移除后留下空行作为段落分隔
        assert_eq!(result, "正文一\n\n正文二");
    }

    #[test]
    fn strip_no_comments() {
        let input = "纯粹的正文，没有注释。";
        let result = strip_anchor_comments(input);
        assert_eq!(result, "纯粹的正文，没有注释。");
    }

    #[test]
    fn collect_empty_manuscript() {
        let ir = StoryIR::new();
        let ms = collect_manuscript(&ir);
        assert_eq!(ms.book_title, "无题");
        assert!(ms.chapters.is_empty());
    }

    #[test]
    fn collect_with_content() {
        let mut ir = StoryIR::new();

        // Book
        ir.document.insert(DocNode::Book(BookNode {
            id: StableId::new_unchecked("b_001"),
            title: "测试书".to_string(),
            symbol: None,
        }));

        // Chapter
        ir.document.insert(DocNode::Chapter(ChapterNode {
            id: StableId::new_unchecked("ch_01"),
            title: "第一章".to_string(),
            symbol: None,
            prev: None,
            next: None,
            story_time_span: None,
        }));
        ir.document.set_children(
            StableId::new_unchecked("b_001"),
            vec![StableId::new_unchecked("ch_01")],
        );

        // Scene
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_01"),
            chapter: StableId::new_unchecked("ch_01"),
            title: Some("场景一".to_string()),
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
        ir.document.set_children(
            StableId::new_unchecked("ch_01"),
            vec![StableId::new_unchecked("sc_01")],
        );

        // Fragment
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: Some(StableId::new_unchecked("sc_01")),
            prev: None,
            next: None,
            text: "林风推开门。".to_string(),
            anchors: vec![],
        }));
        ir.document.set_children(
            StableId::new_unchecked("sc_01"),
            vec![StableId::new_unchecked("f_001")],
        );

        let ms = collect_manuscript(&ir);
        assert_eq!(ms.book_title, "测试书");
        assert_eq!(ms.chapters.len(), 1);
        assert_eq!(ms.chapters[0].title, "第一章");
        assert_eq!(ms.chapters[0].number, 1);
        assert_eq!(ms.chapters[0].scenes.len(), 1);
        assert_eq!(ms.chapters[0].scenes[0].title.as_deref(), Some("场景一"));
        assert!(ms.chapters[0].scenes[0].body.contains("林风推开门"));
    }
}
