//! AI 写作特征检测（AI 味诊断）
//!
//! 灵感来自 dbs-ai-check：扫描小说正文中的 AI 生成痕迹，输出结构化检测报告。
//! 设计为独立的能力层，**不**纳入 Compiler 的确定性 Diagnostic 体系——
//! AI 检测是概率性的（同一文本两次结果可能不同），与确定性校验严格隔离。
//!
//! 体裁适配：针对「小说正文」从 dbs-ai-check 的 22 条特征中精选 11 条，
//! 并新增 7 条小说高频 AI 味，共 18 条。

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use story_ai_runtime::provider::{GenerateRequest, Message, StopReason};
use story_ai_runtime::AiProvider;
use story_project_format::ProjectPaths;

use crate::error::StudioError;

// ============================================================================
// 数据结构
// ============================================================================

/// 严重度分级（对应 dbs-ai-check 的 🔴 ⚠️ 💡）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AiCheckSeverity {
    /// 🔴 强信号 — 几乎只有 AI 会这样做
    Strong,
    /// ⚠️ 中信号 — AI 高频但人也可能做
    Medium,
    /// 💡 弱信号 — 需要结合上下文判断
    Weak,
}

/// 单处 AI 特征命中
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCheckItem {
    /// 命中处原文引用（让用户一眼看到是哪段话有问题）
    pub quote: String,
    /// 特征编号（如 "A"、"14"、"19"；解析失败时为 "parse_error"）
    pub feature_id: String,
    /// 特征名称（如 "形容词堆砌的场景描写"）
    pub feature_name: String,
    /// 严重度
    pub severity: AiCheckSeverity,
    /// 一两句话说明这段话的问题，具体、直接、不堆术语
    pub explanation: String,
    /// 批量检测（场景/章节级）时命中所在的片段；单片段检测为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment_id: Option<String>,
    /// 片段归属是否为推测（引文未能匹配回原文，归到块内首片段时 true）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fragment_uncertain: bool,
}

/// AI 检测报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCheckReport {
    /// 被检测的 fragment ID；纯文本检测时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fragment_id: Option<String>,
    /// 命中的特征列表（按文本顺序）
    pub items: Vec<AiCheckItem>,
    /// 一两句话概括最突出的问题，不罗列
    pub summary: String,
    /// 本次检测的 token 消耗
    pub tokens_used: usize,
}

impl AiCheckReport {
    /// 命中特征数
    pub fn hit_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.feature_id != "parse_error")
            .count()
    }
}

// ============================================================================
// Prompt
// ============================================================================

// 检测 system prompt（18 条小说适配特征 + 严重度 + 误伤警告 + 输出格式）
// 已文件化：src/prompts/ai_check.md

/// LLM 返回的 JSON 结构（用于反序列化校验）
#[derive(Debug, Deserialize)]
struct LlmReport {
    items: Vec<LlmItem>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize)]
struct LlmItem {
    quote: String,
    feature_id: String,
    feature_name: String,
    severity: String,
    explanation: String,
}

// ============================================================================
// 核心函数
// ============================================================================

/// 检测指定 fragment 的 AI 写作特征
///
/// 流程：读取 fragment（剥 frontmatter 取正文）→ 构造请求 → retry_generate → 解析 JSON。
/// 结果（含失败）计入 AI 台账，与其他 AI 用途同口径。
pub fn check_fragment(
    dir: &Path,
    fragment_id: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckReport, StudioError> {
    let started = std::time::Instant::now();
    let provider_id = provider.id().to_string();
    let provider_model = provider.model().to_string();
    let result = check_fragment_inner(dir, fragment_id, provider);
    match &result {
        Ok(report) => crate::ai_ledger::record(
            dir,
            "AI 审校",
            &provider_id,
            &provider_model,
            report.tokens_used as u64,
            started.elapsed().as_millis() as u64,
            true,
            None,
        ),
        Err(e) => crate::ai_ledger::record(
            dir,
            "AI 审校",
            &provider_id,
            &provider_model,
            0,
            started.elapsed().as_millis() as u64,
            false,
            Some(&e.to_string()),
        ),
    }
    result
}

fn check_fragment_inner(
    dir: &Path,
    fragment_id: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckReport, StudioError> {
    let paths = ProjectPaths::new(dir);
    let text = read_fragment_body(&paths, fragment_id)?;
    let (items, summary, tokens_used) = check_text_inner(&text, provider)?;
    Ok(AiCheckReport {
        fragment_id: Some(fragment_id.to_string()),
        items,
        summary,
        tokens_used,
    })
}

/// 检测纯文本的 AI 写作特征
pub fn check_text(
    text: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckReport, StudioError> {
    let (items, summary, tokens_used) = check_text_inner(text, provider)?;
    Ok(AiCheckReport {
        fragment_id: None,
        items,
        summary,
        tokens_used,
    })
}

// ============================================================================
// 批量检测（场景 / 章节级）
// ============================================================================

/// 批量检测范围
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckScope {
    Scene,
    Chapter,
}

impl CheckScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckScope::Scene => "scene",
            CheckScope::Chapter => "chapter",
        }
    }
}

/// 批量检测报告（场景/章节级）——多片段聚合，命中项带片段归因
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCheckBatchReport {
    /// "scene" | "chapter"
    pub scope: String,
    pub scope_id: String,
    /// 命中项（按阅读顺序，fragment_id 指明所在片段）
    pub items: Vec<AiCheckItem>,
    pub summary: String,
    pub tokens_used: usize,
    /// 检测覆盖的片段（阅读序）与各自命中数
    pub fragment_hits: Vec<(String, usize)>,
}

impl AiCheckBatchReport {
    /// 真实命中数（排除 parse_error）
    pub fn hit_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.feature_id != "parse_error")
            .count()
    }
}

/// 单次 LLM 检测的正文规模上限：超出则按片段边界分块检测。
/// 上限兼顾检测质量（跨片段的重复句式能在一块里看出来）与输出配额。
const SCOPE_CHUNK_CHAR_CAP: usize = 6000;

