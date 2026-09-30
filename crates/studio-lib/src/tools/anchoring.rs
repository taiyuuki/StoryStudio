//! 写入时理解锚定 — 用 LLM 在片段写入后做指代消解，把"那个人""这个女人"
//! 这类文本匹配原理上认不到的出场落为显式锚点。
//!
//! 分工：
//! - 确定性匹配（名字/别名/派生分段）在解析层做，零成本即时；
//! - 本模块只补它的盲区：代词、称谓、身份描述。提示词明确要求
//!   "正文中出现全名或别名的无需标注"，两层互补不重复计数；
//! - 解析不了的指代（名册外的新人物）作为"未锚定指代"上报，
//!   不再静默丢失。
//!
//! Best-effort 语义：写入本身不因锚定失败而回滚。

use std::sync::Arc;

use story_ai_runtime::provider::{AiProvider, GenerateRequest, Message};
use story_ai_runtime::{Tool, ToolDef};
use story_project_format::{EntityFile, ProjectPaths};

use super::{get_str, get_str_opt, object_schema, ToolContext};

/// 名册上限：超出的实体（按 角色→地点→道具→组织 顺序截断）不参与本次锚定
const ROSTER_CAP: usize = 40;

/// 实体名册条目
struct RosterEntry {
    id: String,
    /// display_name（供数字型指代的防误锚判定与提示）
    name: String,
    line: String,
}

/// 构建候选实体名册（id + 名称/别名/档案要点），按类型排序后截断。
fn build_roster(paths: &ProjectPaths) -> Vec<RosterEntry> {
    let mut entries = Vec::new();
    for dir in [
        paths.characters_dir(),
        paths.locations_dir(),
        paths.items_dir(),
        paths.organizations_dir(),
    ] {
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.filter_map(|e| e.ok()) {
            let fname = f.file_name().to_string_lossy().to_string();
            if !fname.ends_with(".toml") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(f.path()) else {
                continue;
            };
            let Ok(entity) = EntityFile::from_toml_str(&content) else {
                continue;
            };
            let id = fname.trim_end_matches(".toml").to_string();
            let name = entity.display_name.clone();
            let mut line = format!("{id} {}（", entity.display_name);
            let mut parts: Vec<String> = Vec::new();
            if !entity.aliases.is_empty() {
                parts.push(format!("别名：{}", entity.aliases.join("/")));
            }
            if let Some(p) = &entity.profile {
                if let Some(o) = &p.occupation {
                    parts.push(o.clone());
                }
                if let Some(r) = &p.race {
                    parts.push(r.clone());
                }
            }
            if let Some(g) = &entity.geography {
                if let Some(k) = &g.kind {
                    parts.push(k.clone());
                }
            }
            line.push_str(&parts.join("，"));
            line.push('）');
            entries.push(RosterEntry { id, name, line });
        }
    }
    entries.truncate(ROSTER_CAP);
    entries
}

/// 理解锚定结果
pub struct AnchorOutcome {
    /// 已落锚：(实体 ID, 角色, 指代原文)
    pub anchored: Vec<(String, String, String)>,
    /// 名册外的指代：(原文, 模型给的提示)
    pub unresolved: Vec<(String, String)>,
}

