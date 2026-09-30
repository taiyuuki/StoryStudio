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

use super::{get_str_opt, object_schema, ToolContext};

/// 名册上限：超出的实体（按 角色→地点→道具→组织 顺序截断）不参与本次锚定
const ROSTER_CAP: usize = 40;

/// 实体名册条目
struct RosterEntry {
    id: String,
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
            entries.push(RosterEntry { id, line });
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
    let path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| format!("无效的片段 ID: {fragment_id}"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("读取片段失败: {e}"))?;
    let body = if let Some(rest) = content.strip_prefix("---") {
        rest.find("---")
            .map(|end| content[3 + end + 3..].trim_start_matches(['\r', '\n']).to_string())
            .unwrap_or(content.clone())
    } else {
        content.clone()
    };
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
2. 只标注指代消解后能确定身份的；拿不准就不标；\n3. 指代对象在名册中找不到的，\
放入 unresolved 并给一句提示（如\"疑似新角色\"）。\n\
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

    parse_anchor_response(&response.text, &roster)
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
pub fn anchor_and_apply(
    ctx: &ToolContext,
    provider: &Arc<dyn AiProvider>,
    fragment_id: &str,
) -> Result<AnchorOutcome, String> {
    let outcome = comprehension_anchor(ctx, provider, fragment_id)?;
    if !outcome.anchored.is_empty() {
        let pairs: Vec<(String, String)> = outcome
            .anchored
            .iter()
            .map(|(id, role, _)| (id.clone(), role.clone()))
            .collect();
        crate::mutations::replace_fragment_anchors(&ctx.paths(), fragment_id, &pairs)
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
        note.push_str("\n未锚定指代（名册中无匹配，若为新角色请建档）：");
        for (quote, hint) in &outcome.unresolved {
            note.push_str(&format!("\n  - \"{quote}\"（{hint}）"));
        }
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
        let mut failed: Vec<String> = Vec::new();
        for fid in &names {
            match anchor_and_apply(&self.ctx, provider, fid) {
                Ok(o) => anchored_count += o.anchored.len(),
                Err(e) => failed.push(format!("{fid}: {e}")),
            }
        }

        let mut out = format!(
            "批量锚定完成：处理 {} 个片段（共 {total} 个{}），落锚 {anchored_count} 处",
            names.len(),
            if total > BATCH_CAP { "，达单次上限，请分批" } else { "" },
        );
        if !failed.is_empty() {
            out.push_str(&format!("\n失败 {} 个：{}", failed.len(), failed.join("；")));
        }
        Ok(out)
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
            "那名佣兵靠在门边，一言不发。后来他开口了。",
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
    }

    #[test]
    fn comprehension_anchor_rejects_unknown_entity() {
        // 名册外的 entity 直接被过滤
        let roster_outcome = crate::tools::anchoring::tests_support::parse_with_empty_roster(
            "{\"anchors\": [{\"entity\": \"c_9999\", \"role\": \"appears\", \"quote\": \"x\"}]}",
        );
        assert!(roster_outcome.anchored.is_empty());
    }
}

/// 测试支撑（crate 内可见）
pub mod tests_support {
    use super::*;

    pub fn parse_with_empty_roster(text: &str) -> AnchorOutcome {
        parse_anchor_response(text, &[]).unwrap()
    }
}
