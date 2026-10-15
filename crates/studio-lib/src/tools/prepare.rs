//! prepare_writing — 动笔前的一站式上下文聚合工具。
//!
//! 提示词的「先读后写」铁律要求动笔前串联 4-5 次上下文查询
//! （脉络/前情/角色状态/线索认知/场景上下文），多轮往返既慢又容易漏。
//! 本工具一次调用聚齐这些维度：所有数据源共享 ToolContext 的增量解析
//! 缓存，底层读取近乎零成本；各 section 均有限量，输出保持"一屏可读"。

use story_ai_runtime::{Tool, ToolDef};
use story_ir::StableId;
use story_project_format::chapters_file::ChapterOutlineDef;
use story_project_format::{AssertionsFile, ChaptersFile};

use super::entities::{render_outline_entry, render_thread_briefs};
use super::query::frontier_scene_with_pov;
use super::{format_brief, get_str_opt, object_schema, ToolContext};

/// 角色状态断言：每人最多带几条（全量可调 get_character_state）
const ASSERTION_CAP_PER_CHAR: usize = 6;
/// 状态断言区块最多覆盖几个角色（POV 优先）
const ASSERTION_CHAR_CAP: usize = 6;
/// 前情回顾：目标之前最近几个已写场景
const RECAP_SCENE_COUNT: usize = 3;

pub struct PrepareWritingTool {
    ctx: ToolContext,
}
impl PrepareWritingTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

/// 上章钩子承接提示：本章开头尚未落笔、上一章已写且规划了章末钩子时，
/// 返回提示行。
///
/// 章末钩子不是线索（伏笔沉寂预警只扫 threads.toml，覆盖不到 hook 字段），
/// 长篇连载中最易遗忘的就是上一章结尾留下的张力——动笔准备时把钩子
/// 显式带到眼前。本章已有正文（开头已写）或上一章未动笔（钩子还是
/// 规划态、写它时自会处理）时不再提示。
fn prev_chapter_hook_note(
    chapters: &[ChapterOutlineDef],
    outline_index: usize, // 1-based，当前目标章
    document: &story_ir::DocumentGraph,
) -> Option<String> {
    let prev = chapters.get(outline_index.checked_sub(2)?)?;
    let hook = prev
        .hook
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())?;
    // 上一章是虚拟章（未落实结构）→ 正文必然未写，钩子无从承接
    let prev_ch_id = prev.chapter_id.as_deref()?;
    let prev_written = document
        .scenes_of_chapter(&StableId::new_unchecked(prev_ch_id))
        .iter()
        .any(|sc| !document.fragments_of_scene(sc).is_empty());
    if !prev_written {
        return None;
    }
    // 本章已有正文 → 开场承接的时机已过
    if let Some(cur_ch_id) = chapters[outline_index - 1].chapter_id.as_deref() {
        let cur_written = document
            .scenes_of_chapter(&StableId::new_unchecked(cur_ch_id))
            .iter()
            .any(|sc| !document.fragments_of_scene(sc).is_empty());
        if cur_written {
            return None;
        }
    }
    Some(format!(
        "\n[上章钩子承接] 第{}章「{}」的章末钩子：{hook}——\
         本章开头应有意识地承接（或明确切换时间线/视角），勿让上章张力悬空\n",
        outline_index - 1,
        prev.title
    ))
}

/// 最近一个已有正文的章节 ID（阅读序最后一个首场景有片段的章）。
fn last_written_chapter(paths: &story_project_format::ProjectPaths) -> Option<String> {
    let structure = crate::mutations::load_structure(paths).ok()?;
    let mut last: Option<String> = None;
    for ch_id in &structure.book.chapters {
        let ch = structure.chapter.get(ch_id)?;
        if ch.scenes.iter().any(|sc| {
            structure
                .scene
                .get(sc)
                .is_some_and(|s| !s.fragments.is_empty())
        }) {
            last = Some(ch_id.clone());
        }
    }
    last
}

