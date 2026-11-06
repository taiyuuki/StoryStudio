//! StoryStudio 桌面端入口

pub mod commands;
pub mod dto;
pub mod watch;

use tauri::{LogicalPosition, LogicalSize, WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
use tauri::TitleBarStyle;

/// 初始窗口的理想尺寸
const DEFAULT_WINDOW_SIZE: (f64, f64) = (1440.0, 960.0);
/// 窗口最小尺寸（工作区塞不下默认值时同步放宽）
const MIN_WINDOW_SIZE: (f64, f64) = (1100.0, 720.0);

/// 把窗口钳进显示器工作区（排除任务栏 / Dock）并居中。
///
/// 固定 1440×960 在高 DPI 缩放或小屏笔记本上会超出可视高度，
/// 底部压住任务栏；builder 的 `center()` 按整屏居中，任务栏在底部时
/// 同样偏低。这里按工作区收缩尺寸、按工作区中心摆位。
fn clamp_window_to_work_area(window: &tauri::WebviewWindow) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return; // 拿不到显示器信息就维持 builder 的默认尺寸与居中
    };

    // 工作区是物理像素，窗口 API 用逻辑像素
    let scale = monitor.scale_factor();
    let wa = monitor.work_area();
    let (wa_x, wa_y) = (wa.position.x as f64 / scale, wa.position.y as f64 / scale);
    let (wa_w, wa_h) = (wa.size.width as f64 / scale, wa.size.height as f64 / scale);

    // 给窗口阴影与边线留一点呼吸；下限保证极端小屏（低于最小尺寸的
    // 工作区）仍能完整放下窗口，此时同步放宽最小尺寸约束
    let margin = 12.0;
    let target_w = DEFAULT_WINDOW_SIZE
        .0
        .min((wa_w - margin * 2.0).max(MIN_WINDOW_SIZE.0.min(wa_w)));
    let target_h = DEFAULT_WINDOW_SIZE
        .1
        .min((wa_h - margin * 2.0).max(MIN_WINDOW_SIZE.1.min(wa_h)));

    if target_w < MIN_WINDOW_SIZE.0 || target_h < MIN_WINDOW_SIZE.1 {
        let _ = window.set_min_size(Some(LogicalSize::new(target_w, target_h)));
    }

    let _ = window.set_size(LogicalSize::new(target_w, target_h));
    let _ = window.set_position(LogicalPosition::new(
        wa_x + (wa_w - target_w) / 2.0,
        wa_y + (wa_h - target_h) / 2.0,
    ));
}

fn build_main_window(app: &mut tauri::App) -> Result<(), tauri::Error> {
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("StoryStudio")
        .inner_size(DEFAULT_WINDOW_SIZE.0, DEFAULT_WINDOW_SIZE.1)
        .min_inner_size(MIN_WINDOW_SIZE.0, MIN_WINDOW_SIZE.1)
        .center()
        .visible(false)
        .shadow(true);

    // macOS：系统红绿灯内嵌在自绘标题栏中；Windows/Linux：自绘整条标题栏
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true);

    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);

    let window = builder.build()?;
    clamp_window_to_work_area(&window);
    window.show()?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            build_main_window(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 项目
            commands::create_project,
            commands::open_project,
            commands::refresh_workspace,
            commands::peek_projects,
            // 结构
            commands::create_chapter,
            commands::rename_chapter,
            commands::delete_chapter,
            commands::create_scene,
            commands::rename_scene,
            commands::delete_scene,
            commands::update_scene,
            commands::get_scene,
            commands::read_chapter,
            commands::insert_fragment,
            commands::write_fragment,
            commands::delete_fragment,
            commands::move_fragment,
            // 片段锚点（手动编辑）
            commands::set_fragment_anchor,
            commands::remove_fragment_anchor,
            // 片段版本历史
            commands::fragment_versions,
            commands::fragment_version_read,
            commands::fragment_restore_version,
            commands::fragment_version_diff,
            commands::thread_knowledge_matrix,
            // 码字统计 / AI 账本
            commands::stats_summary,
            commands::stats_set_goal,
            commands::stats_sync_clock,
            commands::ai_ledger_summary,
            // 实体 / 关系
            commands::create_entity,
            commands::delete_entity,
            commands::save_entity,
            commands::merge_entities,
            commands::resolve_unresolved,
            commands::ignore_unresolved_refs,
            commands::delete_unresolved_refs,
            commands::add_relation,
            commands::remove_relation,
            commands::update_relation,
            // 叙事线索
            commands::create_thread,
            commands::update_thread,
            commands::advance_thread,
            commands::delete_thread,
            commands::set_thread_knowledge,
            commands::remove_thread_knowledge,
            // 知识断言
            commands::list_knowledge,
            commands::add_assertion,
            commands::set_assertion_status,
            commands::delete_assertion,
            // 世界观
            commands::get_worldview,
            commands::save_worldview,
            // 提示词自定义
            commands::list_prompts,
            commands::save_prompt,
            commands::reset_prompt,
            // 书籍信息
            commands::get_book_meta,
            commands::save_book_meta,
            // 章节脉络
            commands::get_chapter_outline,
            commands::save_chapter_outline,
            // 编译 / 导出
            commands::compile_project,
            commands::export_markdown,
            commands::export_chapters,
            commands::write_text_file,
            // AI
            commands::get_ai_config,
            commands::set_ai_config,
            commands::provider_presets,
            commands::ai_list_models,
            commands::ai_test_profile,
            commands::ai_write_scene,
            commands::ai_complete_fragment,
            commands::ai_complete_cancel,
            commands::ai_check,
            commands::ai_anchor_fragment,
            commands::scene_of_fragment,
            commands::search_text,
            commands::git_status,
            commands::git_init,
            commands::git_commit,
            commands::git_log,
            commands::git_restore,
            commands::git_diff_summary,
            commands::git_diff_file,
            commands::agent_chat,
            commands::agent_respond,
            commands::agent_cancel,
            commands::agent_drop_session,
            commands::chat_list,
            commands::chat_load,
            commands::chat_save,
            commands::chat_delete,
            // AI 建书（分步向导）
            commands::ai_book_concepts,
            commands::create_book_project,
            commands::ai_book_world,
            commands::save_book_world,
            commands::ai_book_outline_batch,
            commands::save_book_outline_batch,
            commands::ai_book_cancel,
            // 逐章成书流水线
            commands::workflow_run,
            commands::workflow_cancel,
            commands::workflow_status,
            commands::chapter_progress,
            // MCP 服务（外部 Agent 接入）
            commands::mcp_server_start,
            commands::mcp_server_stop,
            commands::mcp_server_status,
            // 文件监听（RFC-0019：磁盘 → 前端实时同步）
            commands::watch_start,
            commands::watch_stop,
            commands::watch_status,
        ])
        .run(tauri::generate_context!())
        .expect("StoryStudio 启动失败");
}
