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

use std::collections::HashMap;
use std::sync::Arc;

use story_ai_runtime::provider::{AiProvider, GenerateRequest, Message};
use story_ai_runtime::{Tool, ToolDef};
use story_project_format::{EntityFile, FragmentFile, ProjectPaths, StructureFile};

use super::{get_str, get_str_opt, get_u32_opt, object_schema, ToolContext};

/// 名册上限：超出的实体不参与本次锚定
const ROSTER_CAP: usize = 40;

/// 实体名册条目
struct RosterEntry {
    id: String,
    /// display_name（供数字型指代的防误锚判定与提示）
    name: String,
    line: String,
}

/// 本场景各片段 frontmatter 已锚定的实体 ID 集合。
///
/// 名册上限之外的排序依据：场景里已出场/被提及的实体对指代消解最相关，
/// 优先入册——否则长篇后期角色超过上限时，地点和道具永远排不进名册，
/// 在锚定里"隐形"。
fn scene_anchor_entities(
    paths: &ProjectPaths,
    fragment_id: &str,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(structure) = std::fs::read_to_string(paths.structure_toml())
        .ok()
        .and_then(|c| StructureFile::from_toml_str(&c).ok())
    else {
        return out;
    };
    let Some((_sid, scene)) = structure
        .scene
        .iter()
        .find(|(_, sc)| sc.fragments.iter().any(|f| f == fragment_id))
    else {
        return out;
    };
    for fid in &scene.fragments {
        let Some(path) = paths.file_for_id(fid) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(file) = FragmentFile::parse(&content) else {
            continue;
        };
        for a in file.frontmatter.anchors {
            out.insert(a.entity);
        }
    }
    out
}

/// 构建候选实体名册（id + 名称/别名/档案要点）。
/// `focus` 中的实体优先（场景相关），其余按 角色→地点→道具→组织 排序补足，
/// 合计截断到 [`ROSTER_CAP`]。
fn build_roster(
    paths: &ProjectPaths,
    focus: &std::collections::HashSet<String>,
) -> Vec<RosterEntry> {
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
    // 场景相关实体优先（保持原有类型顺序），其余按原顺序补足
    entries.sort_by_key(|e| !focus.contains(&e.id));
    entries.truncate(ROSTER_CAP);
    entries
}

/// 理解锚定结果
#[derive(Default)]
pub struct AnchorOutcome {
    /// 已落锚：(实体 ID, 角色, 指代原文)
    pub anchored: Vec<(String, String, String)>,
    /// 名册外的指代：(原文, 模型给的提示)
    pub unresolved: Vec<(String, String)>,
}

/// 写入类工具的锚定时限。LLM 慢（含重试退避）时先行返回回执，锚定转后台完成。
///
/// MCP 客户端普遍有 30 秒左右的调用时限，超时即向调用方报错——而服务端
/// 仍会执行完；调用方误以为失败而重试会造成正文重复追加等假失败危害。
/// 锚定只是写入回执的增强信息（正文已先行落盘），不值得把整个写工具
/// 拖过时限。
pub const ANCHOR_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// 带时限锚定的执行结果
pub enum AnchorRun {
    /// 时限内完成（锚点已落盘，结果用于回执）
    Done(AnchorOutcome),
    /// 超过时限：锚定移入后台线程继续（含台账与锚点落盘），完成后自动生效
    Deferred,
}

/// [`anchor_and_apply`] 的带时限版本，供写入类工具（write/append/split）使用。
///
/// 无 provider 时直接返回空结果（与调用方原有语义一致）。超时后工作线程
/// detach 继续跑完落盘——调用方回执应提示「已保存、勿重试、稍后查看」。
pub fn anchor_and_apply_bounded(ctx: &ToolContext, fragment_id: &str) -> Result<AnchorRun, String> {
    anchor_and_apply_with_budget(ctx, fragment_id, ANCHOR_BUDGET)
}

/// 可指定时限的实现（测试用小预算触发 Deferred 路径）
pub(crate) fn anchor_and_apply_with_budget(
    ctx: &ToolContext,
    fragment_id: &str,
    budget: std::time::Duration,
) -> Result<AnchorRun, String> {
    let Some(provider) = ctx.provider.clone() else {
        return Ok(AnchorRun::Done(AnchorOutcome::default()));
    };
    let fid = fragment_id.to_string();
    let worker_ctx = ctx.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("story-anchor-bounded".into())
        .spawn(move || {
            let res = anchor_and_apply(&worker_ctx, &provider, &fid);
            // 接收端已放弃（超时先行返回）：锚点仍会落盘，仅回执不可见
            let err_note = res.as_ref().err().cloned();
            if tx.send(res).is_err() {
                match err_note {
                    Some(e) => eprintln!("[tool] 后台锚定失败（调用方已先行返回）: {e}"),
                    None => eprintln!("[tool] 后台锚定已完成（调用方已先行返回）: {fid}"),
                }
            }
        })
        .map_err(|e| format!("创建锚定线程失败：{e}"))?;
    match rx.recv_timeout(budget) {
        Ok(res) => res.map(AnchorRun::Done),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            drop(worker); // detach：后台继续完成台账与锚点落盘
            Ok(AnchorRun::Deferred)
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("锚定线程异常退出（正文不受影响，可稍后用 anchor_fragments 补锚）".to_string())
        }
    }
}

