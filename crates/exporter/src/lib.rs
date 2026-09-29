//! Export — IR 到可发布格式的转换 (RFC-0013)
//!
//! Export 是 IR 的纯消费者：
//! - 只读 IR，不修改 IR
//! - 渲染信息（章节编号、页码）在管道内计算，不写回 IR
//! - 锚点注释在导出时剥离
//!
//! v0.1 实现 Markdown 导出。EPUB / PDF 留待后续。

pub mod manuscript;
pub mod markdown;

pub use manuscript::{
    collect_manuscript, strip_anchor_comments, ChapterText, Manuscript, SceneText,
};
pub use markdown::MarkdownExporter;

use serde::{Deserialize, Serialize};
use story_ir::StoryIR;
use thiserror::Error;

/// 导出格式
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Markdown,
    Html,
    Epub,
    Pdf,
    Custom(String),
}

/// 导出选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    /// 导出范围
    pub scope: ExportScope,
    /// 是否包含附录（角色表、地点表）
    pub include_appendix: bool,
    /// 是否包含场景标题（默认 false：不带场景标题，场景之间用分割线分隔）
    pub scene_titles: bool,
    /// 是否添加章节编号前缀「第N章 」（默认 false：不加，仅用章节标题原文）
    pub chapter_numbering: bool,
    /// 锚点处理方式
    pub anchor_handling: AnchorHandling,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            scope: ExportScope::All,
            include_appendix: false,
            scene_titles: false,
            chapter_numbering: false,
            anchor_handling: AnchorHandling::Strip,
        }
    }
}

/// 导出范围
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportScope {
    All,
    Chapters(Vec<String>),
}

/// 锚点处理方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorHandling {
    /// 剥离所有 <!-- @... --> 注释（成品不含内部标记）
    Strip,
    /// 保留为 HTML 注释（调试用）
    Keep,
}

/// 导出输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOutput {
    pub content: String,
    pub filename: String,
}

/// 分章节导出的单章文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterFile {
    /// 文件名（含扩展名，如 `第01章_雨夜.md`）
    pub filename: String,
    /// 单章 Markdown 内容
    pub content: String,
}

/// 导出错误
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum ExportError {
    #[error("导出失败: {0}")]
    Failed(String),
}

/// Exporter trait —— 所有导出格式的统一接口
pub trait Exporter {
    fn format(&self) -> ExportFormat;
    fn export(&self, ir: &StoryIR, options: &ExportOptions) -> Result<ExportOutput, ExportError>;
}

/// 便捷入口：使用指定 Exporter 导出
pub fn export(
    exporter: &dyn Exporter,
    ir: &StoryIR,
    options: &ExportOptions,
) -> Result<ExportOutput, ExportError> {
    exporter.export(ir, options)
}
