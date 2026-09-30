//! 逐章生文流水线（Chapter Workflow）
//!
//! 把 chapters.toml 的逐章脉络自动落地为正文：
//! 取下一章 → 生成节拍 → 落实 structure → 流式生成整章 → 经 mutations 提交
//! → 质检（确定性编译器 + 规则检查）→ 硬伤停机等人决策 / 通过则推进状态。
//!
//! 设计要点：
//! - **停机等确认 = 返回 Paused**：不阻塞线程等待人。chapters.toml 的
//!   status（planned→writing→completed）本身就是持久化游标；断点细节
//!   存 `.novelstudio/workflow/state.json`，流式半成品存
//!   `.novelstudio/workflow/drafts/<idx>.md`（崩溃/取消不丢已生成部分）。
//! - **质检硬伤**：字数不足、输出截断（MaxTokens）、整段复读、编译器
//!   新增 Error（与运行开始时的基线诊断 diff，存量问题不计数）。
//! - **写入全部走 mutations 层**
//!   同一份变更逻辑。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use story_ai_runtime::provider::{AiError, AiProvider, GenerateRequest, Message, StreamEvent};
use story_ai_runtime::stream_generate_with_cancel;
use story_ai_runtime::task::{ChapterMaterials, Task, WriteChapterTask};
use story_ai_runtime::CancelToken;
use story_ir::{StableId, StoryIR};
use story_project_format::chapters_file::{STATUS_COMPLETED, STATUS_PLANNED, STATUS_WRITING};
use story_project_format::{ChapterOutlineDef, ChaptersFile, OutlineFile, ProjectPaths};

use crate::compile::compile_project;
use crate::error::StudioError;
use crate::mutations;

// ============================================================================
// 类型
// ============================================================================

/// 流水线运行配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// 每章目标字数
    pub target_words: u32,
    /// 字数硬伤阈值比例（实际字数 < target × min_ratio 判定字数不足）
    pub min_ratio: f32,
    /// 单章最大重写次数（记录用；重写由用户决策驱动）
    pub max_rewrites: u32,
    /// 本次调用最多生成的章数（None = 不限，直到暂停或全部完成）
    pub max_chapters: Option<u32>,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            target_words: 2000,
            min_ratio: 0.6,
            max_rewrites: 2,
            max_chapters: None,
        }
    }
}

/// 恢复动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowAction {
    /// 正常运行 / 断点续写
    Run,
    /// 批准当前暂停的章节（接受现状，推进到 completed）
    Approve,
    /// 重写当前暂停的章节（带原稿参考与失败原因）
    Rewrite,
    /// 跳过当前暂停的章节（标记 completed，交还用户处理）
    Skip,
}

/// 质检问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityIssue {
    /// 问题代码：word_count / truncated / dup_paragraph / compile_error / compile_warning
    pub code: String,
    /// 硬伤（true = 停机等决策）；软伤仅报告
    pub hard: bool,
    pub message: String,
}

/// 暂停信息（持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseInfo {
    /// 章节脉络 index（1-based）
    pub index: usize,
    pub title: String,
    pub issues: Vec<QualityIssue>,
    /// 已重写次数
    pub rewrites_used: u32,
}

/// 持久化运行状态（.novelstudio/workflow/state.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub version: u32,
    /// idle / paused / done
    pub phase: String,
    pub paused: Option<PauseInfo>,
    pub config: WorkflowConfig,
    /// 运行开始时的基线诊断签名（code: message），用于区分存量与新增问题
    pub baseline: Vec<String>,
    pub updated_at: String,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            version: 1,
            phase: "idle".to_string(),
            paused: None,
            config: WorkflowConfig::default(),
            baseline: Vec::new(),
            updated_at: String::new(),
        }
    }
}

/// 流水线事件
#[derive(Debug, Clone, Serialize)]
pub enum WorkflowEvent {
    RunStarted {
        /// planned + writing 的章数
        pending: usize,
        /// 是否断点续写（存在未完成进度）
        resume: bool,
    },
    ChapterStarted {
        index: usize,
        title: String,
    },
    BeatsGenerated {
        index: usize,
        beats: Vec<String>,
    },
    ChapterMaterialized {
        index: usize,
        chapter_id: String,
        scene_id: String,
    },
    Delta {
        index: usize,
        delta: String,
    },
    ChapterCommitted {
        index: usize,
        fragment_id: String,
        words: u32,
    },
    QualityCheck {
        index: usize,
        passed: bool,
        issues: Vec<QualityIssue>,
    },
    ChapterCompleted {
        index: usize,
    },
    Paused {
        index: usize,
        title: String,
        issues: Vec<QualityIssue>,
    },
    Cancelled,
    Finished {
        outcome: String,
        completed_this_run: usize,
        /// 仍为 planned/writing 的章数
        remaining: usize,
    },
}

/// 单次调用结果
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowReport {
    /// done / paused / cancelled / batch_limit
    pub outcome: String,
    pub completed_this_run: usize,
    pub remaining: usize,
    pub paused: Option<PauseInfo>,
}

// ============================================================================
// 运行互斥 + 取消注册表
// ============================================================================

/// 单项目同一时间只允许一个流水线运行（跨进程无锁——文件层由用户自律，
static RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 当前注册的取消 token（进程内唯一运行）
static CANCEL_SLOT: OnceLock<RwLock<Option<CancelToken>>> = OnceLock::new();

/// 取消正在运行的流水线。
pub fn workflow_cancel() -> bool {
    let slot = CANCEL_SLOT.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = slot.read() {
        if let Some(token) = guard.as_ref() {
            token.cancel();
            return true;
        }
    }
    false
}

fn register_cancel(token: &CancelToken) {
    let slot = CANCEL_SLOT.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = slot.write() {
        *guard = Some(token.clone());
    }
}

fn clear_cancel() {
    let slot = CANCEL_SLOT.get_or_init(|| RwLock::new(None));
    if let Ok(mut guard) = slot.write() {
        *guard = None;
    }
}

// ============================================================================
// 主入口
// ============================================================================

pub type WorkflowEventCallback = Arc<dyn Fn(WorkflowEvent) + Send + Sync>;