impl Tool for PrepareWritingTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "prepare_writing".into(),
            description: "动笔前的一站式上下文聚合：一次调用返回本章脉络（目标/节拍/钩子/状态）、\
            上章钩子承接提示（本章开头未落笔时带出上一章的章末钩子，防连载遗忘伏笔张力）、\
            前情回顾（目标之前最近几个已写场景）、本章线索认知快照（各观察者最新认知）、\
            伏笔沉寂预警（主线已埋未收且超过阈值章数无动静的线索）、\
            涉及角色速查（场景级聚焦：本幕在场上/本章待命两档，含外貌/习惯/个性与真名遮蔽）、\
            角色当前状态断言、题材提示与写作要点。\
            续写已有场景传 scene_id；写新章节传 chapter（脉络序号，1-based）；\
            两者都省略时默认取写作最前沿场景。\
            chapter/scene 的选择与量级以用户本次指令为准（说写一章就只准备该章），\
            勿自行扩大写作范围。\
            替代动笔前依次调用 get_chapter_outline / get_story_recap / get_character_state /\
            get_scene_context 的多次往返；需要更深的单一维度（完整前情、完整时间线、\
            题材专项指导）再补对应查询。"
                .into(),
            parameters: object_schema(vec![
                (
                    "scene_id",
                    "string",
                    "要续写的场景 ID（如 sc_0003）；与 chapter 二选一",
                    false,
                ),
                (
                    "chapter",
                    "integer",
                    "要动笔的章节脉络序号（1-based，与 get_chapter_outline 展示一致）；与 scene_id 二选一",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let parse_output = self.ctx.parse()?;
        let ir = &parse_output.ir;
        let document = &ir.document;
        let semantic = &ir.semantic;
        let paths = self.ctx.paths();

        // —— 目标解析：scene_id → chapter 序号 → 写作前沿回退 ——
        let scene_id_opt = get_str_opt(arguments, "scene_id");
        let chapter_opt = arguments
            .get("chapter")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        if scene_id_opt.is_some() && chapter_opt.is_some() {
            return Err(
                "scene_id 与 chapter 只能传一个（续写场景传 scene_id，写新章节传 chapter）".into(),
            );
        }

        let (scene_id, chapter_index): (Option<String>, Option<usize>) =
            if let Some(sid_str) = scene_id_opt {
                let sid = StableId::new_unchecked(&sid_str);
                if document.get_scene(&sid).is_none() {
                    return Err(format!("场景 '{sid_str}' 不存在"));
                }
                (Some(sid_str), None)
            } else if let Some(index) = chapter_opt {
                (None, Some(index))
            } else {
                // 缺参回退：写作最前沿场景（阅读序最后一个设了 POV 的场景）
                let scene = frontier_scene_with_pov(document).ok_or_else(|| {
                    "缺少参数: scene_id 或 chapter（项目中也没有可回退的前沿场景——\
                  请传 chapter（脉络序号）准备写新章节）"
                        .to_string()
                })?;
                (Some(scene.id.as_str().to_string()), None)
            };

        // —— 脉络条目定位：场景 → 绑定章节 ID → 脉络；或直接按序号 ——
        let chapters_file: ChaptersFile =
            crate::mutations::load_chapters_file(&paths).unwrap_or_default();
        let (outline_entry, outline_index): (Option<&ChapterOutlineDef>, Option<usize>) =
            if let Some(index) = chapter_index {
                let entry = chapters_file.chapters.get(index.saturating_sub(1)).ok_or_else(
                    || format!("章节脉络序号 {index} 不存在（共 {} 条，可用 get_chapter_outline 查看）", chapters_file.chapters.len()),
                )?;
                (Some(entry), Some(index))
            } else if let Some(sid_str) = &scene_id {
                let sid = StableId::new_unchecked(sid_str);
                let found = document
                    .chapter_of_scene(&sid)
                    .and_then(|ch_id| {
                        chapters_file
                            .chapters
                            .iter()
                            .position(|c| c.chapter_id.as_deref() == Some(ch_id.as_str()))
                    })
                    .map(|pos| (&chapters_file.chapters[pos], pos + 1));
                match found {
                    Some((entry, idx)) => (Some(entry), Some(idx)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };

        // —— 标题行 ——
        let mut out = String::from("[准备动笔]\n");
        if let Some(entry) = outline_entry {
            let volume = if entry.volume_order > 0 {
                format!("第{}卷 · ", entry.volume_order)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "{}第{}章「{}」（状态: {}）\n",
                volume,
                outline_index.unwrap_or_default(),
                entry.title,
                entry.status
            ));
        }
        if let Some(sid_str) = &scene_id {
            let sid = StableId::new_unchecked(sid_str);
            let scene = document.get_scene(&sid).unwrap();
            let chapter_title = document
                .chapter_of_scene(&sid)
                .and_then(|ch| document.get_chapter(ch))
                .map(|ch| ch.title.as_str())
                .unwrap_or("未分章");
            let pov_label = scene.pov.as_ref().and_then(|p| {
                semantic
                    .get(p)
                    .map(|n| format!("（POV: {}）", n.display_name()))
            });
            out.push_str(&format!(
                "目标场景: {sid_str}「{}」（所属: {chapter_title}）{}\n",
                scene.title.as_deref().unwrap_or("无标题"),
                pov_label.unwrap_or_default()
            ));
        }

        // —— 1. 本章脉络 ——
        if let Some(entry) = outline_entry {
            if let Some(brief) = render_outline_entry(entry) {
                out.push_str("\n本章脉络:\n");
                out.push_str(&brief);
            }
            if let Some(summary) = entry
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push_str(&format!("    梗概: {}\n", truncate_line(summary, 120)));
            }
            // —— 1.5 上章钩子承接 ——
            if let Some(idx) = outline_index {
                if let Some(note) = prev_chapter_hook_note(&chapters_file.chapters, idx, document) {
                    out.push_str(&note);
                }
            }
        }

        // —— 2. 前情回顾：目标之前最近 N 个已写场景（只用已有摘要/正文截断，不调 LLM）——
        let target_pos: u32 = scene_id
            .as_deref()
            .and_then(|s| document.reading_index(&StableId::new_unchecked(s)))
            .or_else(|| {
                // 章节目标：以该章第一个场景的位置为界；虚拟章（未落实结构）则不限
                outline_entry
                    .and_then(|e| e.chapter_id.as_deref())
                    .and_then(|ch| {
                        document
                            .scenes_of_chapter(&StableId::new_unchecked(ch))
                            .first()
                            .and_then(|sc| document.reading_index(sc))
                    })
            })
            .unwrap_or(u32::MAX);

        let mut written: Vec<(u32, &story_ir::SceneNode)> = document
            .all_nodes()
            .filter_map(|(id, node)| match node {
                story_ir::DocNode::Scene(s) if !document.fragments_of_scene(id).is_empty() => {
                    document.reading_index(id).map(|r| (r, s))
                }
                _ => None,
            })
            .filter(|(r, _)| *r < target_pos)
            .collect();
        written.sort_by_key(|(r, _)| *r);
        if !written.is_empty() {
            out.push_str("\n前情回顾（最近已写场景）:\n");
            for (_, scene) in written.iter().rev().take(RECAP_SCENE_COUNT).rev() {
                let content = scene
                    .summary
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| truncate_line(s, 100))
                    .unwrap_or_else(|| {
                        // 无摘要：取第一个片段正文截断
                        document
                            .fragments_of_scene(&scene.id)
                            .iter()
                            .find_map(|fid| document.get_fragment(fid))
                            .map(|f| format_brief(&f.text, 80))
                            .unwrap_or_else(|| "（空）".into())
                    });
                out.push_str(&format!(
                    "  - {}「{}」: {}\n",
                    scene.id.as_str(),
                    scene.title.as_deref().unwrap_or("无标题"),
                    content
                ));
            }
        }

        // —— 3. 本章线索认知快照 ——
        if let Some(entry) = outline_entry {
            if let Some(brief) = render_thread_briefs(&self.ctx, &entry.threads) {
                out.push_str("\n本章线索（各观察者最新认知）:\n");
                out.push_str(&brief);
            }
        }

        // —— 3.5 伏笔沉寂预警（跨章）——
        // 被遗忘的线索恰恰不在本章脉络名单里，所以要扫全部已埋/已揭线索的
        // 最近动静（锚点 + 认知时间线），与当前写作位置比对
        let scene_chapter_pos = build_scene_chapter_pos(document);
        let target_ch_pos: Option<usize> = scene_id
            .as_deref()
            .and_then(|s| scene_chapter_pos.get(s).copied())
            .or_else(|| {
                outline_entry
                    .and_then(|e| e.chapter_id.as_deref())
                    .and_then(|ch| book_chapter_ids(document).iter().position(|c| c == ch))
            })
            .or_else(|| outline_index.map(|i| i.saturating_sub(1)));
        let stale = thread_stale_warnings(&self.ctx, &scene_chapter_pos, target_ch_pos);
        if !stale.is_empty() {
            out.push_str("\n⚠️ 伏笔沉寂预警（主线已埋未收且久无动静，考虑在本章推进——advance_thread / set_thread_knowledge）:\n");
            for line in stale {
                out.push_str(&line);
            }
        }

        // —— 4. 涉及角色速查（场景级聚焦：本幕在场上 / 本章待命两档）——
        // 在场判定：POV（镜头所在）∪ 本场景片段已锚定角色 ∪ 节拍文本
        // （function/title）点名的角色。脉络名单里其余角色收进「待命」
        // 一档——高亮而不隐藏：待命角色只列称呼不铺档案，出场前再调档，
        // 速查区聚焦真正要下笔的人，同时不丢「本章还有谁可用」的信息。
        let pov_id: Option<String> = scene_id.as_deref().and_then(|s| {
            document
                .get_scene(&StableId::new_unchecked(s))
                .and_then(|sc| sc.pov.as_ref().map(|p| p.as_str().to_string()))
        });
        let mut involved: Vec<String> = outline_entry
            .map(|e| e.characters.clone())
            .unwrap_or_default();
        if let Some(pov) = &pov_id {
            if !involved.iter().any(|c| c == pov) {
                involved.push(pov.clone());
            }
        }
        let mut anchored_chars: Vec<String> = Vec::new();
        if let Some(sid_str) = &scene_id {
            let sid = StableId::new_unchecked(sid_str);
            for aid in document
                .fragments_of_scene(&sid)
                .iter()
                .filter_map(|fid| document.get_fragment(fid))
                .flat_map(|frag| frag.anchors.iter().map(|a| a.entity.as_str().to_string()))
            {
                let is_char = semantic
                    .get(&StableId::new_unchecked(&aid))
                    .map(|n| n.type_name() == "character")
                    .unwrap_or(false);
                if is_char && !involved.iter().any(|c| c == &aid) {
                    involved.push(aid.clone());
                }
                if is_char && !anchored_chars.iter().any(|c| c == &aid) {
                    anchored_chars.push(aid);
                }
            }
        }
        // 节拍点名文本：目标场景的 function（populate 存的完整节拍）+ 标题
        let named_text = scene_id
            .as_deref()
            .and_then(|s| document.get_scene(&StableId::new_unchecked(s)))
            .map(|sc| {
                let mut t = sc.function.clone().unwrap_or_default();
                if let Some(title) = sc.title.as_deref() {
                    t.push(' ');
                    t.push_str(title);
                }
                t
            })
            .unwrap_or_default();
        let on_stage: Vec<String> = involved
            .iter()
            .filter(|c| {
                pov_id.as_deref() == Some(c.as_str())
                    || anchored_chars.iter().any(|a| a == *c)
                    || character_named_in(&paths, c, &named_text)
            })
            .cloned()
            .collect();
        let standby: Vec<String> = involved
            .iter()
            .filter(|c| !on_stage.contains(c))
            .cloned()
            .collect();

        if on_stage.is_empty() || standby.is_empty() {
            // 无聚焦信息（无 POV/锚点/节拍点名），或全员在场：维持单一平铺名单
            if let Some(block) = super::entities::render_character_lines(
                &self.ctx,
                &involved,
                pov_id.as_deref(),
                semantic,
            ) {
                out.push_str("\n涉及角色:\n");
                out.push_str(&block);
            }
        } else {
            if let Some(block) = super::entities::render_character_lines(
                &self.ctx,
                &on_stage,
                pov_id.as_deref(),
                semantic,
            ) {
                out.push_str("\n涉及角色（本幕在场上，据 POV/锚点/节拍点名）:\n");
                out.push_str(&block);
            }
            let labels: Vec<String> = standby
                .iter()
                .map(|cid| {
                    let name = semantic
                        .get(&StableId::new_unchecked(cid))
                        .map(|n| n.display_name().to_string())
                        .unwrap_or_else(|| cid.clone());
                    format!("{name}（{cid}）")
                })
                .collect();
            out.push_str(&format!(
                "\n本章待命角色（脉络名单其余角色，出场前可用 get_character_state 调档）: {}\n",
                labels.join("、")
            ));
        }

        // —— 5. 角色当前状态断言（POV 优先，限量）——
        let assertions_file: AssertionsFile = std::fs::read_to_string(paths.assertions_toml())
            .ok()
            .and_then(|c| AssertionsFile::from_toml_str(&c).ok())
            .unwrap_or_default();
        let mut state_ids: Vec<String> = Vec::new();
        if let Some(pov) = &pov_id {
            state_ids.push(pov.clone());
        }
        for cid in &involved {
            if state_ids.len() >= ASSERTION_CHAR_CAP {
                break;
            }
            if !state_ids.iter().any(|c| c == cid) {
                state_ids.push(cid.clone());
            }
        }
        let mut state_lines = Vec::new();
        for cid in &state_ids {
            let name = semantic
                .get(&StableId::new_unchecked(cid))
                .map(|n| n.display_name().to_string())
                .unwrap_or_else(|| cid.clone());
            let items: Vec<&story_project_format::knowledge_file::AssertionDef> = assertions_file
                .assertions
                .iter()
                .filter(|a| a.subject == *cid && (a.status.is_empty() || a.status == "active"))
                .collect();
            if items.is_empty() {
                continue;
            }
            let shown: Vec<String> = items
                .iter()
                .take(ASSERTION_CAP_PER_CHAR)
                .map(|a| format!("{}={}", a.predicate, compact_value(&a.object)))
                .collect();
            let more = if items.len() > ASSERTION_CAP_PER_CHAR {
                format!(" …（共 {} 条）", items.len())
            } else {
                String::new()
            };
            state_lines.push(format!("  {name}: {}{more}", shown.join("; ")));
        }
        if !state_lines.is_empty() {
            out.push_str("\n角色当前状态（active 断言，完整快照可调 get_character_state）:\n");
            out.push_str(&state_lines.join("\n"));
            out.push('\n');
        }

        // —— 6. 题材提示（指针，不内嵌全文指导）——
        if let Ok(content) = std::fs::read_to_string(paths.outline_toml()) {
            if let Ok(outline) = story_project_format::OutlineFile::from_toml_str(&content) {
                if let Some(genre) = outline
                    .story
                    .genre
                    .as_deref()
                    .map(str::trim)
                    .filter(|g| !g.is_empty())
                {
                    out.push_str(&format!(
                        "\n题材: {genre}（类型专项指导可调 get_writing_guide）\n"
                    ));
                }
            }
        }

        // —— 上一章开场指纹：让本章第一笔有意识地错开上一章 ——
        // 确定性统计（与 ai_check 跨章开场降级同源），不调 LLM
        if let Some(last_ch) = last_written_chapter(&self.ctx.paths()) {
            if let Some((streak, kind)) =
                super::ai_check::opening_streak(&self.ctx.project_dir, &last_ch)
            {
                let advice = if streak >= 2 {
                    format!(
                        "已连续 {streak} 章以「{kind}」开场，建议本章换成{}",
                        match kind {
                            "对话" => "动作或景物起笔".to_string(),
                            "时间景描" => "对话或动作直入".to_string(),
                            "景物" => "对话或人物动作直入".to_string(),
                            _ => "对话或景物起笔".to_string(),
                        }
                    )
                } else {
                    format!("上一章为「{kind}」起笔，可保持也可错开（连续 2 章以上同型会提示）")
                };
                out.push_str(&format!("\n[开场指纹] 上一章开场类型：{kind}。{advice}。"));
            }
        }

        // —— 尾注：与 get_scene_context 相同的动笔前文风提醒 ——
        out.push_str(
            "\n[写作要点] 白描为主，少修辞，少破折号/省略号；情绪靠动作对话暗示，\
不滥写生理反应；在场角色都要按各自性格行事，台词对照上方速查的\
习惯/个性（句长、语气、口癖因人而异，遮住名字能认出谁在说话）；\
对话要承载功能；留一个未完成的张力；锁定 POV 视角，只写其五感所及；\
章末设翻页钩子。写完正文后状态会自动推进（planned→writing），\
整章场景齐稿后再用 update_chapter_outline 标记 completed。",
        );
        Ok(out)
    }
}

