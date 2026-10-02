//! Tauri 命令 — story-studio-lib 的薄封装
//!
//! 约定：
//! - 所有命令第一个参数是项目根目录 `root`（前端持有，无服务端会话状态）
//! - 错误统一 `Result<T, String>`（StudioError 的中文 Display 消息）

use std::path::Path;

use story_project_format::ProjectPaths;
use story_studio_lib as lib;
use story_studio_lib::mutations::{self, EntityKind, SceneUpdate};

use crate::dto::*;

type R<T> = Result<T, String>;

fn paths(root: &str) -> ProjectPaths {
    ProjectPaths::new(root)
}

fn e<T: std::fmt::Display>(err: T) -> String {
    err.to_string()
}

// ============================================================================
// 项目
// ============================================================================

#[tauri::command]
pub fn create_project(parent_dir: String, book_name: String) -> R<String> {
    let name = book_name.trim();
    if name.is_empty() {
        return Err("书名不能为空".into());
    }
    lib::create_project(Path::new(&parent_dir), name)
        .map(|p| p.display().to_string())
        .map_err(e)
}

#[tauri::command]
pub fn open_project(root: String) -> R<WorkspaceDto> {
    let info = lib::project_info(Path::new(&root)).map_err(e)?;
    build_workspace(std::path::Path::new(&root), &info)
}

#[tauri::command]
pub fn refresh_workspace(root: String) -> R<WorkspaceDto> {
    open_project(root)
}

/// 首页书案：批量轻量窥视最近项目（扫片段文件，走线程池避免阻塞主线程）
#[tauri::command]
pub async fn peek_projects(roots: Vec<String>) -> R<Vec<lib::PeekEntry>> {
    tauri::async_runtime::spawn_blocking(move || lib::peek_projects(&roots))
        .await
        .map_err(|err| err.to_string())
}

// ============================================================================
// 结构：Chapter / Scene
// ============================================================================

#[tauri::command]
pub fn create_chapter(root: String, title: String) -> R<ChapterDto> {
    let id = mutations::create_chapter(&paths(&root), title.trim()).map_err(e)?;
    refresh_chapter(&root, &id)
}

#[tauri::command]
pub fn rename_chapter(root: String, chapter_id: String, title: String) -> R<()> {
    mutations::rename_chapter(&paths(&root), &chapter_id, title.trim()).map_err(e)
}

#[tauri::command]
pub fn delete_chapter(root: String, chapter_id: String) -> R<()> {
    mutations::delete_chapter(&paths(&root), &chapter_id).map_err(e)
}

#[tauri::command]
pub fn create_scene(root: String, chapter_id: String, title: String, pov: Option<String>) -> R<SceneDetailDto> {
    let id = mutations::create_scene(
        &paths(&root),
        &chapter_id,
        title.trim(),
        pov.as_deref(),
    )
    .map_err(e)?;
    get_scene(root, id)
}

#[tauri::command]
pub fn rename_scene(root: String, scene_id: String, title: String) -> R<()> {
    mutations::rename_scene(&paths(&root), &scene_id, title.trim()).map_err(e)
}

#[tauri::command]
pub fn delete_scene(root: String, scene_id: String) -> R<()> {
    mutations::delete_scene(&paths(&root), &scene_id).map_err(e)
}

/// 场景属性补丁（三态语义：缺省=不改 / null=清空 / 值=设置）
#[tauri::command]
pub fn update_scene(root: String, scene_id: String, patch: ScenePatchDto) -> R<Vec<String>> {
    let title = patch.title.flatten();
    let update = SceneUpdate {
        title: title.as_deref(),
        pov: patch.pov.as_ref().map(|v| v.as_deref()),
        mood: patch.mood.as_ref().map(|v| v.as_deref()),
        function: patch.function.as_ref().map(|v| v.as_deref()),
        summary: patch.summary.as_ref().map(|v| v.as_deref()),
        story_time: patch.story_time.clone(),
    };
    mutations::update_scene(&paths(&root), &scene_id, &update)
        .map_err(e)
        .map(|v| v.iter().map(|s| s.to_string()).collect())
}

// ============================================================================
// 结构：Fragment
// ============================================================================

#[tauri::command]
pub fn get_scene(root: String, scene_id: String) -> R<SceneDetailDto> {
    let p = paths(&root);
    let structure = story_project_format::StructureFile::from_toml_str(
        &read_file(&p.structure_toml())?,
    )
    .map_err(|err| format!("structure.toml 解析失败: {err}"))?;

    let (chapter_id, chapter_title, scene_def) = structure
        .chapter
        .iter()
        .find(|(_, ch_def)| ch_def.scenes.iter().any(|s| s == &scene_id))
        .and_then(|(ch_id, ch_def)| {
            structure
                .scene
                .get(&scene_id)
                .map(|sd| (ch_id.clone(), ch_def.title.clone(), sd.clone()))
        })
        .ok_or_else(|| format!("场景 '{scene_id}' 不存在"))?;

    let mut fragments = Vec::new();
    for fid in &scene_def.fragments {
        if let Ok(ff) = load_fragment_file(std::path::Path::new(&root), fid) {
            fragments.push(to_fragment_dto(&ff));
        }
    }

    Ok(SceneDetailDto {
        id: scene_id.clone(),
        chapter_id,
        chapter_title,
        title: scene_def.title,
        pov: scene_def.pov,
        mood: scene_def.mood,
        function: scene_def.function,
        summary: scene_def.summary,
        story_time: scene_def.story_time,
        fragments,
    })
}