/// 运行逐章生文流水线。
///
/// 阻塞直到：全部完成（done）、质检硬伤暂停（paused）、用户取消
/// （cancelled）或达到 `max_chapters`（batch_limit）。暂停后的决策通过
/// `action` 再次调用本函数表达（Approve / Rewrite / Skip）。
///
/// `provider` 以 Arc 传入：流式生成的取消依赖 'static 线程。
pub fn run_workflow(
    dir: &Path,
    provider: Arc<dyn AiProvider>,
    action: WorkflowAction,
    config: Option<WorkflowConfig>,
    on_event: WorkflowEventCallback,
    cancel_token: &CancelToken,
) -> Result<WorkflowReport, StudioError> {
    let _guard = RUN_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| StudioError::Ai("流水线运行锁中毒".into()))?;

    register_cancel(cancel_token);

    let result = run_workflow_inner(dir, provider, action, config, on_event, cancel_token);

    clear_cancel();
    result
}

fn run_workflow_inner(
    dir: &Path,
    provider: Arc<dyn AiProvider>,
    action: WorkflowAction,
    config: Option<WorkflowConfig>,
    on_event: WorkflowEventCallback,
    cancel_token: &CancelToken,
) -> Result<WorkflowReport, StudioError> {
    let paths = ProjectPaths::new(dir);
    let workflow_dir = workflow_dir(dir);
    std::fs::create_dir_all(workflow_dir.join("drafts"))?;

    let mut state = load_state(dir)?.unwrap_or_default();
    if let Some(cfg) = &config {
        state.config = cfg.clone();
    }

    // 重写目标（Rewrite 动作设定，循环内优先处理）与其质检上下文
    let mut rewrite_target: Option<usize> = None;
    let mut rewrite_pause_ctx: Option<PauseInfo> = None;

    match action {
        WorkflowAction::Approve | WorkflowAction::Skip => {
            let pause = state
                .paused
                .take()
                .ok_or_else(|| StudioError::Config("流水线当前没有暂停，无需批准/跳过".into()))?;
            mutations::set_chapter_outline_status(&paths, pause.index, STATUS_COMPLETED)?;
            let _ = std::fs::remove_file(draft_path(dir, pause.index));
            state.phase = "idle".to_string();
            save_state(dir, &state)?;
            on_event(WorkflowEvent::ChapterCompleted { index: pause.index });
        }
        WorkflowAction::Rewrite => {
            let pause = state
                .paused
                .take()
                .ok_or_else(|| StudioError::Config("流水线当前没有暂停，无法重写".into()))?;
            if pause.rewrites_used >= state.config.max_rewrites {
                return Err(StudioError::Config(format!(
                    "本章已重写 {} 次，达到上限 {}，请批准或跳过",
                    pause.rewrites_used, state.config.max_rewrites
                )));
            }
            rewrite_target = Some(pause.index);
            rewrite_pause_ctx = Some(pause);
            state.phase = "idle".to_string();
            save_state(dir, &state)?;
        }
        WorkflowAction::Run => {
            if state.phase == "paused" {
                return Err(StudioError::Config(
                    "流水线处于暂停状态，请用 approve / rewrite / skip 决策后继续".into(),
                ));
            }
            // 基线诊断：首次运行时快照；已有基线则沿用（用户可能已修复存量问题，
            // 沿用最多导致漏报新增，不会误报存量）
            if state.baseline.is_empty() {
                let baseline_result = compile_project(dir)?;
                state.baseline = diagnostic_signatures(&baseline_result);
            }
        }
    }

    let cfg = state.config.clone();
    let mut completed_this_run = 0usize;

    on_event(WorkflowEvent::RunStarted {
        pending: pending_count(&paths)?,
        resume: action == WorkflowAction::Run && had_progress(&paths, dir),
    });

    loop {
        if cancel_token.is_cancelled() {
            on_event(WorkflowEvent::Cancelled);
            state.phase = "idle".to_string();
            save_state(dir, &state)?;
            let report = finish_report("cancelled", completed_this_run, &paths, None);
            emit_finished(&on_event, &report);
            return Ok(report);
        }

        // 取目标章：Rewrite 指定章 > 第一个 writing（中断续写）> 第一个 planned
        let chapters = mutations::load_chapters_file(&paths)?;
        let target = match rewrite_target.take() {
            Some(idx) => chapters
                .chapters
                .get(idx.checked_sub(1).unwrap_or(usize::MAX))
                .map(|c| (idx, c.clone())),
            None => chapters
                .chapters
                .iter()
                .enumerate()
                .find(|(_, c)| c.status == STATUS_WRITING || c.status == STATUS_PLANNED)
                .map(|(i, c)| (i + 1, c.clone())),
        };

        let Some((index, def)) = target else {
            // 没有可做的章节了
            state.phase = "done".to_string();
            state.paused = None;
            save_state(dir, &state)?;
            let report = finish_report("done", completed_this_run, &paths, None);
            emit_finished(&on_event, &report);
            return Ok(report);
        };

        on_event(WorkflowEvent::ChapterStarted {
            index,
            title: def.title.clone(),
        });

        // ---- 1. 节拍（beats 为空时先生成并立即持久化 = checkpoint） ----
        if def.beats.is_empty() {
            let beats = generate_beats(dir, &provider, &chapters, index, cancel_token)?;
            if beats.is_empty() {
                return Err(StudioError::Ai("节拍生成为空，无法继续".into()));
            }
            mutations::update_chapter_outline_beats(&paths, index, beats.clone())?;
            on_event(WorkflowEvent::BeatsGenerated {
                index,
                beats: beats.clone(),
            });
        }

        // ---- 2. 落实 structure（chapter / scene / 回填）= checkpoint ----
        let chapter_id = match &def.chapter_id {
            Some(id) => id.clone(),
            None => {
                let id = mutations::create_chapter(&paths, &def.title)?;
                mutations::backfill_chapter_outline_id(&paths, index, &id)?;
                id
            }
        };
        let scene_id = match def.scenes.first() {
            Some(sid) => sid.clone(),
            None => {
                let pov = def.characters.first().cloned();
                let sid = mutations::create_scene(&paths, &chapter_id, &def.title, pov.as_deref())?;
                mutations::append_chapter_outline_scene(&paths, index, &sid)?;
                sid
            }
        };
        if def.status == STATUS_PLANNED {
            mutations::set_chapter_outline_status(&paths, index, STATUS_WRITING)?;
        }
        on_event(WorkflowEvent::ChapterMaterialized {
            index,
            chapter_id: chapter_id.clone(),
            scene_id: scene_id.clone(),
        });

        // ---- 3. 上下文组装 + 流式生成 ----
        let is_rewrite = rewrite_pause_ctx.is_some();
        let rewrite_pause = rewrite_pause_ctx.take();
        let partial = if is_rewrite {
            String::new() // 重写从头生成
        } else {
            read_partial_draft(dir, index).unwrap_or_default()
        };

        let compile_result = compile_project(dir)?;
        let materials = build_materials(
            &paths,
            &chapters,
            index,
            &compile_result.ir,
            &cfg,
            &partial,
            &rewrite_pause,
        )?;
        let task = WriteChapterTask { materials };
        let ctx = task.build_context(&compile_result.ir);
        let request = GenerateRequest {
            system: ctx.system,
            messages: vec![Message::user(ctx.user)],
            model: String::new(), // provider 用自身配置的默认模型
            temperature: ctx.model.temperature,
            max_tokens: ctx.model.max_tokens,
            tools: Vec::new(),
        };

        // 流式 + 每 2s 落盘半成品 = checkpoint
        let chapter_gen_started = std::time::Instant::now();
        let dp = draft_path(dir, index);
        let prefix = partial.clone();
        let generated = Mutex::new(String::new());
        let last_flush = Mutex::new(Instant::now());
        let idx_for_cb = index;
        let ev_cb = Arc::clone(&on_event);
        let stream_result = stream_generate_with_cancel(
            Arc::clone(&provider),
            &request,
            cancel_token,
            Arc::new(move |event| {
                if let StreamEvent::Delta { delta } = event {
                    ev_cb(WorkflowEvent::Delta {
                        index: idx_for_cb,
                        delta: delta.clone(),
                    });
                    if let Ok(mut g) = generated.lock() {
                        g.push_str(&delta);
                    }
                    if let Ok(mut t) = last_flush.lock() {
                        if t.elapsed() >= Duration::from_secs(2) {
                            let snapshot = format!(
                                "{}{}",
                                prefix,
                                generated.lock().map(|g| g.clone()).unwrap_or_default()
                            );
                            let _ = std::fs::write(&dp, snapshot);
                            *t = Instant::now();
                        }
                    }
                }
            }),
        );

        let chapter_label = format!("逐章成书·{}", def.title);
        match &stream_result {
            Ok(resp) => crate::ai_ledger::record(
                dir,
                &chapter_label,
                provider.id(),
                resp.usage.total() as u64,
                chapter_gen_started.elapsed().as_millis() as u64,
                true,
                None,
            ),
            Err(e) => crate::ai_ledger::record(
                dir,
                &chapter_label,
                provider.id(),
                0,
                chapter_gen_started.elapsed().as_millis() as u64,
                false,
                Some(&e.to_string()),
            ),
        }
        let response = match stream_result {
            Ok(resp) => resp,
            Err(AiError::Cancelled) => {
                // 取消：保留半成品草稿文件，章节停留在 writing，下次 Run 续写
                on_event(WorkflowEvent::Cancelled);
                state.phase = "idle".to_string();
                save_state(dir, &state)?;
                let report = finish_report("cancelled", completed_this_run, &paths, None);
                emit_finished(&on_event, &report);
                return Ok(report);
            }
            Err(e) => return Err(StudioError::Ai(format!("章节生成失败: {e}"))),
        };

        // ---- 4. 提交（mutations 层） ----
        let gen_text = response.text.trim().to_string();
        if gen_text.is_empty() {
            return Err(StudioError::Ai(format!(
                "第 {index} 章生成结果为空（stop_reason: {:?}）",
                response.stop_reason
            )));
        }
        let drafts = task
            .parse_output(&gen_text)
            .map_err(|e| StudioError::Ai(format!("第 {index} 章输出解析失败: {e}")))?;
        let draft = drafts
            .first()
            .ok_or_else(|| StudioError::Ai("章节输出解析为空".to_string()))?;
        let final_text = if partial.is_empty() {
            draft.text.trim().to_string()
        } else {
            format!("{partial}{}", draft.text.trim())
        };

        // 重写时复用已有 fragment（ID 稳定），否则新建
        let existing_fragment = first_fragment_of_scene(&compile_result.ir, &scene_id);
        let fragment_id = match &existing_fragment {
            Some(fid) => {
                mutations::write_fragment_text_anchored(&paths, fid, &final_text, &draft.anchors)?;
                fid.clone()
            }
            None => {
                mutations::create_fragment_anchored(&paths, &scene_id, &final_text, &draft.anchors)?
            }
        };

        let words = count_words(&final_text);
        on_event(WorkflowEvent::ChapterCommitted {
            index,
            fragment_id: fragment_id.clone(),
            words,
        });

        // ---- 5. 质检 ----
        let issues = quality_checks(dir, &final_text, words, &cfg, response.stop_reason, &state)?;
        let passed = !issues.iter().any(|i| i.hard);
        on_event(WorkflowEvent::QualityCheck {
            index,
            passed,
            issues: issues.clone(),
        });

        if passed {
            mutations::set_chapter_outline_status(&paths, index, STATUS_COMPLETED)?;
            let _ = std::fs::remove_file(draft_path(dir, index));
            on_event(WorkflowEvent::ChapterCompleted { index });
            completed_this_run += 1;

            if let Some(max) = cfg.max_chapters {
                if completed_this_run >= max as usize {
                    let report = finish_report("batch_limit", completed_this_run, &paths, None);
                    emit_finished(&on_event, &report);
                    return Ok(report);
                }
            }
        } else {
            let rewrites_used = rewrite_pause
                .as_ref()
                .map(|p| p.rewrites_used + 1)
                .unwrap_or(0);
            let pause = PauseInfo {
                index,
                title: def.title.clone(),
                issues,
                rewrites_used,
            };
            state.phase = "paused".to_string();
            state.paused = Some(pause.clone());
            save_state(dir, &state)?;
            on_event(WorkflowEvent::Paused {
                index,
                title: pause.title.clone(),
                issues: pause.issues.clone(),
            });
            let report = finish_report("paused", completed_this_run, &paths, Some(pause));
            emit_finished(&on_event, &report);
            return Ok(report);
        }
    }
}