/// 角色是否被文本点名（display_name / 非空 alias 出现即命中）。
///
/// 与写入回执的 mentioned_entity_briefs 同口径，但只查单个候选、
/// 返回布尔——场景级聚焦（节拍文本点名谁，谁就在场上）用。
fn character_named_in(paths: &story_project_format::ProjectPaths, cid: &str, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let Some(path) = paths.file_for_id(cid) else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(entity) = story_project_format::EntityFile::from_toml_str(&content) else {
        return false;
    };
    (!entity.display_name.is_empty() && text.contains(&entity.display_name))
        || entity
            .aliases
            .iter()
            .any(|a| !a.is_empty() && text.contains(a))
}

/// 压平 TOML 值为单行短串（断言对象用）
fn compact_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => truncate_line(s, 30),
        toml::Value::Array(items) => {
            let joined: Vec<String> = items
                .iter()
                .filter_map(|i| i.as_str().map(|s| truncate_line(s, 20)))
                .collect();
            joined.join("、")
        }
        other => truncate_line(&other.to_string(), 30),
    }
}

// ============================================================================
// 伏笔沉寂预警
// ============================================================================

/// 主线线索沉寂阈值（章）：已埋/已揭后超过这么多章无任何动静 → 预警
const STALE_MAIN_CHAPTERS: usize = 10;
/// 支线阈值（章）
const STALE_MINOR_CHAPTERS: usize = 20;