/// 在指定片段后插入新片段（场景末尾追加时 after_id 为 None）
#[tauri::command]
pub fn insert_fragment(root: String, scene_id: String, after_id: Option<String>, text: String) -> R<FragmentDto> {
    let p = paths(&root);
    let new_id = mutations::create_fragment(&p, &scene_id, &text).map_err(e)?;

    // create_fragment 追加到末尾；若指定了 after_id 则移动到其后
    if let Some(after) = after_id {
        // 计算 after 在移除 new_id 后的目标位置
        let structure = story_project_format::StructureFile::from_toml_str(
            &read_file(&p.structure_toml())?,
        )
        .map_err(|err| format!("structure.toml 解析失败: {err}"))?;
        let scene = structure
            .scene
            .get(&scene_id)
            .ok_or_else(|| format!("场景 '{scene_id}' 不存在"))?;
        let mut ids = scene.fragments.clone();
        ids.retain(|f| f != &new_id);
        let target = ids
            .iter()
            .position(|f| f == &after)
            .map(|pos| pos + 1)
            .unwrap_or(ids.len());
        mutations::move_fragment(&p, &scene_id, &new_id, target).map_err(e)?;
    }

    let ff = load_fragment_file(std::path::Path::new(&root), &new_id)?;
    Ok(to_fragment_dto(&ff))
}

#[tauri::command]
pub fn write_fragment(root: String, fragment_id: String, text: String) -> R<()> {
    mutations::write_fragment_text(&paths(&root), &fragment_id, &text).map_err(e)
}

#[tauri::command]
pub fn delete_fragment(root: String, scene_id: String, fragment_id: String) -> R<()> {
    let _ = scene_id;
    mutations::delete_fragment(&paths(&root), &fragment_id).map_err(e)
}

#[tauri::command]
pub fn move_fragment(root: String, scene_id: String, fragment_id: String, new_index: usize) -> R<()> {
    mutations::move_fragment(&paths(&root), &scene_id, &fragment_id, new_index).map_err(e)
}

// ── 片段版本历史 ─────────────────────────────────────────

/// 列出片段的本地版本（节流快照，存 .novelstudio/history/）
#[tauri::command]
pub fn fragment_versions(root: String, fragment_id: String) -> R<Vec<lib::history::FragmentVersion>> {
    lib::history::list_versions(&paths(&root), &fragment_id).map_err(e)
}

/// 读取指定版本的完整内容（预览用）
#[tauri::command]
pub fn fragment_version_read(root: String, fragment_id: String, timestamp: u64) -> R<String> {
    lib::history::read_version(&paths(&root), &fragment_id, timestamp).map_err(e)
}

/// 恢复指定版本（恢复前自动快照当前内容，恢复动作本身可撤销）
#[tauri::command]
pub fn fragment_restore_version(root: String, fragment_id: String, timestamp: u64) -> R<FragmentDto> {
    let p = paths(&root);
    lib::history::restore_version(&p, &fragment_id, timestamp).map_err(e)?;
    let ff = load_fragment_file(std::path::Path::new(&root), &fragment_id)?;
    Ok(to_fragment_dto(&ff))
}

// ── 码字统计 / AI 账本 ───────────────────────────────────

#[tauri::command]
pub fn stats_summary(root: String) -> R<lib::stats::WordStatsSummary> {
    Ok(lib::stats::summary(Path::new(&root), 7))
}

#[tauri::command]
pub fn stats_set_goal(root: String, goal: u32) -> R<()> {
    lib::stats::set_daily_goal(Path::new(&root), goal);
    Ok(())
}

/// 同步本地时区偏移（分钟，JS getTimezoneOffset 同号约定），打开项目时调用
#[tauri::command]
pub fn stats_sync_clock(root: String, tz_offset_min: i32) -> R<()> {
    lib::stats::sync_tz(Path::new(&root), tz_offset_min);
    Ok(())
}

#[tauri::command]
pub fn ai_ledger_summary(root: String) -> R<lib::ai_ledger::AiLedgerSummary> {
    Ok(lib::ai_ledger::summary(Path::new(&root), lib::ai_ledger::MAX_ENTRIES))
}

// ============================================================================
// 实体 / 关系
// ============================================================================

#[tauri::command]
pub fn create_entity(root: String, kind: String, name: String) -> R<EntityDto> {
    let k = EntityKind::from_type_str(&kind).map_err(e)?;
    let id = mutations::create_entity(&paths(&root), k, name.trim()).map_err(e)?;
    let p = paths(&root);
    let path = p.file_for_id(&id).ok_or("无效 ID")?;
    let content = read_file(&path)?;
    let ef = story_project_format::EntityFile::from_toml_str(&content)
        .map_err(|err| format!("解析实体失败: {err}"))?;
    // kind 原样回传（前端用单数形式；from_type_str 两种都接受）
    Ok(to_entity_dto(&kind, &ef))
}

#[tauri::command]
pub fn delete_entity(root: String, entity_id: String) -> R<()> {
    mutations::delete_entity(&paths(&root), &entity_id).map_err(e)
}

#[tauri::command]
pub fn save_entity(root: String, entity: EntityDto) -> R<EntityDto> {
    save_entity_file(std::path::Path::new(&root), &entity)?;
    Ok(entity)
}

#[tauri::command]
pub fn add_relation(root: String, from: String, to: String, relation: String) -> R<RelationDto> {
    let id = mutations::add_relation(&paths(&root), &from, &to, &relation).map_err(e)?;
    let p = paths(&root);
    let file = load_toml::<story_project_format::RelationsFile>(&p.relations_toml())?;
    let def = file
        .relations
        .iter()
        .find(|r| r.id == id)
        .ok_or("关系创建后未找到")?;
    Ok(to_relation_dto(def))
}

#[tauri::command]
pub fn remove_relation(root: String, relation_id: String) -> R<()> {
    mutations::remove_relation(&paths(&root), &relation_id).map_err(e)
}

#[tauri::command]
pub fn merge_entities(root: String, from_entity_id: String, into_entity_id: String) -> R<String> {
    let report = mutations::merge_entities(&paths(&root), &from_entity_id, &into_entity_id)
        .map_err(e)?;
    Ok(report.summary(&from_entity_id, &into_entity_id))
}

#[tauri::command]
pub fn resolve_unresolved(
    root: String,
    fragment_id: String,
    quote: String,
    action: String,
    target_entity_id: Option<String>,
    name: Option<String>,
    kind: Option<String>,
) -> R<String> {
    mutations::resolve_unresolved_ref(
        &paths(&root),
        &fragment_id,
        &quote,
        &action,
        target_entity_id.as_deref(),
        name.as_deref(),
        kind.as_deref(),
        None,
    )
    .map_err(e)
}