/// 读取片段正文（剥 frontmatter）
fn fragment_body(paths: &ProjectPaths, fragment_id: &str) -> Result<String, String> {
    let path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| format!("无效的片段 ID: {fragment_id}"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取片段失败: {e}"))?;
    Ok(if let Some(rest) = content.strip_prefix("---") {
        rest.find("---")
            .map(|end| content[3 + end + 3..].trim_start_matches(['\r', '\n']).to_string())
            .unwrap_or(content.clone())
    } else {
        content.clone()
    })
}

/// 对单个片段执行理解锚定。
///
/// LLM 失败时返回 Err（调用方决定是否吞掉），片段内容不受影响。
pub fn comprehension_anchor(
    ctx: &ToolContext,
    provider: &Arc<dyn AiProvider>,
    fragment_id: &str,
) -> Result<AnchorOutcome, String> {
    let paths = ctx.paths();

    // 读正文（剥 frontmatter）
    let body = fragment_body(&paths, fragment_id)?;
    if body.trim().is_empty() {
        return Ok(AnchorOutcome { anchored: Vec::new(), unresolved: Vec::new() });
    }

    let roster = build_roster(&paths);
    if roster.is_empty() {
        return Ok(AnchorOutcome { anchored: Vec::new(), unresolved: Vec::new() });
    }
    let roster_text: String = roster.iter().map(|e| e.line.as_str()).collect::<Vec<_>>().join("\n");

    let system = "你是小说文本的实体出场标注器。给定正文片段和实体名册，\
找出正文中**以指代方式**出现的实体——代词（他/她/那人）、称谓（馆长/判官）、\
身份描述（那名佣兵/穿灰袍的女人）。\n\
规则：\n1. 正文原样出现名册中的全名或别名的，不要标注（系统已自动识别）；\n\
2. 只标注指代消解后能确定身份的；拿不准就不标；\n\
3. 纯数字/编号型指代（如\"十七号\"\"三楼\"\"丙-17\"）极易与门牌、档案编号、\
楼层等巧合撞名，一律放入 unresolved（hint 说明疑似指向），不要直接锚定；\n\
4. 指代对象在名册中找不到的，放入 unresolved 并给一句提示（如\"疑似新角色\"）。\n\
只输出 JSON，格式：\n\
{\"anchors\": [{\"entity\": \"c_0008\", \"role\": \"referenced\", \"quote\": \"那名佣兵\"}],\
 \"unresolved\": [{\"quote\": \"高个子\", \"hint\": \"疑似新角色\"}]}\n\
role 取值：appears（指代对象在场有言行）/ mentioned（被谈起）/ referenced（被指代，\
大多数代词用这个）。";

    let user = format!("实体名册：\n{roster_text}\n\n正文片段 {fragment_id}：\n\n{body}");

    let request = GenerateRequest {
        system: system.to_string(),
        messages: vec![Message::user(user)],
        model: String::new(),
        temperature: 0.1,
        // 思考型模型的 reasoning 计入配额；输出 JSON 本身不大，
        // 上限只需容纳思考 + 完整 JSON（档案级「输出上限」可覆盖）
        max_tokens: 65536,
        tools: Vec::new(),
    };

    let response = super::retry_generate(provider, &request)
        .map_err(|e| format!("锚定调用失败: {e}"))?;

    let mut outcome = parse_anchor_response(&response.text, &roster)?;
    downgrade_numeric_anchors(&mut outcome, &roster);
    Ok(outcome)
}

/// 数字型指代防误锚（程序化兜底，提示词之外的第二道闸）：
/// 模型把「十七号」这类纯数字指代锚到名册实体——尤其当实体名称里恰好
/// 含该数字串（门牌「槐安路十七号」）时——几乎总是上下文巧合而非确定
/// 指代。这类锚一律降级为未锚定指代，进台账人工消歧确认，不直接落锚
/// 污染出场记录；确认归拢后目标实体的锚点会自动并入（见 anchor_and_apply）。
fn downgrade_numeric_anchors(outcome: &mut AnchorOutcome, roster: &[RosterEntry]) {
    let mut downgraded: Vec<(String, String)> = Vec::new();
    outcome.anchored.retain(|(id, _role, quote)| {
        let q = quote.trim();
        if q.is_empty() || !story_parser::is_number_like(q) {
            return true;
        }
        let name = roster
            .iter()
            .find(|e| e.id == *id)
            .map(|e| e.name.as_str())
            .unwrap_or(id);
        downgraded.push((
            q.to_string(),
            format!(
                "数字/编号型指代，模型疑似锚到「{name}」——可能是门牌、档案编号等巧合撞名，请人工确认"
            ),
        ));
        false
    });
    outcome.unresolved.extend(downgraded);
}

/// 宽松解析模型的锚定输出（复用 design_chapters 的容错思路）
fn parse_anchor_response(text: &str, roster: &[RosterEntry]) -> Result<AnchorOutcome, String> {
    use super::chapters::parse_json_array_lenient;

    let mut outcome = AnchorOutcome { anchored: Vec::new(), unresolved: Vec::new() };

    // 模型可能输出对象 {"anchors": [...], "unresolved": [...]}，也可能直接输出数组
    let trimmed = text.trim();
    let anchors_json: Vec<serde_json::Value> = if let Ok(v) =
        serde_json::from_str::<serde_json::Value>(trimmed)
    {
        if let Some(arr) = v.get("anchors").and_then(|a| a.as_array()) {
            arr.clone()
        } else if v.is_array() {
            v.as_array().unwrap().clone()
        } else {
            return Ok(outcome);
        }
    } else {
        // 剥围栏/截取后重试
        let t = trimmed
            .trim_start_matches("```")
            .trim_start_matches("json")
            .trim();
        let t = t.strip_suffix("```").unwrap_or(t);
        match serde_json::from_str::<serde_json::Value>(t.trim()) {
            Ok(v) => match v.get("anchors").and_then(|a| a.as_array()) {
                Some(arr) => arr.clone(),
                None => return Ok(outcome),
            },
            Err(_) => {
                // 数组形态兜底
                let _ = parse_json_array_lenient(trimmed, "anchors");
                return Ok(outcome);
            }
        }
    };

    let valid_ids: std::collections::HashSet<&str> =
        roster.iter().map(|e| e.id.as_str()).collect();

    for item in &anchors_json {
        let entity = item.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        if !valid_ids.contains(entity) {
            continue;
        }
        let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("referenced");
        let quote = item.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        if outcome.anchored.iter().any(|(id, _, _)| id == entity) {
            continue;
        }
        outcome.anchored.push((entity.to_string(), role.to_string(), quote.to_string()));
    }

    // unresolved（对象形态才有）
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(arr) = v.get("unresolved").and_then(|a| a.as_array()) {
            for item in arr {
                let quote = item.get("quote").and_then(|x| x.as_str()).unwrap_or("");
                let hint = item.get("hint").and_then(|x| x.as_str()).unwrap_or("");
                if !quote.is_empty() {
                    outcome.unresolved.push((quote.to_string(), hint.to_string()));
                }
            }
        }
    }

    Ok(outcome)
}

