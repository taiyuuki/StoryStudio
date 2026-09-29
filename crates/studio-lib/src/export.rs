//! 导出编排 — parse → export → 返回内容

use std::path::Path;

use story_exporter::{export, ChapterFile, ExportOptions, ExportOutput, MarkdownExporter};
use story_parser::parse_project;

use crate::error::StudioError;

// ============================================================================
// API
// ============================================================================

/// 导出为 Markdown
///
/// 解析项目 → 导出 IR → 返回内容。
/// - `include_appendix` 为 true 时追加角色表附录
/// - `scene_titles` 为 true 时渲染场景标题（### ）；为 false 时场景之间用分割线分隔
/// - `chapter_numbering` 为 true 时添加「第N章 」前缀；为 false 时只用标题原文
pub fn export_markdown(
    dir: &Path,
    include_appendix: bool,
    scene_titles: bool,
    chapter_numbering: bool,
) -> Result<ExportOutput, StudioError> {
    // 解析项目
    let parse_output = parse_project(dir).map_err(StudioError::from)?;

    // 导出
    let exporter = MarkdownExporter::new();
    let options = ExportOptions {
        include_appendix,
        scene_titles,
        chapter_numbering,
        ..Default::default()
    };

    export(&exporter, &parse_output.ir, &options).map_err(|e| StudioError::Export(e.to_string()))
}

/// 分章节导出为 Markdown — 每章一个独立的文件
///
/// 解析项目 → 导出 IR → 返回每章的文件名与内容。
/// - `include_appendix` 为 true 时，角色表附录追加到最后一章的文件末尾
/// - `scene_titles` 为 true 时渲染场景标题（### ）；为 false 时场景之间用分割线分隔
/// - `chapter_numbering` 为 true 时添加「第N章 」前缀；为 false 时只用标题原文
pub fn export_chapters_markdown(
    dir: &Path,
    include_appendix: bool,
    scene_titles: bool,
    chapter_numbering: bool,
) -> Result<Vec<ChapterFile>, StudioError> {
    // 解析项目
    let parse_output = parse_project(dir).map_err(StudioError::from)?;

    // 导出
    let exporter = MarkdownExporter::new();
    let options = ExportOptions {
        include_appendix,
        scene_titles,
        chapter_numbering,
        ..Default::default()
    };

    exporter
        .export_chapters(&parse_output.ir, &options)
        .map_err(|e| StudioError::Export(e.to_string()))
}