fn emit_finished(cb: &WorkflowEventCallback, report: &WorkflowReport) {
    cb(WorkflowEvent::Finished {
        outcome: report.outcome.clone(),
        completed_this_run: report.completed_this_run,
        remaining: report.remaining,
    });
}

fn finish_report(
    outcome: &str,
    completed_this_run: usize,
    paths: &ProjectPaths,
    paused: Option<PauseInfo>,
) -> WorkflowReport {
    let remaining = mutations::load_chapters_file(paths)
        .map(|f| {
            f.chapters
                .iter()
                .filter(|c| c.status != STATUS_COMPLETED)
                .count()
        })
        .unwrap_or(0);
    WorkflowReport {
        outcome: outcome.to_string(),
        completed_this_run,
        remaining,
        paused,
    }
}

// ============================================================================
// 状态持久化
// ============================================================================

fn workflow_dir(dir: &Path) -> PathBuf {
    dir.join(".novelstudio").join("workflow")
}

fn state_path(dir: &Path) -> PathBuf {
    workflow_dir(dir).join("state.json")
}

fn draft_path(dir: &Path, index: usize) -> PathBuf {
    workflow_dir(dir)
        .join("drafts")
        .join(format!("{index:03}.md"))
}

fn read_partial_draft(dir: &Path, index: usize) -> Option<String> {
    let p = draft_path(dir, index);
    let s = std::fs::read_to_string(p).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn load_state(dir: &Path) -> Result<Option<WorkflowState>, StudioError> {
    let p = state_path(dir);
    if !p.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&p)?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|e| StudioError::Config(format!("解析 workflow state.json 失败: {e}")))
}

