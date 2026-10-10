//! AI 检测工具 — 让创作 Agent 自我监督 AI 写作特征
//!
//! 提供 `ai_check_fragment` 工具，Agent 写完正文后可调用它自检，
//! 拿到 AI 味检测报告后据此反思是否需要调整。

use story_ai_runtime::{Tool, ToolDef};
use story_project_format::{ProjectPaths, StructureFile};

use super::{get_str, object_schema, ToolContext};
use crate::ai_check::{self, AiCheckSeverity};

// ============================================================================
// 跨章开场统计（确定性，不调 LLM）
// ============================================================================

/// 开场型特征（"冷叙述/硬切入开场"）：单次出现是合理文学风格，
/// 只有连续多章同型才构成模式化问题。LLM 单章检测没有全局视野、
/// 必然逐章重复报——这里用确定性跨章统计在报告层降级/升级。
fn is_opening_feature(feature_id: &str, feature_name: &str) -> bool {
    feature_id == "12" || feature_name.contains("开场") || feature_name.contains("切入")
}

/// 章开场分类：取该章阅读序第一个片段的首行非空文字，
/// 按保守前缀规则归类。规则粗，但全书口径一致即可支撑"连续性"比较。
pub(crate) fn classify_opening(body: &str) -> &'static str {
    let Some(line) = body.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return "空";
    };
    let first = line.chars().next().unwrap_or(' ');
    if matches!(first, '"' | '“' | '「' | '『' | '\'') {
        return "对话";
    }
    const TIME_STARTS: &[&str] = &[
        "清晨", "黄昏", "入夜", "深夜", "午夜", "傍晚", "黎明", "翌日", "次日", "当晚", "是夜",
        "夜半", "正午", "午后", "月夜", "雨夜", "雪夜", "这一夜", "那一夜", "转眼",
    ];
    if TIME_STARTS.iter().any(|w| line.starts_with(w)) {
        return "时间景描";
    }
    const SCENE_STARTS: &[char] = &[
        '雨', '雪', '风', '雾', '霜', '云', '山', '海', '河', '湖', '街', '巷', '檐', '窗',
        '灯', '炉', '月', '日', '天',
    ];
    if SCENE_STARTS.contains(&first) {
        return "景物";
    }
    "叙述"
}

/// 跨章开场统计：返回 (目标章的同型连续开场数, 目标章开场类型)。
///
/// 任一章无法解析开场（缺场景/片段/文件）则返回 None——统计失败时
/// 不降级不升级，保持报告原样（fail-open）。
pub(crate) fn opening_streak(dir: &std::path::Path, chapter_id: &str) -> Option<(usize, &'static str)> {
    let paths = ProjectPaths::new(dir);
    let structure =
        StructureFile::from_toml_str(&std::fs::read_to_string(paths.structure_toml()).ok()?).ok()?;

    let mut kinds: Vec<&'static str> = Vec::with_capacity(structure.book.chapters.len());
    for ch_id in &structure.book.chapters {
        let chapter = structure.chapter.get(ch_id)?;
        let scene = chapter
            .scenes
            .first()
            .and_then(|s| structure.scene.get(s))?;
        let frag = scene.fragments.first()?;
        let content =
            std::fs::read_to_string(paths.fragments_dir().join(format!("{frag}.md"))).ok()?;
        kinds.push(classify_opening(crate::fragment::fragment_body(&content)));
    }

    let idx = structure.book.chapters.iter().position(|c| c == chapter_id)?;
    let kind = kinds[idx];
    let mut streak = 1usize;
    let mut i = idx;
    while i > 0 && kinds[i - 1] == kind {
        streak += 1;
        i -= 1;
    }
    Some((streak, kind))
}