/// 书内章节顺序（阅读序）
fn book_chapter_ids(document: &story_ir::DocumentGraph) -> Vec<String> {
    let Some(book_id) = document
        .all_nodes()
        .find(|(_, n)| matches!(n, story_ir::DocNode::Book(_)))
        .map(|(id, _)| id.clone())
    else {
        return Vec::new();
    };
    document
        .children_of(&book_id)
        .iter()
        .map(|c| c.as_str().to_string())
        .collect()
}

/// 场景 → 章节序位（0-based，阅读序）
fn build_scene_chapter_pos(
    document: &story_ir::DocumentGraph,
) -> std::collections::HashMap<String, usize> {
    let mut map = std::collections::HashMap::new();
    for ch_id in book_chapter_ids(document) {
        let pos = map.len();
        let sid = StableId::new_unchecked(&ch_id);
        for sc in document.scenes_of_chapter(&sid) {
            map.insert(sc.as_str().to_string(), pos);
        }
    }
    map
}

/// 全书线索的沉寂扫描：已埋（planted）/已揭（revealed）未收的线索，
/// 最近一次动静（叙事锚点 or 认知时间线任一场景）距写作前沿超过阈值时预警。
///
/// 「最近动静」由既有数据推导，不新增逐章触碰记录——锚点与认知时间线
/// 已经是线索活动的完整足迹。planned 线索不走这里（due_volume 卷级
/// 体系已在编译诊断里分级）。
fn thread_stale_warnings(
    ctx: &ToolContext,
    scene_chapter_pos: &std::collections::HashMap<String, usize>,
    frontier_chapter_pos: Option<usize>,
) -> Vec<String> {
    let Some(frontier) = frontier_chapter_pos else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(ctx.paths().threads_toml()) else {
        return Vec::new();
    };
    let Ok(file) = story_project_format::ThreadsFile::from_toml_str(&content) else {
        return Vec::new();
    };

    let mut warnings: Vec<(usize, bool, String)> = Vec::new();
    for t in &file.threads {
        let status = super::knowledge::derive_thread_status(t);
        if status != "planted" && status != "revealed" {
            continue;
        }
        let is_main = t.priority == "main";
        // 收集全部触点场景 → 最大章节序位
        let touch_scenes: Vec<&String> = t
            .planted_at
            .as_ref()
            .into_iter()
            .chain(t.revealed_at.as_ref())
            .chain(t.resolved_at.as_ref())
            .chain(
                t.knowledge
                    .iter()
                    .flat_map(|k| k.state_timeline.iter().map(|e| &e.scene)),
            )
            .collect();
        let last_touch = touch_scenes
            .iter()
            .filter_map(|s| scene_chapter_pos.get(s.as_str()).copied())
            .max();
        let Some(last_touch) = last_touch else {
            continue; // 锚点指向的场景不在结构里（坏引用另有编译诊断）
        };
        let dormant = frontier.saturating_sub(last_touch);
        let threshold = if is_main {
            STALE_MAIN_CHAPTERS
        } else {
            STALE_MINOR_CHAPTERS
        };
        if dormant >= threshold {
            warnings.push((
                dormant,
                is_main,
                format!(
                    "  - {}（{}·{status}·{}）最近动静在第 {} 章，已沉寂 {dormant} 章\n",
                    t.title,
                    t.id,
                    if is_main { "主线" } else { "支线" },
                    last_touch + 1,
                ),
            ));
        }
    }
    // 沉寂越久越靠前；同沉寂时主线优先
    warnings.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    warnings.into_iter().map(|(_, _, line)| line).collect()
}

