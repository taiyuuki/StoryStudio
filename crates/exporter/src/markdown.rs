//! Markdown Exporter — 导出为 Markdown (RFC-0013 §8.1)
//!
//! 最简单的导出器：IR 本身就是 Markdown 拼接。
//! 章节用 ## 标题，场景用 ### 标题，Fragment 无缝拼接。

use story_ir::StoryIR;

use crate::manuscript::collect_manuscript;
use crate::{
    AnchorHandling, ChapterFile, ExportError, ExportFormat, ExportOptions, ExportOutput, Exporter,
};

// ============================================================================
// MarkdownExporter
// ============================================================================

#[derive(Debug)]
pub struct MarkdownExporter;

impl MarkdownExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownExporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Exporter for MarkdownExporter {
    fn format(&self) -> ExportFormat {
        ExportFormat::Markdown
    }

    fn export(&self, ir: &StoryIR, options: &ExportOptions) -> Result<ExportOutput, ExportError> {
        let manuscript = collect_manuscript(ir);
        let mut md = String::new();

        // 书名
        md.push_str(&format!("# {}\n\n", manuscript.book_title));

        // 各章
        for chapter in &manuscript.chapters {
            // 章节范围过滤
            if let crate::ExportScope::Chapters(ref ids) = options.scope {
                if !ids.iter().any(|id| id == chapter.id.as_str()) {
                    continue;
                }
            }

            md.push_str(&format!(
                "## {}\n\n",
                format_chapter_heading(chapter, options)
            ));

            for (i, scene) in chapter.scenes.iter().enumerate() {
                // 场景之间加分割线（不带场景标题时）
                if i > 0 && !options.scene_titles {
                    md.push_str("---\n\n");
                }

                // 场景标题（可选）
                if options.scene_titles {
                    if let Some(ref title) = scene.title {
                        md.push_str(&format!("### {title}\n\n"));
                    }
                }

                // 锚点处理
                let body = match options.anchor_handling {
                    AnchorHandling::Strip => scene.body.clone(), // collect_manuscript 已剥离
                    AnchorHandling::Keep => scene.body.clone(),  // TODO: Keep 模式需保留原文
                };

                md.push_str(&body);
                md.push_str("\n\n");
            }
        }

        // 附录
        if options.include_appendix {
            md.push_str(&Self::generate_appendix(ir));
        }

        Ok(ExportOutput {
            content: md,
            filename: "manuscript.md".to_string(),
        })
    }
}

impl MarkdownExporter {
    /// 分章节导出 — 每章一个独立的 Markdown 文件 (RFC-0013 §8.1 扩展)
    ///
    /// 遍历 `collect_manuscript` 的每一章，为每章生成独立的 Markdown 内容。
    /// 锚点注释已在 `collect_manuscript` 中剥离。
    pub fn export_chapters(
        &self,
        ir: &StoryIR,
        options: &ExportOptions,
    ) -> Result<Vec<ChapterFile>, ExportError> {
        let manuscript = collect_manuscript(ir);
        let mut files = Vec::new();

        for chapter in &manuscript.chapters {
            // 章节范围过滤
            if let crate::ExportScope::Chapters(ref ids) = options.scope {
                if !ids.iter().any(|id| id == chapter.id.as_str()) {
                    continue;
                }
            }

            let mut md = String::new();
            md.push_str(&format!(
                "## {}\n\n",
                format_chapter_heading(chapter, options)
            ));

            for (i, scene) in chapter.scenes.iter().enumerate() {
                // 场景之间加分割线（不带场景标题时）
                if i > 0 && !options.scene_titles {
                    md.push_str("---\n\n");
                }

                // 场景标题（可选）
                if options.scene_titles {
                    if let Some(ref title) = scene.title {
                        md.push_str(&format!("### {title}\n\n"));
                    }
                }

                let body = match options.anchor_handling {
                    AnchorHandling::Strip => scene.body.clone(), // collect_manuscript 已剥离
                    AnchorHandling::Keep => scene.body.clone(),  // TODO: Keep 模式需保留原文
                };

                md.push_str(&body);
                md.push_str("\n\n");
            }

            // 附录（仅追加到最后一章的文件）
            if options.include_appendix && chapter.number == manuscript.chapters.len() {
                md.push_str(&Self::generate_appendix(ir));
            }

            files.push(ChapterFile {
                filename: make_chapter_filename(chapter.number, &chapter.title),
                content: md,
            });
        }

        Ok(files)
    }

    /// 生成附录（角色表）
    fn generate_appendix(ir: &StoryIR) -> String {
        let mut appendix = String::new();
        appendix.push_str("---\n\n## 附录：角色表\n\n");

        let mut characters: Vec<_> = ir
            .semantic
            .all_entities()
            .filter(|e| matches!(e, story_ir::SemanticNode::Character(_)))
            .collect();

        // 按 display_name 排序（确定性）
        characters.sort_by_key(|e| e.display_name().to_string());

        if characters.is_empty() {
            appendix.push_str("（无角色）\n");
        } else {
            for entity in &characters {
                if let story_ir::SemanticNode::Character(char) = entity {
                    appendix.push_str(&format!("- **{}**", char.display_name));
                    if !char.aliases.is_empty() {
                        appendix.push_str(&format!("（{}）", char.aliases.join("、")));
                    }
                    appendix.push('\n');
                }
            }
        }

        appendix
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 格式化章节标题文本。
///
/// - `chapter_numbering = true`：`第N章 标题`（或仅 `第N章`，标题为空时）
/// - `chapter_numbering = false`（默认）：`标题`（或 `第N章`，标题为空时 fallback）
fn format_chapter_heading(
    chapter: &crate::manuscript::ChapterText,
    options: &ExportOptions,
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

/// 生成章节文件名。
///
/// 序号补零（至少两位），标题过滤文件系统非法字符。
/// 文件名始终带序号前缀以确保排序正确，不受 `chapter_numbering` 影响。
fn make_chapter_filename(number: usize, title: &str) -> String {
    let sanitized = sanitize_filename(title);
    if sanitized.is_empty() {
        format!("第{:02}章.md", number)
    } else {
        format!("第{:02}章_{}.md", number, sanitized)
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
            attributes: Default::default(),
        }));

        ir
    }

