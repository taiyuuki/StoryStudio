//! Query Tools — 查询与回顾类工具
//!
//! 集中实现"读取型"聚合查询工具，补齐知识库可查询化与正文可检索化能力：
//! - `get_story_recap`：全书/章节回顾（聚合 scene.summary + 按需 LLM 摘要）
//! - `get_relations`：关系图查询（按实体/类型过滤 SemanticGraph 边）
//! - `query_assertions`：断言查询（按 subject/predicate/status 过滤）
//! - `find_mentions`：实体出场查询（复用 LocationIndex，含显式 anchors + 隐式 mentions）
//! - `search_text`：关键词全文检索（遍历 fragment 正文）
//!
//! 所有工具遵循 studio-lib/tools 的统一约定：
//! - 持有 `ToolContext`，通过 `ctx.paths()` 定位项目文件
//! - 读取型查询优先复用 `story_parser::parse_project` 的 IR + LocationIndex
//! - 返回纯文本多行报告（非 JSON），失败返回中文短语错误信息

use std::sync::Arc;

use story_ai_runtime::provider::{GenerateRequest, Message};
use story_ai_runtime::{AiProvider, Tool, ToolDef};
use story_exporter::collect_manuscript;
use story_ir::StableId;
use story_parser::parse_project;
use story_project_format::knowledge_file::AssertionDef;
use story_project_format::{AssertionsFile, ChaptersFile, RelationDef, RelationsFile};

use super::{format_brief, get_str, get_str_opt, object_schema, ToolContext};

// ============================================================================
// get_story_recap
// ============================================================================

pub struct GetStoryRecapTool {
    ctx: ToolContext,
}
impl GetStoryRecapTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

/// 回顾的详细度
enum Detail {
    /// 每场景一行（标题 + 已有摘要首句或 fragment 截断）
    Brief,
    /// 每场景 2-3 句（默认）
    Normal,
    /// 每场景含关键出场实体与事件
    Detailed,
}

impl Detail {
    fn parse(s: Option<&str>) -> Detail {
        match s {
            Some("brief") => Detail::Brief,
            Some("detailed") => Detail::Detailed,
            _ => Detail::Normal,
        }
    }
}

/// 单次按需生成摘要的 fragment 正文 token 上限（防超长）
const SUMMARIZE_TEXT_MAX_CHARS: usize = 4000;
/// 超过此场景数时不再逐个生成摘要，仅返回结构骨架
const MAX_SCENES_FOR_LLM_SUMMARY: usize = 15;

impl Tool for GetStoryRecapTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_story_recap".into(),
            description: "回顾已有正文：按章节→场景顺序输出故事进展。\
            优先使用场景已保存的 summary；缺失时若 provider 可用则现场生成摘要。\
            用于动笔新章节前快速了解前情，避免逐个读 fragment 浪费 token。"
                .into(),
            parameters: object_schema(vec![
                (
                    "scope",
                    "string",
                    "范围：省略或 'all' 为全书回顾；'chapter:<id>' 限定单章（如 chapter:ch_01）",
                    false,
                ),
                (
                    "detail",
                    "string",
                    "详细度：brief（每场景一行）/ normal（默认，2-3句）/ detailed（含出场实体）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let scope = get_str_opt(arguments, "scope").unwrap_or_else(|| "all".to_string());
        let detail = Detail::parse(get_str_opt(arguments, "detail").as_deref());

        let parse_output = parse_project(&self.ctx.project_dir).map_err(|e| format!("{e:?}"))?;
        let ir = &parse_output.ir;

        // 解析 scope：all 或 chapter:<id>
        let target_chapter: Option<StableId> = if scope == "all" {
            None
        } else if let Some(ch_id) = scope.strip_prefix("chapter:") {
            let sid = StableId::new_unchecked(ch_id);
            if ir.document.get_chapter(&sid).is_none() {
                return Err(format!("章节 '{ch_id}' 不存在"));
            }
            Some(sid)
        } else {
            return Err(format!(
                "无效的 scope '{scope}'（支持 'all' 或 'chapter:<id>'）"
            ));
        };

        // 收集要回顾的章节
        let book_id = ir
            .document
            .all_nodes()
            .find_map(|(id, n)| matches!(n, story_ir::DocNode::Book(_)).then_some(id))
            .ok_or_else(|| "文档结构缺失 Book 根节点".to_string())?;
        let chapter_ids: Vec<StableId> = ir
            .document
            .children_of(book_id)
            .iter()
            .filter(|c| target_chapter.as_ref().is_none_or(|t| *t == **c))
            .cloned()
            .collect();

        if chapter_ids.is_empty() {
            return Ok("尚无章节内容，无法回顾。".to_string());
        }

        // 场景过多（长篇全书回顾）：降级为章节粒度，防止几千行输出撑爆上下文
        const RECAP_SCENE_COLLAPSE_THRESHOLD: usize = 200;
        let total_scenes: usize = chapter_ids
            .iter()
            .map(|ch| ir.document.scenes_of_chapter(ch).len())
            .sum();
        let degraded = total_scenes > RECAP_SCENE_COLLAPSE_THRESHOLD;

        // 第一遍：统计需要生成摘要的场景数（cost 控制），并记录明细防误读
        let mut pending_scenes: Vec<String> = Vec::new();
        for ch_id in &chapter_ids {
            for sc_id in ir.document.scenes_of_chapter(ch_id) {
                let scene = match ir.document.get_scene(sc_id) {
                    Some(s) => s,
                    None => continue,
                };
                if scene.summary.as_ref().is_none_or(|s| s.trim().is_empty()) {
                    let title = scene.title.as_deref().unwrap_or("无标题");
                    pending_scenes.push(format!("{sc_id} {title}"));
                }
            }
        }
        let scenes_needing_summary = pending_scenes.len();

        let llm_available = self.ctx.provider.is_some();
        let will_generate = scenes_needing_summary > 0 && llm_available;
        // 若需生成摘要的场景过多，拒绝逐个生成（避免高成本），降级为结构骨架
        let suppress_generation =
            will_generate && scenes_needing_summary > MAX_SCENES_FOR_LLM_SUMMARY;

        let mut out = String::new();
        out.push_str("故事回顾:\n");
        if target_chapter.is_some() {
            out.push_str(&format!("  范围: {scope}\n"));
        }
        out.push_str(&format!("  章节数: {}\n", chapter_ids.len()));
        if scenes_needing_summary > 0 {
            let shown: Vec<&str> = pending_scenes
                .iter()
                .take(6)
                .map(|s| s.as_str())
                .collect();
            let more = if pending_scenes.len() > 6 {
                format!(" 等 {}", pending_scenes.len())
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  待摘要场景: {}（{}{more}）",
                scenes_needing_summary,
                shown.join("、")
            ));
            if !llm_available {
                out.push_str("（无 provider，使用正文截断代替）");
            } else if suppress_generation {
                out.push_str(&format!(
                    "（数量超过 {MAX_SCENES_FOR_LLM_SUMMARY}，跳过自动生成，建议按章节分批回顾）"
                ));
            }
            out.push('\n');
        }
        if degraded {
            out.push_str(&format!(
                "  （全书 {} 个场景，超过 {RECAP_SCENE_COLLAPSE_THRESHOLD}，已折叠为章节粒度；\
                 需要场景细节请用 scope=chapter:<id> 分章回顾）\n",
                total_scenes,
            ));
        }
        out.push('\n');

        let provider = self.ctx.provider.as_ref();

        for ch_id in &chapter_ids {
            let chapter = match ir.document.get_chapter(ch_id) {
                Some(c) => c,
                None => continue,
            };
            out.push_str(&format!("## {} {}\n", ch_id, chapter.title));

            let scene_ids = ir.document.scenes_of_chapter(ch_id);
            if scene_ids.is_empty() {
                out.push_str("  （本章无场景）\n\n");
                continue;
            }

            // 降级模式：本章一行（场景标题 + 已有摘要首句，整章截断），
            // 不再做 POV / 实体明细 / 现场生成摘要
            if degraded {
                let parts: Vec<String> = scene_ids
                    .iter()
                    .filter_map(|sc_id| {
                        let scene = ir.document.get_scene(sc_id)?;
                        let title = scene.title.as_deref().unwrap_or("无标题");
                        Some(match scene.summary.as_ref().filter(|s| !s.trim().is_empty()) {
                            Some(s) => format!("{title}：{}", format_brief(s.trim(), 40)),
                            None => title.to_string(),
                        })
                    })
                    .collect();
                out.push_str(&format!("  {}\n\n", format_brief(&parts.join("；"), 300)));
                continue;
            }

            for sc_id in scene_ids {
                let scene = match ir.document.get_scene(sc_id) {
                    Some(s) => s,
                    None => continue,
                };
                let title = scene.title.as_deref().unwrap_or("无标题");
                // 时间线索（有则附注，供跨场景时序判断）
                let mut time_note = String::new();
                if let Some(cal) = &scene.story_time {
                    time_note = cal.year.to_string();
                }
                if let Some(anchor) = &scene.story_time_anchor {
                    if !time_note.is_empty() {
                        time_note.push('，');
                    }
                    time_note.push_str(anchor);
                }
                if let Some(gap) = &scene.story_time_gap {
                    if !time_note.is_empty() {
                        time_note.push('，');
                    }
                    time_note.push_str(&format!("距前 {gap}"));
                }
                if time_note.is_empty() {
                    out.push_str(&format!("- 场景 {sc_id}: {title}\n"));
                } else {
                    out.push_str(&format!("- 场景 {sc_id}: {title}（{time_note}）\n"));
                }

                // POV（detailed 时显示）
                if matches!(detail, Detail::Detailed) {
                    if let Some(pov) = &scene.pov {
                        let pov_name = ir
                            .semantic
                            .get(pov)
                            .map(|n| n.display_name().to_string())
                            .unwrap_or_else(|| pov.as_str().to_string());
                        out.push_str(&format!("  POV: {pov_name}\n"));
                    }
                }

                // 摘要：优先用已存的 summary
                let summary = if let Some(s) =
                    scene.summary.as_ref().filter(|s| !s.trim().is_empty())
                {
                    Some(s.clone())
                } else {
                    // 缺失 → 现场生成或降级
                    let fragments = ir.document.fragments_of_scene(sc_id);
                    self.summarize_scene(provider, fragments, &ir.document, suppress_generation)?
                };

                if let Some(s) = summary {
                    let trimmed = match detail {
                        Detail::Brief => format_brief(s.trim(), 80),
                        Detail::Normal => format_brief(s.trim(), 240),
                        Detail::Detailed => format_brief(s.trim(), 400),
                    };
                    out.push_str(&format!("  摘要: {trimmed}\n"));
                } else {
                    out.push_str("  摘要: （无正文，无法生成）\n");
                }

                // detailed：列出该场景出场的实体
                if matches!(detail, Detail::Detailed) {
                    let mut entities: Vec<String> = Vec::new();
                    for fid in ir.document.fragments_of_scene(sc_id) {
                        if let Some(frag) = ir.document.get_fragment(fid) {
                            for anchor in &frag.anchors {
                                let name = ir
                                    .semantic
                                    .get(&anchor.entity)
                                    .map(|n| n.display_name().to_string())
                                    .unwrap_or_else(|| anchor.entity.as_str().to_string());
                                let entry = format!("{name}({})", anchor.entity);
                                if !entities.contains(&entry) {
                                    entities.push(entry);
                                }
                            }
                        }
                    }
                    if !entities.is_empty() {
                        out.push_str(&format!("  出场: {}\n", entities.join(", ")));
                    }
                }
            }
            out.push('\n');
        }

        // 若现场生成了摘要，提示 AI 可以选择落盘
        if will_generate && !suppress_generation {
            out.push_str(
                "提示: 部分场景摘要是现场生成的（未保存）。可用 update_scene(scene_id, summary=...) 落盘以便下次复用。\n",
            );
        }

        Ok(out)
    }
}