fn save_state(dir: &Path, state: &WorkflowState) -> Result<(), StudioError> {
    let mut state = state.clone();
    state.updated_at = chrono_now();
    let p = state_path(dir);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&state)
        .map_err(|e| StudioError::Config(format!("序列化 workflow state 失败: {e}")))?;
    std::fs::write(p, content)?;
    Ok(())
}

/// 轻量时间戳（避免引入 chrono：仅用于展示）
fn chrono_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// 查询流水线状态（供 UI 冷启动展示）。
pub fn workflow_status(dir: &Path) -> Result<WorkflowState, StudioError> {
    Ok(load_state(dir)?.unwrap_or_default())
}

/// 章节进度概览（CLI / UI 展示用）
#[derive(Debug, Clone, Serialize)]
pub struct ChapterProgress {
    /// 章节脉络 index（1-based）
    pub index: usize,
    pub title: String,
    pub status: String,
}

/// 读取 chapters.toml 的章节进度列表。
pub fn chapter_progress(dir: &Path) -> Result<Vec<ChapterProgress>, StudioError> {
    let paths = ProjectPaths::new(dir);
    let file = mutations::load_chapters_file(&paths)?;
    Ok(file
        .chapters
        .iter()
        .enumerate()
        .map(|(i, c)| ChapterProgress {
            index: i + 1,
            title: c.title.clone(),
            status: c.status.clone(),
        })
        .collect())
}

fn pending_count(paths: &ProjectPaths) -> Result<usize, StudioError> {
    Ok(mutations::load_chapters_file(paths)?
        .chapters
        .iter()
        .filter(|c| c.status == STATUS_PLANNED || c.status == STATUS_WRITING)
        .count())
}

fn had_progress(paths: &ProjectPaths, dir: &Path) -> bool {
    let has_writing = mutations::load_chapters_file(paths)
        .map(|f| f.chapters.iter().any(|c| c.status == STATUS_WRITING))
        .unwrap_or(false);
    has_writing || workflow_dir(dir).join("drafts").exists()
}

// ============================================================================
// 质检
// ============================================================================

/// 诊断签名（code: message），用于基线 diff
fn diagnostic_signatures(result: &crate::compile::CompileResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

fn quality_checks(
    dir: &Path,
    text: &str,
    words: u32,
    cfg: &WorkflowConfig,
    stop_reason: story_ai_runtime::StopReason,
    state: &WorkflowState,
) -> Result<Vec<QualityIssue>, StudioError> {
    let mut issues = Vec::new();

    // 1. 字数不足（硬伤）
    let min_words = (cfg.target_words as f32 * cfg.min_ratio).ceil() as u32;
    if words < min_words {
        issues.push(QualityIssue {
            code: "word_count".into(),
            hard: true,
            message: format!(
                "字数不足：{words} 字，低于目标 {} 字的 {}%（最低 {min_words} 字）",
                cfg.target_words,
                (cfg.min_ratio * 100.0) as u32
            ),
        });
    }

    // 2. 输出截断（硬伤）
    if stop_reason == story_ai_runtime::StopReason::MaxTokens {
        issues.push(QualityIssue {
            code: "truncated".into(),
            hard: true,
            message: "输出被 max_tokens 截断，正文可能不完整".into(),
        });
    }

    // 3. 整段复读（硬伤）：归一化后长度 ≥ 30 字的段落重复出现
    for (para, count) in duplicate_paragraphs(text) {
        issues.push(QualityIssue {
            code: "dup_paragraph".into(),
            hard: true,
            message: format!(
                "整段重复出现 {count} 次：{}……",
                para.chars().take(40).collect::<String>()
            ),
        });
    }

    // 4. 编译器：与基线 diff，新增 Error = 硬伤，新增 Warning = 软伤
    let compile_result = compile_project(dir)?;
    let baseline: std::collections::BTreeSet<&String> = state.baseline.iter().collect();
    for d in compile_result.diagnostics.iter() {
        let sig = format!("{}: {}", d.code, d.message);
        if baseline.contains(&sig) {
            continue;
        }
        let is_error = d.severity == story_compiler::Severity::Error;
        issues.push(QualityIssue {
            code: if is_error {
                "compile_error".into()
            } else {
                "compile_warning".into()
            },
            hard: is_error,
            message: format!("{}: {}", d.code, d.message),
        });
    }

    Ok(issues)
}

/// 归一化段落（去首尾空白、折叠连续空白）并统计重复。
fn normalize_paragraph(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out.trim().to_string()
}

/// 返回重复出现（≥2 次）且长度 ≥ 30 字的段落（归一化文本, 次数），按首次出现顺序。
fn duplicate_paragraphs(text: &str) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let norm = normalize_paragraph(line);
        if norm.chars().count() < 30 {
            continue;
        }
        let entry = counts.entry(norm.clone()).or_insert(0);
        *entry += 1;
        if *entry == 2 {
            order.push(norm);
        }
    }
    order
        .into_iter()
        .map(|p| {
            let c = counts[&p];
            (p, c)
        })
        .collect()
}