// ============================================================================
// 叙事线索
// ============================================================================

#[tauri::command]
pub fn create_thread(
    root: String,
    title: String,
    description: String,
    priority: String,
    due_volume: Option<u32>,
) -> R<ThreadDto> {
    let id = mutations::create_thread(
        &paths(&root),
        title.trim(),
        &description,
        &priority,
        None,
        due_volume,
    )
    .map_err(e)?;
    load_thread(&root, &id)
}

#[tauri::command]
pub fn update_thread(
    root: String,
    thread_id: String,
    title: Option<String>,
    description: Option<String>,
    goal: Option<String>,
    priority: Option<String>,
    // 计划埋设卷号：不传 = 不变；0 = 清除（恢复全书长线）；n = 设为第 n 卷
    due_volume: Option<u32>,
) -> R<()> {
    // Tauri IPC 的 JSON null 与缺省不可区分，用 0 表示清除
    let due: Option<Option<u32>> = match due_volume {
        None => None,
        Some(0) => Some(None),
        Some(n) => Some(Some(n)),
    };
    mutations::update_thread(
        &paths(&root),
        &thread_id,
        title.as_deref(),
        description.as_deref(),
        goal.as_deref(),
        priority.as_deref(),
        None,
        due,
    )
    .map_err(e)
    .map(|_| ())
}

#[tauri::command]
pub fn delete_thread(root: String, thread_id: String) -> R<()> {
    mutations::delete_thread(&paths(&root), &thread_id).map_err(e)
}

fn load_thread(root: &str, thread_id: &str) -> R<ThreadDto> {
    let file = load_toml::<story_project_format::ThreadsFile>(&paths(root).threads_toml())?;
    file.threads
        .iter()
        .find(|t| t.id == thread_id)
        .map(to_thread_dto)
        .ok_or_else(|| format!("线索 '{thread_id}' 不存在"))
}

// ============================================================================
// 世界观（概念 / 体系 / 历法）
// ============================================================================

#[tauri::command]
pub fn get_worldview(root: String) -> R<WorldviewDto> {
    build_worldview(Path::new(&root))
}

#[tauri::command]
pub fn save_worldview(root: String, worldview: WorldviewDto) -> R<()> {
    crate::dto::save_worldview(Path::new(&root), &worldview)
}

// ============================================================================
// 提示词自定义（项目级覆盖，prompts/{id}.md）
// ============================================================================

/// 列出全部可自定义提示词（meta + 内置全文 + 当前覆盖）
#[tauri::command]
pub fn list_prompts(root: String) -> R<Vec<PromptDto>> {
    Ok(lib::prompt_custom::list(Path::new(&root))
        .into_iter()
        .map(PromptDto::from)
        .collect())
}

/// 保存项目覆盖（校验 id 在册、内容非空、必需占位符齐全）
#[tauri::command]
pub fn save_prompt(root: String, id: String, content: String) -> R<()> {
    lib::prompt_custom::save(Path::new(&root), &id, &content).map_err(e)
}

/// 删除项目覆盖恢复内置；返回覆盖文件是否原本存在
#[tauri::command]
pub fn reset_prompt(root: String, id: String) -> R<bool> {
    lib::prompt_custom::reset(Path::new(&root), &id).map_err(e)
}

// ============================================================================
// 章节脉络（chapters.toml：卷分组 + 逐章脉络）
// ============================================================================

#[tauri::command]
pub fn get_chapter_outline(root: String) -> R<ChaptersOutlineDto> {
    build_chapters_outline(Path::new(&root))
}

#[tauri::command]
pub fn save_chapter_outline(root: String, outline: ChaptersOutlineDto) -> R<()> {
    crate::dto::save_chapters_outline(Path::new(&root), &outline)
}

// ============================================================================
// 编译诊断 / 导出
// ============================================================================

#[tauri::command]
pub fn compile_project(root: String) -> R<CompileDto> {
    match lib::compile_project(Path::new(&root)) {
        Ok(result) => {
            let diagnostics = result
                .diagnostics
                .0
                .iter()
                .map(|d| DiagDto {
                    severity: d.severity.as_str().to_string(),
                    code: d.code.as_str().to_string(),
                    message: d.message.clone(),
                    rule: match &d.source {
                        story_compiler::DiagnosticSource::Compiler { rule_id } => {
                            Some(rule_id.clone())
                        }
                        _ => None,
                    },
                    entity: d.loc.entity.as_ref().map(|id| id.as_str().to_string()),
                    fragment: d.loc.fragment.as_ref().map(|id| id.as_str().to_string()),
                })
                .collect();
            Ok(CompileDto {
                ok: true,
                error: None,
                diagnostics,
                stats: StatsDto {
                    fragment_count: result.stats.fragment_count,
                    entity_count: result.stats.entity_count,
                    edge_count: result.stats.edge_count,
                    assertion_count: result.stats.assertion_count,
                    evidence_count: result.stats.evidence_count,
                },
            })
        }
        Err(err) => Ok(CompileDto {
            ok: false,
            error: Some(e(err)),
            diagnostics: Vec::new(),
            stats: StatsDto::default(),
        }),
    }
}

#[tauri::command]
pub fn export_markdown(
    root: String,
    include_appendix: bool,
    scene_titles: bool,
    chapter_numbering: bool,
) -> R<story_exporter_types::ExportOutputDto> {
    let output = lib::export_markdown(
        Path::new(&root),
        include_appendix,
        scene_titles,
        chapter_numbering,
    )
    .map_err(e)?;
    Ok(story_exporter_types::ExportOutputDto {
        content: output.content,
        filename: output.filename,
    })
}

#[tauri::command]
pub fn export_chapters(
    root: String,
    include_appendix: bool,
    scene_titles: bool,
    chapter_numbering: bool,
) -> R<Vec<story_exporter_types::ChapterFileDto>> {
    let files = lib::export_chapters_markdown(
        Path::new(&root),
        include_appendix,
        scene_titles,
        chapter_numbering,
    )
    .map_err(e)?;
    Ok(files
        .into_iter()
        .map(|f| story_exporter_types::ChapterFileDto {
            filename: f.filename,
            content: f.content,
        })
        .collect())
}

