//! TXT Exporter — 导出为纯文本（RFC-0013 §8.1 扩展）
//!
//! 面向网文投稿、有声书稿等纯文本交付场景：
//! - 剥离行内 Markdown 语法（粗体、斜体、链接等），只留文字
//! - `paragraph_indent` 选项：段首加两个全角空格（中文稿件习惯排版）
//! - 结构与 Markdown 导出一致：书名、章节标题独立成行（不带 `#` 前缀），
//!   场景按选项带标题或用 `---` 分割线分隔
//!
//! 锚点注释已在 `collect_manuscript` 中剥离（`AnchorHandling::Strip`）。

use story_ir::StoryIR;

use crate::manuscript::{
    collect_manuscript, format_chapter_heading, make_chapter_filename, sorted_characters,
};
use crate::{ChapterFile, ExportError, ExportFormat, ExportOptions, ExportOutput, Exporter};

// ============================================================================
// TxtExporter
// ============================================================================

#[derive(Debug)]
pub struct TxtExporter;

impl TxtExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TxtExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Exporter for TxtExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Txt
    }

    fn export(&self, ir: &StoryIR, options: &ExportOptions) -> Result<ExportOutput, ExportError> {
        let manuscript = collect_manuscript(ir);
        let mut txt = String::new();

        // 书名（纯文本，无 # 前缀）
        txt.push_str(&format!("{}\n\n", manuscript.book_title));

        // 各章
        for chapter in manuscript.chapters.iter().filter(|c| in_scope(c, options)) {
            txt.push_str(&format!("{}\n\n", format_chapter_heading(chapter, options)));

            for (i, scene) in chapter.scenes.iter().enumerate() {
                // 场景之间加分割线（不带场景标题时）
                if i > 0 && !options.scene_titles {
                    txt.push_str("---\n\n");
                }

                // 场景标题（可选）
                if options.scene_titles {
                    if let Some(ref title) = scene.title {
                        txt.push_str(&format!("{title}\n\n"));
                    }
                }

                txt.push_str(&render_body(&scene.body, options));
                txt.push_str("\n\n");
            }
        }

        // 附录
        if options.include_appendix {
            txt.push_str(&generate_appendix(ir));
        }

        Ok(ExportOutput {
            content: txt,
            filename: "manuscript.txt".to_string(),
        })
    }
}

impl TxtExporter {
    /// 分章节导出 — 每章一个独立的 TXT 文件
    ///
    /// 遍历 `collect_manuscript` 的每一章，为每章生成独立的纯文本内容。
    /// 章文件不含书名行，只有章节标题与正文。
    pub fn export_chapters(
        &self,
        ir: &StoryIR,
        options: &ExportOptions,
    ) -> Result<Vec<ChapterFile>, ExportError> {
        let manuscript = collect_manuscript(ir);
        let mut files = Vec::new();

        for chapter in manuscript.chapters.iter().filter(|c| in_scope(c, options)) {
            let mut txt = String::new();
            txt.push_str(&format!("{}\n\n", format_chapter_heading(chapter, options)));

            for (i, scene) in chapter.scenes.iter().enumerate() {
                // 场景之间加分割线（不带场景标题时）
                if i > 0 && !options.scene_titles {
                    txt.push_str("---\n\n");
                }

                // 场景标题（可选）
                if options.scene_titles {
                    if let Some(ref title) = scene.title {
                        txt.push_str(&format!("{title}\n\n"));
                    }
                }

                txt.push_str(&render_body(&scene.body, options));
                txt.push_str("\n\n");
            }

            // 附录（仅追加到最后一章的文件）
            if options.include_appendix && chapter.number == manuscript.chapters.len() {
                txt.push_str(&generate_appendix(ir));
            }

            files.push(ChapterFile {
                filename: make_chapter_filename(chapter.number, &chapter.title, "txt"),
                content: txt,
            });
        }

        Ok(files)
    }
}

// ============================================================================
// 正文渲染
// ============================================================================