/// 单行截断（压平换行 + 按字符数截断）
fn truncate_line(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max).collect();
        format!("{truncated}…")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ai_runtime::Tool;
    use story_project_format::init::init_project;

    fn setup() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    /// 双章结构：ch_01 已写完（completed），ch_02 待写（planned），
    /// 脉络/线索/断言/角色档案齐备
    fn write_fixture(dir: &std::path::Path) {
        // 角色档案（含习惯/个性——声线速查素材）
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            r#"id = "c_0001"
display_name = "林风"

[profile]
appearance = "白衣胜雪"
habits = "说话极简，惯用单字应答"
personality = "内敛寡言，外冷内热"
"#,
        )
        .unwrap();

        std::fs::write(
            dir.join("entities/characters/c_0002.toml"),
            r#"id = "c_0002"
display_name = "叶孤鸿"
aliases = ["叶老"]
"#,
        )
        .unwrap();

        let structure = r#"[book]
id = "b_root"
title = "测试"
chapters = ["ch_01", "ch_02"]

[chapter.ch_01]
title = "第一章 雨夜"
scenes = ["sc_01_01"]

[chapter.ch_02]
title = "第二章 码头"
scenes = ["sc_02_01"]

[scene.sc_01_01]
title = "客栈投宿"
pov = "c_0001"
summary = "林风投宿客栈，察觉有人跟踪。"
fragments = ["f_0001"]