    #[test]
    fn export_markdown_basic() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(output.content.contains("# 雨夜行"));
        // 默认不加章节编号前缀，只用标题原文
        assert!(output.content.contains("## 雨夜"));
        assert!(!output.content.contains("## 第1章"));
        // 默认不带场景标题
        assert!(!output.content.contains("### 渡口"));
        assert!(output.content.contains("林风推开门"));
        assert_eq!(output.filename, "manuscript.md");
    }

    #[test]
    fn export_markdown_with_chapter_numbering() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let options = ExportOptions {
            chapter_numbering: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        assert!(output.content.contains("## 第1章 雨夜"));
    }

    #[test]
    fn export_markdown_with_scene_titles() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let options = ExportOptions {
            scene_titles: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        assert!(output.content.contains("### 渡口"));
        // 单个场景不带分割线
        assert!(!output.content.contains("---"));
    }

    #[test]
    fn export_markdown_with_appendix() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let options = ExportOptions {
            include_appendix: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        assert!(output.content.contains("附录：角色表"));
        assert!(output.content.contains("**林风**"));
        assert!(output.content.contains("林少侠"));
    }

    #[test]
    fn export_markdown_no_appendix_by_default() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(!output.content.contains("附录"));
    }

    #[test]
    fn export_markdown_empty_ir() {
        let ir = StoryIR::new();
        let exporter = MarkdownExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        assert!(output.content.contains("# 无题"));
    }

    #[test]
    fn export_chapters_basic() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let files = exporter
            .export_chapters(&ir, &ExportOptions::default())
            .unwrap();

        assert_eq!(files.len(), 1);
        // 文件名始终带序号前缀（保证排序）
        assert!(files[0].filename.starts_with("第01章"));
        assert!(files[0].filename.ends_with(".md"));
        // 默认不加章节编号前缀，只用标题原文
        assert!(files[0].content.contains("## 雨夜"));
        assert!(!files[0].content.contains("## 第1章"));
        // 默认不带场景标题
        assert!(!files[0].content.contains("### 渡口"));
        assert!(files[0].content.contains("林风推开门"));
        // 分章导出不含书名（# 标题），只有章节标题
        assert!(!files[0].content.contains("# 雨夜行\n"));
    }

    #[test]
    fn export_chapters_filename_sanitized() {
        let ir = make_test_ir();
        let exporter = MarkdownExporter::new();
        let files = exporter
            .export_chapters(&ir, &ExportOptions::default())
            .unwrap();

        // 文件名中不应包含非法字符
        assert!(!files[0].filename.contains('/'));
        assert!(!files[0].filename.contains('\\'));
    }

    /// 构建含两个场景的测试 IR，验证场景间分割线行为
    fn make_multi_scene_ir() -> StoryIR {
        let mut ir = make_test_ir();
        // 在已有的 ch_01 下添加第二个场景
        ir.document.insert(DocNode::Scene(SceneNode {
            id: StableId::new_unchecked("sc_02"),
            chapter: StableId::new_unchecked("ch_01"),
            title: Some("客栈".to_string()),
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
        ir.document.insert(DocNode::Fragment(FragmentNode {
            id: StableId::new_unchecked("f_002"),
            scene: Some(StableId::new_unchecked("sc_02")),
            prev: None,
            next: None,
            text: "听雨客栈的二楼临窗。".to_string(),
            anchors: vec![],
        }));
        ir.document.set_children(
            StableId::new_unchecked("ch_01"),
            vec![
                StableId::new_unchecked("sc_01"),
                StableId::new_unchecked("sc_02"),
            ],
        );
        ir.document.set_children(
            StableId::new_unchecked("sc_02"),
            vec![StableId::new_unchecked("f_002")],
        );
        ir
    }

    #[test]
    fn export_multi_scene_no_titles_with_divider() {
        let ir = make_multi_scene_ir();
        let exporter = MarkdownExporter::new();
        let output = exporter.export(&ir, &ExportOptions::default()).unwrap();

        // 默认不带场景标题
        assert!(!output.content.contains("### 渡口"));
        assert!(!output.content.contains("### 客栈"));
        // 场景之间有分割线
        assert!(output.content.contains("---"));
        assert!(output.content.contains("林风推开门"));
        assert!(output.content.contains("听雨客栈"));
    }

    #[test]
    fn export_multi_scene_with_titles_no_divider() {
        let ir = make_multi_scene_ir();
        let exporter = MarkdownExporter::new();
        let options = ExportOptions {
            scene_titles: true,
            ..Default::default()
        };
        let output = exporter.export(&ir, &options).unwrap();

        // 带场景标题时显示标题，不加分割线
        assert!(output.content.contains("### 渡口"));
        assert!(output.content.contains("### 客栈"));
        assert!(!output.content.contains("---"));
    }
}