/// 把导出内容写入用户选择的路径
#[tauri::command]
pub fn write_text_file(path: String, content: String) -> R<()> {
    std::fs::write(&path, content).map_err(|err| format!("写入 {} 失败: {err}", path))
}

// ============================================================================
// AI
// ============================================================================

#[tauri::command]
pub fn get_ai_config() -> R<AiConfigDto> {
    let config = lib::read_global_ai_config().map_err(e)?;
    Ok(AiConfigDto::from(&config))
}

#[tauri::command]
pub fn set_ai_config(config: AiConfigDto) -> R<()> {
    lib::write_global_ai_config(&(&config).into()).map_err(e)
}

/// 内置 Provider 预设（常见服务商模板）
#[tauri::command]
pub fn provider_presets() -> Vec<lib::ProviderPreset> {
    lib::provider_presets()
}

/// 测试档案连通性（先试零消耗的模型列表，失败退最小生成调用）
#[tauri::command]
pub fn ai_test_profile(profile: AiProfileDto) -> lib::ProfileTestRecord {
    lib::test_profile(&(&profile).into())
}

/// 拉取模型列表（用编辑中的档案参数，不必先保存）
#[tauri::command]
pub fn ai_list_models(
    provider: String,
    api_url: Option<String>,
    api_url_full: Option<bool>,
    api_key: Option<String>,
) -> R<Vec<String>> {
    lib::list_models(&provider, api_url.as_deref(), api_url_full.unwrap_or(false), api_key.as_deref())
        .map_err(e)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiWriteResultDto {
    pub fragment_ids: Vec<String>,
    pub tokens_used: usize,
}

/// 从全局配置构建指定用途的 Provider
///
/// 用途 → 档案：有分配用分配（档案删除后悬空自动回退默认），
/// 未分配的用途走激活档案；一个档案都没有时退回旧字段 / 环境变量。
fn active_provider(
    purpose: &str,
) -> Result<std::sync::Arc<dyn story_ai_runtime::AiProvider>, String> {
    let provider = lib::create_provider_for_purpose(purpose, "anthropic", None).map_err(e)?;
    Ok(std::sync::Arc::from(provider))
}

// ── 流式续写 ──────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteStreamMsg {
    /// "delta" — text 为增量文本；"done" — text 为新段落 ID（逗号分隔，可能为空）
    pub kind: String,
    pub text: String,
}

/// AI 流式续写当前场景（增量经 Channel 推送，结束后自动保存为新段落）
///
/// 整体超时 5 分钟 — AI 服务长时间无响应时返回错误，避免前端永远等待。
#[tauri::command]
pub async fn ai_write_scene(
    root: String,
    scene_id: String,
    instruction: Option<String>,
    on_event: tauri::ipc::Channel<WriteStreamMsg>,
) -> R<AiWriteResultDto> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::WRITE)?;

        let result = lib::write_fragment_streaming(
            Path::new(&root),
            &scene_id,
            provider.as_ref(),
            instruction.as_deref(),
            &|ev| {
                if let story_ai_runtime::StreamEvent::Delta { delta } = ev {
                    let _ = on_event.send(WriteStreamMsg {
                        kind: "delta".into(),
                        text: delta,
                    });
                }
            },
        )
        .map_err(e)?;

        let ids = lib::save_write_result(Path::new(&root), &scene_id, &result).map_err(e)?;

        // 完成信号走 Channel（带结果），命令返回值作兜底——两条通路谁先到都生效
        let _ = on_event.send(WriteStreamMsg {
            kind: "done".into(),
            text: ids.join(","),
        });

        Ok(AiWriteResultDto {
            fragment_ids: ids,
            tokens_used: result.tokens_used,
        })
    });

    match tokio::time::timeout(std::time::Duration::from_secs(300), task).await {
        Ok(res) => res.map_err(|err| format!("任务执行失败: {err}"))?,
        Err(_) => Err("AI 服务长时间没有响应，请稍后重试或检查网络与 AI 设置".to_string()),
    }
}

// ── 行内补全（幽灵补全）──────────────────────────────────

/// 行内补全的取消槽：同一时刻只保留最新一次补全，新请求自动顶掉旧请求
static COMPLETE_CANCEL: std::sync::Mutex<Option<story_ai_runtime::CancelToken>> =
    std::sync::Mutex::new(None);

/// 占用补全槽位：取消旧令牌，登记并返回新令牌
fn begin_complete_slot() -> story_ai_runtime::CancelToken {
    let mut guard = COMPLETE_CANCEL.lock().unwrap();
    if let Some(old) = guard.take() {
        old.cancel();
    }
    let token = story_ai_runtime::CancelToken::new();
    *guard = Some(token.clone());
    token
}

/// 主动取消当前补全（无补全在跑时是空操作）
fn cancel_complete_slot() {
    if let Some(token) = COMPLETE_CANCEL.lock().unwrap().take() {
        token.cancel();
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCompleteResultDto {
    pub text: String,
    pub tokens_used: usize,
}

/// AI 行内补全（编辑器幽灵补全，流式）
///
/// 增量经 Channel 推送（kind="delta"），结束发 kind="done"（text 为完整补全），
/// 返回值作兜底。补全讲究快：超时 90 秒；新请求 / 取消命令会终止旧请求。
#[tauri::command]
pub async fn ai_complete_fragment(
    root: String,
    fragment_id: String,
    before: String,
    after: Option<String>,
    on_event: tauri::ipc::Channel<WriteStreamMsg>,
) -> R<AiCompleteResultDto> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        // 快慢权衡在 complete_with_fallback 里：先按档案配置（缺省关思考）
        // 小配额快跑，遇到强制思考模型（拒收参数 / 配额被吃空）自动
        // 以 Auto + 大配额重试一次
        let token = begin_complete_slot();

        let done_channel = on_event.clone();
        let on_delta: std::sync::Arc<dyn Fn(&str) + Send + Sync> =
            std::sync::Arc::new(move |delta: &str| {
                let _ = on_event.send(WriteStreamMsg {
                    kind: "delta".into(),
                    text: delta.to_string(),
                });
            });

        let result = lib::complete_with_fallback(
            Path::new(&root),
            &fragment_id,
            &before,
            after.as_deref(),
            &token,
            &on_delta,
        )
        .map_err(e)?;

        let _ = done_channel.send(WriteStreamMsg {
            kind: "done".into(),
            text: result.text.clone(),
        });

        Ok(AiCompleteResultDto {
            text: result.text,
            tokens_used: result.tokens_used,
        })
    });

    match tokio::time::timeout(std::time::Duration::from_secs(90), task).await {
        Ok(res) => res.map_err(|err| format!("任务执行失败: {err}"))?,
        Err(_) => {
            cancel_complete_slot();
            Err("行内补全超时，请稍候再试".to_string())
        }
    }
}

