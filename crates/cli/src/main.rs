//! Story Studio CLI — 命令行入口
//!
//! 只负责参数解析 + 输出格式化 + exit code。
//! 业务逻辑全部委托给 story_studio_lib。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use story_studio_lib::StudioError;

#[derive(Parser)]
#[command(name = "story-studio")]
#[command(about = "Story Studio — 面向长篇小说创作的 IDE")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 初始化新项目
    Init {
        name: String,
        #[arg(short, long, default_value = ".")]
        location: String,
    },

    /// 一次性修复数据一致性（片段残留 YAML 块、断言证据引用）
    Repair {
        #[arg(short, long, default_value = ".")]
        dir: String,
    },

    /// 编译项目（解析 + 检查）
    Compile {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(long)]
        full: bool,
    },

    /// 导出项目为指定格式
    Export {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long, default_value = "markdown")]
        format: String,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        appendix: bool,
    },

    /// 显示项目信息
    Info {
        #[arg(short, long, default_value = ".")]
        dir: String,
    },

    /// AI 续写 Fragment
    Write {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long)]
        scene: String,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(short, long)]
        model: Option<String>,
        #[arg(long)]
        instruction: Option<String>,
        #[arg(long)]
        save: bool,
    },

    /// AI 抽取知识断言
    Extract {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long)]
        fragment: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(long, default_value = "alive,age")]
        predicates: String,
    },

    /// 逐章生文流水线（节拍 → 流式写整章 → 编译质检 → 硬伤暂停等人决策）
    Workflow {
        #[command(subcommand)]
        action: WorkflowCommands,
    },

    /// 启动 MCP 服务器：把项目工具集暴露给外部 Agent（stdio 或 HTTP 传输）
    Mcp {
        #[arg(short, long, default_value = ".")]
        dir: String,
        /// 只读模式：只暴露查询类工具，不修改项目
        #[arg(long)]
        read_only: bool,
        /// 传输方式：stdio（由客户端拉起）或 http（本方常驻监听）
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// HTTP 监听端口（仅 --transport http 时生效）
        #[arg(long, default_value_t = story_mcp::DEFAULT_HTTP_PORT)]
        port: u16,
        /// HTTP 监听地址（仅 --transport http 时生效，默认仅本机回环）
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// 重置持久化 Token（仅 --transport http：生成新值写入 ~/.story-studio/mcp-token）
        #[arg(long)]
        new_token: bool,
    },
}

#[derive(Subcommand)]
enum WorkflowCommands {
    /// 运行 / 断点续写（硬伤暂停后需先 approve / rewrite / skip）
    Run {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
        /// 每章目标字数
        #[arg(long, default_value_t = 2000)]
        target_words: u32,
        /// 本次最多生成的章数
        #[arg(long)]
        max_chapters: Option<u32>,
    },

    /// 查看流水线状态与章节进度
    Status {
        #[arg(short, long, default_value = ".")]
        dir: String,
    },

    /// 批准暂停中的章节，继续下一章
    Approve {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
    },

    /// 重写暂停中的章节（带原稿参考与失败原因）
    Rewrite {
        #[arg(short, long, default_value = ".")]
        dir: String,
        #[arg(short, long)]
        provider: Option<String>,
        #[arg(short = 'm', long)]
        model: Option<String>,
    },

    /// 跳过暂停中的章节（标记完成，交还人工处理）
    Skip {
        #[arg(short, long, default_value = ".")]
        dir: String,
    },
}