/// 执行锚定并落盘（合并 helper：供 write/append 工具与批量工具共用）
///
/// 除 LLM 锚点外还会：
/// 1. 把本次上报的未锚定指代同步进 anchoring.toml 台账（含改稿清理），
///    回执中的 unresolved 替换为**仍待处置**的指代——已忽略/已归拢的不再提示；
/// 2. 已归拢（resolved 且有 target）且 quote 仍在正文中的记录，
///    把目标实体并入锚点集，处置真正生效到出场索引。
pub fn anchor_and_apply(
    ctx: &ToolContext,
    provider: &Arc<dyn AiProvider>,
    fragment_id: &str,
) -> Result<AnchorOutcome, String> {
    let paths = ctx.paths();
    let mut outcome = comprehension_anchor(ctx, provider, fragment_id)?;
    let body = fragment_body(&paths, fragment_id)?;

    // 台账同步（best-effort：失败不阻断锚定）
    let pending = crate::mutations::sync_unresolved_refs(&paths, fragment_id, &body, &outcome.unresolved)
        .unwrap_or_default();
    outcome.unresolved = pending;

    // 已归拢目标的锚点并入 LLM 权威集（按实体去重，LLM 的 role 优先）
    let mut pairs: Vec<(String, String)> = outcome
        .anchored
        .iter()
        .map(|(id, role, _)| (id.clone(), role.clone()))
        .collect();
    for r in crate::mutations::load_unresolved_refs(&paths).unwrap_or_default() {
        if r.fragment_id == fragment_id
            && r.status == "resolved"
            && body.contains(&r.quote)
        {
            if let Some(target) = &r.target {
                if !pairs.iter().any(|(id, _)| id == target) {
                    pairs.push((target.clone(), "referenced".to_string()));
                }
            }
        }
    }

    if !pairs.is_empty() {
        crate::mutations::replace_fragment_anchors(&paths, fragment_id, &pairs)
            .map_err(|e| e.to_string())?;
    }
    Ok(outcome)
}

/// 把锚定结果格式化为回执片段（无内容返回空串）
pub fn format_anchor_note(outcome: &AnchorOutcome, roster_paths: &ProjectPaths) -> String {
    let _ = roster_paths;
    let mut note = String::new();
    if !outcome.anchored.is_empty() {
        note.push_str("\n理解锚定（指代出场已落锚点）：");
        for (id, role, quote) in &outcome.anchored {
            note.push_str(&format!("\n  - {id}（{role}：\"{quote}\"）"));
        }
    }
    if !outcome.unresolved.is_empty() {
        note.push_str("\n未锚定指代（名册中无匹配，待处置）：");
        for (quote, hint) in &outcome.unresolved {
            note.push_str(&format!("\n  - \"{quote}\"（{hint}）"));
        }
        note.push_str(
            "\n处置方式：resolve_unresolved_ref 建档（匿名指代可用占位名如「九一真凶」）、\
             归拢到已有实体或忽略；已处置的后续不再提示。",
        );
    }
    note
}