/// 读取片段正文（剥 frontmatter）
fn fragment_body(paths: &ProjectPaths, fragment_id: &str) -> Result<String, String> {
    let path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| format!("无效的片段 ID: {fragment_id}"))?;
    let content = std::fs::read_to_string(&path).map_err(|e| format!("读取片段失败: {e}"))?;
    Ok(if let Some(rest) = content.strip_prefix("---") {
        rest.find("---")
            .map(|end| {
                content[3 + end + 3..]
                    .trim_start_matches(['\r', '\n'])
                    .to_string()
            })
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
        return Ok(AnchorOutcome {
            anchored: Vec::new(),
            unresolved: Vec::new(),
        });
    }

    let roster = build_roster(&paths, &scene_anchor_entities(&paths, fragment_id));
    if roster.is_empty() {
        return Ok(AnchorOutcome {
            anchored: Vec::new(),
            unresolved: Vec::new(),
        });
    }
    let roster_text: String = roster
        .iter()
        .map(|e| e.line.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let system = "你是小说文本的实体出场标注器。给定正文片段和实体名册，\
找出正文中**以指代方式**出现的实体——代词（他/她/那人）、称谓（馆长/判官）、\
身份描述（那名佣兵/穿灰袍的女人）。\n\
规则：\n1. 正文原样出现名册中的全名或别名的，不要标注（系统已自动识别）；\n\
2. 只标注指代消解后能确定身份的；拿不准就不标；\n\
3. 纯数字/编号型指代（如\"十七号\"\"三楼\"\"丙-17\"）极易与门牌、档案编号、\
楼层等巧合撞名，一律放入 unresolved（hint 说明疑似指向），不要直接锚定；\n\
4. 指代对象在名册中找不到的，放入 unresolved 并给一句提示（如\"疑似新角色\"）；\n\
5. 非特指的泛指（有人/某人/谁/不知名者/众人等没有具体指代对象的文学性表达）\
不算指代，既不锚定也不上报；\n\
6. 数量词+群体称谓（如\"六个女弟子\"\"几名守卫\"）视为背景群演，不上报；\
除非其中某个体被单独点名或有独立言行，才只针对该个体处理。\n\
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

    let response =
        super::retry_generate(provider, &request).map_err(|e| format!("锚定调用失败: {e}"))?;
    // 截断的锚定 JSON 会整体解析失败；提前报错让调用方调参重试
    crate::llm_json::ensure_not_truncated(response.stop_reason, 65536)
        .map_err(|e| format!("锚定结果{e}"))?;

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

/// 宽松解析模型的锚定输出（统一走 llm_json 容错梯度）
///
/// 模型可能输出对象 `{"anchors": [...], "unresolved": [...]}`，也可能直接
/// 输出数组。解析失败返回 Err 让调用方重试，不再静默返回空结果——
/// 空结果会被上层当作"无指代可锚"跳过落锚，出场记录就此缺档。
fn parse_anchor_response(text: &str, roster: &[RosterEntry]) -> Result<AnchorOutcome, String> {
    let value = crate::llm_json::parse_value(text).map_err(|e| {
        format!("锚定结果解析失败（{e}）。请原样重试一次；反复失败建议换更稳定的模型")
    })?;

    let anchors_json: Vec<serde_json::Value> = match &value {
        serde_json::Value::Array(arr) => arr.clone(),
        serde_json::Value::Object(_) => value
            .get("anchors")
            .and_then(|a| a.as_array())
            .cloned()
            .ok_or_else(|| {
                "锚定结果对象缺少 anchors 字段（应为 {\"anchors\":[...],\"unresolved\":[...]}）。请重试"
                    .to_string()
            })?,
        _ => {
            return Err(
                "锚定结果顶层既不是对象也不是数组，格式不符预期。请重试".to_string()
            )
        }
    };

    let mut outcome = AnchorOutcome {
        anchored: Vec::new(),
        unresolved: Vec::new(),
    };

    let valid_ids: std::collections::HashSet<&str> = roster.iter().map(|e| e.id.as_str()).collect();

    for item in &anchors_json {
        let entity = item.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        if !valid_ids.contains(entity) {
            continue;
        }
        let role = item
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("referenced");
        let quote = item.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        if outcome.anchored.iter().any(|(id, _, _)| id == entity) {
            continue;
        }
        outcome
            .anchored
            .push((entity.to_string(), role.to_string(), quote.to_string()));
    }

    // unresolved（对象形态才有）
    if let Some(arr) = value.get("unresolved").and_then(|a| a.as_array()) {
        for item in arr {
            let quote = item.get("quote").and_then(|x| x.as_str()).unwrap_or("");
            let hint = item.get("hint").and_then(|x| x.as_str()).unwrap_or("");
            if !quote.is_empty() {
                outcome
                    .unresolved
                    .push((quote.to_string(), hint.to_string()));
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
    let pending =
        crate::mutations::sync_unresolved_refs(&paths, fragment_id, &body, &outcome.unresolved)
            .unwrap_or_default();
    outcome.unresolved = pending;

    // 已归拢目标的锚点并入 LLM 权威集（按实体去重，LLM 的 role 优先）
    let mut pairs: Vec<(String, String)> = outcome
        .anchored
        .iter()
        .map(|(id, role, _)| (id.clone(), role.clone()))
        .collect();
    for r in crate::mutations::load_unresolved_refs(&paths).unwrap_or_default() {
        if r.fragment_id == fragment_id && r.status == "resolved" && body.contains(&r.quote) {
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

/// 单片段理解锚定（合并语义）：编排与 [`anchor_and_apply`] 相同，但落盘用
/// `set_fragment_anchors` 合并——已有显式锚点（手动登记 / 手动修正的 role）
/// 保留，只追加或按 LLM 最新判定更新 role。
///
/// 供前端「AI 锚定本段」手动触发：用户改稿后按新正文补锚，不应清掉自己
/// 刚修好的锚点；误锚的清理走手动删除。AI 批量路径（anchor_fragments 工具）
/// 维持权威集替换语义——导入书稿建档场景下旧锚点本就是机器产物。
pub fn anchor_fragment_merge(
    ctx: &ToolContext,
    provider: &Arc<dyn AiProvider>,
    fragment_id: &str,
) -> Result<AnchorOutcome, String> {
    let paths = ctx.paths();
    let mut outcome = comprehension_anchor(ctx, provider, fragment_id)?;
    let body = fragment_body(&paths, fragment_id)?;

    // 台账同步（best-effort：失败不阻断锚定），与 anchor_and_apply 一致
    let pending =
        crate::mutations::sync_unresolved_refs(&paths, fragment_id, &body, &outcome.unresolved)
            .unwrap_or_default();
    outcome.unresolved = pending;

    // 已归拢目标的锚点并入（按实体去重，LLM 的 role 优先）
    let mut pairs: Vec<(String, String)> = outcome
        .anchored
        .iter()
        .map(|(id, role, _)| (id.clone(), role.clone()))
        .collect();
    for r in crate::mutations::load_unresolved_refs(&paths).unwrap_or_default() {
        if r.fragment_id == fragment_id && r.status == "resolved" && body.contains(&r.quote) {
            if let Some(target) = &r.target {
                if !pairs.iter().any(|(id, _)| id == target) {
                    pairs.push((target.clone(), "referenced".to_string()));
                }
            }
        }
    }

    if !pairs.is_empty() {
        crate::mutations::set_fragment_anchors(&paths, fragment_id, &pairs)
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
             归拢到已有实体或忽略；已处置的后续不再提示。\
             多条指代若指向同一实体，可在一次调用中用 refs 批量处置。\n\
             💡 若是已稳定出场、真名未揭的代称角色（如「刻印匠」），更优路径是\
             create_character 传 placeholder 建档（正文继续用代称），\
             真名揭露当章调 reveal_character_name 切换。",
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
导入旧书稿后跑一遍即可建立完整出场索引。"
                .into(),
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
                    format_anchor_note(&outcome, &self.ctx.paths())
                        .trim()
                        .to_string()
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
            out.push_str(&format!(
                "\n失败 {} 个：{}",
                failed.len(),
                failed.join("；")
            ));
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
            以及已忽略/已归拢的处置记录。台账随篇幅累积，中后期建议用 chapter_id /\
            volume 过滤只看相关范围；一次性龙套（背景人物泛指）可批量 ignore。\
            待处置项用 resolve_unresolved_ref 处置。"
                .into(),
            parameters: object_schema(vec![
                (
                    "fragment_id",
                    "string",
                    "片段 ID（只看该片段的指代）",
                    false,
                ),
                (
                    "chapter_id",
                    "string",
                    "只看该章节的指代（结构树章节 ID，如 ch_01）",
                    false,
                ),
                (
                    "volume",
                    "integer",
                    "只看该卷的指代（卷号，对应 get_chapter_outline 的 volume_order）",
                    false,
                ),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let fid = get_str_opt(arguments, "fragment_id");
        let chapter_filter = get_str_opt(arguments, "chapter_id");
        let volume_filter = get_u32_opt(arguments, "volume");

        // chapter/volume 过滤需要 片段→章节→卷 映射；只在使用时构建一次
        let frag_locations = if chapter_filter.is_some() || volume_filter.is_some() {
            Some(build_fragment_locations(&self.ctx)?)
        } else {
            None
        };

        let refs =
            crate::mutations::load_unresolved_refs(&self.ctx.paths()).map_err(|e| e.to_string())?;
        let selected: Vec<_> = refs
            .iter()
            .filter(|r| {
                if let Some(f) = &fid {
                    if &r.fragment_id != f {
                        return false;
                    }
                }
                if let Some(map) = &frag_locations {
                    // 已不在结构中的片段（被删）按章节过滤时无从归属，跳过
                    let Some((ch, vol)) = map.get(&r.fragment_id) else {
                        return false;
                    };
                    if let Some(cf) = &chapter_filter {
                        if ch.as_deref() != Some(cf.as_str()) {
                            return false;
                        }
                    }
                    if let Some(vf) = volume_filter {
                        if *vol != Some(vf) {
                            return false;
                        }
                    }
                }
                true
            })
            .collect();

        if selected.is_empty() {
            let scoped = chapter_filter.is_some() || volume_filter.is_some() || fid.is_some();
            return Ok(if scoped {
                "该范围内没有未锚定指代记录。".to_string()
            } else {
                "未锚定指代台账为空。".to_string()
            });
        }

        // 范围描述（有过滤时提示当前口径）
        let mut scope = String::new();
        if let Some(f) = &fid {
            scope.push_str(&format!("片段 {f}"));
        }
        if let Some(c) = &chapter_filter {
            if !scope.is_empty() {
                scope.push_str(" · ");
            }
            scope.push_str(&format!("章节 {c}"));
        }
        if let Some(v) = volume_filter {
            if !scope.is_empty() {
                scope.push_str(" · ");
            }
            scope.push_str(&format!("第 {v} 卷"));
        }

        let pending: Vec<_> = selected.iter().filter(|r| r.is_pending()).collect();
        let mut out = if scope.is_empty() {
            format!(
                "共 {} 条记录，待处置 {} 条：",
                selected.len(),
                pending.len()
            )
        } else {
            format!(
                "范围[{scope}] 共 {} 条记录，待处置 {} 条：",
                selected.len(),
                pending.len()
            )
        };
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

/// 片段 ID → (章节 ID, 卷号)。章节 ID 来自结构树，卷号经 chapters.toml 的
/// chapter_id → volume_order 映射（未回填 chapter_id 的章节卷号为 None）。
type FragmentLocations = HashMap<String, (Option<String>, Option<u32>)>;

fn build_fragment_locations(ctx: &ToolContext) -> Result<FragmentLocations, String> {
    let parse = ctx.parse()?;
    let doc = &parse.ir.document;
    let vol_of: HashMap<String, u32> = crate::mutations::load_chapters_file(&ctx.paths())
        .map(|file| {
            file.chapters
                .iter()
                .filter_map(|c| Some((c.chapter_id.clone()?, c.volume_order)))
                .collect()
        })
        .unwrap_or_default();

    let mut map = HashMap::new();
    for (fid, _) in doc.all_fragments() {
        let chapter_id = doc
            .scene_of_fragment(fid)
            .and_then(|sid| doc.chapter_of_scene(sid))
            .map(|ch| ch.as_str().to_string());
        let volume = chapter_id
            .as_deref()
            .and_then(|cid| vol_of.get(cid).copied());
        map.insert(fid.as_str().to_string(), (chapter_id, volume));
    }
    Ok(map)
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
            description: "处置未锚定指代（先可用 list_unresolved_refs 查看待处置清单）：\
            action=create 为其新建档案（匿名指代用占位名，如悬疑里的凶手指代\
            \"摆的人/写的人\"可建档为「九一真凶」，真名揭晓后 merge_entities 合并）；\
            action=merge 归拢到已有实体（需 target_entity_id）；\
            action=ignore 忽略，后续锚定不再提示。\
            建档/归拢会自动为所在片段补锚点。\
            create 角色时可同时传入 gender/occupation/appearance/background/personality/habits\
            一步建全档案，无需再 update_entity。\
            多条指代指向同一实体时（如同一凶手的多个代称），用 refs 一次批量处置，\
            create 只建一个档案。".into(),
            parameters: object_schema(vec![
                ("fragment_id", "string", "指代所在片段 ID（单条处置时必填）", false),
                ("quote", "string", "指代原文（台账中的 quote，需完全一致；单条处置时必填）", false),
                (
                    "refs",
                    "object-array",
                    "批量处置：数组，每项为原生 JSON 对象 {\"fragment_id\": \"...\", \"quote\": \"...\"}（可跨片段）。与 fragment_id/quote 二选一",
                    false,
                ),
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
                ("gender", "string", "create 角色时的性别", false),
                ("occupation", "string", "create 角色时的职业", false),
                ("appearance", "string", "create 角色时的外貌（多行自由文本）", false),
                ("background", "string", "create 角色时的背景（多行自由文本）", false),
                ("personality", "string", "create 角色时的性格（多行自由文本）", false),
                ("habits", "string", "create 角色时的习性：口头禅、小动作等（多行自由文本）", false),
            ]),
        }
    }

    fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let action = get_str(arguments, "action")?;
        let profile = parse_character_profile(arguments);

        // 批量：refs 数组（兼容字符串与原生数组形态），action/target/name/kind/profile 对每条同样生效
        if let Some(items) = super::get_json_array_opt(arguments, "refs")? {
            if items.is_empty() {
                return Err("refs 不能为空数组".into());
            }
            // 项归一化：模型常把每项序列化成字符串传入
            let items = super::normalize_object_items(items, "refs")?;
            let mut refs: Vec<(String, String)> = Vec::with_capacity(items.len());
            for item in &items {
                refs.push((get_str(item, "fragment_id")?, get_str(item, "quote")?));
            }
            return crate::mutations::resolve_unresolved_refs(
                &self.ctx.paths(),
                &refs,
                &action,
                get_str_opt(arguments, "target_entity_id").as_deref(),
                get_str_opt(arguments, "name").as_deref(),
                get_str_opt(arguments, "kind").as_deref(),
                profile.as_ref(),
            )
            .map_err(|e| e.to_string());
        }

        let fragment_id = get_str(arguments, "fragment_id")?;
        let quote = get_str(arguments, "quote")?;
        let summary = crate::mutations::resolve_unresolved_ref(
            &self.ctx.paths(),
            &fragment_id,
            &quote,
            &action,
            get_str_opt(arguments, "target_entity_id").as_deref(),
            get_str_opt(arguments, "name").as_deref(),
            get_str_opt(arguments, "kind").as_deref(),
            profile.as_ref(),
        )
        .map_err(|e| e.to_string())?;
        Ok(summary)
    }
}

/// 从工具参数提取 create 一步建档的角色档案字段（全空返回 None）。
fn parse_character_profile(
    args: &serde_json::Value,
) -> Option<crate::mutations::CharacterProfileInput> {
    let profile = crate::mutations::CharacterProfileInput {
        gender: get_str_opt(args, "gender"),
        occupation: get_str_opt(args, "occupation"),
        appearance: get_str_opt(args, "appearance"),
        background: get_str_opt(args, "background"),
        personality: get_str_opt(args, "personality"),
        habits: get_str_opt(args, "habits"),
    };
    if profile.is_empty() {
        None
    } else {
        Some(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;
    use story_ai_runtime::provider::{
        AiError, GenerateRequest, GenerateResponse, StopReason, TokenUsage,
    };
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
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
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

    /// anchor_fragment_merge（前端「AI 锚定本段」）：合并语义——
    /// 已有的手动锚点保留，LLM 判定只追加/更新，不清空重写
    #[test]
    fn anchor_fragment_merge_preserves_manual_anchors() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 两个角色：c_0001 由 LLM 指代锚定补锚；c_0002 是用户手动登记的锚点
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"洛恩·巴克尔\"\naliases = [\"洛恩\"]\n\n[profile]\noccupation = \"佣兵\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0002.toml"),
            "id = \"c_0002\"\ndisplay_name = \"艾达\"\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid =
            crate::mutations::create_fragment(&paths, &sc, "那名佣兵靠在门边，一言不发。").unwrap();

        // 用户手动锚定 c_0002（如删改稿后手动修正的出场记录）
        crate::mutations::set_fragment_anchor(&paths, &fid, "c_0002", "appears").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [{\"entity\": \"c_0001\", \"role\": \"referenced\", \"quote\": \"那名佣兵\"}], \"unresolved\": []}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        let outcome = anchor_fragment_merge(&ctx, &provider, &fid).unwrap();
        assert_eq!(outcome.anchored.len(), 1);

        // 手动锚点与 AI 补锚并存（replace 语义会丢掉 c_0002）
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert!(content.contains("entity: c_0001"), "{content}");
        assert!(content.contains("entity: c_0002"), "{content}");
    }

    /// refs 每项被模型序列化成字符串时应宽松解析回对象，
    /// 而非报「缺少参数: fragment_id」
    #[test]
    fn resolve_refs_accepts_string_items() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 预置一条待处置指代台账
        std::fs::write(
            paths.anchoring_toml(),
            "[[ref]]\nfragment_id = \"f_0001\"\nquote = \"穿灰袍的女人\"\nhint = \"疑似新角色\"\n",
        )
        .unwrap();

        let out = ResolveUnresolvedRefTool::new(ToolContext::new(dir.to_path_buf()))
            .execute(&serde_json::json!({
                "refs": ["{\"fragment_id\": \"f_0001\", \"quote\": \"穿灰袍的女人\"}"],
                "action": "ignore"
            }))
            .unwrap();
        assert!(out.contains("已忽略"), "{out}");

        // 台账状态已落盘为 ignored（宽松解析后 quote 与台账对上了）
        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs[0].status, "ignored");
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
            None,
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

        crate::mutations::resolve_unresolved_ref(
            &paths,
            &fid,
            "写的人",
            "ignore",
            None,
            None,
            None,
            None,
        )
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
        assert_eq!(
            crate::mutations::load_unresolved_refs(&paths)
                .unwrap()
                .len(),
            1
        );

        // 改稿：正文不再包含该指代（重写片段），重新锚定后台账自动清理
        crate::mutations::write_fragment_text(&paths, &fid, "现场已经清理干净。").unwrap();
        let provider_clean: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": []}".to_string(),
        });
        anchor_and_apply(&ctx, &provider_clean, &fid).unwrap();
        assert!(crate::mutations::load_unresolved_refs(&paths)
            .unwrap()
            .is_empty());
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
    fn roster_prioritizes_scene_anchored_entities() {
        // 41 个角色 > 名册上限 40：按类型顺序地点永远排不进去；
        // 本场景已锚定的地点应优先入册，锚定才能生效
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::create_dir_all(dir.join("entities/locations")).unwrap();
        for i in 1..=41 {
            std::fs::write(
                dir.join(format!("entities/characters/c_{i:04}.toml")),
                format!("id = \"c_{i:04}\"\ndisplay_name = \"角色{i}\"\n"),
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("entities/locations/l_0001.toml"),
            "id = \"l_0001\"\ndisplay_name = \"档案楼\"\n\n[geography]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "他在档案楼里整理红档。").unwrap();
        crate::mutations::replace_fragment_anchors(
            &paths,
            &fid,
            &[("l_0001".to_string(), "appears".to_string())],
        )
        .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [{\"entity\": \"l_0001\", \"role\": \"appears\", \"quote\": \"他\"}], \"unresolved\": []}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        let outcome = anchor_and_apply(&ctx, &provider, &fid).unwrap();
        assert_eq!(outcome.anchored.len(), 1, "{:?}", outcome.anchored);
        assert_eq!(outcome.anchored[0].0, "l_0001");
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
        let fid =
            crate::mutations::create_fragment(&paths, &sc, "医生说：什么十七号。他记了下来。")
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

    // —— 批量处置：多个代称归到同一实体 ——

    /// 同一片段的两个代称（「摆的人」「写的人」）一次 merge 归拢到同一实体：
    /// 台账全部 resolved，片段上该实体只留一个锚点。
    #[test]
    fn batch_resolve_merges_quotes_to_same_entity() {
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
            "摆的人懂行，知道我们先看什么。写的人不慌。",
        )
        .unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"摆的人\", \"hint\": \"疑似凶手\"}, {\"quote\": \"写的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        anchor_and_apply(&ctx, &provider, &fid).unwrap();

        let tool = ResolveUnresolvedRefTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({
                "refs": format!(
                    "[{{\"fragment_id\": \"{fid}\", \"quote\": \"摆的人\"}}, {{\"fragment_id\": \"{fid}\", \"quote\": \"写的人\"}}]"
                ),
                "action": "merge",
                "target_entity_id": "c_0001"
            }))
            .unwrap();
        assert!(out.contains("已批量归拢 2 条指代到 c_0001"), "{out}");

        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs.len(), 2);
        assert!(
            refs.iter()
                .all(|r| r.status == "resolved" && r.target.as_deref() == Some("c_0001")),
            "{refs:?}"
        );

        // 同片段同实体只应有一处锚点
        let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
        assert_eq!(
            content.matches("entity: c_0001").count(),
            1,
            "同实体不应重复落锚: {content}"
        );
    }

    /// 同一匿名对象的代称散布在两个片段：create 批量处置只建**一个**档案，
    /// 两个片段都补锚点。
    #[test]
    fn batch_resolve_create_builds_single_entity_across_fragments() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        // 名册非空锚定才会执行（此处放一个无关角色做诱饵）
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"顾岩\"\n\n[profile]\n",
        )
        .unwrap();

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid1 = crate::mutations::create_fragment(&paths, &sc, "摆的人懂行。").unwrap();
        let fid2 = crate::mutations::create_fragment(&paths, &sc, "写的人不慌。").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"摆的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        let ctx = ToolContext::new(dir.to_path_buf());
        anchor_and_apply(&ctx, &provider, &fid1).unwrap();
        let provider2: Arc<dyn AiProvider> = Arc::new(MockAnchorProvider {
            response: "{\"anchors\": [], \"unresolved\": [{\"quote\": \"写的人\", \"hint\": \"疑似凶手\"}]}".to_string(),
        });
        anchor_and_apply(&ctx, &provider2, &fid2).unwrap();

        let tool = ResolveUnresolvedRefTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({
                "refs": format!(
                    "[{{\"fragment_id\": \"{fid1}\", \"quote\": \"摆的人\"}}, {{\"fragment_id\": \"{fid2}\", \"quote\": \"写的人\"}}]"
                ),
                "action": "create",
                "name": "九一真凶"
            }))
            .unwrap();
        assert!(out.contains("九一真凶"), "{out}");
        assert!(out.contains("2 条指代全部归拢"), "{out}");

        // 只新建了一个实体档案（原有 1 个诱饵 + 1 个新档案）
        let chars_dir = dir.join("entities/characters");
        let files: Vec<_> = std::fs::read_dir(&chars_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".toml"))
            .collect();
        assert_eq!(files.len(), 2, "create 批量只应建一个新档案");

        // 两个片段都锚定了同一新实体，台账同目标
        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        let target = refs[0].target.clone().unwrap();
        assert_ne!(target, "c_0001", "不应归拢到诱饵实体");
        assert!(refs
            .iter()
            .all(|r| r.status == "resolved" && r.target.as_deref() == Some(&target)));
        for fid in [&fid1, &fid2] {
            let content = std::fs::read_to_string(dir.join(format!("fragments/{fid}.md"))).unwrap();
            assert!(
                content.contains(&format!("entity: {target}")),
                "{fid}: {content}"
            );
        }
    }

    /// 批量里混入台账中不存在的 quote：整体不失败，回执点名未找到的条目。
    #[test]
    fn batch_resolve_reports_missing_quotes_without_failing() {
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

        let tool = ResolveUnresolvedRefTool::new(ctx);
        let out = tool
            .execute(&serde_json::json!({
                "refs": format!(
                    "[{{\"fragment_id\": \"{fid}\", \"quote\": \"摆的人\"}}, {{\"fragment_id\": \"{fid}\", \"quote\": \"不存在的人\"}}]"
                ),
                "action": "ignore"
            }))
            .unwrap();
        assert!(out.contains("已批量忽略 1 条指代"), "{out}");
        assert!(out.contains("未找到 1 条"), "{out}");
        assert!(out.contains("不存在的人"), "{out}");

        let refs = crate::mutations::load_unresolved_refs(&paths).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].status, "ignored");
    }

    /// 单条处置（fragment_id + quote）未找到仍然报错——与批量语义区分。
    #[test]
    fn single_resolve_missing_quote_still_errors() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let fid = crate::mutations::create_fragment(&paths, &sc, "现场干净。").unwrap();

        let result = crate::mutations::resolve_unresolved_ref(
            &paths,
            &fid,
            "不存在的指代",
            "ignore",
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    /// chapter_id / volume 过滤：只看指定章节或卷的指代；无过滤行为不变。
    #[test]
    fn list_unresolved_refs_filters_by_chapter_and_volume() {
        let tmp = setup_test_project();
        let dir = tmp.path();

        let structure = r#"[book]
id = "b_root"
title = "测试"
chapters = ["ch_01", "ch_02"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_01_01"]

[scene.sc_01_01]
title = "开场"
fragments = ["f_0001"]

[chapter.ch_02]
title = "第二章"
scenes = ["sc_02_01"]

[scene.sc_02_01]
title = "夜行"
fragments = ["f_0002"]
"#;
        std::fs::write(dir.join("structure.toml"), structure).unwrap();
        std::fs::write(
            dir.join("fragments/f_0001.md"),
            "---\nid: f_0001\n---\n\n他看见摆的人。",
        )
        .unwrap();
        std::fs::write(
            dir.join("fragments/f_0002.md"),
            "---\nid: f_0002\n---\n\n卖果子的妇人喊了一声。",
        )
        .unwrap();

        let chapters = r#"[[chapters]]
chapter_id = "ch_01"
title = "第一章"
volume_order = 1
status = "completed"

[[chapters]]
chapter_id = "ch_02"
title = "第二章"
volume_order = 2
status = "writing"
"#;
        std::fs::write(dir.join("chapters.toml"), chapters).unwrap();

        let anchoring = r#"[[ref]]
fragment_id = "f_0001"
quote = "摆的人"
hint = "疑似新角色"

[[ref]]
fragment_id = "f_0002"
quote = "卖果子的妇人"
hint = "疑似路人"

[[ref]]
fragment_id = "f_0002"
quote = "推车的搬运工"
hint = "疑似路人"
status = "ignored"
"#;
        std::fs::write(dir.join("anchoring.toml"), anchoring).unwrap();

        let tool = ListUnresolvedRefsTool::new(ToolContext::new(dir.to_path_buf()));

        // 无过滤：3 条（含已忽略）
        let out = tool.execute(&serde_json::json!({})).unwrap();
        assert!(out.contains("共 3 条记录，待处置 2 条"), "{out}");

        // chapter_id 过滤：只剩 ch_01 的 1 条
        let out = tool
            .execute(&serde_json::json!({"chapter_id": "ch_01"}))
            .unwrap();
        assert!(out.contains("范围[章节 ch_01] 共 1 条记录"), "{out}");
        assert!(out.contains("摆的人"), "{out}");
        assert!(!out.contains("卖果子的妇人"), "{out}");

        // volume 过滤：第 2 卷的 2 条（含 1 条已忽略）
        let out = tool.execute(&serde_json::json!({"volume": 2})).unwrap();
        assert!(
            out.contains("范围[第 2 卷] 共 2 条记录，待处置 1 条"),
            "{out}"
        );
        assert!(out.contains("推车的搬运工"), "{out}");
        assert!(!out.contains("摆的人"), "{out}");

        // 范围内无记录
        let out = tool.execute(&serde_json::json!({"volume": 3})).unwrap();
        assert!(out.contains("该范围内没有"), "{out}");
    }

    // —— 带时限锚定（写入工具防 MCP 客户端超时假失败）——

    /// 可控延迟的 provider：模拟慢 LLM
    #[derive(Debug)]
    struct SlowAnchorProvider {
        delay_ms: u64,
    }
    impl AiProvider for SlowAnchorProvider {
        fn id(&self) -> &str {
            "mock-slow-anchor"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
            Ok(GenerateResponse {
                text: "{\"anchors\": [], \"unresolved\": []}".to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    /// 建一章一场景一片段，返回片段 ID。
    /// 含一个角色档案：名册为空时 comprehension_anchor 会提前返回（不调 LLM），
    /// 时限路径的测试必须让 LLM 真正被调用。
    fn budget_fixture(dir: &std::path::Path) -> String {
        let paths = ProjectPaths::new(dir);
        std::fs::create_dir_all(dir.join("entities/characters")).unwrap();
        std::fs::write(
            dir.join("entities/characters/c_0001.toml"),
            "id = \"c_0001\"\ndisplay_name = \"洛恩\"\n",
        )
        .unwrap();
        let ch = crate::mutations::create_chapter(&paths, "章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        crate::mutations::create_fragment(&paths, &sc, "他推门进来，把伞收在了门边。").unwrap()
    }

    #[test]
    fn bounded_anchor_returns_done_when_fast() {
        let tmp = setup_test_project();
        let fid = budget_fixture(tmp.path());
        let ctx = ToolContext::with_provider(
            tmp.path().to_path_buf(),
            Arc::new(MockAnchorProvider {
                response: "{\"anchors\": [], \"unresolved\": []}".to_string(),
            }),
        );
        let run =
            anchor_and_apply_with_budget(&ctx, &fid, std::time::Duration::from_secs(5)).unwrap();
        assert!(matches!(run, AnchorRun::Done(_)), "快 provider 应正常完成");
    }

    #[test]
    fn bounded_anchor_defers_when_slow() {
        let tmp = setup_test_project();
        let fid = budget_fixture(tmp.path());
        let ctx = ToolContext::with_provider(
            tmp.path().to_path_buf(),
            Arc::new(SlowAnchorProvider { delay_ms: 800 }),
        );
        let started = std::time::Instant::now();
        let run =
            anchor_and_apply_with_budget(&ctx, &fid, std::time::Duration::from_millis(50)).unwrap();
        assert!(matches!(run, AnchorRun::Deferred), "慢 provider 应转入后台");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "应按时限先行返回，而不是等慢 provider 跑完"
        );
        // 等后台线程完成落盘，避免临时目录在其持有文件句柄时被清理
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    #[test]
    fn bounded_anchor_without_provider_is_noop() {
        let tmp = setup_test_project();
        let fid = budget_fixture(tmp.path());
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let run =
            anchor_and_apply_with_budget(&ctx, &fid, std::time::Duration::from_secs(1)).unwrap();
        assert!(
            matches!(&run, AnchorRun::Done(o) if o.anchored.is_empty() && o.unresolved.is_empty()),
            "无 provider 时空结果，与调用方原有语义一致"
        );
    }
}

/// 测试支撑（crate 内可见）
pub mod tests_support {
    use super::*;

    pub fn parse_with_empty_roster(text: &str) -> AnchorOutcome {
        parse_anchor_response(text, &[]).unwrap()
    }
}