impl GetStoryRecapTool {
    /// 对一个场景的 fragment 集合生成摘要。
    /// - `suppress=true`：不调 LLM，直接取正文截断
    /// - provider=None：取正文截断
    /// - 否则：调 LLM 生成
    fn summarize_scene(
        &self,
        provider: Option<&Arc<dyn AiProvider>>,
        fragment_ids: &[StableId],
        document: &story_ir::DocumentGraph,
        suppress: bool,
    ) -> Result<Option<String>, String> {
        // 拼接正文
        let mut text = String::new();
        for fid in fragment_ids {
            if let Some(frag) = document.get_fragment(fid) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&frag.text);
            }
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            return Ok(None);
        }

        // 无 provider 或被抑制 → 正文截断
        let provider = match provider {
            Some(p) if !suppress => p,
            _ => return Ok(Some(format_brief(&text, 120))),
        };

        // 调 LLM 生成摘要
        let truncated = format_brief(&text, SUMMARIZE_TEXT_MAX_CHARS);
        let system = crate::prompts::SCENE_SUMMARY;
        let user = format!("场景正文：\n\n{truncated}");
        let request = GenerateRequest {
            system: system.to_string(),
            messages: vec![Message::user(user)],
            model: String::new(),
            temperature: 0.2,
            max_tokens: 256,
            tools: Vec::new(),
        };

        match super::retry_generate(provider, &request) {
            Ok(resp) => {
                let summary = resp.text.trim().to_string();
                if summary.is_empty() {
                    Ok(Some(format_brief(&text, 120)))
                } else {
                    Ok(Some(summary))
                }
            }
            Err(e) => {
                // LLM 失败不致命，降级为正文截断
                eprintln!("[get_story_recap] 摘要生成失败，降级为正文截断: {e}");
                Ok(Some(format_brief(&text, 120)))
            }
        }
    }
}

// ============================================================================
// get_relations
// ============================================================================