fn main() {
    let cli = Cli::parse();

    // MCP 的 stdio 传输占用 stdout 作为协议通道，日志必须走 stderr，
    // 否则任何一条 tracing 输出都会破坏 JSON-RPC 帧
    if matches!(cli.command, Commands::Mcp { .. }) {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }

    let result = match cli.command {
        Commands::Init { name, location } => cmd_init(&name, &location),
        Commands::Repair { dir } => cmd_repair(&dir),
        Commands::Compile { dir, full } => cmd_compile(&dir, full),
        Commands::Export {
            dir,
            format,
            output,
            appendix,
        } => cmd_export(&dir, &format, output.as_deref(), appendix),
        Commands::Info { dir } => cmd_info(&dir),
        Commands::Write {
            dir,
            scene,
            provider,
            model,
            instruction,
            save,
        } => cmd_write(
            &dir,
            &scene,
            provider.as_deref(),
            model.as_deref(),
            instruction.as_deref(),
            save,
        ),
        Commands::Extract {
            dir,
            fragment,
            provider,
            model,
            predicates,
        } => cmd_extract(
            &dir,
            &fragment,
            provider.as_deref(),
            model.as_deref(),
            &predicates,
        ),
        Commands::Workflow { action } => match action {
            WorkflowCommands::Run {
                dir,
                provider,
                model,
                target_words,
                max_chapters,
            } => cmd_workflow_run(
                &dir,
                provider.as_deref(),
                model.as_deref(),
                target_words,
                max_chapters,
            ),
            WorkflowCommands::Status { dir } => cmd_workflow_status(&dir),
            WorkflowCommands::Approve {
                dir,
                provider,
                model,
            } => cmd_workflow_action(
                &dir,
                story_studio_lib::WorkflowAction::Approve,
                provider.as_deref(),
                model.as_deref(),
            ),
            WorkflowCommands::Rewrite {
                dir,
                provider,
                model,
            } => cmd_workflow_action(
                &dir,
                story_studio_lib::WorkflowAction::Rewrite,
                provider.as_deref(),
                model.as_deref(),
            ),
            WorkflowCommands::Skip { dir } => {
                cmd_workflow_action(&dir, story_studio_lib::WorkflowAction::Skip, None, None)
            }
        },
        Commands::Mcp {
            dir,
            read_only,
            transport,
            port,
            bind,
            new_token,
        } => cmd_mcp(&dir, read_only, &transport, port, &bind, new_token),
    };

    if let Err(e) = result {
        eprintln!("✗ {e}");
        std::process::exit(1);
    }
}

// ============================================================================
// mcp — MCP 服务器
// ============================================================================

fn cmd_mcp(
    dir: &str,
    read_only: bool,
    transport: &str,
    port: u16,
    bind: &str,
    new_token: bool,
) -> Result<(), StudioError> {
    let options = story_mcp::McpServerOptions {
        project_dir: PathBuf::from(dir),
        read_only,
    };
    match transport {
        "stdio" => story_mcp::run_server(options),
        "http" => {
            let bind_addr: std::net::IpAddr = bind
                .parse()
                .map_err(|e| StudioError::Config(format!("无效的监听地址 {bind:?}：{e}")))?;
            if new_token {
                let token = story_mcp::regenerate_token()?;
                println!("已重置 Token：{token}");
            }
            let handle = story_mcp::start_server_http(
                options,
                story_mcp::HttpServerOptions {
                    bind: bind_addr,
                    port,
                    token: None,
                    session_path: None,
                },
            )?;
            println!("MCP HTTP 服务已启动：{}", handle.url());
            println!(
                "  Token：{}（持久化于 ~/.story-studio/mcp-token，重启不轮换；接入需携带 Authorization: Bearer <token>）",
                handle.token()
            );
            println!(
                "  工具数：{}（只读模式：{}）",
                handle.tool_count(),
                read_only
            );
            println!("  Ctrl-C 退出。");
            // 阻塞直到服务线程结束（Ctrl-C 直接结束进程，无需信号处理）
            handle.wait();
            Ok(())
        }
        other => Err(StudioError::Config(format!(
            "不支持的传输方式：{other}（支持: stdio, http）"
        ))),
    }
}

// ============================================================================
// init
// ============================================================================

fn cmd_init(name: &str, location: &str) -> Result<(), StudioError> {
    // 目录名由书名 slug 化得到（git 友好），书名原样写入 project.toml
    let root = story_studio_lib::create_project(std::path::Path::new(location), name)?;
    let dir_name = root.file_name().and_then(|s| s.to_str()).unwrap_or(name);

    println!("✓ 项目已创建：{}", root.display());
    println!("  目录结构：");
    println!("    {}/", dir_name);
    println!("    ├── project.toml");
    println!("    ├── structure.toml");
    println!("    ├── fragments/");
    println!("    ├── entities/");
    println!("    │   ├── characters/");
    println!("    │   ├── locations/");
    println!("    │   ├── items/");
    println!("    │   └── organizations/");
    println!("    ├── knowledge/");
    println!("    ├── config/");
    println!("    │   └── compiler.toml");
    println!("    └── .gitignore");
    println!();
    println!("  下一步：");
    println!("    1. 在 entities/characters/ 添加角色");
    println!("    2. 在 fragments/ 添加 Fragment");
    println!("    3. 编辑 structure.toml 组织章节结构");
    println!("    4. 运行 story-studio compile 检查");

    Ok(())
}