/// 字数统计（全工作区统一口径，实现见 [`story_ir::count_words`]）。
pub fn count_words(text: &str) -> u32 {
    story_ir::count_words(text)
}

// ============================================================================
// 上下文组装（借鉴逐章生文配方：本章卡 + 前章结尾 + 下章衔接 + 角色卡）
// ============================================================================

fn load_outline(paths: &ProjectPaths) -> OutlineFile {
    let p = paths.outline_toml();
    let content = std::fs::read_to_string(p).unwrap_or_default();
    if content.trim().is_empty() {
        return OutlineFile::default();
    }
    OutlineFile::from_toml_str(&content).unwrap_or_default()
}

/// 实体属性值的展示文本（AttributeValue 未实现 Display）
fn attribute_value_text(v: &story_ir::AttributeValue) -> String {
    match v {
        story_ir::AttributeValue::String(s) => s.clone(),
        story_ir::AttributeValue::Integer(n) => n.to_string(),
        story_ir::AttributeValue::Boolean(b) => b.to_string(),
        story_ir::AttributeValue::Reference(id) => id.as_str().to_string(),
    }
}

fn build_materials(
    paths: &ProjectPaths,
    chapters: &ChaptersFile,
    index: usize,
    ir: &StoryIR,
    cfg: &WorkflowConfig,
    partial: &str,
    rewrite_pause: &Option<PauseInfo>,
) -> Result<ChapterMaterials, StudioError> {
    let def: &ChapterOutlineDef = chapters
        .chapters
        .get(index - 1)
        .ok_or_else(|| StudioError::NotFound(format!("章节脉络 index={index} 不存在")))?;

    let outline = load_outline(paths);
    let mut outline_brief = String::new();
    if let Some(premise) = outline.story.premise.as_deref().map(str::trim) {
        if !premise.is_empty() {
            outline_brief.push_str(premise);
        }
    }
    if let Some(theme) = outline.story.theme.as_deref().map(str::trim) {
        if !theme.is_empty() {
            outline_brief.push_str(&format!("主题：{theme}。"));
        }
    }

    // 本章卡
    let mut chapter_card = format!("第 {index} 章《{}》", def.title);
    if let Some(s) = &def.summary {
        chapter_card.push_str(&format!("：{s}"));
    }
    chapter_card.push('\n');
    if let Some(g) = &def.goal {
        chapter_card.push_str(&format!("本章目标：{g}\n"));
    }
    if !def.beats.is_empty() {
        chapter_card.push_str("节拍（按顺序写完）：\n");
        for (i, b) in def.beats.iter().enumerate() {
            chapter_card.push_str(&format!("{}. {b}\n", i + 1));
        }
    }
    if let Some(h) = &def.hook {
        chapter_card.push_str(&format!("章末钩子：{h}\n"));
    }

    // 角色卡（≤8 个）
    let mut character_cards = String::new();
    for cid in def.characters.iter().take(8) {
        if let Ok(id) = StableId::new(cid.clone()) {
            if let Some(story_ir::SemanticNode::Character(c)) = ir.semantic.get(&id) {
                let mut line = format!("- {}", c.display_name);
                if !c.aliases.is_empty() {
                    line.push_str(&format!("（别名：{}）", c.aliases.join("、")));
                }
                for (k, v) in c.attributes.custom.iter().take(4) {
                    line.push_str(&format!("，{k}: {}", attribute_value_text(v)));
                }
                character_cards.push_str(&line);
                character_cards.push('\n');
            }
        }
    }

    // 上一章结尾 ~600 字
    let previous_tail = previous_chapter_tail(chapters, index - 1, ir, 600);

    // 下一章摘要
    let next_summary = chapters
        .chapters
        .get(index)
        .map(|next| {
            let mut s = format!("第 {} 章《{}》", index + 1, next.title);
            if let Some(sum) = &next.summary {
                s.push_str(&format!("：{sum}"));
            } else if let Some(g) = &next.goal {
                s.push_str(&format!("：{g}"));
            }
            s
        })
        .unwrap_or_default();

    // 重写参考
    let rewrite_reference = rewrite_pause
        .as_ref()
        .map(|p| {
            let issues: Vec<String> = p.issues.iter().map(|i| format!("- {}", i.message)).collect();
            format!(
                "上一稿存在以下问题，请规避（不要照抄原稿）：\n{}\n原稿已保存在正文中，重写时保留可用部分。",
                issues.join("\n")
            )
        })
        .unwrap_or_default();

    Ok(ChapterMaterials {
        outline_brief,
        chapter_card,
        character_cards,
        previous_tail,
        next_summary,
        target_words: cfg.target_words,
        partial_text: partial.to_string(),
        rewrite_reference,
    })
}

/// 取第 `chapter_index`（1-based）章的全部正文结尾 `max_chars` 字。
fn previous_chapter_tail(
    chapters: &ChaptersFile,
    chapter_index: usize,
    ir: &StoryIR,
    max_chars: usize,
) -> String {
    let Some(def) = chapters.chapters.get(chapter_index.wrapping_sub(1)) else {
        return String::new();
    };
    let Some(ch_id) = &def.chapter_id else {
        return String::new();
    };
    let Ok(ch_sid) = StableId::new(ch_id.clone()) else {
        return String::new();
    };

    let mut full = String::new();
    for scene_id in ir.document.children_of(&ch_sid) {
        for frag_id in ir.document.children_of(scene_id) {
            if let Some(node) = ir.document.get_fragment(frag_id) {
                full.push_str(&node.text);
                full.push('\n');
            }
        }
    }
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= max_chars {
        full.trim().to_string()
    } else {
        chars[chars.len() - max_chars..]
            .iter()
            .collect::<String>()
            .trim()
            .to_string()
    }
}

/// 场景下的第一个 fragment ID（重写时复用）。
fn first_fragment_of_scene(ir: &StoryIR, scene_id: &str) -> Option<String> {
    let sid = StableId::new(scene_id.to_string()).ok()?;
    ir.document
        .children_of(&sid)
        .first()
        .map(|f| f.as_str().to_string())
}

// ============================================================================
// 节拍生成
// ============================================================================