/// 渲染场景正文：剥离行内 Markdown 语法，可选段首缩进
fn render_body(body: &str, options: &ExportOptions) -> String {
    let plain = strip_inline_markdown(body);
    if options.paragraph_indent {
        indent_paragraphs(&plain)
    } else {
        plain
    }
}

/// 章节是否在导出范围内
fn in_scope(chapter: &crate::manuscript::ChapterText, options: &ExportOptions) -> bool {
    match &options.scope {
        crate::ExportScope::All => true,
        crate::ExportScope::Chapters(ids) => ids.iter().any(|id| id == chapter.id.as_str()),
    }
}

// ============================================================================
// 行内 Markdown 剥离
// ============================================================================

/// 剥离文本中的行内 Markdown 语法，只保留文字内容。
///
/// 处理：
/// - 图片 `![alt](url)` → 整体删除
/// - 链接 `[text](url)` → 保留 text
/// - 粗体 `**x**` / `__x__`、斜体 `*x*`、删除线 `~~x~~`、行内代码 `` `x` `` → 保留 x
/// - 行首标题标记 `#` / `##` / … → 删除标记
///
/// 遵循 CommonMark 的 flanking 规则（开标记后、闭标记前不能是空白），
/// 因此 `2 * 3 * 4` 这类算式不会被误伤。不成对的标记原样保留。
fn strip_inline_markdown(text: &str) -> String {
    text.lines().map(strip_line).collect::<Vec<_>>().join("\n")
}