// ============================================================================
// compile
// ============================================================================

fn cmd_repair(dir: &str) -> Result<(), StudioError> {
    // 修复是批量写操作，与桌面端 / MCP 在同一把项目锁上排队
    let _gate =
        story_studio_lib::project_lock::ProjectLock::for_root(std::path::Path::new(dir)).write();
    let report = story_studio_lib::repair_project(std::path::Path::new(dir))?;
    if report.fragments_cleaned.is_empty()
        && report.assertions_fixed == 0
        && report.evidence_skipped == 0
    {
        println!("数据一致，无需修复。");
        return Ok(());
    }
    if !report.fragments_cleaned.is_empty() {
        println!(
            "清除正文残留 YAML 块的片段 {} 个: {}",
            report.fragments_cleaned.len(),
            report.fragments_cleaned.join(", ")
        );
    }
    if report.assertions_fixed > 0 {
        println!("重建证据引用的断言 {} 条", report.assertions_fixed);
    }
    if report.evidence_skipped > 0 {
        println!("{} 条证据指向不存在的片段，已保留", report.evidence_skipped);
    }
    Ok(())
}

fn cmd_compile(dir: &str, force_full: bool) -> Result<(), StudioError> {
    let project_dir = PathBuf::from(dir);
    let result = if force_full {
        story_studio_lib::compile_project_full(&project_dir)?
    } else {
        story_studio_lib::compile_project(&project_dir)?
    };

    // 显示解析警告
    if !result.warnings.is_empty() {
        eprintln!("⚠ 解析警告（{} 个）：", result.warnings.len());
        for warn in &result.warnings {
            eprintln!("  [Warning] {}", warn.kind);
        }
        eprintln!();
    }

    // 输出统计
    println!("编译完成。");
    println!("  Fragment: {}", result.stats.fragment_count);
    println!("  Entity:   {}", result.stats.entity_count);
    println!("  Edge:     {}", result.stats.edge_count);
    if result.stats.assertion_count > 0 || result.stats.evidence_count > 0 {
        println!("  Assertion: {}", result.stats.assertion_count);
        println!("  Evidence:  {}", result.stats.evidence_count);
    }
    println!("  诊断:     {} 条", result.diagnostics.len());
    println!();

    if result.diagnostics.is_empty() {
        println!("✓ 无诊断问题。");
    } else {
        for diag in result.diagnostics.iter() {
            let icon = match diag.severity {
                story_compiler::Severity::Error => "✗",
                story_compiler::Severity::Warning => "⚠",
                story_compiler::Severity::Hint => "→",
                story_compiler::Severity::Info => "·",
            };
            let loc_str = format_location(&diag.loc);
            println!(
                "  {icon} [{severity}] {code}: {msg}",
                icon = icon,
                severity = diag.severity,
                code = diag.code,
                msg = diag.message,
            );
            if !loc_str.is_empty() {
                println!("      @ {loc_str}");
            }
        }

        if result.diagnostics.has_errors() {
            std::process::exit(1);
        }
    }

    Ok(())
}

// ============================================================================
// export
// ============================================================================

fn cmd_export(
    dir: &str,
    format: &str,
    output: Option<&str>,
    include_appendix: bool,
) -> Result<(), StudioError> {
    if format != "markdown" && format != "md" {
        return Err(StudioError::Config(format!(
            "不支持的导出格式：{format}（支持: markdown）"
        )));
    }

    let project_dir = PathBuf::from(dir);
    let result = story_studio_lib::export_markdown(&project_dir, include_appendix, false, false)?;

    match output {
        Some(path) => {
            std::fs::write(path, &result.content)?;
            let lines = result.content.lines().count();
            let chars = result.content.chars().count();
            println!("✓ 已导出到 {path}");
            println!("  格式：{format}");
            println!("  行数：{lines}");
            println!("  字符：{chars}");
        }
        None => {
            print!("{}", result.content);
        }
    }

    Ok(())
}

