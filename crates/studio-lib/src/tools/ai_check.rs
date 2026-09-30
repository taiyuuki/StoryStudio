//! AI 检测工具 — 让创作 Agent 自我监督 AI 写作特征
//!
//! 提供 `ai_check_fragment` 工具，Agent 写完正文后可调用它自检，
//! 拿到 AI 味检测报告后据此反思是否需要调整。

use story_ai_runtime::{Tool, ToolDef};

use super::{get_str, object_schema, ToolContext};
use crate::ai_check::{self, AiCheckSeverity};

// ============================================================================
// ai_check_fragment
// ============================================================================

pub struct AiCheckTool {
    ctx: ToolContext,
}
impl AiCheckTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AiCheckTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "ai_check_fragment".into(),
            description: "检测指定片段正文的 AI 写作特征（俗称「AI 味」），\
            返回按文本顺序排列的命中报告。\
            覆盖 18 条小说适配特征：形容词堆砌、对话千人一面、情绪直接命名、比喻密度过高、\
            匀速排比、让步模板反复、情绪曲线光滑、没有任何犹豫、段段金句、句子节奏均匀、\
            连接词过度使用、同义词替换、中文翻译腔、对深刻过拟合等。\
            写完正文后建议调用自检，但不必每次都调——只在拿不准某段是否「太光滑」时使用；\
            整章/整场景交付前用 ai_check_chapter / ai_check_scene 批量过一遍更省事。"
                .into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "要检测的片段 ID（如 f_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;

        let provider = self.ctx.provider.as_ref().ok_or_else(|| {
            "ai_check_fragment 需要 provider，但当前未配置。请确保通过项目模式运行。".to_string()
        })?;

        let dir = self.ctx.project_dir.clone();
        let report = ai_check::check_fragment(&dir, &fragment_id, provider)
            .map_err(|e| format!("AI 检测失败: {e}"))?;

        Ok(format_report_for_llm(&report))
    }
}

// ============================================================================
// ai_check_chapter / ai_check_scene — 范围级批量检测
// ============================================================================

pub struct AiCheckScopeTool {
    ctx: ToolContext,
    scope: ai_check::CheckScope,
    /// 工具名（ai_check_chapter / ai_check_scene）
    name: &'static str,
    /// 参数名（chapter_id / scene_id）
    param: &'static str,
    /// 参数说明
    param_desc: &'static str,
    label: &'static str,
}
impl AiCheckScopeTool {
    pub fn chapter(ctx: ToolContext) -> Self {
        Self {
            ctx,
            scope: ai_check::CheckScope::Chapter,
            name: "ai_check_chapter",
            param: "chapter_id",
            param_desc: "章节 ID（如 ch_01）",
            label: "整章",
        }
    }

    pub fn scene(ctx: ToolContext) -> Self {
        Self {
            ctx,
            scope: ai_check::CheckScope::Scene,
            name: "ai_check_scene",
            param: "scene_id",
            param_desc: "场景 ID（如 sc_01_01）",
            label: "场景",
        }
    }
}

impl Tool for AiCheckScopeTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name.into(),
            description: format!(
                "对整个{}做 AI 写作特征批量检测（检测特征与 ai_check_fragment 相同），\
                正文按阅读序聚合、超长自动分块，命中项标注所在片段。\
                {}交付前过一遍用它，不用逐片段调用。",
                self.label,
                if self.scope == ai_check::CheckScope::Chapter {
                    "整章"
                } else {
                    "整场景"
                }
            ),
            parameters: object_schema(vec![(self.param, "string", self.param_desc, true)]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scope_id = get_str(arguments, self.param)?;

        let provider = self.ctx.provider.as_ref().ok_or_else(|| {
            format!("{} 需要 provider，但当前未配置。请确保通过项目模式运行。", self.name)
        })?;

        let dir = self.ctx.project_dir.clone();
        let report = ai_check::check_scope(&dir, self.scope, &scope_id, provider)
            .map_err(|e| format!("AI 检测失败: {e}"))?;

        Ok(format_batch_report_for_llm(&report))
    }
}