[scene.sc_02_01]
title = "码头对峙"
pov = "c_0001"
function = "林风在码头狭路相逢，与跟踪者对峙"
fragments = []
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n雨夜，林风推开客栈的门。",
        )
        .unwrap();

        let chapters = r#"[[chapters]]
chapter_id = "ch_01"
title = "第一章 雨夜"
volume_order = 1
status = "completed"
hook = "黑衣人夜探客栈，林风浑然不觉"

[[chapters]]
chapter_id = "ch_02"
title = "第二章 码头"
volume_order = 1
status = "planned"
goal = "林风在码头与跟踪者对峙"
beats = ["夜探码头", "狭路相逢"]
hook = "跟踪者亮出信物"
characters = ["c_0001", "c_0002"]
threads = ["t_0001"]
"#;
        std::fs::write(dir.join("chapters.toml"), chapters).unwrap();

        let threads = r#"[[thread]]
id = "t_0001"
title = "跟踪者身份"
priority = "main"

[[thread.knowledge]]
observer = "reader"
state_timeline = [{ scene = "sc_01_01", state = "suspected" }]

[[thread.knowledge]]
observer = { character = "c_0001" }
state_timeline = [{ scene = "sc_01_01", state = "known" }]
"#;
        std::fs::write(dir.join("threads.toml"), threads).unwrap();

        let assertions = r#"[[assertion]]