// ============================================================================
// info
// ============================================================================

fn cmd_info(dir: &str) -> Result<(), StudioError> {
    let project_dir = PathBuf::from(dir);
    let info = story_studio_lib::project_info(&project_dir)?;

    println!("Story Studio 项目信息");
    println!("  目录：{}", info.root.display());
    println!();

    println!("  书名：{}", info.name);
    if let Some(author) = &info.author {
        println!("  作者：{author}");
    }
    if let Some(lang) = &info.language {
        println!("  语言：{lang}");
    }
    println!("  IR 版本：{}", info.ir_version);
    println!();
    println!("  文件统计：");
    println!("    Fragment: {}", info.file_counts.fragments);
    println!("    Character: {}", info.file_counts.characters);
    println!("    Location: {}", info.file_counts.locations);
    println!("    Item: {}", info.file_counts.items);

    Ok(())
}

// ============================================================================
// write — AI 续写 Fragment
// ============================================================================

fn cmd_write(
    dir: &str,
    scene_id_str: &str,
    provider_name: Option<&str>,
    model_name: Option<&str>,
    instruction: Option<&str>,
    save: bool,
) -> Result<(), StudioError> {
    let project_dir = PathBuf::from(dir);

    let provider = story_studio_lib::create_provider(
        provider_name.unwrap_or("anthropic"),
        model_name,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    println!("正在生成...");
    let result = story_studio_lib::write_fragment(
        &project_dir,
        scene_id_str,
        provider.as_ref(),
        instruction,
    )?;

    println!(
        "✓ 生成完成（{} 个片段，{} tokens）",
        result.drafts.len(),
        result.tokens_used
    );

    if save {
        // 落盘瞬间进项目锁（生成阶段不持锁），与桌面端 / MCP 写操作排队
        let _gate = story_studio_lib::project_lock::ProjectLock::for_root(&project_dir).write();
        match story_studio_lib::save_write_result(&project_dir, scene_id_str, &result) {
            Ok(saved_ids) => {
                println!("✓ 已保存 {} 个 Fragment：", saved_ids.len());
                for id in &saved_ids {
                    println!("  fragments/{id}.md");
                }
                println!("  structure.toml 已更新（Scene {scene_id_str}）");
            }
            Err(e) => {
                eprintln!("✗ 保存失败：{e}");
                // 保存失败时仍然打印内容，不丢失
                for (i, draft) in result.drafts.iter().enumerate() {
                    println!("\n--- 片段 {} ---", i + 1);
                    println!("{}", draft.text);
                }
            }
        }
    } else {
        println!();
        for (i, draft) in result.drafts.iter().enumerate() {
            println!("--- 片段 {} ---", i + 1);
            if !draft.anchors.is_empty() {
                println!(
                    "（anchors: {:?}）",
                    draft
                        .anchors
                        .iter()
                        .map(|a| format!("{}:{}", a.entity, a.role))
                        .collect::<Vec<_>>()
                );
            }
            println!("{}", draft.text);
        }
        println!("\n提示：使用 --save 自动保存到项目");
    }

    Ok(())
}

// ============================================================================
// extract — AI 抽取知识断言
// ============================================================================

fn cmd_extract(
    dir: &str,
    fragment_id_str: &str,
    provider_name: Option<&str>,
    model_name: Option<&str>,
    predicates_str: &str,
) -> Result<(), StudioError> {
    let project_dir = PathBuf::from(dir);

    let provider = story_studio_lib::create_provider(
        provider_name.unwrap_or("anthropic"),
        model_name,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let predicates: Vec<String> = predicates_str
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    println!("正在抽取...");
    let result = story_studio_lib::extract_assertions(
        &project_dir,
        fragment_id_str,
        provider.as_ref(),
        &predicates,
    )?;

    if result.drafts.is_empty() {
        println!("✓ 未发现可抽取的断言（{} tokens）", result.tokens_used);
    } else {
        println!(
            "✓ 抽取到 {} 条断言（{} tokens）\n",
            result.drafts.len(),
            result.tokens_used
        );
        for (i, d) in result.drafts.iter().enumerate() {
            println!("  {}. {} {} = {}", i + 1, d.subject, d.predicate, d.object);
            println!("     证据：\"{}\"", d.quote);
            if let Some(y) = d.from_year {
                println!(
                    "     时间：{} - {}",
                    y,
                    d.until_year.map_or("至今".to_string(), |u| u.to_string())
                );
            }
        }
    }

    Ok(())
}

// ============================================================================
// 辅助
// ============================================================================

fn format_location(loc: &story_compiler::SourceLocation) -> String {
    let mut parts = Vec::new();
    if let Some(e) = &loc.entity {
        parts.push(format!("entity:{e}"));
    }
    if let Some(f) = &loc.fragment {
        parts.push(format!("fragment:{f}"));
    }
    if let Some(s) = &loc.structure {
        parts.push(format!("structure:{s}"));
    }
    if let Some(a) = &loc.assertion {
        parts.push(format!("assertion:{a}"));
    }
    parts.join(", ")
}

// ============================================================================
// workflow — 逐章生文流水线
// ============================================================================

/// 从全局配置解析默认 provider/model（-p/-m 参数优先）
fn resolve_provider_defaults(
    provider: Option<&str>,
    model: Option<&str>,
) -> (String, Option<String>) {
    if let Some(p) = provider {
        return (p.to_string(), model.map(String::from));
    }
    let config = story_studio_lib::read_global_ai_config().unwrap_or_default();
    if let Some(p) = config.active_profile() {
        let name = p
            .provider
            .clone()
            .unwrap_or_else(|| "anthropic".to_string());
        let m = model.map(String::from).or_else(|| p.model.clone());
        return (name, m);
    }
    let name = config
        .default_provider
        .unwrap_or_else(|| "anthropic".to_string());
    let m = model
        .map(String::from)
        .or_else(|| config.default_model.clone());
    (name, m)
}

fn cmd_workflow_run(
    dir: &str,
    provider: Option<&str>,
    model: Option<&str>,
    target_words: u32,
    max_chapters: Option<u32>,
) -> Result<(), StudioError> {
    let (provider_name, model_name) = resolve_provider_defaults(provider, model);
    let provider =
        story_studio_lib::create_provider_from_global(&provider_name, model_name.as_deref())?;

    let config = story_studio_lib::WorkflowConfig {
        target_words,
        min_ratio: 0.6,
        max_rewrites: 2,
        max_chapters,
    };

    let report = run_workflow_cli(
        dir,
        provider,
        story_studio_lib::WorkflowAction::Run,
        Some(config),
    )?;
    print_workflow_report(&report);
    Ok(())
}

fn cmd_workflow_action(
    dir: &str,
    action: story_studio_lib::WorkflowAction,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<(), StudioError> {
    // Approve / Skip 不需要 LLM；Rewrite 需要
    let provider: Box<dyn story_ai_runtime::AiProvider> =
        if matches!(action, story_studio_lib::WorkflowAction::Rewrite) {
            let (provider_name, model_name) = resolve_provider_defaults(provider, model);
            story_studio_lib::create_provider_from_global(&provider_name, model_name.as_deref())?
        } else {
            // 占位 provider（不会被调用）
            story_studio_lib::create_provider(
                "anthropic",
                None,
                None,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )?
        };

    let report = run_workflow_cli(dir, provider, action, None)?;
    print_workflow_report(&report);
    Ok(())
}

fn run_workflow_cli(
    dir: &str,
    provider: Box<dyn story_ai_runtime::AiProvider>,
    action: story_studio_lib::WorkflowAction,
    config: Option<story_studio_lib::WorkflowConfig>,
) -> Result<story_studio_lib::WorkflowReport, StudioError> {
    use std::io::Write as _;
    use std::sync::Arc;

    let dir_owned = dir.to_string();
    let on_event: story_studio_lib::WorkflowEventCallback = Arc::new(move |event| {
        use story_studio_lib::WorkflowEvent as E;
        match &event {
            E::RunStarted { pending, resume } => println!(
                "{}逐章生文流水线（待写 {pending} 章）",
                if *resume { "继续 " } else { "开始 " }
            ),
            E::ChapterStarted { index, title } => {
                println!();
                println!("▶ 第 {index} 章《{title}》");
            }
            E::BeatsGenerated { beats, .. } => {
                println!("  生成 {} 个节拍：{}", beats.len(), beats.join(" / "));
            }
            E::ChapterMaterialized {
                chapter_id,
                scene_id,
                ..
            } => println!("  落实结构 {chapter_id} / {scene_id}"),
            E::Delta { delta, .. } => {
                print!("{delta}");
                let _ = std::io::stdout().flush();
            }
            E::ChapterCommitted {
                fragment_id, words, ..
            } => println!("\n  提交 {fragment_id}（{words} 字）"),
            E::QualityCheck { passed, issues, .. } => {
                if *passed {
                    println!("  质检通过");
                } else {
                    println!("  质检问题（{} 项）：", issues.len());
                    for issue in issues {
                        println!(
                            "    {} {}",
                            if issue.hard { "✗" } else { "⚠" },
                            issue.message
                        );
                    }
                }
            }
            E::ChapterCompleted { .. } => println!("✓ 本章完成"),
            E::Paused {
                index,
                title,
                issues,
                ..
            } => {
                println!();
                println!(
                    "⏸ 第 {index} 章《{title}》暂停待审（{} 项问题）：",
                    issues.len()
                );
                for issue in issues {
                    println!(
                        "    {} {}",
                        if issue.hard { "✗" } else { "⚠" },
                        issue.message
                    );
                }
                println!(
                    "  决策后继续：story-studio workflow approve | rewrite | skip -d {dir_owned}"
                );
            }
            E::Cancelled => println!("\n已取消（进度已保存，可随时 workflow run 续写）"),
            E::Finished {
                outcome,
                completed_this_run,
                remaining,
            } => {
                println!();
                println!(
                    "结束：{outcome}（本次完成 {completed_this_run} 章，剩余 {remaining} 章）"
                );
            }
        }
    });

    story_studio_lib::run_workflow(
        std::path::Path::new(dir),
        Arc::from(provider),
        action,
        config,
        on_event,
        &story_ai_runtime::CancelToken::new(),
    )
}

fn print_workflow_report(report: &story_studio_lib::WorkflowReport) {
    match report.outcome.as_str() {
        "done" => println!("全部章节已完成。"),
        "paused" => {}
        "cancelled" => {}
        "batch_limit" => println!("达到批次上限，剩余章节下次运行继续。"),
        other => println!("结果：{other}"),
    }
}

fn cmd_workflow_status(dir: &str) -> Result<(), StudioError> {
    let state = story_studio_lib::workflow_status(std::path::Path::new(dir))?;
    println!("流水线状态：{}", state.phase);
    println!(
        "  配置：目标 {} 字/章，重写上限 {} 次",
        state.config.target_words, state.config.max_rewrites
    );
    if let Some(pause) = &state.paused {
        println!(
            "  暂停在：第 {} 章《{}》（已重写 {} 次，{} 项问题）",
            pause.index,
            pause.title,
            pause.rewrites_used,
            pause.issues.len()
        );
        for issue in &pause.issues {
            println!(
                "    {} {}",
                if issue.hard { "✗" } else { "⚠" },
                issue.message
            );
        }
        println!("  继续：story-studio workflow approve | rewrite | skip -d {dir}");
    }

    let progress = story_studio_lib::chapter_progress(std::path::Path::new(dir))?;
    if progress.is_empty() {
        println!();
        println!("chapters.toml 尚无章节设计。先用 AI 助手 design_chapters 或手写章节脉络。");
        return Ok(());
    }
    println!();
    println!("章节进度：");
    for c in &progress {
        let mark = match c.status.as_str() {
            "completed" => "✓",
            "writing" => "…",
            _ => "·",
        };
        println!("  {mark} 第 {} 章 {}", c.index, c.title);
    }
    Ok(())
}