/// 取消进行中的行内补全（编辑器里继续打字 / 关闭幽灵时调用）
#[tauri::command]
pub fn ai_complete_cancel() -> bool {
    let had = COMPLETE_CANCEL.lock().unwrap().is_some();
    cancel_complete_slot();
    had
}

// ── AI 味检测（手动触发，结果交妙笔改进） ─────────────────

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCheckItemDto {
    /// 命中处原文引用
    pub quote: String,
    /// 特征编号（如 "A"、"14"）
    pub feature_id: String,
    /// 特征名称
    pub feature_name: String,
    /// Strong | Medium | Weak
    pub severity: String,
    /// 一两句话说明问题
    pub explanation: String,
    /// 命中所在片段（fragment 范围检测时回填为被检片段）
    pub fragment_id: Option<String>,
    /// 片段归属是否为推测（引文未能匹配回原文，归到块内首片段时 true）
    pub fragment_uncertain: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCheckResultDto {
    /// "fragment" | "scene" | "chapter"
    pub scope: String,
    pub scope_id: String,
    /// 一两句话概括最突出的问题
    pub summary: String,
    pub tokens_used: usize,
    pub items: Vec<AiCheckItemDto>,
}

/// AI 味检测（单片段 / 整场景 / 整章）
///
/// 前端手动触发：结果回传格式化后填入妙笔输入框，让 Agent 逐条改进。
#[tauri::command]
pub async fn ai_check(root: String, scope: String, id: String) -> R<AiCheckResultDto> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::CHECK)?;

        let map_items = |items: &[lib::AiCheckItem],
                         fallback_fid: Option<&str>| {
            items
                .iter()
                .map(|it| AiCheckItemDto {
                    quote: it.quote.clone(),
                    feature_id: it.feature_id.clone(),
                    feature_name: it.feature_name.clone(),
                    severity: format!("{:?}", it.severity),
                    explanation: it.explanation.clone(),
                    fragment_id: it.fragment_id.clone().or_else(|| fallback_fid.map(str::to_string)),
                    fragment_uncertain: it.fragment_uncertain,
                })
                .collect()
        };

        match scope.as_str() {
            "fragment" => {
                let r = lib::check_fragment(Path::new(&root), &id, &provider).map_err(e)?;
                let items = map_items(&r.items, Some(&id));
                Ok(AiCheckResultDto {
                    scope,
                    scope_id: id,
                    summary: r.summary,
                    tokens_used: r.tokens_used,
                    items,
                })
            }
            "scene" | "chapter" => {
                let cs = if scope == "scene" {
                    lib::CheckScope::Scene
                } else {
                    lib::CheckScope::Chapter
                };
                let r = lib::check_scope(Path::new(&root), cs, &id, &provider).map_err(e)?;
                Ok(AiCheckResultDto {
                    scope: r.scope,
                    scope_id: r.scope_id,
                    summary: r.summary,
                    tokens_used: r.tokens_used,
                    items: map_items(&r.items, None),
                })
            }
            other => Err(format!(
                "未知检测范围: {other}（支持 fragment / scene / chapter）"
            )),
        }
    });

    match tokio::time::timeout(std::time::Duration::from_secs(300), task).await {
        Ok(res) => res.map_err(|err| format!("任务执行失败: {err}"))?,
        Err(_) => Err("AI 检测长时间没有响应，请稍后重试".to_string()),
    }
}

/// 定位片段所属场景（审校报告精确跳转用）
#[tauri::command]
pub fn scene_of_fragment(root: String, fragment_id: String) -> R<String> {
    lib::scene_of_fragment(&paths(&root), &fragment_id).map_err(e)
}

// ── Git 版本档案 ─────────────────────────────────────────

/// 仓库状态检测（git 可用性 / 已初始化 / 嵌套 / 未存档变更）
#[tauri::command]
pub fn git_status(root: String) -> R<lib::GitStatusDto> {
    Ok(lib::git_status(Path::new(&root)))
}

/// 初始化版本仓库（幂等；嵌套仓库拒绝接管）
#[tauri::command]
pub fn git_init(root: String) -> R<lib::GitStatusDto> {
    lib::git_init(Path::new(&root)).map_err(e)
}

/// 存档当前状态（无变更自动跳过）
#[tauri::command]
pub fn git_commit(root: String, message: Option<String>) -> R<lib::GitCommitResult> {
    let msg = message.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "手动存档".to_string());
    lib::git_commit(Path::new(&root), &msg).map_err(e)
}

/// 存档历史（新 → 旧）
#[tauri::command]
pub fn git_log(root: String, limit: Option<u32>) -> R<Vec<lib::GitCommitDto>> {
    lib::git_log(Path::new(&root), limit.unwrap_or(50)).map_err(e)
}

/// 恢复到指定存档（先自动备份当前状态，历史全程保留）
#[tauri::command]
pub fn git_restore(root: String, commit_id: String) -> R<lib::GitCommitResult> {
    lib::git_restore(Path::new(&root), &commit_id).map_err(e)
}