/// 剥离单行内的行内 Markdown 语法
fn strip_line(line: &str) -> String {
    let line = strip_heading_marker(line);
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;

    while i < chars.len() {
        // 图片 ![alt](url)：整体删除
        if chars[i] == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some((_, end)) = split_link(&chars, i + 1) {
                i = end;
                continue;
            }
        }

        // 链接 [text](url)：保留 text
        if chars[i] == '[' {
            if let Some((text_end, link_end)) = split_link(&chars, i) {
                let inner: String = chars[i + 1..text_end].iter().collect();
                out.push_str(&strip_line(&inner)); // 链接文字内可能还有强调
                i = link_end;
                continue;
            }
        }

        // 成对包裹标记：**x** / __x__ / *x* / ~~x~~ / `x`
        if let Some(marker) = opening_marker(&chars, i) {
            let mlen = marker.chars().count();
            // flanking：开标记后不能是空白
            if chars.get(i + mlen).is_some_and(|c| !c.is_whitespace()) {
                if let Some(close) = find_closing_marker(&chars, i + mlen, marker) {
                    let inner: String = chars[i + mlen..close].iter().collect();
                    out.push_str(&strip_line(&inner)); // 递归处理嵌套强调
                    i = close + mlen;
                    continue;
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

/// 剥离行首标题标记：`### 标题` → `标题`（防御性处理，正文一般不含）
fn strip_heading_marker(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if hashes <= 6 {
            let rest = &trimmed[hashes..];
            if rest.starts_with(' ') {
                return rest.trim_start();
            }
        }
    }
    line
}

/// 解析位置 `i` 处（`chars[i] == '['`）的 `[text](url)`：
/// 返回 `(text 结束下标, 整个链接结束下标)`。不是行内链接则返回 None。
fn split_link(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let text_end = chars[i + 1..].iter().position(|&c| c == ']')? + i + 1;
    if chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let close = chars[text_end + 2..].iter().position(|&c| c == ')')? + text_end + 2;
    Some((text_end, close + 1))
}

/// 位置 `i` 处的成对包裹标记（双字符优先，避免 `**` 被当成两个 `*`）
fn opening_marker(chars: &[char], i: usize) -> Option<&'static str> {
    const MARKERS: [&str; 5] = ["**", "__", "~~", "*", "`"];
    MARKERS
        .iter()
        .find(|m| {
            let mc: Vec<char> = m.chars().collect();
            chars[i..].starts_with(&mc)
        })
        .copied()
}

/// 从 `from` 开始查找闭标记，带 flanking 检查（闭标记前不能是空白）
fn find_closing_marker(chars: &[char], from: usize, marker: &str) -> Option<usize> {
    let mc: Vec<char> = marker.chars().collect();
    let mut i = from;
    while i + mc.len() <= chars.len() {
        if chars[i..].starts_with(&mc) && !chars[i - 1].is_whitespace() {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ============================================================================
// 段首缩进
// ============================================================================

/// 段首缩进：每个非空行前加两个全角空格
///
/// 已有缩进（全角空格、半角空白开头）的行不再追加，避免双重缩进。
fn indent_paragraphs(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() || line.starts_with(['　', ' ', '\t']) {
                line.to_string()
            } else {
                format!("　　{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ============================================================================
// 附录
// ============================================================================

/// 生成附录（角色表）
fn generate_appendix(ir: &StoryIR) -> String {
    let mut appendix = String::new();
    appendix.push_str("---\n\n附录：角色表\n\n");

    let characters = sorted_characters(ir);

    if characters.is_empty() {
        appendix.push_str("（无角色）\n");
    } else {
        for char in characters {
            appendix.push_str(&format!("- {}", char.display_name));
            if !char.aliases.is_empty() {
                appendix.push_str(&format!("（{}）", char.aliases.join("、")));
            }
            appendix.push('\n');
        }
    }

    appendix
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ir::*;

    fn make_test_ir() -> StoryIR {
        let mut ir = StoryIR::new();

        ir.document.insert(DocNode::Book(BookNode {
            id: StableId::new_unchecked("b_001"),
            title: "雨夜行".to_string(),
            symbol: None,
        }));

        ir.document.insert(DocNode::Chapter(ChapterNode {
            id: StableId::new_unchecked("ch_01"),
            title: "雨夜".to_string(),
            symbol: None,
            prev: None,
            next: None,
            story_time_span: None,
        }));
        ir.document.set_children(
            StableId::new_unchecked("b_001"),
            vec![StableId::new_unchecked("ch_01")],
        );

        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_01"),
            chapter: StableId::new_unchecked("ch_01"),
            title: Some("渡口".to_string()),
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

        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: Some(StableId::new_unchecked("sc_01")),
            prev: None,
            next: None,
            text: "林风推开门。\n雨水顺着发梢滴落。".to_string(),
            anchors: vec![],
        }));
        ir.document.set_children(
            StableId::new_unchecked("sc_01"),
            vec![StableId::new_unchecked("f_001")],
        );

        // 角色
        ir.semantic.insert(SemanticNode::Character(CharacterNode {
            id: StableId::new_unchecked("c_001"),
            symbol: None,
            display_name: "林风".to_string(),
            aliases: vec!["林少侠".to_string()],
            name_concealed: false,
            attributes: Default::default(),
        }));

        ir
    }

    #[test]
    fn export_txt_basic() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        // 书名独立成行，无 # 前缀
        assert!(output.content.starts_with("雨夜行\n"));
        // 章节标题独立成行，无 ## 前缀
        assert!(output.content.contains("\n雨夜\n"));
        assert!(!output.content.contains('#'));
        assert!(output.content.contains("林风推开门"));
        assert_eq!(output.filename, "manuscript.txt");
    }

    #[test]
    fn export_txt_with_chapter_numbering() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let options = ExportOptions {
            chapter_numbering: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        assert!(output.content.contains("第1章 雨夜"));
    }

    #[test]
    fn export_txt_with_scene_titles() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let options = ExportOptions {
            scene_titles: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        // 场景标题独立成行，无 ### 前缀
        assert!(output.content.contains("\n渡口\n"));
        // 单个场景不带分割线
        assert!(!output.content.contains("---"));
    }

    #[test]
    fn export_txt_strips_inline_markdown() {
        let mut ir = make_test_ir();
        // 替换 Fragment 正文，混入各种行内语法
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_001"),
            scene: Some(StableId::new_unchecked("sc_01")),
            prev: None,
            next: None,
            text: "**剑光**一闪，他想起[剑谱](https://example.com/jianpu)的记载。\n第二段有*斜体*与~~删改~~。".to_string(),
            anchors: vec![],
        }));

        let exporter = TxtExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(output.content.contains("剑光一闪"));
        assert!(output.content.contains("想起剑谱的记载"));
        // 链接 URL 不残留
        assert!(!output.content.contains("https://"));
        assert!(output.content.contains("第二段有斜体与删改"));
        assert!(!output.content.contains("**"));
        assert!(!output.content.contains("~~"));
    }

    #[test]
    fn export_txt_indent() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let options = ExportOptions {
            paragraph_indent: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        // 正文段落缩进两个全角空格
        assert!(output.content.contains("　　林风推开门。"));
        assert!(output.content.contains("　　雨水顺着发梢滴落。"));
        // 书名与章节标题不缩进
        assert!(output.content.starts_with("雨夜行\n"));
        assert!(output.content.contains("\n雨夜\n"));
    }

    #[test]
    fn export_txt_no_indent_by_default() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(!output.content.contains("　　林风推开门"));
    }

    #[test]
    fn export_txt_appendix_plain() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let options = ExportOptions {
            include_appendix: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        assert!(output.content.contains("附录：角色表"));
        assert!(output.content.contains("- 林风（林少侠）"));
        // 纯文本附录不带 Markdown 强调
        assert!(!output.content.contains("**"));
    }

    #[test]
    fn export_txt_empty_ir() {
        let ir = StoryIR::new();
        let exporter = TxtExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(output.content.starts_with("无题\n"));
    }

    #[test]
    fn export_txt_chapters_basic() {
        let ir = make_test_ir();
        let exporter = TxtExporter::new();
        let files = exporter
            .export_chapters(&ir, &ExportOptions::default())
            .unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].filename.starts_with("第01章"));
        assert!(files[0].filename.ends_with(".txt"));
        // 章文件不含书名行
        assert!(!files[0].content.contains("雨夜行"));
        assert!(files[0].content.contains("林风推开门"));
    }

    // ---------------------------------------------------------------------------
    // 行内剥离单元测试
    // ---------------------------------------------------------------------------

    #[test]
    fn strip_preserves_plain_text() {
        assert_eq!(strip_inline_markdown("纯粹的正文。"), "纯粹的正文。");
    }

    #[test]
    fn strip_emphasis_variants() {
        assert_eq!(strip_inline_markdown("**粗体**"), "粗体");
        assert_eq!(strip_inline_markdown("__粗体__"), "粗体");
        assert_eq!(strip_inline_markdown("*斜体*"), "斜体");
        assert_eq!(strip_inline_markdown("~~删除~~"), "删除");
        assert_eq!(strip_inline_markdown("`代码`"), "代码");
    }

    #[test]
    fn strip_nested_emphasis() {
        assert_eq!(strip_inline_markdown("**粗*斜*体**"), "粗斜体");
    }

    #[test]
    fn strip_links_and_images() {
        assert_eq!(
            strip_inline_markdown("见[剑谱](https://example.com)记载"),
            "见剑谱记载"
        );
        assert_eq!(
            strip_inline_markdown("![插图](https://example.com/i.png)后续"),
            "后续"
        );
    }

    #[test]
    fn strip_unpaired_markers_pass_through() {
        // 乘号算式不应被吞（flanking 规则保护）
        assert_eq!(strip_inline_markdown("2 * 3 * 4 = 24"), "2 * 3 * 4 = 24");
        // 不成对标记原样保留
        assert_eq!(strip_inline_markdown("一颗*星"), "一颗*星");
    }

    #[test]
    fn strip_heading_markers() {
        assert_eq!(strip_inline_markdown("### 小标题"), "小标题");
        assert_eq!(strip_inline_markdown("井号#在句中"), "井号#在句中");
    }

    #[test]
    fn strip_empty_and_multi_line() {
        assert_eq!(strip_inline_markdown(""), "");
        assert_eq!(
            strip_inline_markdown("第一行\n\n**第二行**"),
            "第一行\n\n第二行"
        );
    }

    #[test]
    fn indent_skips_already_indented() {
        assert_eq!(
            indent_paragraphs("首段\n　　已缩进\n\n次段"),
            "　　首段\n　　已缩进\n\n　　次段"
        );
    }
}