/// 片段 → 所属章节 ID（structure.toml 反查）。
fn chapter_of_fragment(dir: &std::path::Path, fragment_id: &str) -> Option<String> {
    let paths = ProjectPaths::new(dir);
    let structure =
        StructureFile::from_toml_str(&std::fs::read_to_string(paths.structure_toml()).ok()?).ok()?;
    structure
        .chapter
        .iter()
        .find(|(_, ch)| {
            ch.scenes.iter().any(|s| {
                structure
                    .scene
                    .get(s)
                    .is_some_and(|sc| sc.fragments.iter().any(|f| f == fragment_id))
            })
        })
        .map(|(id, _)| id.clone())
}

/// 场景 → 所属章节 ID。
fn chapter_of_scene(dir: &std::path::Path, scene_id: &str) -> Option<String> {
    let paths = ProjectPaths::new(dir);
    let structure =
        StructureFile::from_toml_str(&std::fs::read_to_string(paths.structure_toml()).ok()?).ok()?;
    structure
        .chapter
        .iter()
        .find(|(_, ch)| ch.scenes.iter().any(|s| s == scene_id))
        .map(|(id, _)| id.clone())
}

/// 渲染开场型特征的严重度与附注。
///
/// streak ≥3：保持原级并附"确属模式化"；不足 3：降为弱信号并说明
/// 单章冷切入属合理风格。无统计（解析失败/无此特征）返回 None 不干预。
fn opening_adjustment(
    item_feature_id: &str,
    item_feature_name: &str,
    stat: Option<(usize, &'static str)>,
) -> Option<(&'static str, String)> {
    if !is_opening_feature(item_feature_id, item_feature_name) {
        return None;
    }
    let (streak, kind) = stat?;
    if streak >= 3 {
        Some((
            "保持原级",
            format!(
                "\n   📊 跨章统计：已连续 {streak} 章以「{kind}」开场，确属模式化，优先处理"
            ),
        ))
    } else {
        Some((
            "💡弱信号",
            format!(
                "\n   📊 降级提示：跨章统计未构成连续同型开场（本章为「{kind}」起笔），\
                 单章冷切入是合理的叙事风格，非连续模式不按问题处理"
            ),
        ))
    }
}

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

        // 开场型特征需要跨章视野：按需计算统计（无此特征不读文件）
        let opening_stat = if report
            .items
            .iter()
            .any(|i| is_opening_feature(&i.feature_id, &i.feature_name))
        {
            chapter_of_fragment(&dir, &fragment_id).and_then(|c| opening_streak(&dir, &c))
        } else {
            None
        };

        Ok(format_report_for_llm(&report, opening_stat))
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

        // 开场型特征需要跨章视野（chapter 范围直接用 scope_id，scene 反查所属章）
        let opening_stat = if report
            .items
            .iter()
            .any(|i| is_opening_feature(&i.feature_id, &i.feature_name))
        {
            let chapter = if report.scope == "chapter" {
                Some(scope_id.clone())
            } else {
                chapter_of_scene(&dir, &scope_id)
            };
            chapter.and_then(|c| opening_streak(&dir, &c))
        } else {
            None
        };

        Ok(format_batch_report_for_llm(&report, opening_stat))
    }
}

