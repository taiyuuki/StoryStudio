//! 项目级操作 — init / open / info

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use story_project_format::{slugify, InitError, ProjectConfig, ProjectPaths};

use crate::error::StudioError;

// ============================================================================
// Project — 打开后的项目句柄
// ============================================================================

/// 一个已打开的项目，持有路径信息和配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// 项目根目录
    pub root: PathBuf,
    /// project.toml 的配置（已解析）
    pub config: ProjectConfig,
}

impl Project {
    /// 返回路径辅助器
    pub fn paths(&self) -> ProjectPaths {
        ProjectPaths::new(&self.root)
    }
}

// ============================================================================
// ProjectInfo — 可序列化的项目概要
// ============================================================================

/// 项目信息（对应 CLI 的 info 命令输出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub name: String,
    pub author: Option<String>,
    pub language: Option<String>,
    pub ir_version: u32,
    /// 文件统计
    pub file_counts: FileCounts,
}

/// 各类文件的计数
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileCounts {
    pub fragments: usize,
    pub characters: usize,
    pub locations: usize,
    pub items: usize,
}

// ============================================================================
// API
// ============================================================================

/// 初始化新项目
///
/// 在 `root` 下创建名为 `name` 的项目目录。
/// 返回创建后的路径信息。
pub fn init_project(root: &Path, name: &str) -> Result<ProjectPaths, StudioError> {
    story_project_format::init_project(root, name).map_err(init_error_to_studio_error)
}

/// 创建新项目（git 友好的目录名）
///
/// 在 `parent_dir` 下根据书名生成 git 友好的拼音 slug 作为子目录名
/// （书名仍以原始中文写入 `project.toml` / `structure.toml`）。
/// 若 slug 已存在，自动追加 `-2`、`-3`……直到找到可用目录。
/// 返回最终的项目根路径。
pub fn create_project(parent_dir: &Path, book_name: &str) -> Result<PathBuf, StudioError> {
    let base_slug = slugify(book_name);
    let mut candidate = parent_dir.join(&base_slug);
    let mut counter = 2;
    while candidate.exists() {
        candidate = parent_dir.join(format!("{base_slug}-{counter}"));
        counter += 1;
    }
    init_project(&candidate, book_name).map(|paths| paths.root().to_path_buf())
}

/// 打开已有项目
///
/// 读取 project.toml，返回 Project 句柄。
pub fn open_project(dir: &Path) -> Result<Project, StudioError> {
    let paths = ProjectPaths::new(dir);
    let toml_path = paths.project_toml();

    if !toml_path.exists() {
        return Err(StudioError::NotFound(format!(
            "project.toml 不存在于 {}",
            toml_path.display()
        )));
    }

    let content = std::fs::read_to_string(&toml_path)?;
    let config = ProjectConfig::from_toml_str(&content)
        .map_err(|e| StudioError::Config(format!("project.toml 解析失败: {e}")))?;

    Ok(Project {
        root: dir.to_path_buf(),
        config,
    })
}

/// 获取项目信息
///
/// 读取 project.toml + 统计文件数量。
pub fn project_info(dir: &Path) -> Result<ProjectInfo, StudioError> {
    let project = open_project(dir)?;
    let paths = project.paths();

    let file_counts = FileCounts {
        fragments: count_files(&paths.fragments_dir(), "md"),
        characters: count_files(&paths.characters_dir(), "toml"),
        locations: count_files(&paths.locations_dir(), "toml"),
        items: count_files(&paths.items_dir(), "toml"),
    };

    Ok(ProjectInfo {
        root: project.root,
        name: project.config.project.name.clone(),
        author: project.config.project.author.clone(),
        language: project.config.project.language.clone(),
        ir_version: project.config.project.ir_version.0,
        file_counts,
    })
}

// ============================================================================
// 内部辅助
// ============================================================================

fn init_error_to_studio_error(e: InitError) -> StudioError {
    StudioError::Io(format!("初始化失败: {e}"))
}

/// 统计目录下指定后缀的文件数
fn count_files(dir: &Path, ext: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some(ext))
                .count()
        })
        .unwrap_or(0)
}