fn generate_beats(
    dir: &Path,
    provider: &Arc<dyn AiProvider>,
    chapters: &ChaptersFile,
    index: usize,
    cancel_token: &CancelToken,
) -> Result<Vec<String>, StudioError> {
    if cancel_token.is_cancelled() {
        return Err(StudioError::Ai("已取消".to_string()));
    }
    let def = &chapters.chapters[index - 1];
    let paths = ProjectPaths::new(dir);
    let outline = load_outline(&paths);

    let mut user = String::new();
    if let Some(premise) = outline.story.premise.as_deref().map(str::trim) {
        if !premise.is_empty() {
            user.push_str(&format!("【大纲】{premise}\n"));
        }
    }
    if index >= 2 {
        if let Some(prev) = chapters.chapters.get(index - 2) {
            let prev_desc = prev
                .summary
                .as_deref()
                .or(prev.goal.as_deref())
                .unwrap_or("（无梗概）");
            user.push_str(&format!("【上一章】《{}》：{}\n", prev.title, prev_desc));
        }
    }
    user.push_str(&format!("【本章】《{}》", def.title));
    if let Some(s) = &def.summary {
        user.push_str(&format!("：{s}"));
    }
    user.push('\n');
    if let Some(g) = &def.goal {
        user.push_str(&format!("本章目标：{g}\n"));
    }
    if let Some(h) = &def.hook {
        user.push_str(&format!("章末钩子：{h}\n"));
    }
    user.push_str(crate::prompts::CHAPTER_BEATS_TASK);

    let request = GenerateRequest {
        system: crate::prompts::CHAPTER_BEATS_SYSTEM.into(),
        messages: vec![Message::user(user)],
        model: String::new(),
        temperature: 0.4,
        // 需覆盖思考模式的预算，否则思考耗尽配额后正文为空
        //（与 AgentConfig 默认一致；档案级「输出上限」可整体覆盖）
        max_tokens: 65536,
        tools: Vec::new(),
    };

    // 流式调用：部分兼容端点在思考模式下非流式返回空文本；
    // 流式通道已在正文生成中验证可靠，且支持取消
    let response = stream_generate_with_cancel(
        Arc::clone(provider),
        &request,
        cancel_token,
        std::sync::Arc::new(|_| {}),
    )
    .map_err(|e| {
        if matches!(e, AiError::Cancelled) {
            StudioError::Ai("已取消".into())
        } else {
            StudioError::Ai(format!("节拍生成失败: {e}"))
        }
    })?;

    Ok(parse_beats(&response.text))
}