/// 把批量检测报告格式化为给 LLM 看的纯文本
fn format_batch_report_for_llm(
    report: &ai_check::AiCheckBatchReport,
    opening_stat: Option<(usize, &'static str)>,
) -> String {
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
        let default_sev = match item.severity {
            AiCheckSeverity::Strong => "🔴强信号",
            AiCheckSeverity::Medium => "⚠️中信号",
            AiCheckSeverity::Weak => "💡弱信号",
        };
        // 开场型特征按跨章统计降级/升级（保持原级时附"确属模式化"注记）
        let (sev, note) = match opening_adjustment(&item.feature_id, &item.feature_name, opening_stat)
        {
            Some(("保持原级", n)) => (default_sev, n),
            Some((forced, n)) => (forced, n),
            None => (default_sev, String::new()),
        };
        // 带 ? 表示引文未能匹配回原文、归属为就近推测，以引文实际所在片段为准
        let fid = match item.fragment_id.as_deref() {
            None => "?".to_string(),
            Some(f) if item.fragment_uncertain => format!("{f}?"),
            Some(f) => f.to_string(),
        };
        let quote_display = if item.quote.is_empty() {
            "（无原文引用）"
        } else {
            &item.quote
        };
        out.push_str(&format!(
            "\n{}. [{sev} 特征#{} {}｜{}]\n   「{}」\n   → {}{note}",
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
fn format_report_for_llm(
    report: &ai_check::AiCheckReport,
    opening_stat: Option<(usize, &'static str)>,
) -> String {
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
        let default_sev = match item.severity {
            AiCheckSeverity::Strong => "🔴强信号",
            AiCheckSeverity::Medium => "⚠️中信号",
            AiCheckSeverity::Weak => "💡弱信号",
        };
        let (sev, note) = match opening_adjustment(&item.feature_id, &item.feature_name, opening_stat)
        {
            Some(("保持原级", n)) => (default_sev, n),
            Some((forced, n)) => (forced, n),
            None => (default_sev, String::new()),
        };
        let quote_display = if item.quote.is_empty() {
            "（无原文引用）"
        } else {
            &item.quote
        };
        out.push_str(&format!(
            "{}. [{sev} 特征#{} {}]\n   「{}」\n   → {}{note}\n\n",
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

// ============================================================================
// ai_check_pov — POV 视角越权软检测
// ============================================================================

pub struct AiCheckPovTool {
    ctx: ToolContext,
}
impl AiCheckPovTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AiCheckPovTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "ai_check_pov".into(),
            description: "检测限知视角（POV）场景的视角越权硬伤：穿墙窥视他人内心、\
            叙述直陈 POV 尚不知道的线索事实、越权感知不在场信息。\
            以场景 POV 与认知矩阵（各线索在该时点的 unknown/suspected/known）\
            为基准做 LLM 软检测——这是语义判断，编译器规则无法确定性拦截。\
            场景须已设置 pov。交付关键 POV 章节前过一遍。"
                .into(),
            parameters: object_schema(vec![(
                "scene_id",
                "string",
                "场景 ID（如 sc_01_01，须已设置 POV）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        use story_ai_runtime::provider::{GenerateRequest, Message};

        let scene_id = get_str(arguments, "scene_id")?;
        let provider = self.ctx.provider.as_ref().ok_or_else(|| {
            "ai_check_pov 需要 provider，但当前未配置。请确保通过项目模式运行。".to_string()
        })?;
        let paths = self.ctx.paths();
        let structure = story_project_format::StructureFile::from_toml_str(
            &std::fs::read_to_string(paths.structure_toml())
                .map_err(|e| format!("读取 structure.toml 失败: {e}"))?,
        )
        .map_err(|e| format!("解析 structure.toml 失败: {e}"))?;
        let scene = structure
            .scene
            .get(&scene_id)
            .ok_or_else(|| format!("场景 '{scene_id}' 不存在"))?;
        let pov = scene
            .pov
            .clone()
            .ok_or_else(|| format!("场景 {scene_id} 未设置 POV，先 update_scene(pov_character_id=...) 再检测"))?;
        let pov_name = std::fs::read_to_string(
            paths
                .file_for_id(&pov)
                .unwrap_or_else(|| paths.characters_dir().join(format!("{pov}.toml"))),
        )
        .ok()
        .and_then(|c| story_project_format::EntityFile::from_toml_str(&c).ok())
        .map(|e| e.display_name)
        .unwrap_or_else(|| pov.clone());

        // 场景正文（按 fragments 顺序拼接）
        let mut text = String::new();
        for fid in &scene.fragments {
            if let Ok(c) = std::fs::read_to_string(paths.fragments_dir().join(format!("{fid}.md"))) {
                text.push_str(crate::fragment::fragment_body(&c));
                text.push('\n');
            }
        }
        if text.trim().is_empty() {
            return Err(format!("场景 {scene_id} 无正文，无从检测"));
        }

        // POV 认知清单：各线索在该场景时点的状态（阅读序 ≤ 本场景的最后一条）
        let reading_order: Vec<String> = structure
            .book
            .chapters
            .iter()
            .filter_map(|c| structure.chapter.get(c))
            .flat_map(|c| c.scenes.clone())
            .collect();
        let at = reading_order
            .iter()
            .position(|s| s == &scene_id)
            .unwrap_or(usize::MAX);
        let mut cognition = String::new();
        if let Ok(tc) = std::fs::read_to_string(paths.threads_toml()) {
            if let Ok(file) = story_project_format::ThreadsFile::from_toml_str(&tc) {
                for t in &file.threads {
                    let state = t
                        .knowledge
                        .iter()
                        .find(|k| {
                            matches!(
                                &k.observer,
                                story_project_format::threads_file::ObserverDef::Character { character }
                                    if character == &pov
                            )
                        })
                        .and_then(|k| {
                            let mut best: Option<(&str, usize)> = None;
                            for e in &k.state_timeline {
                                if let Some(pos) = reading_order.iter().position(|s| s == &e.scene) {
                                    if pos <= at && best.is_none_or(|(_, bp)| pos >= bp) {
                                        best = Some((e.state.as_str(), pos));
                                    }
                                }
                            }
                            best.map(|(s, _)| s.to_string())
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    cognition.push_str(&format!("- 线索 {}「{}」：{}\n", t.id, t.title, state));
                }
            }
        }

        let system = crate::prompt_custom::resolve(
            &crate::prompt_custom::POV_CHECK,
            Some(self.ctx.project_dir.as_path()),
        );
        let user = format!(
            "POV 角色：{pov_name}（{pov}）\n场景：{scene_id}\n\nPOV 在本场景的认知状态：\n{cognition}\n场景正文：\n\n{}",
            {
                // 超长截断保护（与 ai_check 的送审上限同量级）
                const MAX_CHARS: usize = 6000;
                let mut t = text.clone();
                if t.chars().count() > MAX_CHARS {
                    t = t.chars().take(MAX_CHARS).collect::<String>();
                    t.push_str("\n…（超长截断）");
                }
                t
            }
        );
        let request = GenerateRequest {
            system,
            messages: vec![Message::user(user)],
            model: String::new(),
            temperature: 0.2,
            max_tokens: 2048,
            tools: Vec::new(),
        };
        let resp = super::retry_generate(provider, &request)
            .map_err(|e| format!("视角检测调用失败: {e}"))?;
        let raw = crate::llm_json::strip_think(&resp.text);
        // 容错解析：剥围栏后取首个 JSON 对象
        let json_text = raw
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let parsed: serde_json::Value = serde_json::from_str(json_text)
            .map_err(|e| format!("视角检测报告解析失败: {e}\n原始输出开头: {}", raw.chars().take(200).collect::<String>()))?;

        let leaks = parsed
            .get("leaks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let summary = parsed
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if leaks.is_empty() {
            return Ok(format!(
                "场景 {scene_id}（POV {pov_name}）未发现视角越权。{summary}"
            ));
        }
        let mut out = format!(
            "场景 {scene_id}（POV {pov_name}）发现 {} 处疑似视角越权：\n",
            leaks.len()
        );
        for (i, leak) in leaks.iter().enumerate() {
            out.push_str(&format!(
                "{}. [{}]\n   「{}」\n   → {}\n",
                i + 1,
                leak.get("kind").and_then(|v| v.as_str()).unwrap_or("越权"),
                leak.get("quote").and_then(|v| v.as_str()).unwrap_or("（无引文）"),
                leak.get("explanation").and_then(|v| v.as_str()).unwrap_or("")
            ));
        }
        if !summary.is_empty() {
            out.push_str(&format!("总结：{summary}"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_opening_by_prefix_rules() {
        assert_eq!(classify_opening("\n\n「你来了。」他头也不抬。"), "对话");
        assert_eq!(classify_opening("“等等！”她喊住他。"), "对话");
        assert_eq!(classify_opening("入夜后山门落锁。"), "时间景描");
        assert_eq!(classify_opening("翌日清晨，雨停了。"), "时间景描");
        assert_eq!(classify_opening("雨水沿瓦檐滴落。"), "景物");
        assert_eq!(classify_opening("石阶一共十七级，苏格昨天数过。"), "叙述");
        assert_eq!(classify_opening(""), "空");
    }

    /// 三章连续对话开场 → streak=3；前一章不同型 → streak=1。
    #[test]
    fn opening_streak_counts_consecutive_same_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("fragments")).unwrap();

        let mut structure = String::from("[book]\nid = \"b_root\"\ntitle = \"t\"\nchapters = [\"ch_01\", \"ch_02\", \"ch_03\"]\n");
        let openings = ["他推门进来。", "「站住。」", "「又见面了。」"]; // ch_01 叙述，ch_02/03 对话
        for (i, ch) in ["ch_01", "ch_02", "ch_03"].iter().enumerate() {
            let fid = format!("f_00{}", i + 1);
            structure.push_str(&format!(
                "\n[chapter.{ch}]\ntitle = \"第{}章\"\nscenes = [\"sc_{ch}_01\"]\n",
                i + 1
            ));
            structure.push_str(&format!(
                "\n[scene.sc_{ch}_01]\ntitle = \"s\"\nfragments = [\"{fid}\"]\n"
            ));
            std::fs::write(
                dir.join(format!("fragments/{fid}.md")),
                format!("---\nid: {fid}\n---\n{}\n正文后续。", openings[i]),
            )
            .unwrap();
        }
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        // ch_03：ch_02/ch_03 连续对话 → streak=2（< 3，应降级）
        assert_eq!(opening_streak(dir, "ch_03"), Some((2, "对话")));
        // ch_01：前无章节，叙述 → streak=1
        assert_eq!(opening_streak(dir, "ch_01"), Some((1, "叙述")));

        // 补一章对话开场，ch_03 变成连续 3 章 → 不降级
        std::fs::write(
            dir.join("structure.toml"),
            std::fs::read_to_string(dir.join("structure.toml"))
                .unwrap()
                .replace("ch_03\"]\n", "ch_03\", \"ch_04\"]\n")
                + "\n[chapter.ch_04]\ntitle = \"第四章\"\nscenes = [\"sc_ch_04_01\"]\n\n[scene.sc_ch_04_01]\ntitle = \"s\"\nfragments = [\"f_0004\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("fragments/f_0004.md"),
            "---\nid: f_0004\n---\n「别走。」\n正文后续。",
        )
        .unwrap();
        assert_eq!(opening_streak(dir, "ch_04"), Some((3, "对话")));

        // 未知章节 → None（fail-open）
        assert_eq!(opening_streak(dir, "ch_99"), None);
    }

    /// 降级/保持逻辑：streak<3 强制弱信号；≥3 保持并注明模式化。
    #[test]
    fn opening_adjustment_downgrades_and_escalates() {
        let (sev, note) =
            opening_adjustment("12", "冷叙述/硬切入开场", Some((1, "叙述"))).unwrap();
        assert_eq!(sev, "💡弱信号");
        assert!(note.contains("合理的叙事风格"), "{note}");

        let (sev, note) =
            opening_adjustment("12", "冷叙述/硬切入开场", Some((3, "对话"))).unwrap();
        assert_eq!(sev, "保持原级");
        assert!(note.contains("模式化"), "{note}");

        // 非开场特征不干预
        assert!(opening_adjustment("3", "匀速排比", Some((3, "对话"))).is_none());
        // 无统计不干预
        assert!(opening_adjustment("12", "冷叙述/硬切入开场", None).is_none());
    }
}