/// 文件级改动统计：from → to（to 省略为工作区）
#[tauri::command]
pub fn git_diff_summary(root: String, from: String, to: Option<String>) -> R<Vec<lib::GitDiffFileDto>> {
    lib::git_diff_summary(Path::new(&root), &from, to.as_deref()).map_err(e)
}

/// 单文件 diff 明细（hunk + 增删行）
#[tauri::command]
pub fn git_diff_file(
    root: String,
    from: String,
    to: Option<String>,
    path: String,
) -> R<lib::GitDiffDetailDto> {
    lib::git_diff_file(Path::new(&root), &from, to.as_deref(), &path).map_err(e)
}

// ── Agent 对话（妙笔） ───────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventDto {
    /// ai | toolCall | toolResult | error | plan | planProgress | reasoning
    /// | compacted | ask | retrying
    pub kind: String,
    /// 主文本（AI 增量 / 问题 / 思考 / 错误 / 摘要）
    pub text: String,
    pub tool: Option<String>,
    /// 工具参数 / 工具结果 / 计划明细
    pub detail: Option<String>,
    /// ask 的预设选项
    pub options: Vec<String>,
    pub prompt_id: Option<String>,
    pub extra: Option<serde_json::Value>,
}

impl From<&story_ai_runtime::AgentStep> for AgentEventDto {
    fn from(step: &story_ai_runtime::AgentStep) -> Self {
        use story_ai_runtime::AgentStep as S;
        let base = || AgentEventDto {
            kind: String::new(),
            text: String::new(),
            tool: None,
            detail: None,
            options: vec![],
            prompt_id: None,
            extra: None,
        };
        match step {
            S::UserMessage { content } => AgentEventDto { kind: "user".into(), text: content.clone(), ..base() },
            S::AiText { content } => AgentEventDto { kind: "ai".into(), text: content.clone(), ..base() },
            S::ToolCall { tool_name, arguments } => AgentEventDto {
                kind: "toolCall".into(),
                tool: Some(tool_name.clone()),
                detail: Some(arguments.to_string()),
                ..base()
            },
            S::ToolResult { tool_name, content } => AgentEventDto {
                kind: "toolResult".into(),
                tool: Some(tool_name.clone()),
                detail: Some(content.clone()),
                ..base()
            },
            S::Error { message } => AgentEventDto { kind: "error".into(), text: message.clone(), ..base() },
            S::Planning { plan } => AgentEventDto {
                kind: "plan".into(),
                extra: serde_json::to_value(plan).ok(),
                ..base()
            },
            S::PlanProgress { subtask_index, total, description } => AgentEventDto {
                kind: "planProgress".into(),
                text: description.clone(),
                extra: Some(serde_json::json!({ "current": subtask_index + 1, "total": total })),
                ..base()
            },
            S::Reasoning { thought } => AgentEventDto { kind: "reasoning".into(), text: thought.clone(), ..base() },
            S::ContextCompacted { summary, tokens_saved } => AgentEventDto {
                kind: "compacted".into(),
                text: summary.clone(),
                extra: Some(serde_json::json!({ "tokensSaved": tokens_saved })),
                ..base()
            },
            S::AskUser { prompt_id, question, options } => AgentEventDto {
                kind: "ask".into(),
                text: question.clone(),
                options: options.clone(),
                prompt_id: Some(prompt_id.clone()),
                ..base()
            },
            S::Retrying { attempt, max_retries, reason } => AgentEventDto {
                kind: "retrying".into(),
                text: reason.clone(),
                extra: Some(serde_json::json!({ "attempt": attempt, "max": max_retries })),
                ..base()
            },
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTraceDto {
    pub total_tokens: usize,
    pub tool_call_count: usize,
}

/// Agent 对话（项目模式，按 session 保持记忆）
///
/// 运行中步骤经 Channel 推送；遇 AskUser 时命令会阻塞在内层，
/// 前端调用 `agent_respond` 提交回答后继续。
#[tauri::command]
pub async fn agent_chat(
    root: String,
    session_id: String,
    message: String,
    on_event: tauri::ipc::Channel<AgentEventDto>,
) -> R<AgentTraceDto> {
    tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::AGENT)?;
        let on_step: story_ai_runtime::StepCallback =
            std::sync::Arc::new(move |step: story_ai_runtime::AgentStep| {
                let _ = on_event.send(AgentEventDto::from(&step));
            });
        let trace = lib::agent_chat_streaming(
            Path::new(&root),
            &session_id,
            provider,
            &message,
            on_step,
        )
        .map_err(e)?;
        Ok(AgentTraceDto {
            total_tokens: trace.total_tokens,
            tool_call_count: trace.tool_call_count,
        })
    })
    .await
    .map_err(|err| format!("任务执行失败: {err}"))?
}

/// 回答 Agent 的提问（唤醒阻塞中的对话）
#[tauri::command]
pub fn agent_respond(session_id: String, answer: String) -> bool {
    lib::tools::respond_agent(&session_id, &answer)
}

/// 停止当前 Agent 运行
#[tauri::command]
pub fn agent_cancel(session_id: String) -> bool {
    lib::tools::cancel_agent(&session_id)
}

/// 结束会话并清理记忆
#[tauri::command]
pub fn agent_drop_session(session_id: String) {
    lib::tools::drop_session(&session_id)
}

// ── 妙笔会话历史（存于项目目录 .novelstudio/chats/，随项目走） ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMetaDto {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFileDto {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    /// 时间线条目（前端自有结构，原样存取）
    pub items: Vec<serde_json::Value>,
}

fn chats_dir(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".novelstudio").join("chats")
}

fn read_chat(path: &Path) -> Result<ChatFileDto, String> {
    let content = read_file(path)?;
    serde_json::from_str(&content).map_err(|e| format!("会话文件解析失败: {e}"))
}

#[tauri::command]
pub fn chat_list(root: String) -> R<Vec<ChatMetaDto>> {
    let dir = chats_dir(&root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    let mut list = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(chat) = read_chat(&path) {
            list.push(ChatMetaDto {
                id: chat.id,
                title: chat.title,
                updated_at: chat.updated_at,
            });
        }
    }
    list.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
    Ok(list)
}