pub struct GetRelationsTool {
    ctx: ToolContext,
}
impl GetRelationsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetRelationsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_relations".into(),
            description: "查询角色/实体之间的关系。可按实体 ID 或关系类型过滤，\
            省略参数则列出全部关系。用于核对人物关系网、检查关系一致性。"
                .into(),
            parameters: object_schema(vec![
                (
                    "entity_id",
                    "string",
                    "实体 ID（如 c_0001）。提供时返回该实体参与的所有关系（from 或 to）",
                    false,
                ),
                (
                    "relation_type",
                    "string",
                    "关系类型过滤（如 mentor_of / enemy_of / family_of / knows）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_filter = get_str_opt(arguments, "entity_id");
        let type_filter = get_str_opt(arguments, "relation_type");

        let paths = self.ctx.paths();
        let relations_path = paths.relations_toml();
        // 文件不存在视为空（未记录任何关系）
        let file = match std::fs::read_to_string(&relations_path) {
            Ok(content) => RelationsFile::from_toml_str(&content)
                .map_err(|e| format!("解析 relations.toml 失败: {e}"))?,
            Err(_) => RelationsFile::default(),
        };

        // 可选：解析 IR 以便把实体 ID 翻译成名称
        let parse_output = parse_project(&self.ctx.project_dir).ok();
        let display_name = |id: &str| -> String {
            parse_output
                .as_ref()
                .and_then(|po| po.ir.semantic.get(&StableId::new_unchecked(id)))
                .map(|n| n.display_name().to_string())
                .unwrap_or_else(|| id.to_string())
        };

        let mut filtered: Vec<&RelationDef> = file
            .relations
            .iter()
            .filter(|r| {
                let entity_match = entity_filter
                    .as_ref()
                    .is_none_or(|e| e == &r.from || e == &r.to);
                let type_match = type_filter.as_ref().is_none_or(|t| t == &r.relation);
                entity_match && type_match
            })
            .collect();

        // 按 id 排序保证输出稳定
        filtered.sort_by_key(|r| r.id.as_str());

        let mut out = String::new();
        out.push_str(&format!("关系列表({}):\n", filtered.len()));
        if filtered.is_empty() {
            if entity_filter.is_some() || type_filter.is_some() {
                out.push_str("  （无匹配关系）");
            } else {
                out.push_str("  （尚未记录任何关系，可用 add_relation 添加）");
            }
            return Ok(out);
        }

        for r in filtered {
            let from_name = display_name(&r.from);
            let to_name = display_name(&r.to);
            let evidence = if r.evidence.is_empty() {
                String::new()
            } else {
                format!(" (证据: {})", r.evidence.join(", "))
            };
            out.push_str(&format!(
                "  {}: {}({}) --{}--> {}({}){}\n",
                r.id, from_name, r.from, r.relation, to_name, r.to, evidence
            ));
        }
        Ok(out)
    }
}

// ============================================================================
// query_assertions
// ============================================================================

pub struct QueryAssertionsTool {
    ctx: ToolContext,
}
impl QueryAssertionsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for QueryAssertionsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "query_assertions".into(),
            description: "查询知识库断言。可按主体实体、谓词、状态过滤，\
            默认只返回 active 状态。用于核对角色当前状态（年龄/存活/持有物/位置等）、\
            检查事实一致性。predicate 如 alive/age/holds/located_at/member_of/identity。"
                .into(),
            parameters: object_schema(vec![
                (
                    "subject",
                    "string",
                    "断言主体（实体 ID，如 c_0001）。省略则不限主体",
                    false,
                ),
                (
                    "predicate",
                    "string",
                    "谓词过滤（如 alive / age / holds / located_at）。省略则不限谓词",
                    false,
                ),
                (
                    "status",
                    "string",
                    "状态过滤：active（默认）/ superseded / contradicted / all",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let subject_filter = get_str_opt(arguments, "subject");
        let predicate_filter = get_str_opt(arguments, "predicate");
        let status_filter =
            get_str_opt(arguments, "status").unwrap_or_else(|| "active".to_string());

        let paths = self.ctx.paths();
        let assertions_path = paths.assertions_toml();
        // 文件不存在视为空
        let file = match std::fs::read_to_string(&assertions_path) {
            Ok(content) => AssertionsFile::from_toml_str(&content)
                .map_err(|e| format!("解析 assertions.toml 失败: {e}"))?,
            Err(_) => AssertionsFile::default(),
        };

        // 可选：解析 IR 以把主体 ID 翻译成名称
        let parse_output = parse_project(&self.ctx.project_dir).ok();
        let display_name = |id: &str| -> String {
            parse_output
                .as_ref()
                .and_then(|po| po.ir.semantic.get(&StableId::new_unchecked(id)))
                .map(|n| n.display_name().to_string())
                .unwrap_or_else(|| id.to_string())
        };

        let filtered: Vec<&AssertionDef> = file
            .assertions
            .iter()
            .filter(|a| {
                let subj_match = subject_filter.as_ref().is_none_or(|s| s == &a.subject);
                let pred_match = predicate_filter.as_ref().is_none_or(|p| p == &a.predicate);
                let status_match = status_filter == "all"
                    || status_filter == "active" && (a.status.is_empty() || a.status == "active")
                    || status_filter == a.status;
                subj_match && pred_match && status_match
            })
            .collect();

        let mut out = String::new();
        out.push_str(&format!("断言列表({}):\n", filtered.len()));
        if filtered.is_empty() {
            if subject_filter.is_some() || predicate_filter.is_some() {
                out.push_str("  （无匹配断言）");
            } else {
                out.push_str("  （知识库为空，可用 add_assertion 添加）");
            }
            return Ok(out);
        }

        for a in filtered {
            let subj_name = display_name(&a.subject);
            let status_display = if a.status.is_empty() {
                "active"
            } else {
                a.status.as_str()
            };
            let evidence = if a.evidence.is_empty() {
                String::new()
            } else {
                format!(", 证据: {}", a.evidence.join(", "))
            };
            out.push_str(&format!(
                "  {}: {}({}).{} = {} ({}){}\n",
                a.id, subj_name, a.subject, a.predicate, a.object, status_display, evidence
            ));
        }
        Ok(out)
    }
}

// ============================================================================
// find_mentions
// ============================================================================

pub struct FindMentionsTool {
    ctx: ToolContext,
}
impl FindMentionsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for FindMentionsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "find_mentions".into(),
            description: "查找指定实体在正文片段中的出场记录（含显式锚点与隐式文本匹配）。\
            返回出场列表 + 角色类型（appears/mentioned/referenced）+ 所属场景 + 正文摘要，\
            最多列出 50 条；可用 scope（chapter:<id> / scene:<id>）限定章节或场景。\
            用于检查角色/道具的历史出场、核对一致性、避免重复描述。"
                .into(),
            parameters: object_schema(vec![
                ("entity_id", "string", "实体 ID（如 c_0001）", true),
                (
                    "scope",
                    "string",
                    "范围：省略为全书；'chapter:<id>' 限定章节；'scene:<id>' 限定场景",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;
        let scope = get_str_opt(arguments, "scope");
        let parse_output = parse_project(&self.ctx.project_dir).map_err(|e| format!("{e:?}"))?;
        let sid = StableId::new_unchecked(&entity_id);
        let document = &parse_output.ir.document;

        // 解析 scope（与 search_text 同语法）
        let scope_chapter: Option<StableId> = scope
            .as_deref()
            .and_then(|s| s.strip_prefix("chapter:"))
            .map(StableId::new_unchecked);
        let scope_scene: Option<StableId> = scope
            .as_deref()
            .and_then(|s| s.strip_prefix("scene:"))
            .map(StableId::new_unchecked);
        if let Some(ch) = &scope_chapter {
            if document.get_chapter(ch).is_none() {
                return Err(format!("章节 '{}' 不存在", ch));
            }
        }
        if let Some(sc) = &scope_scene {
            if document.get_scene(sc).is_none() {
                return Err(format!("场景 '{}' 不存在", sc));
            }
        }
        let in_scope = |fid: &StableId| -> bool {
            let owner = document.scene_of_fragment(fid);
            if let Some(sc) = &scope_scene {
                return owner.is_some_and(|s| s == sc);
            }
            if let Some(ch) = &scope_chapter {
                return owner
                    .and_then(|s| document.chapter_of_scene(s))
                    .is_some_and(|c| c == ch);
            }
            true
        };

        // 实体存在性校验
        if parse_output.ir.semantic.get(&sid).is_none() {
            // 退一步：检查磁盘文件是否存在（可能尚未编译进 IR）
            let paths = self.ctx.paths();
            if paths.file_for_id(&entity_id).is_none_or(|p| !p.exists()) {
                return Err(format!("实体 '{entity_id}' 不存在"));
            }
        }

        // 出场记录：优先用 LocationIndex（含显式 anchors + 隐式 mentions）；
        // 若为空则回退扫描 fragment 的显式 anchors（与 get_entity_detail 一致）。
        let mut appearances: Vec<(StableId, String)> = parse_output
            .location_index
            .appearances_of(&sid)
            .iter()
            .map(|a| (a.fragment.clone(), format!("{}", a.role)))
            .collect();

        if appearances.is_empty() {
            // 回退：直接扫描 fragment 的显式 anchors
            for (fid, fnode) in document.all_fragments() {
                if fnode.anchors.iter().any(|a| a.entity == sid) {
                    let role = fnode
                        .anchors
                        .iter()
                        .find(|a| a.entity == sid)
                        .map(|a| format!("{}", a.role))
                        .unwrap_or_else(|| "appears".to_string());
                    appearances.push((fid.clone(), role));
                }
            }
        }

        // scope 过滤
        appearances.retain(|(fid, _)| in_scope(fid));

        if appearances.is_empty() {
            return Ok(format!("实体 {entity_id} 无出场记录。"));
        }

        let entity_name = parse_output
            .ir
            .semantic
            .get(&sid)
            .map(|n| n.display_name().to_string())
            .unwrap_or_else(|| entity_id.clone());

        // 按片段去重：同一片段内多次出现合并为一条（×N 计数）
        let mut grouped: Vec<(StableId, String, usize)> = Vec::new();
        for (fid, role) in appearances {
            if let Some(entry) = grouped.iter_mut().find(|(f, _, _)| *f == fid) {
                entry.2 += 1;
            } else {
                grouped.push((fid, role, 1));
            }
        }
        let total_mentions: usize = grouped.iter().map(|(_, _, n)| n).sum();

        let mut out = String::new();
        out.push_str(&format!(
            "实体 {}({}) 出场记录({} 个片段，共 {} 次):\n",
            entity_name,
            entity_id,
            grouped.len(),
            total_mentions
        ));

        /// 单次列出的出场条数上限（高频实体全书几百条会撑爆上下文）
        const MENTIONS_MAX_RESULTS: usize = 50;
        let shown = grouped.len().min(MENTIONS_MAX_RESULTS);

        for (frag_id, role, count) in grouped.iter().take(shown) {
            let scene_title = document
                .scene_of_fragment(frag_id)
                .and_then(|sid| document.get_scene(sid))
                .and_then(|s| s.title.clone())
                .unwrap_or_else(|| "无标题".to_string());
            let text = document
                .get_fragment(frag_id)
                .map(|f| format_brief(&f.text, 80))
                .unwrap_or_else(|| "（正文缺失）".to_string());
            let times = if *count > 1 { format!(" ×{}", count) } else { String::new() };
            out.push_str(&format!(
                "  {}[{}] [{}] 场景「{}」: \"{}\"\n",
                frag_id, times, role, scene_title, text
            ));
        }
        if grouped.len() > shown {
            out.push_str(&format!(
                "  ……共 {} 个片段，已列出前 {shown} 条。请用 scope（chapter:<id> / scene:<id>）缩小范围查看其余出场。\n",
                grouped.len()
            ));
        }
        Ok(out)
    }
}

// ============================================================================
// search_text
// ============================================================================

pub struct SearchTextTool {
    ctx: ToolContext,
}
impl SearchTextTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

/// 搜索结果上限（超过则截断并提示）
const SEARCH_MAX_RESULTS: usize = 30;
/// 单条命中上下文窗口（前后各取的字符数）
const SEARCH_CONTEXT_CHARS: usize = 40;

impl Tool for SearchTextTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_text".into(),
            description: "在正文片段中全文检索关键词，返回匹配片段及上下文。\
            可限定范围（chapter:<id> 或 scene:<id>），默认全书。\
            用于查找物品/地点/角色的具体引用、核对前后描述一致性。"
                .into(),
            parameters: object_schema(vec![
                ("query", "string", "搜索关键词", true),
                (
                    "scope",
                    "string",
                    "范围：省略为全书；'chapter:<id>' 限定章节；'scene:<id>' 限定场景",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let query = get_str(arguments, "query")?;
        let scope = get_str_opt(arguments, "scope");
        let parse_output = parse_project(&self.ctx.project_dir).map_err(|e| format!("{e:?}"))?;
        let document = &parse_output.ir.document;

        // 解析 scope
        let scope_chapter: Option<StableId> = scope
            .as_deref()
            .and_then(|s| s.strip_prefix("chapter:"))
            .map(StableId::new_unchecked);
        let scope_scene: Option<StableId> = scope
            .as_deref()
            .and_then(|s| s.strip_prefix("scene:"))
            .map(StableId::new_unchecked);
        if let Some(ch) = &scope_chapter {
            if document.get_chapter(ch).is_none() {
                return Err(format!("章节 '{}' 不存在", ch));
            }
        }
        if let Some(sc) = &scope_scene {
            if document.get_scene(sc).is_none() {
                return Err(format!("场景 '{}' 不存在", sc));
            }
        }

        let query_lower = query.to_lowercase();
        // 记录 (fragment_id, scene_title, excerpt, count_in_fragment)
        let mut hits: Vec<(StableId, String, String, usize)> = Vec::new();
        let mut total_occurrences: usize = 0;

        for (fid, frag) in document.all_fragments() {
            // scope 过滤
            if let Some(sc) = &scope_scene {
                let belongs = document.scene_of_fragment(fid).is_some_and(|s| s == sc);
                if !belongs {
                    continue;
                }
            } else if let Some(ch) = &scope_chapter {
                let belongs = document
                    .scene_of_fragment(fid)
                    .and_then(|s| document.chapter_of_scene(s))
                    .is_some_and(|c| c == ch);
                if !belongs {
                    continue;
                }
            }

            let text_lower = frag.text.to_lowercase();
            // 遍历该 fragment 内的全部匹配，统计出现次数
            let mut indices: Vec<usize> = text_lower
                .match_indices(&query_lower)
                .map(|(i, _)| i)
                .collect();
            if indices.is_empty() {
                continue;
            }
            let count_in_fragment = indices.len();
            total_occurrences += count_in_fragment;

            // 摘录条数受上限约束：接近上限后停止收集，但仍把已遍历的次数计入总数
            if hits.len() >= SEARCH_MAX_RESULTS {
                continue;
            }

            let scene_title = document
                .scene_of_fragment(fid)
                .and_then(|s| document.get_scene(s))
                .and_then(|s| s.title.clone())
                .unwrap_or_else(|| "无标题".to_string());
            // 取首个匹配位置构造摘录（最具代表性）
            let first_idx = indices.swap_remove(0);
            let excerpt = build_excerpt(
                &frag.text,
                first_idx,
                query.chars().count(),
                SEARCH_CONTEXT_CHARS,
            );
            hits.push((fid.clone(), scene_title, excerpt, count_in_fragment));
        }

        let hit_fragments = hits.len();
        let mut out = String::new();
        let scope_desc = scope.as_deref().unwrap_or("全书");
        out.push_str(&format!(
            "搜索 '{}' 结果：共 {} 次出现，分布在 {} 个片段 [范围: {}]\n",
            query, total_occurrences, hit_fragments, scope_desc
        ));
        if hits.is_empty() {
            out.push_str("  （无匹配）");
            return Ok(out);
        }

        for (fid, scene_title, excerpt, cnt) in &hits {
            out.push_str(&format!(
                "  {} 场景「{}」 (×{}): \"{}\"\n",
                fid, scene_title, cnt, excerpt
            ));
        }
        if total_occurrences > SEARCH_MAX_RESULTS {
            out.push_str(&format!(
                "\n（摘录已达上限 {SEARCH_MAX_RESULTS}，已统计全部 {} 次出现；如需查看剩余片段的摘录，请缩小 scope 重试）",
                total_occurrences
            ));
        }
        Ok(out)
    }
}

/// 构造带上下文窗的摘录，用【】标记命中片段
fn build_excerpt(
    text: &str,
    hit_byte_idx: usize,
    query_len: usize,
    context_chars: usize,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    let hit_char_idx = text[..hit_byte_idx.min(text.len())].chars().count();
    let total = chars.len();

    let start = hit_char_idx.saturating_sub(context_chars);
    let end = (hit_char_idx + query_len + context_chars).min(total);

    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push('…');
    }
    for (i, c) in chars[start..end].iter().enumerate() {
        let global = start + i;
        if global == hit_char_idx {
            excerpt.push('【');
        }
        excerpt.push(*c);
        if global + 1 == hit_char_idx + query_len {
            excerpt.push('】');
        }
    }
    if end < total {
        excerpt.push('…');
    }
    excerpt
}

// ============================================================================
// get_character_state
// ============================================================================

pub struct GetCharacterStateTool {
    ctx: ToolContext,
}
impl GetCharacterStateTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetCharacterStateTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_character_state".into(),
            description: "获取某角色的当前状态快照：聚合所有 active 断言（年龄/存活/持有物/位置/\
            身份等）+ 关系网 + 最近出场。用于写涉及该角色的场景前，快速了解其当前状态，\
            避免描述前后矛盾。"
                .into(),
            parameters: object_schema(vec![(
                "entity_id",
                "string",
                "角色或实体 ID（如 c_0001）",
                true,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let entity_id = get_str(arguments, "entity_id")?;
        let parse_output = parse_project(&self.ctx.project_dir).map_err(|e| format!("{e:?}"))?;
        let sid = StableId::new_unchecked(&entity_id);

        // 实体存在性校验
        let entity_name = parse_output
            .ir
            .semantic
            .get(&sid)
            .map(|n| n.display_name().to_string())
            .ok_or_else(|| format!("实体 '{entity_id}' 不存在"))?;

        let mut out = String::new();
        out.push_str(&format!("角色状态快照: {}({})\n", entity_name, entity_id));

        // 档案摘要（写前保护）：即使角色从未出场（断言/关系/出场全空），
        // 快照也携带职业/种族等档案要点，让写作前置查询自足
        let paths = self.ctx.paths();
        if let Some(path) = paths.file_for_id(&entity_id) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(entity) = story_project_format::EntityFile::from_toml_str(&content) {
                    let brief = super::entity_profile_brief(&entity_id, &entity);
                    // brief 形如 名称（id，职业，种族）——去掉与标题重复的名称和 ID
                    let inner = brief
                        .split_once('（')
                        .and_then(|(_, rest)| rest.strip_suffix('）'))
                        .unwrap_or("");
                    if !inner.is_empty() {
                        out.push_str(&format!("  档案: {inner}\n"));
                    }
                }
            }
        }
        out.push('\n');

        // —— 1. active 断言（从 assertions.toml 读取，过滤 active + subject 匹配）——
        let paths = self.ctx.paths();
        let assertions_file: AssertionsFile = match std::fs::read_to_string(paths.assertions_toml())
        {
            Ok(content) => AssertionsFile::from_toml_str(&content)
                .map_err(|e| format!("解析 assertions.toml 失败: {e}"))?,
            Err(_) => AssertionsFile::default(),
        };
        let assertions: Vec<&AssertionDef> = assertions_file
            .assertions
            .iter()
            .filter(|a| a.subject == entity_id && (a.status.is_empty() || a.status == "active"))
            .collect();

        out.push_str(&format!("当前断言({}):\n", assertions.len()));
        if assertions.is_empty() {
            out.push_str("  （无 active 断言）\n");
        } else {
            for a in &assertions {
                let obj = format_object(&a.object, &parse_output.ir);
                out.push_str(&format!("  {}.{} = {}\n", entity_name, a.predicate, obj));
            }
        }
        out.push('\n');

        // —— 2. 关系网（from 或 to 是该实体的所有边）——
        let relations: Vec<_> = parse_output
            .ir
            .semantic
            .all_edges()
            .filter(|(_, e)| e.from == sid || e.to == sid)
            .collect();
        out.push_str(&format!("关系({}):\n", relations.len()));
        if relations.is_empty() {
            out.push_str("  （无关系记录）\n");
        } else {
            for (_, edge) in &relations {
                let direction;
                let other_id;
                if edge.from == sid {
                    direction = "→";
                    other_id = &edge.to;
                } else {
                    direction = "←";
                    other_id = &edge.from;
                }
                let other_name = parse_output
                    .ir
                    .semantic
                    .get(other_id)
                    .map(|n| n.display_name().to_string())
                    .unwrap_or_else(|| other_id.as_str().to_string());
                out.push_str(&format!(
                    "  {} {} {}({}) [{}]\n",
                    entity_name, direction, other_name, other_id, edge.relation
                ));
            }
        }
        out.push('\n');

        // —— 3. 最近出场（LocationIndex + anchors 兜底，按片段去重，最多显示 5 个片段）——
        let mut appearances: Vec<StableId> = parse_output
            .location_index
            .appearances_of(&sid)
            .iter()
            .map(|a| a.fragment.clone())
            .collect();
        if appearances.is_empty() {
            for (fid, fnode) in parse_output.ir.document.all_fragments() {
                if fnode.anchors.iter().any(|a| a.entity == sid) {
                    appearances.push(fid.clone());
                }
            }
        }
        // 同一片段多次出现只记一个条目（×N 显示出现次数）
        let mut recent: Vec<(StableId, usize)> = Vec::new();
        for fid in appearances {
            if let Some(entry) = recent.iter_mut().find(|(f, _)| *f == fid) {
                entry.1 += 1;
            } else {
                recent.push((fid, 1));
            }
        }
        let total_mentions: usize = recent.iter().map(|(_, n)| n).sum();
        let recent_count = recent.len().min(5);
        out.push_str(&format!(
            "最近出场(共 {} 次 / {} 个片段，显示最近 {} 个):\n",
            total_mentions,
            recent.len(),
            recent_count
        ));
        if recent.is_empty() {
            out.push_str("  （无出场记录）\n");
        } else {
            for (fid, count) in recent.iter().rev().take(recent_count) {
                let scene_title = parse_output
                    .ir
                    .document
                    .scene_of_fragment(fid)
                    .and_then(|sid| parse_output.ir.document.get_scene(sid))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "无标题".to_string());
                let text = parse_output
                    .ir
                    .document
                    .get_fragment(fid)
                    .map(|f| format_brief(&f.text, 60))
                    .unwrap_or_else(|| "（正文缺失）".to_string());
                let times = if *count > 1 { format!(" ×{}", count) } else { String::new() };
                out.push_str(&format!(
                    "  {}{} 场景「{}」: \"{}\"\n",
                    fid, times, scene_title, text
                ));
            }
        }

        Ok(out)
    }
}

/// 格式化 assertion 的 object 值为可读字符串。
/// toml::Value 的 String 变体会带引号，这里去掉；引用类 ID 尝试翻译成名称。
fn format_object(v: &toml::Value, ir: &story_ir::StoryIR) -> String {
    match v {
        toml::Value::String(s) => {
            // 若值像实体 ID（c_/l_/i_/o_ 前缀），尝试翻译成名称
            if let Some(name) = ir
                .semantic
                .get(&StableId::new_unchecked(s))
                .map(|n| n.display_name().to_string())
            {
                if name != s.as_str() {
                    return format!("{name}({s})",);
                }
            }
            s.clone()
        }
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Float(f) => f.to_string(),
        other => other.to_string(),
    }
}

// ============================================================================
// get_project_stats
// ============================================================================

pub struct GetProjectStatsTool {
    ctx: ToolContext,
}
impl GetProjectStatsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for GetProjectStatsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "get_project_stats".into(),
            description: "获取项目整体统计：字数、章节/场景/片段数、实体/关系/断言数、\
            线索状态分布。用于了解项目规模、写作进度、设定密度。"
                .into(),
            parameters: object_schema(vec![]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // 参数 schema 为空，但仍调用以保持约定（防止 LLM 传参时静默忽略）
        let _ = arguments;
        let parse_output = parse_project(&self.ctx.project_dir).map_err(|e| format!("{e:?}"))?;
        let ir = &parse_output.ir;

        // —— 字数统计（用 exporter，已剥锚点注释，比手算准）——
        let manuscript = collect_manuscript(ir);
        let total_words: usize = manuscript.chapters.iter().map(|c| c.word_count).sum();

        // —— 结构计数 ——
        let mut chapter_count = 0usize;
        let mut scene_count = 0usize;
        let fragment_count = ir.document.all_fragments().count();
        // chapter/scene 从 book 出发遍历
        let book_id = ir
            .document
            .all_nodes()
            .find_map(|(id, n)| matches!(n, story_ir::DocNode::Book(_)).then_some(id));
        if let Some(book_id) = book_id {
            let chapters = ir.document.children_of(book_id);
            chapter_count = chapters.len();
            for ch_id in chapters {
                scene_count += ir.document.scenes_of_chapter(ch_id).len();
            }
        }

        // —— 知识库计数（实体/关系来自 IR，断言读文件）——
        let entity_count = ir.semantic.node_count();
        let relation_count = ir.semantic.edge_count();
        let paths = self.ctx.paths();
        let assertion_count = match std::fs::read_to_string(paths.assertions_toml()) {
            Ok(content) => AssertionsFile::from_toml_str(&content)
                .map(|f| f.assertions.len())
                .unwrap_or(0),
            Err(_) => 0,
        };

        // —— 线索状态分布 ——
        let mut thread_status: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for (_, thread) in parse_output.threads.all() {
            let status = format!("{:?}", thread.status()).to_lowercase();
            *thread_status.entry(status).or_insert(0) += 1;
        }
        let thread_total = thread_status.values().sum::<usize>();

        // —— 组装输出 ——
        let mut out = String::new();
        out.push_str("项目统计:\n");
        out.push_str(&format!("  总字数: {total_words}\n"));
        out.push_str(&format!("  章节: {chapter_count}\n"));
        out.push_str(&format!("  场景: {scene_count}\n"));
        out.push_str(&format!("  片段: {fragment_count}\n"));
        out.push_str(&format!("  实体: {entity_count}\n"));
        out.push_str(&format!("  关系: {relation_count}\n"));
        out.push_str(&format!("  断言: {assertion_count}\n"));
        out.push_str(&format!("  线索: {thread_total}"));
        if !thread_status.is_empty() {
            let dist: Vec<String> = thread_status
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            out.push_str(&format!(" ({})", dist.join(", ")));
        }
        out.push('\n');

        // —— 章节脉络进度（设计层，补足"写到第几章"）——
        if paths.chapters_toml().exists() {
            if let Ok(content) = std::fs::read_to_string(paths.chapters_toml()) {
                if let Ok(chapters_file) = ChaptersFile::from_toml_str(&content) {
                    let total = chapters_file.chapters.len();
                    if total > 0 {
                        let completed = chapters_file
                            .chapters
                            .iter()
                            .filter(|c| c.status == "completed")
                            .count();
                        let writing = chapters_file
                            .chapters
                            .iter()
                            .filter(|c| c.status == "writing")
                            .count();
                        let planned = total - completed - writing;
                        out.push_str(&format!(
                            "  脉络进度: {completed}/{total} 完成 (writing={writing}, planned={planned})\n"
                        ));
                        if let Some(next) = chapters_file
                            .chapters
                            .iter()
                            .find(|c| c.status == "planned" || c.status == "writing")
                        {
                            let phase = if next.status == "writing" {
                                "进行中"
                            } else {
                                "待写"
                            };
                            out.push_str(&format!("  {phase}章节: {}\n", next.title));
                        }
                    }
                }
            }
        }

        // —— 平均字数（辅助参考）——
        if chapter_count > 0 {
            if let Some(avg) = total_words.checked_div(chapter_count) {
                out.push_str(&format!("  章均字数: {avg}\n"));
            }
        }
        if scene_count > 0 {
            if let Some(avg) = total_words.checked_div(scene_count) {
                out.push_str(&format!("  场均字数: {avg}\n"));
            }
        }

        Ok(out)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ai_runtime::provider::{
        AiError, GenerateRequest, GenerateResponse, StopReason, TokenUsage,
    };
    use story_project_format::init::init_project;
    use tempfile::TempDir;

    fn setup_test_project() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    #[test]
    fn find_mentions_caps_results_and_scope_filters() {
        let tmp = setup_test_project();
        let dir = tmp.path();

        // 2 章 × 30 片段，全部文本匹配"林风"（60 个片段 > 上限 50）
        let mut structure = String::from("[book]\nid = \"b_root\"\ntitle = \"测试\"\nchapters = [\"ch_01\", \"ch_02\"]\n");
        for ch in ["ch_01", "ch_02"] {
            structure.push_str(&format!("\n[chapter.{ch}]\ntitle = \"章{ch}\"\nscenes = [\"sc_{ch}_01\"]\n"));
        }
        for ch in ["ch_01", "ch_02"] {
            let fids: Vec<String> = (1..=30).map(|i| format!("f_{ch}_{i:02}")).collect();
            structure.push_str(&format!(
                "\n[scene.sc_{ch}_01]\ntitle = \"场景\"\nfragments = [{}]\n",
                fids.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ")
            ));
        }
        std::fs::write(dir.join("structure.toml"), structure).unwrap();
        for ch in ["ch_01", "ch_02"] {
            for i in 1..=30 {
                std::fs::write(
                    dir.join(format!("fragments/f_{ch}_{i:02}.md")),
                    format!("---\nid: f_{ch}_{i:02}\n---\n林风在场景里走动。\n"),
                )
                .unwrap();
            }
        }
        std::fs::create_dir_all(dir.join("entities/characters")).ok();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"林风\"\naliases = []\n",
        )
        .unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = FindMentionsTool::new(ctx);

        // 默认：截断到 50 条并提示缩小范围
        let out = tool.execute(&serde_json::json!({"entity_id": "c_0001"})).unwrap();
        assert!(out.contains("60 个片段"), "{out}");
        assert!(out.contains("已列出前 50 条"), "{out}");
        assert!(out.contains("scope"), "{out}");

        // scope 限定章节：该章 30 条全列，无截断提示、无他章条目
        let out = tool
            .execute(&serde_json::json!({"entity_id": "c_0001", "scope": "chapter:ch_01"}))
            .unwrap();
        assert!(out.contains("30 个片段"), "{out}");
        assert!(!out.contains("f_ch_02"), "{out}");
        assert!(!out.contains("已列出前"), "{out}");

        // scope 限定不存在章节：报错
        let err = tool
            .execute(&serde_json::json!({"entity_id": "c_0001", "scope": "chapter:ch_99"}))
            .unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }

    #[test]
    fn story_recap_degrades_to_chapter_granularity_for_huge_books() {
        let tmp = setup_test_project();
        let dir = tmp.path();

        // 1 章 205 个场景（> 阈值 200）
        let sids: Vec<String> = (1..=205).map(|i| format!("sc_big_{i:03}")).collect();
        let mut structure = String::from("[book]\nid = \"b_root\"\ntitle = \"测试\"\nchapters = [\"ch_01\"]\n\n[chapter.ch_01]\ntitle = \"第一章\"\nscenes = [");
        structure.push_str(&sids.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", "));
        structure.push_str("]\n");
        for (i, s) in sids.iter().enumerate() {
            structure.push_str(&format!("\n[scene.{s}]\ntitle = \"场景{i}\"\nfragments = []\nsummary = \"概要{i}\"\n"));
        }
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let out = GetStoryRecapTool::new(ctx)
            .execute(&serde_json::json!({}))
            .unwrap();
        assert!(out.contains("已折叠为章节粒度"), "{out}");
        assert!(!out.contains("- 场景"), "降级后不应逐场景列行: {out}");
        assert!(out.contains("场景0：概要0"), "章节行应含场景概要: {out}");
    }

    /// 写入一个带 summary 的场景 + 角色 + fragment fixture
    fn write_fixture_with_summary(dir: &std::path::Path) {
        // structure.toml：1 章 1 场景 1 片段，场景有 summary
        let structure = r#"[book]
id = "b_root"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章 雨夜"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "客栈投宿"
fragments = ["f_0001"]
mood = "阴郁"
summary = "林风冒雨推开客栈大门，与掌柜交涉住宿。"
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        // 一个 fragment（锚点 c_0001）
        let frag = "---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n林风推开客栈大门，雨水顺着发梢滴落。掌柜抬头看了他一眼。\n";
        std::fs::write(dir.join("fragments/f_0001.md"), frag).unwrap();

        // 两个角色实体
        std::fs::create_dir_all(dir.join("entities/characters")).ok();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"林风\"\naliases = []\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0002.toml"),
            "id = \"c_0002\"\ndisplay_name = \"叶知秋\"\naliases = []\n",
        )
        .unwrap();
    }

    /// 写入 relations + assertions fixture
    fn write_knowledge_fixture(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("knowledge")).ok();
        let relations = r#"[[relation]]