/// 按阅读序收集范围内的片段 ID
fn scope_fragments(
    paths: &ProjectPaths,
    scope: CheckScope,
    id: &str,
) -> Result<Vec<String>, StudioError> {
    let structure_path = paths.structure_toml();
    let content = std::fs::read_to_string(&structure_path)
        .map_err(|e| StudioError::Io(format!("读取 structure.toml 失败: {e}")))?;
    let structure = story_project_format::StructureFile::from_toml_str(&content)
        .map_err(|e| StudioError::Config(format!("解析 structure.toml 失败: {e}")))?;

    let scene_ids: Vec<String> = match scope {
        CheckScope::Scene => vec![id.to_string()],
        CheckScope::Chapter => {
            let chapter = structure
                .chapter
                .get(id)
                .ok_or_else(|| StudioError::NotFound(format!("章节 '{id}' 不存在")))?;
            chapter.scenes.clone()
        }
    };

    let mut fragment_ids = Vec::new();
    for sc in &scene_ids {
        let scene = structure
            .scene
            .get(sc)
            .ok_or_else(|| StudioError::NotFound(format!("场景 '{sc}' 不存在")))?;
        fragment_ids.extend(scene.fragments.iter().cloned());
    }
    Ok(fragment_ids)
}

/// 场景/章节级批量检测：聚合正文按片段边界分块，各块并行调 LLM，
/// 命中项按 quote 回查片段归属（精确→归一化→LCS 模糊，全落空才归块内
/// 首片段并标记推测）。
/// 结果（含失败）计入 AI 台账，token 为各块之和。
pub fn check_scope(
    dir: &Path,
    scope: CheckScope,
    id: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckBatchReport, StudioError> {
    let ledger = ScopeLedgerCtx::new(dir, provider, std::time::Instant::now());
    let result = check_scope_inner(dir, scope, id, provider);
    ledger.record(result.as_ref());
    result
}

/// 引文 → 块内片段归属，返回 (bodies 下标, 是否推测)。
///
/// LLM 的引文不总忠实于原文（省略、改标点、跨段落拼接），精确 contains
/// 失配时依次降级：去空白归一化匹配 → 最长公共子串评分（连续一致部分
/// 覆盖引文 ≥60% 才采信）；全部落空才归块内首片段并标记 uncertain，
/// 让调用方提示「归属为推测」，避免错标块首片段被当真。
fn attribute_quote(chunk: &[usize], bodies: &[(String, String)], quote: &str) -> (usize, bool) {
    if !quote.is_empty() {
        // 1. 精确子串：正文原样包含引文
        if let Some(&i) = chunk.iter().find(|&&i| bodies[i].1.contains(quote)) {
            return (i, false);
        }
        let nq = normalize_ws(quote);
        let nq_len = nq.chars().count();
        // 2. 去空白匹配：引文把跨段落的文字拼成一句、或换行差异；太短易误命中
        if nq_len >= 6 {
            if let Some(&i) = chunk
                .iter()
                .find(|&&i| normalize_ws(&bodies[i].1).contains(&nq))
            {
                return (i, false);
            }
        }
        // 3. 模糊评分：与某片段的最长公共子串覆盖引文 ≥60% 即归它（并列取阅读序更早的）
        if nq_len >= 8 {
            let mut best: Option<(usize, usize)> = None;
            for &i in chunk {
                let score = lcs_substring_len(&nq, &normalize_ws(&bodies[i].1));
                if score * 10 >= nq_len * 6 && best.is_none_or(|(_, b)| score > b) {
                    best = Some((i, score));
                }
            }
            if let Some((i, _)) = best {
                return (i, false);
            }
        }
    }
    (*chunk.first().expect("检测块非空"), true)
}

/// 去除全部空白字符（空格、换行、制表、全角空格）
fn normalize_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 最长公共子串（连续）长度，滚动数组 DP。
/// 引文（几十~两百字）对片段正文（≤6000 字）开销可忽略。
fn lcs_substring_len(a: &str, b: &str) -> usize {
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    let long_chars: Vec<char> = long.chars().collect();
    let mut prev = vec![0usize; long_chars.len() + 1];
    let mut curr = vec![0usize; long_chars.len() + 1];
    let mut best = 0usize;
    for sc in short.chars() {
        for (j, &lc) in long_chars.iter().enumerate() {
            let v = if sc == lc { prev[j] + 1 } else { 0 };
            curr[j + 1] = v;
            if v > best {
                best = v;
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    best
}

/// 单块检测结果（错误串化以便跨线程传递，组装时包回 StudioError::Ai）
type ChunkResult = Result<(Vec<AiCheckItem>, String, usize), String>;

/// 检测计划：阅读序正文与分块方案。并行执行前一次读齐——后台收集
/// 线程不再回读项目文件，避免与后续写入交错。
struct ScopePlan {
    bodies: Vec<(String, String)>,
    /// 每块为 bodies 下标集合（阅读序）
    chunks: Vec<Vec<usize>>,
}

fn plan_scope_check(
    paths: &ProjectPaths,
    scope: CheckScope,
    id: &str,
) -> Result<ScopePlan, StudioError> {
    let fragment_ids = scope_fragments(paths, scope, id)?;
    if fragment_ids.is_empty() {
        return Err(StudioError::Ai(format!(
            "范围 {} {id} 下没有片段",
            scope.as_str()
        )));
    }

    // 阅读序正文（跳过空正文片段）
    let mut bodies: Vec<(String, String)> = Vec::new();
    for fid in &fragment_ids {
        let body = read_fragment_body(paths, fid)?;
        if !body.is_empty() {
            bodies.push((fid.clone(), body));
        }
    }
    if bodies.is_empty() {
        return Err(StudioError::Ai(format!(
            "范围 {} {id} 下的片段正文均为空",
            scope.as_str()
        )));
    }

    // 分块：按片段边界凑块，单片段超上限时独占一块
    let mut chunks: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_chars = 0usize;
    for (idx, (_, body)) in bodies.iter().enumerate() {
        let len = body.chars().count();
        if !current.is_empty() && current_chars + len > SCOPE_CHUNK_CHAR_CAP {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current.push(idx);
        current_chars += len;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(ScopePlan { bodies, chunks })
}

/// 各块结果的按块序归并器（命中项保持阅读序、归因到块内片段）
struct ScopeAggregator {
    all_items: Vec<AiCheckItem>,
    fragment_hits: Vec<(String, usize)>,
    tokens_used: usize,
    best_summary: String,
    best_summary_hits: usize,
}

impl ScopeAggregator {
    fn new(plan: &ScopePlan) -> Self {
        Self {
            all_items: Vec::new(),
            fragment_hits: plan
                .bodies
                .iter()
                .map(|(fid, _)| (fid.clone(), 0))
                .collect(),
            tokens_used: 0,
            best_summary: String::new(),
            best_summary_hits: 0,
        }
    }

    /// 并入一块的检测结果（chunk_idx 为块序）
    fn merge(
        &mut self,
        plan: &ScopePlan,
        chunk_idx: usize,
        items: Vec<AiCheckItem>,
        summary: String,
        tokens: usize,
    ) {
        self.tokens_used += tokens;
        let chunk = &plan.chunks[chunk_idx];
        let mut chunk_hits = 0usize;
        for mut item in items {
            if item.feature_id == "parse_error" {
                self.all_items.push(item);
                continue;
            }
            chunk_hits += 1;
            // 归因：quote 出现在哪个片段
            let (owner, uncertain) = attribute_quote(chunk, &plan.bodies, &item.quote);
            item.fragment_id = Some(plan.bodies[owner].0.clone());
            item.fragment_uncertain = uncertain;
            if let Some(hit) = self
                .fragment_hits
                .iter_mut()
                .find(|(fid, _)| *fid == plan.bodies[owner].0)
            {
                hit.1 += 1;
            }
            self.all_items.push(item);
        }
        if chunk_hits > self.best_summary_hits {
            self.best_summary_hits = chunk_hits;
            self.best_summary = summary;
        }
    }

    fn finish(mut self, scope: CheckScope, id: &str) -> AiCheckBatchReport {
        if self.best_summary.is_empty() {
            self.best_summary = default_summary(&self.all_items);
        }
        AiCheckBatchReport {
            scope: scope.as_str().to_string(),
            scope_id: id.to_string(),
            items: self.all_items,
            summary: self.best_summary,
            tokens_used: self.tokens_used,
            fragment_hits: self.fragment_hits,
        }
    }
}

/// 把各块结果（按块序的槽位）组装成报告。
///
/// 任一块失败则整体失败（沿用原串行语义：一块失败时给出缺块的
/// 「整章报告」只会误导）；槽位为 None 说明工作线程未送出结果
/// （catch_unwind 兜底后理论上不可达，防御性报错）。
fn assemble_scope_report(
    scope: CheckScope,
    id: &str,
    plan: ScopePlan,
    slots: Vec<Option<ChunkResult>>,
) -> Result<AiCheckBatchReport, StudioError> {
    let mut agg = ScopeAggregator::new(&plan);
    for (chunk_idx, slot) in slots.into_iter().enumerate() {
        let (items, summary, tokens) = slot
            .ok_or_else(|| StudioError::Ai("检测工作线程异常退出（未送出结果）".to_string()))?
            .map_err(StudioError::Ai)?;
        agg.merge(&plan, chunk_idx, items, summary, tokens);
    }
    Ok(agg.finish(scope, id))
}

/// 并行执行各块检测，返回按 (块序, 结果) 送出的通道。
///
/// 每块一个线程：分块数受 [`SCOPE_CHUNK_CHAR_CAP`] 约束（单章通常
/// 1~3 块），无需并发上限；端到端等待从各块耗时之和降为最慢一块。
/// catch_unwind + spawn 失败兜底保证每个线程恰好送出一条结果——
/// 收集端按块数收齐即可，worker panic 不会让接收方永久阻塞。
fn spawn_chunk_workers(
    provider: &Arc<dyn AiProvider>,
    plan: &ScopePlan,
) -> std::sync::mpsc::Receiver<(usize, ChunkResult)> {
    let (tx, rx) = std::sync::mpsc::channel();
    for (idx, chunk) in plan.chunks.iter().enumerate() {
        let text = chunk
            .iter()
            .map(|&i| plan.bodies[i].1.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let provider = Arc::clone(provider);
        let spawn = {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("story-ai-check-chunk".into())
                .spawn(move || {
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        check_text_inner(&text, &provider).map_err(|e| e.to_string())
                    }));
                    let outcome = res.unwrap_or_else(|_| {
                        Err("检测线程崩溃（正文不受影响，可重试）".to_string())
                    });
                    let _ = tx.send((idx, outcome));
                })
        };
        if let Err(e) = spawn {
            // 线程创建失败（资源枯竭等）：作为该块的失败送出，维持计数不变量
            let _ = tx.send((idx, Err(format!("创建检测线程失败: {e}"))));
        }
    }
    rx
}

/// AI 台账记账上下文（同步完成与后台收齐两条路径共用）
struct ScopeLedgerCtx {
    dir: std::path::PathBuf,
    provider_id: String,
    provider_model: String,
    started: std::time::Instant,
}

impl ScopeLedgerCtx {
    fn new(dir: &Path, provider: &Arc<dyn AiProvider>, started: std::time::Instant) -> Self {
        Self {
            dir: dir.to_path_buf(),
            provider_id: provider.id().to_string(),
            provider_model: provider.model().to_string(),
            started,
        }
    }

    /// 记账：成功记 token，失败记原因（与锚定旁路日志同一取舍）
    fn record(&self, outcome: Result<&AiCheckBatchReport, &StudioError>) {
        match outcome {
            Ok(report) => crate::ai_ledger::record(
                &self.dir,
                "AI 审校",
                &self.provider_id,
                &self.provider_model,
                report.tokens_used as u64,
                self.started.elapsed().as_millis() as u64,
                true,
                None,
            ),
            Err(e) => crate::ai_ledger::record(
                &self.dir,
                "AI 审校",
                &self.provider_id,
                &self.provider_model,
                0,
                self.started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            ),
        }
    }
}

/// 带时限的批量检测结果
pub enum ScopeCheckRun {
    /// 时限内收齐全部块（报告已组装、台账已记）
    Done(AiCheckBatchReport),
    /// 时限耗尽：completed 块已同步收齐，其余转后台；后台收齐后组装
    /// 报告、记账并经 on_complete 送达
    Deferred { completed: usize, total: usize },
}

/// [`check_scope`] 的带时限版本，供 MCP 工具路径使用。
///
/// MCP 客户端对单次工具调用普遍有 30~60 秒硬超时，思考型 provider
/// 单块检测就要 30~50 秒——整章多块同步等待必然撞线：客户端报错而
/// 服务端照跑，token 白烧。超时时限后立即返回 Deferred，余下块由
/// 后台线程收齐，最终报告经 on_complete 回调送出（调用方接到回执
/// 接力等通道送达），与锚定的转后台机制（ANCHOR_BUDGET）同一哲学。
pub fn check_scope_bounded(
    dir: &Path,
    scope: CheckScope,
    id: &str,
    provider: &Arc<dyn AiProvider>,
    budget: std::time::Duration,
    on_complete: impl FnOnce(Result<AiCheckBatchReport, StudioError>) + Send + 'static,
) -> Result<ScopeCheckRun, StudioError> {
    let ledger = ScopeLedgerCtx::new(dir, provider, std::time::Instant::now());
    check_scope_bounded_inner(scope, id, provider, budget, ledger, on_complete)
}

fn check_scope_bounded_inner(
    scope: CheckScope,
    id: &str,
    provider: &Arc<dyn AiProvider>,
    budget: std::time::Duration,
    ledger: ScopeLedgerCtx,
    on_complete: impl FnOnce(Result<AiCheckBatchReport, StudioError>) + Send + 'static,
) -> Result<ScopeCheckRun, StudioError> {
    // 计划失败（无片段等）也记账后返回
    let plan = match plan_scope_check(&ProjectPaths::new(&ledger.dir), scope, id) {
        Ok(p) => p,
        Err(e) => {
            ledger.record(Err(&e));
            return Err(e);
        }
    };
    let rx = spawn_chunk_workers(provider, &plan);
    let total = plan.chunks.len();
    let deadline = std::time::Instant::now() + budget;
    let mut slots: Vec<Option<ChunkResult>> = vec![None; total];
    let mut received = 0usize;
    while received < total {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok((idx, res)) => {
                if slots[idx].replace(res).is_none() {
                    received += 1;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            // catch_unwind 兜底后不应发生（每块必有一条结果）；
            // 真发生则交由后台发现 None 槽位并报错
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if received == total {
        let result = assemble_scope_report(scope, id, plan, slots);
        ledger.record(result.as_ref());
        return result.map(ScopeCheckRun::Done);
    }

    // 超时转后台：已收齐的块随 slots 移交收集线程（零浪费），收齐后
    // 组装报告、记台账、经 on_complete 送达
    let id = id.to_string();
    let spawn_res = std::thread::Builder::new()
        .name("story-ai-check-collect".into())
        .spawn(move || {
            let mut received = received;
            while received < total {
                match rx.recv() {
                    Ok((idx, res)) => {
                        if slots[idx].replace(res).is_none() {
                            received += 1;
                        }
                    }
                    Err(_) => break, // 发送端全退仍未收齐：由 None 槽位报错
                }
            }
            let result = assemble_scope_report(scope, &id, plan, slots);
            ledger.record(result.as_ref());
            on_complete(result);
        });
    match spawn_res {
        Ok(_) => Ok(ScopeCheckRun::Deferred {
            completed: received,
            total,
        }),
        Err(e) => Err(StudioError::Ai(format!("创建后台检测收集线程失败: {e}"))),
    }
}

fn check_scope_inner(
    dir: &Path,
    scope: CheckScope,
    id: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<AiCheckBatchReport, StudioError> {
    let plan = plan_scope_check(&ProjectPaths::new(dir), scope, id)?;
    let rx = spawn_chunk_workers(provider, &plan);
    let total = plan.chunks.len();
    let mut slots: Vec<Option<ChunkResult>> = vec![None; total];
    for _ in 0..total {
        let (idx, res) = rx
            .recv()
            .map_err(|_| StudioError::Ai("检测工作线程异常退出".to_string()))?;
        slots[idx] = Some(res);
    }
    assemble_scope_report(scope, id, plan, slots)
}

/// 实际检测逻辑，返回 (命中项, 概括, token 消耗)
fn check_text_inner(
    text: &str,
    provider: &Arc<dyn AiProvider>,
) -> Result<(Vec<AiCheckItem>, String, usize), StudioError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(StudioError::Ai("待检测文本为空".to_string()));
    }

    let char_count = trimmed.chars().count();
    // 输出空间：检测报告本身不长，但思考模式会占用输出配额（本端点实测
    // 可达 ~10k），下限 16384 保证常规片段（~1500 字）不被截断；
    // 长片段按输入规模放大，上限 32768
    let estimated_output_tokens = (char_count.saturating_mul(3)) + 1024;
    let max_tokens = estimated_output_tokens.clamp(16384, 32768);

    let request = GenerateRequest {
        system: crate::prompts::AI_CHECK.to_string(),
        messages: vec![Message::user(format!("请检测以下小说正文：\n\n{trimmed}"))],
        model: String::new(),
        temperature: 0.3, // 检测要稳定可复现，区别于创作的 0.7
        max_tokens,
        tools: Vec::new(),
    };

    let response = crate::tools::retry_generate(provider, &request)
        .map_err(|e| StudioError::Ai(format!("AI 检测调用失败: {e}")))?;

    // 截断检测：JSON 不完整时不应给出残缺报告
    if matches!(response.stop_reason, StopReason::MaxTokens) {
        return Err(StudioError::Ai(format!(
            "检测结果被截断（达到 max_tokens={max_tokens} 上限）。\
            原文 {char_count} 字过长，建议用 split_fragment 拆分后再检测。"
        )));
    }

    let tokens_used = response.usage.total();
    let items = parse_report(&response.text);
    let summary = extract_summary(&response.text).unwrap_or_else(|| default_summary(&items));

    Ok((items, summary, tokens_used))
}

/// 读取 fragment 文件并剥除 frontmatter，返回纯正文
fn read_fragment_body(paths: &ProjectPaths, fragment_id: &str) -> Result<String, StudioError> {
    let frag_path = paths
        .file_for_id(fragment_id)
        .ok_or_else(|| StudioError::NotFound(format!("无效的 fragment ID: {fragment_id}")))?;
    if !frag_path.exists() {
        return Err(StudioError::NotFound(format!(
            "片段 '{fragment_id}' 不存在"
        )));
    }
    let content = std::fs::read_to_string(&frag_path)
        .map_err(|e| StudioError::Io(format!("读取片段失败: {e}")))?;

    // 复用 ReadFragmentTextTool 的 frontmatter 剥离逻辑
    let body = if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            content[3 + end + 3..]
                .trim_start_matches('\n')
                .trim()
                .to_string()
        } else {
            content.trim().to_string()
        }
    } else {
        content.trim().to_string()
    };

    if body.is_empty() {
        return Err(StudioError::Ai(format!("片段 '{fragment_id}' 正文为空")));
    }
    Ok(body)
}

/// 解析 LLM 返回的报告文本为 AiCheckItem 列表
///
/// 健壮性策略（容错函数统一来自 [`crate::llm_json`]）：
/// 1. 剥思考链后直接 JSON 解析
/// 2. 失败则尝试从 ```json ... ``` 围栏提取
/// 3. 仍失败则截取首 `{` 到末 `}` 的子串
/// 4. 修复字符串内部未转义引号后重试
/// 5. 宽松提取——逐个对象抢救，只救出能解析的 item
/// 6. 全部失败：降级为一个 parse_error item（原文只附截断摘录，不整段倾泻）
fn parse_report(raw: &str) -> Vec<AiCheckItem> {
    let cleaned = crate::llm_json::strip_think(raw).trim().to_string();

    // 尝试 1：直接解析
    if let Some(report) = try_parse_json(&cleaned) {
        return report_to_items(report);
    }

    // 尝试 2：提取 ```json ... ``` 围栏
    if let Some(json_str) = crate::llm_json::extract_json_fence(&cleaned) {
        if let Some(report) = try_parse_json(&json_str) {
            return report_to_items(report);
        }
    }

    // 尝试 3：寻找第一个 { 到最后一个 } 的子串
    if let Some(json_str) = crate::llm_json::extract_json_object(&cleaned) {
        if let Some(report) = try_parse_json(&json_str) {
            return report_to_items(report);
        }
    }

    // 尝试 4：LLM 常在字符串值里嵌未转义双引号导致 JSON 破裂，
    // 把字符串内部的 ASCII 双引号转义后再试一次。
    let repaired = crate::llm_json::repair_unescaped_quotes(&cleaned);
    if let Some(report) = try_parse_json(&repaired).or_else(|| {
        try_parse_json(&crate::llm_json::extract_json_object(&repaired).unwrap_or_default())
    }) {
        return report_to_items(report);
    }

    // 尝试 5：宽松提取——逐个对象抢救，只救出能解析的 item（丢 summary 无所谓）
    let loose_items = loose_extract_items(&cleaned);
    if !loose_items.is_empty() {
        return loose_items;
    }

    // 全部失败：降级为摘录展示（截断，避免整段原文倾泻）
    let excerpt: String = cleaned.chars().take(300).collect();
    let ellipsis = if cleaned.chars().count() > 300 {
        "……"
    } else {
        ""
    };
    vec![AiCheckItem {
        quote: String::new(),
        feature_id: "parse_error".to_string(),
        feature_name: "报告解析失败".to_string(),
        severity: AiCheckSeverity::Weak,
        explanation: format!(
            "LLM 返回的内容无法解析为 JSON（已尝试剥思考链/剥围栏/修复引号），\
             建议重试一次。原始内容开头：\n\n{excerpt}{ellipsis}"
        ),
        fragment_id: None,
        fragment_uncertain: false,
    }]
}

fn try_parse_json(s: &str) -> Option<LlmReport> {
    serde_json::from_str::<LlmReport>(s).ok()
}

/// 修复 JSON 字符串值里未转义的 ASCII 双引号。
///
/// LLM 常在 summary/explanation 里写 `来"升华"情绪` 这种嵌套引号，破坏 JSON。
/// 宽松提取：扫描所有花括号匹配的 {...} 子串（含嵌套），逐个尝试解析为单个 item。
/// 能救一个是一个，丢掉 summary 也无所谓。非 item 结构（如整个 report 对象）会自然解析失败被跳过。
fn loose_extract_items(input: &str) -> Vec<AiCheckItem> {
    let chars: Vec<char> = input.chars().collect();
    // 收集所有 {..} 匹配区间（按 { 出现的位置栈式匹配）
    let candidates = crate::llm_json::collect_brace_spans(&chars);
    let mut items = Vec::new();
    for (s, e) in candidates {
        let obj: String = chars[s..=e].iter().collect();
        // 先原样解析，失败则修复引号后重试
        let parsed = serde_json::from_str::<LlmItem>(&obj).or_else(|_| {
            serde_json::from_str::<LlmItem>(&crate::llm_json::repair_unescaped_quotes(&obj))
        });
        if let Ok(item) = parsed {
            items.push(AiCheckItem {
                quote: item.quote,
                feature_id: item.feature_id,
                feature_name: item.feature_name,
                severity: parse_severity(&item.severity),
                explanation: item.explanation,
                fragment_id: None,
                fragment_uncertain: false,
            });
        }
    }
    items
}

fn report_to_items(report: LlmReport) -> Vec<AiCheckItem> {
    report
        .items
        .into_iter()
        .map(|item| AiCheckItem {
            quote: item.quote,
            feature_id: item.feature_id,
            feature_name: item.feature_name,
            severity: parse_severity(&item.severity),
            explanation: item.explanation,
            fragment_id: None,
            fragment_uncertain: false,
        })
        .collect()
}

fn parse_severity(s: &str) -> AiCheckSeverity {
    match s.to_lowercase().as_str() {
        "strong" => AiCheckSeverity::Strong,
        "medium" => AiCheckSeverity::Medium,
        _ => AiCheckSeverity::Weak,
    }
}

/// 从原始文本中提取 summary（解析失败时用）
fn extract_summary(raw: &str) -> Option<String> {
    let stripped = crate::llm_json::strip_think(raw);
    let cleaned = stripped.trim();
    // 先原样尝试（含围栏/对象提取），失败则修复未转义引号后再试
    if let Some(report) = try_parse_any(cleaned) {
        return Some(report.summary);
    }
    let repaired = crate::llm_json::repair_unescaped_quotes(cleaned);
    let report = try_parse_any(&repaired)?;
    Some(report.summary)
}

/// 依次尝试：直接解析 → 围栏提取 → 对象提取
fn try_parse_any(s: &str) -> Option<LlmReport> {
    try_parse_json(s)
        .or_else(|| crate::llm_json::extract_json_fence(s).and_then(|j| try_parse_json(&j)))
        .or_else(|| crate::llm_json::extract_json_object(s).and_then(|j| try_parse_json(&j)))
}

fn default_summary(items: &[AiCheckItem]) -> String {
    let real: Vec<_> = items
        .iter()
        .filter(|i| i.feature_id != "parse_error")
        .collect();
    if real.is_empty() {
        return "未发现明显 AI 特征".to_string();
    }
    let strong = real
        .iter()
        .filter(|i| i.severity == AiCheckSeverity::Strong)
        .count();
    if strong > 0 {
        format!("最突出的问题有 {strong} 处强信号，建议优先处理")
    } else {
        format!("共命中 {} 处特征，多为中弱信号", real.len())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户真实案例：LLM 在 summary 里嵌了未转义双引号 `来"升华"情绪`，
    /// 破坏了整个 JSON 结构。修复后应能正确提取出 2 个 item。
    #[test]
    fn test_parse_unescaped_quotes_in_summary() {
        let raw = r#"```json
{
  "items": [
    {
      "quote": "画师做的事情，就是把模糊的东西固定住。",
      "feature_id": "13",
      "feature_name": "每个段落都有收束金句",
      "severity": "Medium",
      "explanation": "在画遗像的情节末尾突然拔高，用一句工整的格言式总结收尾。"
    },
    {
      "quote": "他来了，画了两个月的画，赚了一点钱，现在要走了。",
      "feature_id": "3",
      "feature_name": "匀速排比",
      "severity": "Medium",
      "explanation": "三个结构对称、长度相近的分句机械地罗列主角的经历。"
    }
  ],
  "summary": "整体口语化、毛边感足，AI 味很淡。仅在两处段落收尾暴露了 AI 习惯：喜欢用格言式金句或匀速排比来"升华"情绪。"
}
```"#;

        let items = parse_report(raw);

        // 不应出现 parse_error
        assert!(
            !items.iter().any(|i| i.feature_id == "parse_error"),
            "应成功解析，不应降级为 parse_error；实际：{items:?}"
        );
        // 应提取出 2 个 item
        assert_eq!(
            items.len(),
            2,
            "应解析出 2 个 item；实际 {} 个",
            items.len()
        );
        assert_eq!(items[0].feature_id, "13");
        assert_eq!(items[0].severity, AiCheckSeverity::Medium);
        assert_eq!(items[1].feature_id, "3");
        assert_eq!(
            items[1].quote,
            "他来了，画了两个月的画，赚了一点钱，现在要走了。"
        );
    }

    /// summary 也应被修复后提取出来
    #[test]
    fn test_extract_summary_with_unescaped_quotes() {
        let raw = r#"{"items":[],"summary":"喜欢来"升华"情绪。"}"#;
        let summary = extract_summary(raw);
        assert!(summary.is_some(), "summary 应被修复提取");
        assert!(summary.unwrap().contains("升华"));
    }

    /// 正常 JSON 应不受影响
    #[test]
    fn test_parse_normal_json() {
        let raw = r#"{"items":[{"quote":"x","feature_id":"A","feature_name":"test","severity":"Strong","explanation":"y"}],"summary":"ok"}"#;
        let items = parse_report(raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature_id, "A");
        assert_eq!(items[0].severity, AiCheckSeverity::Strong);
    }

    /// repair_unescaped_quotes 不应破坏键名和结构引号
    #[test]
    fn test_repair_preserves_structure() {
        let input = r#"{"key":"value","nested":"a"b"c"}"#;
        let repaired = crate::llm_json::repair_unescaped_quotes(input);
        // 结构性引号和键名引号应保持为 ASCII "
        assert!(
            repaired.contains(r#""key":"value""#),
            "键值对应完整保留: {repaired}"
        );
        // 值内部的引号应被转义保留（不改动内容）
        assert!(
            repaired.contains(r#""nested":"a\"b\"c""#),
            "内部引号应转义: {repaired}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&repaired).is_ok());
    }

    /// 完全无法解析的内容应降级为 parse_error
    #[test]
    fn test_parse_garbage_fallback() {
        let items = parse_report("这不是 JSON，也没有花括号");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].feature_id, "parse_error");
    }

    // ========================================================================
    // check_scope（场景/章节批量）测试
    // ========================================================================

    #[derive(Debug)]
    struct MockCheckProvider {
        response: String,
    }
    impl AiProvider for MockCheckProvider {
        fn id(&self) -> &str {
            "mock-check"
        }
        fn generate(
            &self,
            _request: &story_ai_runtime::provider::GenerateRequest,
        ) -> Result<story_ai_runtime::provider::GenerateResponse, story_ai_runtime::provider::AiError>
        {
            Ok(story_ai_runtime::provider::GenerateResponse {
                text: self.response.clone(),
                usage: story_ai_runtime::provider::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                stop_reason: story_ai_runtime::provider::StopReason::EndTurn,
                tool_calls: Vec::new(),
            })
        }
    }

    fn setup_scope_project() -> (tempfile::TempDir, String, String, Vec<String>) {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "t").unwrap();
        let paths = ProjectPaths::new(tmp.path());

        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let f1 = crate::mutations::create_fragment(&paths, &sc, "第一段正文，字字如金。").unwrap();
        let f2 =
            crate::mutations::create_fragment(&paths, &sc, "第二段正文，情绪没有名字。").unwrap();
        (tmp, ch, sc, vec![f1, f2])
    }

    #[test]
    fn check_scope_chapter_attributes_items_to_fragments() {
        let (tmp, ch, _sc, fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: r#"{"items":[{"quote":"第二段正文","feature_id":"3","feature_name":"匀速排比","severity":"Medium","explanation":"测试归因到第二片段。"}],"summary":"整体尚可。"}"#.to_string(),
        });

        let report = check_scope(tmp.path(), CheckScope::Chapter, &ch, &provider).unwrap();
        assert_eq!(report.scope, "chapter");
        assert_eq!(report.hit_count(), 1);

        // quote 出现在 f2 → 归因到 f2（精确命中，非推测）
        assert_eq!(
            report.items[0].fragment_id.as_deref(),
            Some(fids[1].as_str())
        );
        assert!(!report.items[0].fragment_uncertain);

        // 覆盖两个片段，命中分布正确
        assert_eq!(report.fragment_hits.len(), 2);
        assert_eq!(report.fragment_hits[0], (fids[0].clone(), 0));
        assert_eq!(report.fragment_hits[1], (fids[1].clone(), 1));
        assert_eq!(report.summary, "整体尚可。");
    }

    /// 用户真实案例：LLM 改写了引文（「没有名字」写成「没有影子」），
    /// 精确 contains 失配。旧行为兜底归到块首片段 f1，应改为 LCS 模糊归到 f2。
    #[test]
    fn check_scope_paraphrased_quote_attributed_by_lcs() {
        let (tmp, ch, _sc, fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: r#"{"items":[{"quote":"第二段正文，情绪没有影子。","feature_id":"3","feature_name":"匀速排比","severity":"Medium","explanation":"引文被轻微改写。"}],"summary":"整体尚可。"}"#.to_string(),
        });

        let report = check_scope(tmp.path(), CheckScope::Chapter, &ch, &provider).unwrap();
        assert_eq!(
            report.items[0].fragment_id.as_deref(),
            Some(fids[1].as_str()),
            "改写引文应按最长公共子串归到 f2，而不是兜底块首 f1"
        );
        assert!(!report.items[0].fragment_uncertain);
        assert_eq!(report.fragment_hits[1], (fids[1].clone(), 1));
    }

    /// 引文把片段内跨段落的文字拼成一句（去掉换行），归一化后应精确命中。
    #[test]
    fn check_scope_joined_quote_attributed_after_whitespace_folding() {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "t").unwrap();
        let paths = ProjectPaths::new(tmp.path());
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let f1 = crate::mutations::create_fragment(&paths, &sc, "第一段正文，字字如金。").unwrap();
        let f2 =
            crate::mutations::create_fragment(&paths, &sc, "他停了很久。\n\n她说不出话。").unwrap();

        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: r#"{"items":[{"quote":"他停了很久。她说不出话。","feature_id":"13","feature_name":"段段金句","severity":"Weak","explanation":"拼接了两个段落。"}],"summary":"整体尚可。"}"#.to_string(),
        });

        let report = check_scope(tmp.path(), CheckScope::Chapter, &ch, &provider).unwrap();
        assert_eq!(
            report.items[0].fragment_id.as_deref(),
            Some(f2.as_str()),
            "去空白后应命中 f2，而不是兜底块首 {f1}"
        );
    }

    /// 引文与任何片段都对不上（严重改写或幻觉）：归块首片段，但标记推测。
    #[test]
    fn check_scope_unmatchable_quote_marks_uncertain() {
        let (tmp, ch, _sc, fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: r#"{"items":[{"quote":"这段话在正文里根本不存在。","feature_id":"B","feature_name":"对话千人一面","severity":"Medium","explanation":"完全对不上。"}],"summary":"整体尚可。"}"#.to_string(),
        });

        let report = check_scope(tmp.path(), CheckScope::Chapter, &ch, &provider).unwrap();
        assert_eq!(
            report.items[0].fragment_id.as_deref(),
            Some(fids[0].as_str())
        );
        assert!(
            report.items[0].fragment_uncertain,
            "对不上的引文应标记归属推测"
        );
    }

    #[test]
    fn check_scope_scene_only_covers_scene_fragments() {
        let (tmp, _ch, sc, fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: r#"{"items":[],"summary":"未发现明显 AI 特征。"}"#.to_string(),
        });

        let report = check_scope(tmp.path(), CheckScope::Scene, &sc, &provider).unwrap();
        assert_eq!(report.scope, "scene");
        let covered: Vec<&String> = report.fragment_hits.iter().map(|(f, _)| f).collect();
        assert_eq!(covered, fids.iter().collect::<Vec<_>>());
    }

    #[test]
    fn check_scope_missing_scope_errors() {
        let (tmp, _ch, _sc, _) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: "{}".to_string(),
        });
        assert!(check_scope(tmp.path(), CheckScope::Chapter, "ch_99", &provider).is_err());
        assert!(check_scope(tmp.path(), CheckScope::Scene, "sc_99", &provider).is_err());
    }

    #[test]
    fn check_records_to_ai_ledger() {
        let (tmp, _ch, _sc, fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: "{\"items\": [], \"summary\": \"没有明显 AI 味\"}".to_string(),
        });

        // 单片段检测 → 台账记一条「AI 审校」成功记录
        let report = check_fragment(tmp.path(), &fids[0], &provider).unwrap();
        assert!(report.summary.contains("AI 味"));
        let ledger = crate::ai_ledger::summary(tmp.path(), 10);
        assert_eq!(ledger.total_calls, 1, "审校应计入 AI 台账");
        assert_eq!(ledger.recent[0].scene, "AI 审校");
        assert!(ledger.recent[0].ok, "{:?}", ledger.recent[0]);

        // 失败（片段不存在）也记账
        let _ = check_fragment(tmp.path(), "f_9999", &provider);
        let ledger = crate::ai_ledger::summary(tmp.path(), 10);
        assert_eq!(ledger.total_calls, 2);
        assert!(
            !ledger.recent[0].ok,
            "失败记录在最前: {:?}",
            ledger.recent[0]
        );
    }

    // ========================================================================
    // 并行分块 / 带时限转后台 测试
    // ========================================================================

    /// 两块并行检测（两片段各 6500 字，超出 6000 字/块上限）：命中项按
    /// 阅读序归并、归因正确；mock 按请求内容区分所在块返回
    #[test]
    fn check_scope_parallel_chunks_merge_in_reading_order() {
        let tmp = tempfile::tempdir().unwrap();
        story_project_format::init::init_project(tmp.path(), "t").unwrap();
        let paths = ProjectPaths::new(tmp.path());
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc = crate::mutations::create_scene(&paths, &ch, "场景", None).unwrap();
        let f1 = crate::mutations::create_fragment(
            &paths,
            &sc,
            &format!("{}第一块标记", "很".repeat(6500)),
        )
        .unwrap();
        let f2 = crate::mutations::create_fragment(
            &paths,
            &sc,
            &format!("{}第二块标记", "静".repeat(6500)),
        )
        .unwrap();

        #[derive(Debug)]
        struct ChunkAwareProvider;
        impl AiProvider for ChunkAwareProvider {
            fn id(&self) -> &str {
                "mock-chunks"
            }
            fn generate(
                &self,
                request: &story_ai_runtime::provider::GenerateRequest,
            ) -> Result<
                story_ai_runtime::provider::GenerateResponse,
                story_ai_runtime::provider::AiError,
            > {
                let (quote, feature) = if request.messages[0].content.contains("第一块标记") {
                    ("第一块标记", "A")
                } else {
                    ("第二块标记", "B")
                };
                Ok(story_ai_runtime::provider::GenerateResponse {
                    text: format!(
                        "{{\"items\":[{{\"quote\":\"{quote}\",\"feature_id\":\"{feature}\",\
                         \"feature_name\":\"测试特征\",\"severity\":\"Medium\",\"explanation\":\"x\"}}],\
                         \"summary\":\"ok\"}}"
                    ),
                    usage: story_ai_runtime::provider::TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    stop_reason: story_ai_runtime::provider::StopReason::EndTurn,
                    tool_calls: Vec::new(),
                })
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(ChunkAwareProvider);
        let report = check_scope(tmp.path(), CheckScope::Chapter, &ch, &provider).unwrap();

        // 命中项按阅读序：块 1（f1）的命中在前，且归因正确、token 为两块之和
        assert_eq!(report.items.len(), 2, "{:?}", report.items);
        assert_eq!(report.items[0].feature_id, "A");
        assert_eq!(report.items[0].fragment_id.as_deref(), Some(f1.as_str()));
        assert!(!report.items[0].fragment_uncertain);
        assert_eq!(report.items[1].feature_id, "B");
        assert_eq!(report.items[1].fragment_id.as_deref(), Some(f2.as_str()));
        assert_eq!(report.tokens_used, 4, "每块 usage.total()=2，两块应为 4");
    }

    /// 慢 provider + 小预算：立即返回 Deferred，后台收齐后经回调送出
    /// 完整报告并记账（token 不白烧）
    #[test]
    fn check_scope_bounded_defers_slow_provider_and_delivers() {
        let (tmp, ch, _sc, fids) = setup_scope_project();

        #[derive(Debug)]
        struct SlowProvider {
            response: String,
        }
        impl AiProvider for SlowProvider {
            fn id(&self) -> &str {
                "mock-slow"
            }
            fn generate(
                &self,
                _request: &story_ai_runtime::provider::GenerateRequest,
            ) -> Result<
                story_ai_runtime::provider::GenerateResponse,
                story_ai_runtime::provider::AiError,
            > {
                std::thread::sleep(std::time::Duration::from_millis(300));
                Ok(story_ai_runtime::provider::GenerateResponse {
                    text: self.response.clone(),
                    usage: story_ai_runtime::provider::TokenUsage {
                        input_tokens: 10,
                        output_tokens: 20,
                    },
                    stop_reason: story_ai_runtime::provider::StopReason::EndTurn,
                    tool_calls: Vec::new(),
                })
            }
        }
        let provider: Arc<dyn AiProvider> = Arc::new(SlowProvider {
            response: r#"{"items":[{"quote":"第二段正文","feature_id":"3","feature_name":"匀速排比","severity":"Medium","explanation":"后台送达"}],"summary":"整体尚可。"}"#.to_string(),
        });

        let (tx, rx) = std::sync::mpsc::channel();
        let run = check_scope_bounded(
            tmp.path(),
            CheckScope::Chapter,
            &ch,
            &provider,
            std::time::Duration::from_millis(50),
            move |result| {
                let _ = tx.send(result);
            },
        )
        .unwrap();
        match run {
            ScopeCheckRun::Deferred { completed, total } => {
                assert_eq!(total, 1, "两个小片段合成一块");
                assert_eq!(completed, 0, "300ms 的块在 50ms 预算内不可能收齐");
            }
            ScopeCheckRun::Done(_) => panic!("慢 provider + 50ms 预算应转后台而非同步返回"),
        }

        // 后台收齐后：完整报告经回调送达、归因正确，台账恰记一次
        let report = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("后台收齐应送达回调")
            .expect("后台组装应成功");
        assert_eq!(report.hit_count(), 1);
        assert_eq!(
            report.items[0].fragment_id.as_deref(),
            Some(fids[1].as_str())
        );
        let ledger = crate::ai_ledger::summary(tmp.path(), 10);
        assert_eq!(ledger.total_calls, 1, "Deferred 的台账应由后台收齐后记");
        assert!(ledger.recent[0].ok, "{:?}", ledger.recent[0]);
    }

    /// 快 provider + 宽预算：同步返回 Done，台账恰记一次、不触发后台回调
    #[test]
    fn check_scope_bounded_fast_path_returns_done() {
        let (tmp, ch, _sc, _fids) = setup_scope_project();
        let provider: Arc<dyn AiProvider> = Arc::new(MockCheckProvider {
            response: "{\"items\": [], \"summary\": \"没有明显 AI 味\"}".to_string(),
        });
        let run = check_scope_bounded(
            tmp.path(),
            CheckScope::Chapter,
            &ch,
            &provider,
            std::time::Duration::from_secs(10),
            |_result| panic!("快路径不应触发后台回调"),
        )
        .unwrap();
        assert!(matches!(run, ScopeCheckRun::Done(_)));
        let ledger = crate::ai_ledger::summary(tmp.path(), 10);
        assert_eq!(ledger.total_calls, 1, "Done 路径台账恰记一次");
    }
}