/// 把批量检测报告格式化为给 LLM 看的纯文本
fn format_batch_report_for_llm(report: &ai_check::AiCheckBatchReport) -> String {
    let real: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.feature_id != "parse_error")
        .collect();

    let scope_label = if report.scope == "chapter" {
        "章"
    } else {
        "场景"
    };

    let mut out = format!(
        "{} {} 检测完成：覆盖 {} 个片段，命中 {} 处 AI 写作特征",
        scope_label,
        report.scope_id,
        report.fragment_hits.len(),
        real.len()
    );

    // 各片段命中分布（有过一遍的确认感）
    let hits_line = report
        .fragment_hits
        .iter()
        .map(|(fid, n)| if *n > 0 { format!("{fid}×{n}") } else { fid.clone() })
        .collect::<Vec<_>>()
        .join("、");
    out.push_str(&format!("\n命中分布：{hits_line}"));

    if let Some(err_item) = report.items.iter().find(|i| i.feature_id == "parse_error") {
        out.push_str(&format!(
            "\n\n注意：部分分块的报告解析失败：{}",
            err_item.explanation
        ));
        return out;
    }

    if real.is_empty() {
        out.push_str(&format!("\n{}", report.summary));
        return out;
    }

    out.push('\n');
    for (idx, item) in real.iter().enumerate() {
        let sev = match item.severity {
            AiCheckSeverity::Strong => "🔴强信号",
            AiCheckSeverity::Medium => "⚠️中信号",
            AiCheckSeverity::Weak => "💡弱信号",
        };
        let fid = item.fragment_id.as_deref().unwrap_or("?");
        let quote_display = if item.quote.is_empty() {
            "（无原文引用）"
        } else {
            &item.quote
        };
        out.push_str(&format!(
            "\n{}. [{sev} 特征#{} {}｜{}]\n   「{}」\n   → {}",
            idx + 1,
            item.feature_id,
            item.feature_name,
            fid,
            quote_display,
            item.explanation
        ));
    }

    if !report.summary.is_empty() {
        out.push_str(&format!("\n\n总结：{}", report.summary));
    }
    out
}

/// 把检测报告格式化为给 LLM 看的纯文本
fn format_report_for_llm(report: &ai_check::AiCheckReport) -> String {
    let real: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.feature_id != "parse_error")
        .collect();

    if real.is_empty() {
        if let Some(err_item) = report.items.iter().find(|i| i.feature_id == "parse_error") {
            return format!(
                "片段 {} 的 AI 特征检测完成，但报告解析失败：\n\n{}",
                report.fragment_id.as_deref().unwrap_or("（未知）"),
                err_item.explanation
            );
        }
        // 模型的 summary 按提示词常自带"未发现明显 AI 特征"字样，避免拼接重复
        let summary = report.summary.trim();
        if summary.contains("未发现明显 AI 特征") {
            return format!(
                "片段 {} {}",
                report.fragment_id.as_deref().unwrap_or("（未知）"),
                summary
            );
        }
        return format!(
            "片段 {} 未发现明显 AI 特征。{}",
            report.fragment_id.as_deref().unwrap_or("（未知）"),
            if summary.is_empty() {
                String::new()
            } else {
                summary.to_string()
            }
        );
    }

    let mut out = format!(
        "片段 {} 命中 {} 处 AI 写作特征：\n\n",
        report.fragment_id.as_deref().unwrap_or("（未知）"),
        real.len()
    );

    for (idx, item) in real.iter().enumerate() {
        let sev = match item.severity {
            AiCheckSeverity::Strong => "🔴强信号",
            AiCheckSeverity::Medium => "⚠️中信号",
            AiCheckSeverity::Weak => "💡弱信号",
        };
        let quote_display = if item.quote.is_empty() {
            "（无原文引用）"
        } else {
            &item.quote
        };
        out.push_str(&format!(
            "{}. [{sev} 特征#{} {}]\n   「{}」\n   → {}\n\n",
            idx + 1,
            item.feature_id,
            item.feature_name,
            quote_display,
            item.explanation
        ));
    }

    if !report.summary.is_empty() {
        out.push_str(&format!("总结：{}", report.summary));
    }
    out
}