id = "a_0001"
subject = "c_0001"
predicate = "位置"
object = "临安城码头"
"#;
        std::fs::write(dir.join("knowledge/assertions.toml"), assertions).unwrap();
    }

    #[test]
    fn prepare_by_scene_aggregates_all_dimensions() {
        let tmp = setup();
        write_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);

        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_02_01" }))
            .unwrap();

        // 章节标题行 + 状态
        assert!(out.contains("第二章 码头"), "{out}");
        assert!(out.contains("状态: planned"), "{out}");
        assert!(out.contains("目标场景: sc_02_01"), "{out}");
        // 脉络
        assert!(out.contains("目标: 林风在码头与跟踪者对峙"), "{out}");
        assert!(out.contains("1. 夜探码头"), "{out}");
        assert!(out.contains("章末钩子: 跟踪者亮出信物"), "{out}");
        // 前情：上一章已写场景的摘要
        assert!(out.contains("前情回顾"), "{out}");
        assert!(out.contains("客栈投宿"), "{out}");
        assert!(out.contains("林风投宿客栈"), "{out}");
        // 线索认知快照
        assert!(out.contains("跟踪者身份"), "{out}");
        assert!(out.contains("读者:怀疑"), "{out}");
        // 角色速查：习惯/个性（声线素材）都在
        assert!(out.contains("林风"), "{out}");
        assert!(out.contains("说话极简"), "{out}");
        assert!(out.contains("内敛寡言"), "{out}");
        assert!(out.contains("[POV]"), "{out}");
        // 角色当前状态断言
        assert!(out.contains("位置=临安城码头"), "{out}");
        // 尾注写作要点
        assert!(out.contains("[写作要点]"), "{out}");
    }

    #[test]
    fn prepare_by_chapter_index() {
        let tmp = setup();
        write_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);

        let out = tool.execute(&serde_json::json!({ "chapter": 2 })).unwrap();
        assert!(out.contains("第二章 码头"), "{out}");
        assert!(out.contains("1. 夜探码头"), "{out}");
        // 章节目标的前情以第一章末场景为界
        assert!(out.contains("客栈投宿"), "{out}");
        // 上章钩子承接：ch_01 已写且有钩子，准备 ch_02 时应带出
        assert!(out.contains("[上章钩子承接]"), "{out}");
        assert!(out.contains("黑衣人夜探客栈"), "{out}");

        // 越界序号报错
        let err = tool
            .execute(&serde_json::json!({ "chapter": 9 }))
            .unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }

    /// 第一章无上一章 → 无钩子承接提示
    #[test]
    fn prepare_no_prev_hook_for_first_chapter() {
        let tmp = setup();
        write_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);

        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_01_01" }))
            .unwrap();
        assert!(!out.contains("[上章钩子承接]"), "{out}");
    }

    /// 本章已有正文（开场已写）→ 承接时机已过，不再提示
    #[test]
    fn prepare_no_prev_hook_after_chapter_started() {
        let tmp = setup();
        write_fixture(tmp.path());
        // 给 ch_02 的场景落一个片段 → 本章已动笔
        std::fs::write(
            tmp.path().join("fragments/f_0009.md"),
            "---\nid: f_0009\n---\n码头夜色沉沉。",
        )
        .unwrap();
        let structure = std::fs::read_to_string(tmp.path().join("structure.toml"))
            .unwrap()
            .replace("fragments = []", "fragments = [\"f_0009\"]");
        std::fs::write(tmp.path().join("structure.toml"), structure).unwrap();

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);
        let out = tool.execute(&serde_json::json!({ "chapter": 2 })).unwrap();
        assert!(!out.contains("[上章钩子承接]"), "{out}");
    }

    /// 上一章未动笔（钩子还是规划态）→ 无从承接，不提示
    #[test]
    fn prepare_no_prev_hook_when_prev_unwritten() {
        let tmp = setup();
        write_fixture(tmp.path());
        // 抹掉 ch_01 的正文（片段列表清空）
        let structure = std::fs::read_to_string(tmp.path().join("structure.toml"))
            .unwrap()
            .replace("fragments = [\"f_0001\"]", "fragments = []");
        std::fs::write(tmp.path().join("structure.toml"), structure).unwrap();

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);
        let out = tool.execute(&serde_json::json!({ "chapter": 2 })).unwrap();
        assert!(!out.contains("[上章钩子承接]"), "{out}");
    }

    /// 场景级聚焦：节拍点名 + POV 的角色进「本幕在场上」档，
    /// 脉络名单其余角色收进「本章待命」档（只列称呼不铺档案）
    #[test]
    fn prepare_focuses_characters_by_scene() {
        let tmp = setup();
        write_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);

        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_02_01" }))
            .unwrap();
        // sc_02_01 的节拍只点名林风（又是 POV）→ 在场上；叶孤鸿进待命
        assert!(out.contains("涉及角色（本幕在场上"), "{out}");
        assert!(out.contains("本章待命角色"), "{out}");
        assert!(out.contains("叶孤鸿（c_0002）"), "{out}");
        // 待命角色只进一行名单，不应有速查列表行（档案行）
        assert!(!out.contains("- 叶孤鸿"), "{out}");

        // 章级目标（无 scene_id）没有节拍上下文 → 回退单一平铺名单
        let out = tool.execute(&serde_json::json!({ "chapter": 2 })).unwrap();
        assert!(out.contains("\n涉及角色:\n"), "{out}");
        assert!(!out.contains("本幕在场上"), "{out}");
    }

    /// 别名点名同样算在场：c_0002 的别名「叶老」出现在节拍里时，
    /// 两人都在场上，无待命档
    #[test]
    fn prepare_alias_mention_counts_as_on_stage() {
        let tmp = setup();
        write_fixture(tmp.path());
        let structure = std::fs::read_to_string(tmp.path().join("structure.toml"))
            .unwrap()
            .replace(
                "林风在码头狭路相逢，与跟踪者对峙",
                "叶老现身码头，林风狭路相逢",
            );
        std::fs::write(tmp.path().join("structure.toml"), structure).unwrap();

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_02_01" }))
            .unwrap();
        assert!(out.contains("\n涉及角色:\n"), "{out}");
        assert!(!out.contains("本章待命角色"), "{out}");
    }

    #[test]
    fn prepare_rejects_both_params_and_missing_scene() {
        let tmp = setup();
        write_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = PrepareWritingTool::new(ctx);

        let err = tool
            .execute(&serde_json::json!({ "scene_id": "sc_02_01", "chapter": 2 }))
            .unwrap_err();
        assert!(err.contains("只能传一个"), "{err}");

        let err = tool
            .execute(&serde_json::json!({ "scene_id": "sc_99_99" }))
            .unwrap_err();
        assert!(err.contains("不存在"), "{err}");
    }

    #[test]
    fn auto_advance_planned_to_writing_on_first_write() {
        let tmp = setup();
        write_fixture(tmp.path());
        let paths = story_project_format::ProjectPaths::new(tmp.path());

        // 首次推进：planned → writing
        let note = crate::mutations::auto_advance_outline_writing(&paths, "sc_02_01");
        assert!(note.is_some());
        let file = crate::mutations::load_chapters_file(&paths).unwrap();
        assert_eq!(file.chapters[1].status, "writing");

        // 幂等：writing 状态不再推进，completed 也不回退
        assert!(crate::mutations::auto_advance_outline_writing(&paths, "sc_02_01").is_none());
        assert!(crate::mutations::auto_advance_outline_writing(&paths, "sc_01_01").is_none());

        // 已写完的 ch_01 仍是 completed
        assert_eq!(file.chapters[0].status, "completed");
    }

    /// 12 章结构：第 1 章埋设的主线线索到第 12 章（沉寂 11 章 ≥ 阈值 10）预警；
    /// 支线同场景（沉寂 11 < 20）不警；最近才动过的主线不警
    #[test]
    fn prepare_warns_stale_threads() {
        let tmp = setup();
        let dir = tmp.path();

        // 12 章结构，每章一场景
        let mut structure = String::from("[book]\nid = \"b_root\"\ntitle = \"测试\"\nchapters = [");
        let chapter_ids: Vec<String> = (1..=12).map(|i| format!("ch_{i:02}")).collect();
        structure.push_str(
            &chapter_ids
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", "),
        );
        structure.push_str("]\n");
        for (i, ch) in chapter_ids.iter().enumerate() {
            structure.push_str(&format!(
                "\n[chapter.{ch}]\ntitle = \"第{}章\"\nscenes = [\"sc_{:02}\"]\n",
                i + 1,
                i + 1
            ));
        }
        for i in 1..=12 {
            structure.push_str(&format!(
                "\n[scene.sc_{i:02}]\ntitle = \"场景{i}\"\nfragments = []\n"
            ));
        }
        std::fs::write(dir.join("structure.toml"), structure).unwrap();

        let threads = r#"[[thread]]
id = "t_stale"
title = "旧玉佩"
priority = "main"
planted_at = "sc_01"

[[thread]]
id = "t_minor"
title = "次要传闻"
priority = "minor"
planted_at = "sc_01"

[[thread]]
id = "t_fresh"
title = "新变故"
priority = "main"
planted_at = "sc_11"
"#;
        std::fs::write(dir.join("threads.toml"), threads).unwrap();

        let ctx = ToolContext::new(dir.to_path_buf());
        let tool = PrepareWritingTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({ "scene_id": "sc_12" }))
            .unwrap();

        // 主线沉寂 11 章 → 预警
        assert!(out.contains("伏笔沉寂预警"), "{out}");
        assert!(out.contains("旧玉佩"), "{out}");
        assert!(out.contains("已沉寂 11 章"), "{out}");
        // 支线未到阈值、新线索刚动过 → 不出现
        assert!(!out.contains("次要传闻"), "{out}");
        assert!(!out.contains("新变故"), "{out}");
    }
}