id = "se_0001"
from = "c_0001"
to = "c_0002"
relation = "mentor_of"
evidence = ["f_0001"]

[[relation]]
id = "se_0002"
from = "c_0002"
to = "c_0001"
relation = "enemy_of"
"#;
        std::fs::write(dir.join("relations.toml"), relations).unwrap();

        let assertions = r#"[[assertion]]
id = "a_001"
subject = "c_0001"
predicate = "age"
object = 17
status = "active"
extracted_by = "manual"

[[assertion]]
id = "a_002"
subject = "c_0001"
predicate = "alive"
object = true
status = "active"

[[assertion]]
id = "a_003"
subject = "c_0002"
predicate = "age"
object = 45
status = "superseded"
"#;
        std::fs::write(dir.join("knowledge/assertions.toml"), assertions).unwrap();
    }

    // —— get_story_recap ——

    #[test]
    fn get_story_recap_with_existing_summary() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetStoryRecapTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"scope": "all", "detail": "normal"}))
            .unwrap();
        assert!(result.contains("故事回顾"));
        assert!(result.contains("第一章 雨夜"));
        assert!(result.contains("客栈投宿"));
        // 已有 summary 应被直接使用
        assert!(result.contains("林风冒雨推开客栈"));
    }

    #[test]
    fn get_story_recap_chapter_scope_not_found() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetStoryRecapTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"scope": "chapter:ch_99"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    #[test]
    fn get_story_recap_no_chapter() {
        let tmp = setup_test_project();
        // 空项目，无章节
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetStoryRecapTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("尚无章节"));
    }

    // —— get_relations ——

    #[test]
    fn get_relations_all() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetRelationsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("关系列表(2)"));
        assert!(result.contains("mentor_of"));
        assert!(result.contains("enemy_of"));
        // 实体 ID 应被翻译成名称
        assert!(result.contains("林风"));
        assert!(result.contains("叶知秋"));
    }

    #[test]
    fn get_relations_filter_by_entity() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetRelationsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"entity_id": "c_0001"}))
            .unwrap();
        // c_0001 既是 se_0001.from 又是 se_0002.to
        assert!(result.contains("关系列表(2)"));
    }

    #[test]
    fn get_relations_filter_by_type() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetRelationsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"relation_type": "mentor_of"}))
            .unwrap();
        assert!(result.contains("关系列表(1)"));
        assert!(result.contains("mentor_of"));
    }

    #[test]
    fn get_relations_empty() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetRelationsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("关系列表(0)"));
    }

    // —— query_assertions ——

    #[test]
    fn query_assertions_default_active_only() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path()); // 提供实体名翻译
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = QueryAssertionsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        // a_001 active, a_002 active, a_003 superseded → 默认只返回 2 个
        assert!(result.contains("断言列表(2)"));
        assert!(result.contains("林风"));
        // superseded 的 45 不应出现
        assert!(!result.contains("= 45"));
    }

    #[test]
    fn query_assertions_all_status() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = QueryAssertionsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"status": "all"})).unwrap();
        assert!(result.contains("断言列表(3)"));
    }

    #[test]
    fn query_assertions_filter_by_subject_and_predicate() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        write_knowledge_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = QueryAssertionsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"subject": "c_0001", "predicate": "age"}))
            .unwrap();
        assert!(result.contains("断言列表(1)"));
        assert!(result.contains(".age = 17"));
    }

    // —— find_mentions ——

    #[test]
    fn find_mentions_returns_appearances() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = FindMentionsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"entity_id": "c_0001"}))
            .unwrap();
        // LocationIndex 合并显式 anchor + 隐式文本匹配，至少 1 条
        assert!(result.contains("出场记录("));
        assert!(result.contains("林风"));
        assert!(result.contains("f_0001"));
        assert!(result.contains("客栈投宿")); // 场景标题
    }

    #[test]
    fn find_mentions_nonexistent_entity() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = FindMentionsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"entity_id": "c_9999"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    #[test]
    fn find_mentions_no_appearances() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        // c_0002 存在但无出场（fixture 已写入 c_0002）
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = FindMentionsTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"entity_id": "c_0002"}))
            .unwrap();
        assert!(result.contains("无出场记录"));
    }

    // —— search_text ——

    #[test]
    fn search_text_finds_keyword() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = SearchTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"query": "客栈"})).unwrap();
        assert!(result.contains("分布在 1 个片段")); // 命中 1 个片段
        assert!(result.contains("f_0001"));
        assert!(result.contains("【客栈")); // 命中标记
    }

    #[test]
    fn search_text_no_match() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = SearchTextTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"query": "不存在的词"}))
            .unwrap();
        assert!(result.contains("共 0 次出现"));
        assert!(result.contains("（无匹配）"));
    }

    #[test]
    fn search_text_invalid_scope_scene() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = SearchTextTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"query": "林风", "scope": "scene:sc_99_99"}));
        assert!(result.is_err());
    }

    #[test]
    fn build_excerpt_marks_hit() {
        let text = "林风推开客栈大门，雨水顺着发梢滴落。";
        let lower: String = text.chars().map(|c| c.to_ascii_lowercase()).collect();
        let idx = lower.find("客栈").unwrap();
        let excerpt = build_excerpt(text, idx, "客栈".chars().count(), 5);
        assert!(excerpt.contains("【客栈】"));
    }

    // —— get_story_recap with mock provider（无 summary 时触发现场生成）——

    /// Mock provider，返回固定摘要文本
    #[derive(Debug)]
    struct MockSummaryProvider {
        response: String,
    }

    impl AiProvider for MockSummaryProvider {
        fn id(&self) -> &str {
            "mock-summary"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    /// 写入一个【无 summary】的场景 fixture
    fn write_fixture_without_summary(dir: &std::path::Path) {
        let structure = r#"[book]
id = "b_root"
title = "测试小说"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
fragments = ["f_0001"]
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n林风走进山谷，看到一座古塔。\n",
        )
        .unwrap();
    }

    #[test]
    fn get_story_recap_generates_summary_via_provider() {
        let tmp = setup_test_project();
        write_fixture_without_summary(tmp.path());

        let provider = Arc::new(MockSummaryProvider {
            response: "LLM生成的场景摘要。".to_string(),
        }) as Arc<dyn AiProvider>;
        let ctx = ToolContext::with_provider(tmp.path().to_path_buf(), provider);
        let tool = GetStoryRecapTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();

        assert!(result.contains("LLM生成的场景摘要"));
        assert!(result.contains("现场生成")); // 提示信息
    }

    #[test]
    fn get_story_recap_fallback_without_provider() {
        let tmp = setup_test_project();
        write_fixture_without_summary(tmp.path());

        // 无 provider → 应回退为正文截断
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetStoryRecapTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("林风走进山谷"));
        assert!(result.contains("无 provider")); // 降级提示
    }

    // —— get_character_state ——

    #[test]
    fn get_character_state_aggregates_assertions_and_relations() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path()); // 提供 c_0001/c_0002 实体 + fragment
        write_knowledge_fixture(tmp.path()); // 提供 age/alive 断言 + mentor_of/enemy_of 关系
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetCharacterStateTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"entity_id": "c_0001"}))
            .unwrap();

        assert!(result.contains("角色状态快照"));
        assert!(result.contains("林风"));
        // active 断言：age=17, alive=true（superseded 的 a_003 是 c_0002，不在此）
        assert!(result.contains("age = 17"));
        assert!(result.contains("alive = true"));
        // 关系：c_0001 是 se_0001.from(mentor_of) 和 se_0002.to(enemy_of)
        assert!(result.contains("mentor_of"));
        assert!(result.contains("enemy_of"));
        assert!(result.contains("叶知秋")); // 关系对象名翻译
                                            // 最近出场：f_0001 锚定了 c_0001
        assert!(result.contains("f_0001"));
    }

    #[test]
    fn get_character_state_nonexistent_entity() {
        let tmp = setup_test_project();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetCharacterStateTool::new(ctx);
        let result = tool.execute(&serde_json::json!({"entity_id": "c_9999"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("不存在"));
    }

    #[test]
    fn get_character_state_no_data() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path()); // c_0002 存在但无断言/关系/出场
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetCharacterStateTool::new(ctx);
        let result = tool
            .execute(&serde_json::json!({"entity_id": "c_0002"}))
            .unwrap();
        assert!(result.contains("叶知秋"));
        assert!(result.contains("无 active 断言"));
        assert!(result.contains("无关系记录"));
    }

    // —— get_project_stats ——

    #[test]
    fn get_project_stats_basic() {
        let tmp = setup_test_project();
        write_fixture_with_summary(tmp.path()); // 1 章 1 场景 1 片段 + 2 实体
        write_knowledge_fixture(tmp.path()); // 3 断言 + 2 关系
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetProjectStatsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();

        assert!(result.contains("项目统计"));
        assert!(result.contains("章节: 1"));
        assert!(result.contains("场景: 1"));
        assert!(result.contains("片段: 1"));
        assert!(result.contains("实体: 2"));
        assert!(result.contains("关系: 2"));
        assert!(result.contains("断言: 3"));
        // 字数应该是正数（fragment 有正文）
        assert!(result.contains("总字数:"));
        // 章均/场均字数
        assert!(result.contains("章均字数"));
    }

    #[test]
    fn get_project_stats_empty_project() {
        let tmp = setup_test_project();
        // 空项目，无章节
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = GetProjectStatsTool::new(ctx);
        let result = tool.execute(&serde_json::json!({})).unwrap();
        assert!(result.contains("项目统计"));
        assert!(result.contains("章节: 0"));
        assert!(result.contains("总字数: 0"));
    }

    #[test]
    fn format_object_handles_types() {
        let ir = story_ir::StoryIR::default();
        assert_eq!(format_object(&toml::Value::Integer(17), &ir), "17");
        assert_eq!(format_object(&toml::Value::Boolean(true), &ir), "true");
        assert_eq!(
            format_object(&toml::Value::String("某文本".into()), &ir),
            "某文本"
        );
    }

    #[test]
    fn character_state_includes_profile_for_unseen_character() {
        let tmp = setup_test_project();
        let dir = tmp.path();

        let entity_toml = r#"id = "c_0009"
display_name = "陆判"
aliases = []

[profile]
occupation = "阴司判官"
race = "鬼修"
"#;
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(dir.join("entities/characters/c_0009.toml"), entity_toml).unwrap();

        let ctx = crate::tools::ToolContext::new(dir.to_path_buf());
        let tool = GetCharacterStateTool::new(ctx);
        let out = tool.execute(&serde_json::json!({"entity_id": "c_0009"})).unwrap();
        assert!(
            out.contains("档案: c_0009，阴司判官，鬼修"),
            "未出场角色也应有档案摘要行: {out}"
        );
    }
}