// ============================================================================
// anchor_fragments 工具 — 单片段 / 全书批量重新锚定
// ============================================================================

pub struct AnchorFragmentsTool {
    ctx: ToolContext,
}
impl AnchorFragmentsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for AnchorFragmentsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "anchor_fragments".into(),
            description: "对片段执行理解锚定：用 AI 做指代消解，把\"那人\"\"佣兵\"这类\
文字匹配认不到的出场落为显式锚点（名字/别名已自动识别，无需重跑）。\
省略 fragment_id 则批量处理全书（每次最多 60 个片段，超出请分章执行）。\
导入旧书稿后跑一遍即可建立完整出场索引。".into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "片段 ID（省略则处理全部片段）",
                false,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let provider = self
            .ctx
            .provider
            .as_ref()
            .ok_or_else(|| "anchor_fragments 需要 provider，当前未配置".to_string())?;

        let mut summary = String::new();
        if let Some(fid) = get_str_opt(arguments, "fragment_id") {
            let outcome = anchor_and_apply(&self.ctx, provider, &fid)?;
            summary.push_str(&format!(
                "片段 {fid}：{}",
                if outcome.anchored.is_empty() && outcome.unresolved.is_empty() {
                    "无需指代锚定".to_string()
                } else {
                    format_anchor_note(&outcome, &self.ctx.paths()).trim().to_string()
                }
            ));
            return Ok(summary);
        }

        // 全书批量
        let paths = self.ctx.paths();
        let mut names: Vec<String> = std::fs::read_dir(paths.fragments_dir())
            .map_err(|e| format!("读取片段目录失败: {e}"))?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md"))
            .map(|n| n.trim_end_matches(".md").to_string())
            .collect();
        names.sort();

        const BATCH_CAP: usize = 60;
        let total = names.len();
        if total > BATCH_CAP {
            names.truncate(BATCH_CAP);
        }

        let mut anchored_count = 0usize;
        let mut pending_count = 0usize;
        let mut failed: Vec<String> = Vec::new();
        for fid in &names {
            match anchor_and_apply(&self.ctx, provider, fid) {
                Ok(o) => {
                    anchored_count += o.anchored.len();
                    pending_count += o.unresolved.len();
                }
                Err(e) => failed.push(format!("{fid}: {e}")),
            }
        }

        let mut out = format!(
            "批量锚定完成：处理 {} 个片段（共 {total} 个{}），落锚 {anchored_count} 处，待处置指代 {pending_count} 条（list_unresolved_refs 可查看）",
            names.len(),
            if total > BATCH_CAP { "，达单次上限，请分批" } else { "" },
        );
        if !failed.is_empty() {
            out.push_str(&format!("\n失败 {} 个：{}", failed.len(), failed.join("；")));
        }
        Ok(out)
    }
}

// ============================================================================
// 未锚定指代处置工具 — list_unresolved_refs / resolve_unresolved_ref
// ============================================================================

pub struct ListUnresolvedRefsTool {
    ctx: ToolContext,
}
impl ListUnresolvedRefsTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for ListUnresolvedRefsTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "list_unresolved_refs".into(),
            description: "列出未锚定指代台账：待处置的名册外指代（如\"摆的人\"\"写的人\"），\
            以及已忽略/已归拢的处置记录。省略 fragment_id 则列出全部。\
            待处置项用 resolve_unresolved_ref 处置。".into(),
            parameters: object_schema(vec![(
                "fragment_id",
                "string",
                "片段 ID（省略则列出全部）",
                false,
            )]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fid = get_str_opt(arguments, "fragment_id");
        let refs = crate::mutations::load_unresolved_refs(&self.ctx.paths())
            .map_err(|e| e.to_string())?;
        let selected: Vec<_> = refs
            .iter()
            .filter(|r| fid.as_deref().is_none_or(|f| r.fragment_id == f))
            .collect();

        if selected.is_empty() {
            return Ok("未锚定指代台账为空。".to_string());
        }

        let pending: Vec<_> = selected.iter().filter(|r| r.is_pending()).collect();
        let mut out = format!("共 {} 条记录，待处置 {} 条：", selected.len(), pending.len());
        for r in &selected {
            let status = match r.status.as_str() {
                "ignored" => "已忽略".to_string(),
                "resolved" => match &r.target {
                    Some(t) => format!("已归拢到 {t}"),
                    None => "已归拢".to_string(),
                },
                _ => "待处置".to_string(),
            };
            out.push_str(&format!(
                "\n  - {}「{}」（{}）：{}",
                r.fragment_id, r.quote, r.hint, status
            ));
        }
        Ok(out)
    }
}