/// 宽松解析节拍输出：优先 JSON 数组，退化为逐行。
pub fn parse_beats(output: &str) -> Vec<String> {
    let trimmed = output.trim();

    // JSON 数组（允许包裹在 ```json 围栏中）
    let json_text = trimmed
        .strip_prefix("```")
        .and_then(|s| s.strip_prefix("json"))
        .map(|s| s.trim())
        .and_then(|s| s.strip_suffix("```"))
        .map(|s| s.trim())
        .unwrap_or(trimmed);
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(json_text)
    {
        let beats: Vec<String> = arr
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        if !beats.is_empty() {
            return beats;
        }
    }

    // 逐行：去掉序号/项目符号前缀
    trimmed
        .lines()
        .map(|l| {
            let l = l.trim();
            let l = l.trim_start_matches(|c: char| {
                c.is_ascii_digit() || c == '.' || c == '、' || c == '-' || c == '*' || c == ' '
            });
            l.trim()
        })
        .filter(|l| {
            !l.is_empty() && !l.starts_with("```") && !l.starts_with('[') && !l.starts_with(']')
        })
        .map(|l| l.to_string())
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use story_ai_runtime::provider::{GenerateResponse, StopReason, TokenUsage};
    use story_project_format::init::init_project;

    /// 脚本化 mock provider：
    /// - generate：节拍请求返回固定 JSON 数组；
    /// - stream_generate：按序弹出章节正文，一次性以 Delta+Done 发出。
    #[derive(Debug)]
    struct MockProvider {
        chapters: Vec<String>,
        requests: Mutex<Vec<GenerateRequest>>,
        counter: Mutex<usize>,
    }

    impl MockProvider {
        fn new(chapters: Vec<String>) -> Self {
            Self {
                chapters,
                requests: Mutex::new(Vec::new()),
                counter: Mutex::new(0),
            }
        }

        fn next_chapter(&self) -> String {
            let mut c = self.counter.lock().unwrap();
            let idx = *c;
            *c += 1;
            self.chapters[idx.min(self.chapters.len() - 1)].clone()
        }

        fn requests(&self) -> Vec<GenerateRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl AiProvider for MockProvider {
        fn id(&self) -> &str {
            "mock-workflow"
        }
        fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            self.requests.lock().unwrap().push(request.clone());
            let is_beats = request
                .messages
                .iter()
                .any(|m| m.content.contains("叙事节拍"));
            let text = if is_beats {
                r#"["雨夜投宿","掌柜密谈","黑影现身","旧案悬念"]"#.to_string()
            } else {
                String::new()
            };
            Ok(GenerateResponse {
                text,
                usage: TokenUsage::default(),
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            })
        }
        fn stream_generate(
            &self,
            request: &GenerateRequest,
            on_event: &(dyn Fn(StreamEvent) + Send + Sync),
        ) -> Result<(), AiError> {
            self.requests.lock().unwrap().push(request.clone());
            let text = self.next_chapter();
            on_event(StreamEvent::Delta {
                delta: text.clone(),
            });
            on_event(StreamEvent::Done {
                full_text: text,
                usage: TokenUsage::default(),
                stop_reason: Some(StopReason::EndTurn),
            });
            Ok(())
        }
    }

    fn long_text(seed: &str) -> String {
        // ~150 个 CJK 字，满足 target_words=100 × 60%
        let unit = "夜雨敲窗烛影摇，少年按剑听更漏，远处传来三声犬吠，客栈掌柜低声说了句什么。";
        format!("{seed}{unit}{unit}")
    }

    fn setup_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "workflow-test").unwrap();
        tmp
    }

    fn write_chapters(paths: &ProjectPaths, defs: Vec<ChapterOutlineDef>) {
        let mut file = mutations::load_chapters_file(paths).unwrap();
        file.chapters = defs;
        mutations::save_chapters_file(paths, &file).unwrap();
    }

    fn chapter_def(title: &str, beats: Vec<&str>) -> ChapterOutlineDef {
        ChapterOutlineDef {
            title: title.into(),
            summary: Some(format!("{title}的梗概")),
            goal: Some("推进主线".into()),
            beats: beats.into_iter().map(String::from).collect(),
            ..ChapterOutlineDef::default()
        }
    }

    fn small_config() -> WorkflowConfig {
        WorkflowConfig {
            target_words: 100,
            min_ratio: 0.6,
            max_rewrites: 2,
            max_chapters: None,
        }
    }

    fn run(
        dir: &Path,
        provider: &Arc<MockProvider>,
        action: WorkflowAction,
        config: Option<WorkflowConfig>,
        events: &Arc<Mutex<Vec<WorkflowEvent>>>,
    ) -> Result<WorkflowReport, StudioError> {
        let cb: WorkflowEventCallback = {
            let events = Arc::clone(events);
            Arc::new(move |e| events.lock().unwrap().push(e))
        };
        run_workflow(
            dir,
            Arc::clone(provider) as Arc<dyn AiProvider>,
            action,
            config,
            cb,
            &CancelToken::new(),
        )
    }

    fn event_kinds(events: &[WorkflowEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                WorkflowEvent::RunStarted { .. } => "run_started",
                WorkflowEvent::ChapterStarted { .. } => "chapter_started",
                WorkflowEvent::BeatsGenerated { .. } => "beats_generated",
                WorkflowEvent::ChapterMaterialized { .. } => "materialized",
                WorkflowEvent::Delta { .. } => "delta",
                WorkflowEvent::ChapterCommitted { .. } => "committed",
                WorkflowEvent::QualityCheck { .. } => "quality",
                WorkflowEvent::ChapterCompleted { .. } => "chapter_completed",
                WorkflowEvent::Paused { .. } => "paused",
                WorkflowEvent::Cancelled => "cancelled",
                WorkflowEvent::Finished { .. } => "finished",
            })
            .collect()
    }

    #[test]
    fn happy_path_two_chapters() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(
            &paths,
            vec![
                chapter_def("雨夜", vec!["投宿", "密谈"]),
                chapter_def("渡口", vec![]), // 无节拍 → 走 generate_beats
            ],
        );

        let provider = Arc::new(MockProvider::new(vec![
            long_text("雨越下越大。"),
            long_text("晨雾漫过渡口。"),
        ]));
        let events = Arc::new(Mutex::new(Vec::new()));
        let report = run(
            dir,
            &provider,
            WorkflowAction::Run,
            Some(small_config()),
            &events,
        )
        .unwrap();

        assert_eq!(report.outcome, "done");
        assert_eq!(report.completed_this_run, 2);
        assert_eq!(report.remaining, 0);

        let ev = event_kinds(&events.lock().unwrap());
        assert!(ev.contains(&"beats_generated"), "第二章应生成节拍: {ev:?}");
        assert_eq!(ev.iter().filter(|k| **k == "chapter_completed").count(), 2);

        // 章节状态推进
        let chapters = mutations::load_chapters_file(&paths).unwrap();
        assert!(chapters
            .chapters
            .iter()
            .all(|c| c.status == STATUS_COMPLETED));
        // structure 落实 + fragment 落盘
        assert_eq!(chapters.chapters[0].chapter_id.as_deref(), Some("ch_01"));
        assert_eq!(chapters.chapters[1].chapter_id.as_deref(), Some("ch_02"));
        assert_eq!(chapters.chapters[0].scenes.len(), 1);
        let frag1 = paths.fragments_dir().join("f_0001.md");
        let frag2 = paths.fragments_dir().join("f_0002.md");
        assert!(frag1.exists() && frag2.exists());
        let content = std::fs::read_to_string(&frag1).unwrap();
        assert!(content.contains("id: f_0001"));
        assert!(content.contains("雨越下越大"));

        // state.json 落盘为 done
        let state = workflow_status(dir).unwrap();
        assert_eq!(state.phase, "done");

        // 上下文组装：请求顺序 = stream 第一章 → beats 第二章 → stream 第二章
        let requests = provider.requests();
        assert_eq!(requests.len(), 3);
        let ch1 = &requests[0];
        assert!(ch1.messages[0].content.contains("【下一章摘要】"));
        assert!(ch1.messages[0].content.contains("渡口"));
        let ch2 = &requests[2];
        let user = &ch2.messages[0].content;
        assert!(user.contains("【前文】"), "第二章应带上一章结尾");
        assert!(!user.contains("【下一章摘要】"), "末章无下一章");
    }

    #[test]
    fn word_count_pause_then_approve() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(
            &paths,
            vec![
                chapter_def("雨夜", vec!["投宿"]),
                chapter_def("渡口", vec!["上船"]),
            ],
        );

        // 第一次：太短 → 暂停
        let provider = Arc::new(MockProvider::new(vec!["太短了。".to_string()]));
        let events = Arc::new(Mutex::new(Vec::new()));
        let report = run(
            dir,
            &provider,
            WorkflowAction::Run,
            Some(small_config()),
            &events,
        )
        .unwrap();
        assert_eq!(report.outcome, "paused");
        let pause = report.paused.unwrap();
        assert_eq!(pause.index, 1);
        assert!(
            pause.issues.iter().any(|i| i.code == "word_count"),
            "{:?}",
            pause.issues
        );

        // 暂停状态下 Run 被拒绝
        let err = run(
            dir,
            &provider,
            WorkflowAction::Run,
            None,
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap_err();
        assert!(matches!(err, StudioError::Config(_)));

        // Approve → 推进到下一章（mock 继续给短文 → 再次暂停在第 2 章）
        let report = run(
            dir,
            &provider,
            WorkflowAction::Approve,
            None,
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        assert_eq!(report.outcome, "paused");
        assert_eq!(report.paused.unwrap().index, 2);

        let state = workflow_status(dir).unwrap();
        assert_eq!(state.phase, "paused");

        let chapters = mutations::load_chapters_file(&paths).unwrap();
        assert_eq!(chapters.chapters[0].status, STATUS_COMPLETED);
        assert_eq!(chapters.chapters[1].status, STATUS_WRITING);
    }

    #[test]
    fn rewrite_overwrites_same_fragment() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(&paths, vec![chapter_def("雨夜", vec!["投宿"])]);

        // 短文 → 暂停
        let events = Arc::new(Mutex::new(Vec::new()));
        let short = Arc::new(MockProvider::new(vec!["太短了。".to_string()]));
        let report = run(
            dir,
            &short,
            WorkflowAction::Run,
            Some(small_config()),
            &events,
        )
        .unwrap();
        assert_eq!(report.outcome, "paused");

        // Rewrite：换一个返回长文的 provider
        let long = Arc::new(MockProvider::new(vec![long_text("重写后的开头。")]));
        let report = run(
            dir,
            &long,
            WorkflowAction::Rewrite,
            None,
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        assert_eq!(report.outcome, "done");

        // 只有一个 fragment 文件，内容被覆盖
        let entries: Vec<_> = std::fs::read_dir(paths.fragments_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .collect();
        assert_eq!(entries.len(), 1);
        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        assert!(content.contains("重写后的开头"));
        assert!(!content.contains("太短了"));

        // 重写请求应包含失败原因（唯一请求即重写生成）
        let user = &long.requests()[0].messages[0].content;
        assert!(user.contains("【重写说明】"), "{user}");
        assert!(user.contains("字数不足"));
    }

    #[test]
    fn resume_from_partial_draft() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(&paths, vec![chapter_def("雨夜", vec!["投宿", "密谈"])]);

        // 模拟中断：第一章已在 writing，半成品草稿存在
        let ch_id = mutations::create_chapter(&paths, "雨夜").unwrap();
        let scene_id = mutations::create_scene(&paths, &ch_id, "雨夜", None).unwrap();
        mutations::backfill_chapter_outline_id(&paths, 1, &ch_id).unwrap();
        mutations::append_chapter_outline_scene(&paths, 1, &scene_id).unwrap();
        mutations::set_chapter_outline_status(&paths, 1, STATUS_WRITING).unwrap();
        let partial = "林风推开门，雨水顺着发梢滴落。这是一段中断前生成的半成品正文。";
        std::fs::create_dir_all(workflow_dir(dir).join("drafts")).unwrap();
        std::fs::write(draft_path(dir, 1), partial).unwrap();

        let provider = Arc::new(MockProvider::new(vec![long_text("续写的部分。")]));
        let report = run(
            dir,
            &provider,
            WorkflowAction::Run,
            Some(small_config()),
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        assert_eq!(report.outcome, "done");

        // 最终 fragment = 半成品 + 续写
        let frag = paths.fragments_dir().join("f_0001.md");
        let content = std::fs::read_to_string(&frag).unwrap();
        assert!(content.contains("半成品正文"));
        assert!(content.contains("续写的部分"));

        // 请求上下文应带【已写部分】
        let user = &provider.requests()[0].messages[0].content;
        assert!(user.contains("【已写部分】"), "{user}");
        assert!(user.contains("半成品正文"));

        // 草稿文件在完成后清理
        assert!(!draft_path(dir, 1).exists());
    }

    #[test]
    fn duplicate_paragraph_pauses() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(&paths, vec![chapter_def("复读机", vec!["复读"])]);

        let para = "他抬起头望向远方的山峦，山峦之后还是山峦，云雾缭绕之间似乎有什么在移动着。";
        let text = format!("{para}\n{para}\n{para}");
        let provider = Arc::new(MockProvider::new(vec![text]));
        let report = run(
            dir,
            &provider,
            WorkflowAction::Run,
            Some(small_config()),
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        assert_eq!(report.outcome, "paused");
        assert!(report
            .paused
            .unwrap()
            .issues
            .iter()
            .any(|i| i.code == "dup_paragraph"));
        let _ = paths;
    }

    #[test]
    fn max_chapters_batch_limit() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(
            &paths,
            vec![chapter_def("一", vec!["a"]), chapter_def("二", vec!["b"])],
        );

        let mut cfg = small_config();
        cfg.max_chapters = Some(1);
        let provider = Arc::new(MockProvider::new(vec![
            long_text("一。"),
            long_text("二。"),
        ]));
        let report = run(
            dir,
            &provider,
            WorkflowAction::Run,
            Some(cfg),
            &Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        assert_eq!(report.outcome, "batch_limit");
        assert_eq!(report.completed_this_run, 1);
        assert_eq!(report.remaining, 1);
    }

    #[test]
    fn baseline_diff_filters_existing_diagnostics() {
        let tmp = setup_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        write_chapters(&paths, vec![chapter_def("基线", vec!["a"])]);

        // 制造一个存量诊断：场景 POV 指向不存在的角色（orphan_reference）
        let ch = mutations::create_chapter(&paths, "基线").unwrap();
        let sc = mutations::create_scene(&paths, &ch, "基线", Some("c_9999")).unwrap();
        let _ = sc;

        let text = long_text("正文。");
        let cfg = small_config();

        // 第一次质检：存量诊断应作为 issue 出现
        let state = WorkflowState::default();
        let issues = quality_checks(dir, &text, 1000, &cfg, StopReason::EndTurn, &state).unwrap();
        assert!(!issues.is_empty(), "POV 断裂应产生 compile 诊断 issue");

        // 把当前签名写入基线 → 同样的诊断不再计数
        let result = compile_project(dir).unwrap();
        let state = WorkflowState {
            baseline: diagnostic_signatures(&result),
            ..WorkflowState::default()
        };
        let issues = quality_checks(dir, &text, 1000, &cfg, StopReason::EndTurn, &state).unwrap();
        assert!(
            issues.iter().all(|i| !i.code.starts_with("compile")),
            "基线内的诊断不应再报: {issues:?}"
        );
    }

    #[test]
    fn parse_beats_variants() {
        assert_eq!(
            parse_beats(r#"["a","b"]"#),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            parse_beats("```json\n[\"雨夜\", \"密谈\"]\n```"),
            vec!["雨夜".to_string(), "密谈".to_string()]
        );
        assert_eq!(
            parse_beats("1. 开场\n2. 冲突\n- 收束\n"),
            vec!["开场".to_string(), "冲突".to_string(), "收束".to_string()]
        );
        assert!(parse_beats("").is_empty());
    }

    #[test]
    fn count_words_cjk_and_ascii() {
        assert_eq!(count_words("林风推开门"), 5);
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("他说 ok 了"), 4); // 他(1)说(1)ok(1)了(1)
        assert_eq!(count_words("，，，"), 0);
    }

    #[test]
    fn duplicate_paragraph_detection() {
        let para = "这是一段足够长的段落，长度超过三十个字符，用于触发整段复读检测逻辑。";
        let dups = duplicate_paragraphs(&format!("{para}\n中间段\n  {para}  \n{para}"));
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].1, 3);

        // 短段落不参与
        assert!(duplicate_paragraphs("短短\n短短\n短短").is_empty());
    }
}