#[tauri::command]
pub fn chat_load(root: String, id: String) -> R<ChatFileDto> {
    let path = chats_dir(&root).join(format!("{id}.json"));
    if !path.exists() {
        return Err("会话不存在".into());
    }
    read_chat(&path)
}

#[tauri::command]
pub fn chat_save(root: String, chat: ChatFileDto) -> R<()> {
    let dir = chats_dir(&root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建会话目录失败: {e}"))?;
    let path = dir.join(format!("{}.json", chat.id));
    let content =
        serde_json::to_string_pretty(&chat).map_err(|e| format!("序列化会话失败: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("写入会话失败: {e}"))
}

#[tauri::command]
pub fn chat_delete(root: String, id: String) -> R<()> {
    let path = chats_dir(&root).join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("删除会话失败: {e}"))?;
    }
    Ok(())
}

// ── AI 建书（分步向导：构思候选 → 开书 → 设定 → 大纲批次）──

/// 第 1 步：生成构思候选（3 个方向，纯 LLM 无落盘）
#[tauri::command]
pub async fn ai_book_concepts(
    options: lib::BookCreationOptions,
) -> R<Vec<lib::BookConcept>> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::DESIGN)?;
        lib::ai_book_concepts(&options, std::sync::Arc::from(provider)).map_err(e)
    });
    one_call_timeout(task).await
}

/// 第 2 步：用确认后的概念建项目（返回项目根目录）
#[tauri::command]
pub fn create_book_project(
    parent_dir: String,
    concept: lib::BookConcept,
    options: lib::BookCreationOptions,
) -> R<String> {
    lib::create_book_project(Path::new(&parent_dir), &concept, &options).map_err(e)
}

/// 第 3 步：生成设定草稿（世界观/角色/关系/线索；纯 LLM 无落盘）
#[tauri::command]
pub async fn ai_book_world(
    concept: lib::BookConcept,
    options: lib::BookCreationOptions,
) -> R<lib::WorldDraft> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::DESIGN)?;
        lib::ai_book_world(&concept, &options, std::sync::Arc::from(provider)).map_err(e)
    });
    one_call_timeout(task).await
}

/// 第 3 步确认：保存设定草稿（确定性落盘）
#[tauri::command]
pub fn save_book_world(root: String, draft: lib::WorldDraft) -> R<lib::WorldSaveReport> {
    lib::save_book_world(Path::new(&root), &draft).map_err(e)
}

/// 第 4 步 / 续排：生成下一批章节脉络草稿（含已完成章节上下文）
#[tauri::command]
pub async fn ai_book_outline_batch(
    root: String,
    count: u32,
) -> R<lib::OutlineBatchDraft> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let provider = active_provider(lib::purpose::DESIGN)?;
        lib::ai_book_outline_batch(Path::new(&root), count, std::sync::Arc::from(provider))
            .map_err(e)
    });
    one_call_timeout(task).await
}

/// 第 4 步确认 / 续排保存：追加章节脉络到 chapters.toml
#[tauri::command]
pub fn save_book_outline_batch(root: String, draft: lib::OutlineBatchDraft) -> R<usize> {
    lib::save_book_outline_batch(Path::new(&root), &draft).map_err(e)
}

/// 取消进行中的建书阶段调用
#[tauri::command]
pub fn ai_book_cancel() -> bool {
    lib::cancel_book_creation()
}

/// 单次 LLM 阶段调用的统一超时（每次只生成一个阶段，5 分钟足够）
async fn one_call_timeout<T>(
    task: tauri::async_runtime::JoinHandle<Result<T, String>>,
) -> Result<T, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(300), task).await {
        Ok(res) => res.map_err(|err| format!("任务执行失败: {err}"))?,
        Err(_) => Err("这一步等得太久了，已停止等待。可以重试，或在设置里给「AI 建书」换一个更快的档案".to_string()),
    }
}

// ── 逐章成书流水线 ────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowIssueDto {
    /// word_count / truncated / dup_paragraph / compile_error / compile_warning
    pub code: String,
    /// 硬伤（需要停下来等你决定）
    pub hard: bool,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEventDto {
    /// runStarted | chapterStarted | beats | materialized | delta
    /// | committed | qualityCheck | chapterCompleted | paused
    /// | cancelled | finished
    pub kind: String,
    pub index: Option<usize>,
    pub title: Option<String>,
    /// delta 为正文增量；qualityCheck/paused 之外的文本信息
    pub text: Option<String>,
    pub beats: Vec<String>,
    pub words: Option<u32>,
    pub passed: Option<bool>,
    pub issues: Vec<WorkflowIssueDto>,
    pub pending: Option<usize>,
    pub resume: Option<bool>,
    pub completed_this_run: Option<usize>,
    pub remaining: Option<usize>,
}

