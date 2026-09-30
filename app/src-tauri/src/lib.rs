//! StoryStudio 桌面端入口

pub mod commands;
pub mod dto;

use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
use tauri::TitleBarStyle;

fn build_main_window(app: &mut tauri::App) -> Result<(), tauri::Error> {
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("StoryStudio")
        .inner_size(1440.0, 960.0)
        .min_inner_size(1100.0, 720.0)
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
    window.show()?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            build_main_window(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 项目
            commands::create_project,
            commands::open_project,
            commands::refresh_workspace,
            // 结构
            commands::create_chapter,
            commands::rename_chapter,
            commands::delete_chapter,
            commands::create_scene,
            commands::rename_scene,
            commands::delete_scene,
            commands::update_scene,
            commands::get_scene,
            commands::insert_fragment,
            commands::write_fragment,
            commands::delete_fragment,
            commands::move_fragment,
            // 片段版本历史
            commands::fragment_versions,
            commands::fragment_version_read,
            commands::fragment_restore_version,
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
            commands::add_relation,
            commands::remove_relation,
            // 叙事线索
            commands::create_thread,
            commands::update_thread,
            commands::delete_thread,
            // 世界观
            commands::get_worldview,
            commands::save_worldview,
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
        ])
        .run(tauri::generate_context!())
        .expect("StoryStudio 启动失败");
}
