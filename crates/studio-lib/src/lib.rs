//! Story Studio Library — 高层编排 API
//!
//! 本 crate 是所有底层 crate（ir / parser / compiler / knowledge / exporter / ai-runtime）
//! 的薄编排层，提供面向调用方的高层函数。
//!
//! 设计原则：
//! - 每个函数是一个完整的业务操作（编译、导出、AI 续写……）
//! - 返回值全部可 serde 序列化
//! - 错误统一归一化为 StudioError

pub mod ai;
pub mod ai_check;
pub mod ai_ledger;
pub mod book_studio;
pub mod compile;
pub mod error;
pub mod export;
pub mod fragment;
pub mod git;
pub mod history;
pub mod journal;
pub mod llm_json;
pub mod mutations;
pub mod project;
pub mod project_lock;
pub mod prompt_custom;
mod prompts;
pub mod search;
pub mod stats;
pub mod timeline;
pub mod tools;
pub mod workflow;

// 常用类型的便捷 re-export
pub use ai::purpose;
pub use ai::{
    complete_fragment_streaming, complete_with_fallback, create_dynamic_provider_for_purpose,
    create_provider, create_provider_for_purpose, create_provider_for_purpose_with_thinking,
    create_provider_from_global, extract_assertions, list_models, provider_preset,
    provider_presets, read_global_ai_config, save_write_result, test_profile, write_fragment,
    write_fragment_streaming, write_global_ai_config, CompleteResult, DynamicProfileProvider,
    ExtractResult, GlobalAiConfig, ProfileTestRecord, ProviderPreset, ProviderProfile, WriteResult,
};
pub use ai_check::{
    check_fragment, check_scope, check_text, AiCheckBatchReport, AiCheckItem, AiCheckReport,
    AiCheckSeverity, CheckScope,
};
pub use book_studio::{
    ai_book_concepts, ai_book_outline_batch, ai_book_world, cancel_book_creation,
    create_book_project, save_book_outline_batch, save_book_world, BookConcept,
    BookCreationOptions, ChapterItem, OutlineBatchDraft, VolumeItem, WorldDraft, WorldSaveReport,
    CONCEPT_CANDIDATES, MAX_BATCH_CHAPTERS,
};
pub use compile::{compile_project, compile_project_full, CompileResult, CompileStats};
pub use error::StudioError;
pub use export::{export_chapters_markdown, export_markdown};
pub use fragment::{save_drafts, scene_of_fragment};
pub use git::{
    git_commit, git_diff_file, git_diff_summary, git_init, git_log, git_restore, git_status,
    GitCommitDto, GitCommitResult, GitDiffDetailDto, GitDiffFileDto, GitStatusDto,
};
pub use mutations::EntityKind;
pub use mutations::{repair_project, RepairReport};
pub use project::{
    create_project, get_book_meta, init_project, open_project, peek, peek_projects, project_info,
    save_book_meta, FileCounts, PeekEntry, Project, ProjectInfo, ProjectPeek,
};
pub use prompt_custom::{PromptEntry, PromptSpec, PROMPT_SPECS};
pub use story_ai_runtime::ApiMode;
pub use story_exporter::ChapterFile;
pub use story_project_format::StoryMeta;
pub use tools::{
    agent_chat_global_streaming, agent_chat_streaming, cancel_agent, create_tool_registry,
    drop_session, list_sessions, run_agent, run_agent_global, run_agent_global_streaming,
    run_agent_streaming, session_store, unbound_tool_registry, ToolContext,
};
pub use workflow::{
    chapter_progress, run_workflow, workflow_cancel, workflow_status, ChapterProgress, PauseInfo,
    QualityIssue, WorkflowAction, WorkflowConfig, WorkflowEvent, WorkflowEventCallback,
    WorkflowReport, WorkflowState,
};

/// crate 版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