pub struct ResolveUnresolvedRefTool {
    ctx: ToolContext,
}
impl ResolveUnresolvedRefTool {
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

impl Tool for ResolveUnresolvedRefTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "resolve_unresolved_ref".into(),
            description: "处置一条未锚定指代（先可用 list_unresolved_refs 查看待处置清单）：\
            action=create 为其新建档案（匿名指代用占位名，如悬疑里的凶手指代\
            \"摆的人/写的人\"可建档为「九一真凶」，真名揭晓后 merge_entities 合并）；\
            action=merge 归拢到已有实体（需 target_entity_id）；\
            action=ignore 忽略，后续锚定不再提示。\
            建档/归拢会自动为所在片段补锚点。".into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "指代所在片段 ID", true),
                ("quote", "string", "指代原文（台账中的 quote，需完全一致）", true),
                ("action", "string", "处置动作：create | merge | ignore", true),
                (
                    "target_entity_id",
                    "string",
                    "merge 时归拢到的实体 ID",
                    false,
                ),
                ("name", "string", "create 时新档案的名称（可用占位名）", false),
                (
                    "kind",
                    "string",
                    "create 时的实体类型：characters（默认）/ locations / items / organizations",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fragment_id = get_str(arguments, "fragment_id")?;
        let quote = get_str(arguments, "quote")?;
        let action = get_str(arguments, "action")?;
        let summary = crate::mutations::resolve_unresolved_ref(
            &self.ctx.paths(),
            &fragment_id,
            &quote,
            &action,
            get_str_opt(arguments, "target_entity_id").as_deref(),
            get_str_opt(arguments, "name").as_deref(),
            get_str_opt(arguments, "kind").as_deref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;
    use story_ai_runtime::provider::{AiError, GenerateRequest, GenerateResponse, StopReason, TokenUsage};
    use story_project_format::init::init_project;

    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    #[derive(Debug)]
    struct MockAnchorProvider {
        response: String,
    }
    impl AiProvider for MockAnchorProvider {
        fn id(&self) -> &str {
            "mock-anchor"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: self.response.clone(),
                usage: TokenUsage { input_tokens: 10, output_tokens: 20 },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    #[test]
    fn comprehension_anchor_resolves_descriptor_mentions() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 建角色档案（佣兵——正文只用身份指代）
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"洛恩·巴克尔\"\naliases = [\"洛恩\"]\n\n[profile]\noccupation = \"佣兵\"\n",
        )
        .unwrap();

        // 建章节/场景/片段
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(
            &paths,
            &sc,
            "那名佣兵靠在门边，一言不发。门口还站着一个穿灰袍的女人。后来他开口了。",
        )
        .unwrap();

        // mock 返回：指代锚定 + 一条未锚定指代
        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [{\"entity\": \"c_0001\", \"role\": \"appears\", \"quote\": \"那名佣兵\"}], \"unresolved\": [{\"quote\": \"穿灰袍的女人\", \"hint\": \"疑似新角色\"}]}".to_string(),
        });

        let ctx = ToolContext::new(dir.to_path_buf());
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();

        assert_eq!(outcome.anchored.len(), 1);
        assert_eq!(outcome.anchored[0].0, "c_0001");
        assert_eq!(outcome.unresolved.len(), 1);

        // 锚点已落盘
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert!(content.contains("entity: c_0001"), "{content}");
        assert!(content.contains("role: appears"), "{content}");

        // 回执文案
        let note = format_anchor_note(&outcome, &paths);
        assert!(note.contains("那名佣兵"), "{note}");
        assert!(note.contains("穿灰袍的女人"), "{note}");

        // 台账已记录 pending
        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].quote, "穿灰袍的女人");
        assert!(refs[0].is_pending());
    }

    #[test]
    fn resolved_ref_not_reprompted_and_target_anchored() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"沈屹\"\n\n[profile]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(
            &paths,
            &sc,
            "摆的人懂行，知道我们先看什么。沈屹蹲下身。",
        )
        .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"摆的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());

        // 第一次锚定：上报并记录 pending
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert_eq!(outcome.unresolved.len(), 1);

        // 处置：建档为匿名占位实体
        let summary = crate::mutations::resolve_unresolved_ref(
            &paths,
            &fid,
            "摆的人",
            "create",
            None,
            Some("九一真凶"),
            Some("characters"),
        )
        .unwrap();
        assert!(summary.contains("九一真凶"), "{summary}");

        // 再次锚定（LLM 重复上报同一条）：不再出现在回执 pending 里
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert!(outcome.unresolved.is_empty(), "{:?}", outcome.unresolved);

        // 台账状态 resolved 且带目标；目标实体的锚点已并入片段
        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs[0].status, "resolved");
        let target = refs[0].target.clone().unwrap();
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert!(content.contains(&format!("entity: {target}")), "{content}");
    }

    #[test]
    fn ignored_ref_not_reprompted() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"沈屹\"\n\n[profile]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "写的人不慌。").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"写的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        anchor_and_apply(&ctx, &provider, &fid).unwrap();

        crate::mutations::resolve_unresolved_ref(&paths, &fid, "写的人", "ignore", None, None, None)
            .unwrap();

        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert!(outcome.unresolved.is_empty());
    }

    #[test]
    fn stale_ref_cleaned_when_quote_removed_from_body() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"沈屹\"\n\n[profile]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "摆的人懂行。").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"摆的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert_eq!(crate::mutations::load_unresolved_refs(&paths).unwrap().len(), 1);

        // 改稿：正文不再包含该指代（重写片段），重新锚定后台账自动清理
        crate::mutations::write_fragment_text(&paths, &fid, "现场已经清理干净。").unwrap();
        let provider_clean: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": []}".to_string(),
        });
        anchor_and_apply(&ctx, &provider_clean, &fid).unwrap();
        assert!(crate::mutations::load_unresolved_refs(&paths).unwrap().is_empty());
    }

    #[test]
    fn comprehension_anchor_rejects_unknown_entity() {
        // 名册外的 entity 直接被过滤
        let roster_outcome = crate::tools::anchoring::tests_support::parse_with_empty_roster(
            "{\"anchors\": [{\"entity\": \"c_9999\", \"role\": \"appears\", \"quote\": \"x\"}]}",
        );
        assert!(roster_outcome.anchored.is_empty());
    }

    #[test]
    fn numeric_reference_anchor_downgraded_to_unresolved() {
        // 场景：医生口中的红档编号"十七号"被模型锚到地点「槐安路十七号·档案楼」
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/locations")).unwrap();
        std::fs::write(
            dir.join("entities/locations/l_0003.toml"),
            "id = \"l_0003\"\ndisplay_name = \"槐安路十七号·档案楼\"\n\n[geography]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(
            &paths,
            &sc,
            "医生说：什么十七号。他记了下来。",
        )
        .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [{\"entity\": \"l_0003\", \"role\": \"appears\", \"quote\": \"十七号\"}]}".to_string(),
        });

        let ctx = ToolContext::new(dir.to_path_buf());
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();

        // 不直接落锚：数字型指代降级为待人工确认
        assert!(outcome.anchored.is_empty(), "{:?}", outcome.anchored);
        assert_eq!(outcome.unresolved.len(), 1);
        assert_eq!(outcome.unresolved[0].0, "十七号");
        assert!(
            outcome.unresolved[0].1.contains("槐安路十七号·档案楼"),
            "提示应点名疑似实体: {:?}",
            outcome.unresolved[0].1
        );

        // 片段 frontmatter 未写入该锚点
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert!(!content.contains("entity: l_0003"), "{content}");

        // 台账已记为待处置（消歧确认走 resolve_unresolved_ref）
        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].quote, "十七号");
        assert!(refs[0].is_pending());
    }

    #[test]
    fn non_numeric_reference_anchor_unaffected() {
        // 普通指代（身份描述）不受数字型降级影响
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"顾岩\"\n\n[profile]\noccupation = \"守夜人\"\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "那名守夜人点了点头。").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [{\"entity\": \"c_0001\", \"role\": \"appears\", \"quote\": \"那名守夜人\"}]}".to_string(),
        });

        let ctx = ToolContext::new(dir.to_path_buf());
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert_eq!(outcome.anchored.len(), 1);
        assert_eq!(outcome.anchored[0].0, "c_0001");
    }
}

/// 测试支撑（crate 内可见）
pub mod tests_support {
    use super::*;

    pub fn parse_with_empty_roster(text: &str) -> AnchorOutcome {
        parse_anchor_response(text, &[]).unwrap()
    }
}