impl From<&lib::WorkflowEvent> for WorkflowEventDto {
    fn from(ev: &lib::WorkflowEvent) -> Self {
        use lib::WorkflowEvent as E;
        fn to_issues(v: &[lib::QualityIssue]) -> Vec<WorkflowIssueDto> {
            v.iter()
                .map(|i| WorkflowIssueDto {
                    code: i.code.clone(),
                    hard: i.hard,
                    message: i.message.clone(),
                })
                .collect()
        }
        let mut dto = WorkflowEventDto {
            kind: String::new(),
            index: None,
            title: None,
            text: None,
            beats: Vec::new(),
            words: None,
            passed: None,
            issues: Vec::new(),
            pending: None,
            resume: None,
            completed_this_run: None,
            remaining: None,
        };
        match ev {
            E::RunStarted { pending, resume } => {
                dto.kind = "runStarted".into();
                dto.pending = Some(*pending);
                dto.resume = Some(*resume);
            }
            E::ChapterStarted { index, title } => {
                dto.kind = "chapterStarted".into();
                dto.index = Some(*index);
                dto.title = Some(title.clone());
            }
            E::BeatsGenerated { index, beats } => {
                dto.kind = "beats".into();
                dto.index = Some(*index);
                dto.beats = beats.clone();
            }
            E::ChapterMaterialized { index, .. } => {
                dto.kind = "materialized".into();
                dto.index = Some(*index);
            }
            E::Delta { index, delta } => {
                dto.kind = "delta".into();
                dto.index = Some(*index);
                dto.text = Some(delta.clone());
            }
            E::ChapterCommitted { index, words, .. } => {
                dto.kind = "committed".into();
                dto.index = Some(*index);
                dto.words = Some(*words);
            }
            E::QualityCheck {
                index,
                passed,
                issues,
            } => {
                dto.kind = "qualityCheck".into();
                dto.index = Some(*index);
                dto.passed = Some(*passed);
                dto.issues = to_issues(issues);
            }
            E::ChapterCompleted { index } => {
                dto.kind = "chapterCompleted".into();
                dto.index = Some(*index);
            }
            E::Paused { index, title, issues } => {
                dto.kind = "paused".into();
                dto.index = Some(*index);
                dto.title = Some(title.clone());
                dto.issues = to_issues(issues);
            }
            E::Cancelled => dto.kind = "cancelled".into(),
            E::Finished {
                outcome,
                completed_this_run,
                remaining,
            } => {
                dto.kind = "finished".into();
                dto.text = Some(outcome.clone());
                dto.completed_this_run = Some(*completed_this_run);
                dto.remaining = Some(*remaining);
            }
        }
        dto
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReportDto {
    /// done | paused | cancelled | batch_limit
    pub outcome: String,
    pub completed_this_run: usize,
    pub remaining: usize,
    pub paused_index: Option<usize>,
    pub paused_title: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStateDto {
    /// idle | paused | done
    pub phase: String,
    pub paused_index: Option<usize>,
    pub paused_title: Option<String>,
    pub paused_issues: Vec<WorkflowIssueDto>,
    pub target_words: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterProgressDto {
    pub index: usize,
    pub title: String,
    /// planned | writing | completed
    pub status: String,
}

/// 运行逐章成书流水线（事件经 Channel 推送，命令返回值为兜底结果）。
///
/// `action`：run（开始/续写）| approve（接受暂停的章节）| rewrite（重写）| skip（跳过）
#[tauri::command]
pub async fn workflow_run(
    root: String,
    action: String,
    target_words: Option<u32>,
    max_chapters: Option<u32>,
    on_event: tauri::ipc::Channel<WorkflowEventDto>,
) -> R<WorkflowReportDto> {
    let action = match action.as_str() {
        "run" => lib::WorkflowAction::Run,
        "approve" => lib::WorkflowAction::Approve,
        "rewrite" => lib::WorkflowAction::Rewrite,
        "skip" => lib::WorkflowAction::Skip,
        other => return Err(format!("未知的流水线操作: {other}")),
    };

    let task = tauri::async_runtime::spawn_blocking(move || {
        // Approve/Skip 决策后流水线会继续写下一章，统一用真实 Provider
        let provider = active_provider(lib::purpose::WORKFLOW)?;

        let cfg = lib::WorkflowConfig {
            target_words: target_words.unwrap_or(2000),
            min_ratio: 0.6,
            max_rewrites: 2,
            max_chapters,
        };

        let report = lib::run_workflow(
            Path::new(&root),
            provider,
            action,
            Some(cfg),
            std::sync::Arc::new(move |ev: lib::WorkflowEvent| {
                let _ = on_event.send(WorkflowEventDto::from(&ev));
            }),
            &story_ai_runtime::CancelToken::new(),
        )
        .map_err(e)?;

        Ok(WorkflowReportDto {
            outcome: report.outcome,
            completed_this_run: report.completed_this_run,
            remaining: report.remaining,
            paused_index: report.paused.as_ref().map(|p| p.index),
            paused_title: report.paused.as_ref().map(|p| p.title.clone()),
        })
    });

    // 多章长任务：上限 2 小时（正常应远小于此；停止按钮走 workflow_cancel）
    match tokio::time::timeout(std::time::Duration::from_secs(7200), task).await {
        Ok(res) => res.map_err(|err| format!("任务执行失败: {err}"))?,
        Err(_) => Err("成书流水线运行时间过长，已停止等待（进度已保存，可随时继续）".to_string()),
    }
}

/// 停止正在运行的流水线（进度保留，下次继续）
#[tauri::command]
pub fn workflow_cancel() -> bool {
    lib::workflow_cancel()
}

/// 流水线状态（冷启动恢复界面）
#[tauri::command]
pub fn workflow_status(root: String) -> R<WorkflowStateDto> {
    let state = lib::workflow_status(Path::new(&root)).map_err(e)?;
    Ok(WorkflowStateDto {
        phase: state.phase,
        paused_index: state.paused.as_ref().map(|p| p.index),
        paused_title: state.paused.as_ref().map(|p| p.title.clone()),
        paused_issues: state
            .paused
            .map(|p| {
                p.issues
                    .iter()
                    .map(|i| WorkflowIssueDto {
                        code: i.code.clone(),
                        hard: i.hard,
                        message: i.message.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        target_words: state.config.target_words,
    })
}

/// 章节进度列表（来自章节脉络）
#[tauri::command]
pub fn chapter_progress(root: String) -> R<Vec<ChapterProgressDto>> {
    lib::chapter_progress(Path::new(&root))
        .map_err(e)
        .map(|v| {
            v.into_iter()
                .map(|c| ChapterProgressDto {
                    index: c.index,
                    title: c.title,
                    status: c.status,
                })
                .collect()
        })
}

// ============================================================================
// 内部辅助
// ============================================================================

fn refresh_chapter(root: &str, chapter_id: &str) -> R<ChapterDto> {
    let chapters = load_chapter_dtos(std::path::Path::new(root))?;
    chapters
        .into_iter()
        .find(|c| c.id == chapter_id)
        .ok_or_else(|| format!("章节 '{chapter_id}' 不存在"))
}

/// 导出 DTO（story_exporter 类型的 camelCase 镜像）
pub mod story_exporter_types {
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ExportOutputDto {
        pub content: String,
        pub filename: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ChapterFileDto {
        pub filename: String,
        pub content: String,
    }
}
