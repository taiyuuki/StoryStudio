//! 项目级操作 — init / open / info

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use story_project_format::{slugify, InitError, OutlineFile, ProjectConfig, ProjectPaths, StoryMeta, StructureFile};

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
// ProjectPeek — 首页轻量概览（不建 IR）
// ============================================================================

/// 单个项目的轻量概览（书案首页用）
///
/// 只读 project / structure / outline / fragments / stats，
/// 不做 IR 解析，长篇下也能毫秒级返回。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPeek {
    pub root: String,
    pub name: String,
    /// 目标字数（outline.toml [story]；未设为 None）
    pub target_words: Option<u64>,
    pub chapter_count: usize,
    pub scene_count: usize,
    pub fragment_count: usize,
    /// 当前正文总字数（frontmatter 与锚点注释剥离后计数）
    pub word_count: u64,
    pub today: crate::stats::DayCount,
    pub streak_days: u32,
    pub daily_goal: u32,
    pub total: crate::stats::DayCount,
}

/// 批量 peek 的单条结果：目录不存在/损坏时 `peek` 为 None
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeekEntry {
    pub root: String,
    pub peek: Option<ProjectPeek>,
}

/// 轻量窥视一个项目（首页书案用）
pub fn peek(dir: &Path) -> Result<ProjectPeek, StudioError> {
    let project = open_project(dir)?;
    let paths = project.paths();

    // 章/场景数：只反序列化 structure.toml，不建文档图
    let (chapter_count, scene_count) = std::fs::read_to_string(paths.structure_toml())
        .ok()
        .and_then(|c| StructureFile::from_toml_str(&c).ok())
        .map(|s| {
            (
                s.book.chapters.len(),
                s.chapter.values().map(|c| c.scenes.len()).sum(),
            )
        })
        .unwrap_or((0, 0));

    // 目标字数：outline.toml 缺失（手动新建的空项目）视为未设置
    let target_words = std::fs::read_to_string(paths.outline_toml())
        .ok()
        .and_then(|c| OutlineFile::from_toml_str(&c).ok())
        .and_then(|o| o.story.target_words);

    // 正文总字数：剥离 frontmatter 与锚点注释后计数
    let mut fragment_count = 0usize;
    let mut word_count = 0u64;
    if let Ok(entries) = std::fs::read_dir(paths.fragments_dir()) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            fragment_count += 1;
            let body = story_exporter::strip_anchor_comments(crate::fragment::strip_leading_frontmatter(
                &content,
            ));
            word_count += story_ir::count_words(&body) as u64;
        }
    }

    let stats = crate::stats::summary(dir, 0);

    Ok(ProjectPeek {
        root: dir.to_string_lossy().into_owned(),
        name: project.config.project.name.clone(),
        target_words,
        chapter_count,
        scene_count,
        fragment_count,
        word_count,
        today: stats.today,
        streak_days: stats.streak_days,
        daily_goal: stats.daily_goal,
        total: stats.total,
    })
}

/// 批量窥视多个项目（首页最近书案）
///
/// 单个项目失败（目录被移走/损坏）不拖垮整批：对应条目 peek 为 None。
pub fn peek_projects(roots: &[String]) -> Vec<PeekEntry> {
    roots
        .iter()
        .map(|r| PeekEntry {
            root: r.clone(),
            peek: peek(Path::new(r)).ok(),
        })
        .collect()
}

// ============================================================================
// 书籍信息（outline.toml [story] 段）— 手动项目与建书项目共用的创作约束
// ============================================================================

/// 读取书籍信息（题材 / 主线 / 受众 / 视角 / 文风 / 目标字数等）
///
/// outline.toml 不存在（手动新建的空项目）视为未设置，返回空值；
/// 文件存在但解析失败则报错（用户需要知道文件改坏了）。
pub fn get_book_meta(dir: &Path) -> Result<StoryMeta, StudioError> {
    let paths = open_project(dir)?.paths();
    match std::fs::read_to_string(paths.outline_toml()) {
        Ok(content) => OutlineFile::from_toml_str(&content)
            .map(|o| o.story)
            .map_err(|e| StudioError::Config(format!("outline.toml 解析失败: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoryMeta::default()),
        Err(e) => Err(StudioError::Io(format!("读取 outline.toml 失败: {e}"))),
    }
}

/// 保存书籍信息
///
/// 只替换表单管辖的字段；`title / setting / time_span` 保留旧值
/// （分别由建书流程与世界观维护，不在书籍信息表单里）。
/// outline.toml 的其余段（volumes / arcs / storylines / conflicts）原样保留；
/// 文件不存在时创建——手动项目补填书籍信息后即可使用续排大纲。
pub fn save_book_meta(dir: &Path, meta: &StoryMeta) -> Result<(), StudioError> {
    let paths = open_project(dir)?.paths();

    let mut file = match std::fs::read_to_string(paths.outline_toml()) {
        Ok(content) => OutlineFile::from_toml_str(&content)
            .map_err(|e| StudioError::Config(format!("outline.toml 解析失败: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => OutlineFile::default(),
        Err(e) => return Err(StudioError::Io(format!("读取 outline.toml 失败: {e}"))),
    };

    let mut next = meta.clone();
    next.title = file.story.title.clone();
    next.setting = file.story.setting.clone();
    next.time_span = file.story.time_span.clone();
    // 0 视为未设置；单章字数与建书归一一致（1000..=10000）
    if next.target_words == Some(0) {
        next.target_words = None;
    }
    next.chapter_words = next.chapter_words.map(|w| w.clamp(1000, 10000));

    file.story = next;
    let content = file.to_toml_string().map_err(|e| {
        StudioError::Config(format!("outline.toml 序列化失败: {e}"))
    })?;
    crate::journal::write_verified(dir, "save_book_meta", &paths.outline_toml(), &content)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_counts_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ce-shi-xiao-shuo");
        let paths = init_project(&root, "测试小说").unwrap();

        // 1 章 2 场景
        std::fs::write(
            paths.structure_toml(),
            "[book]\nid = \"b_0001\"\ntitle = \"测试小说\"\nchapters = [\"ch_0001\"]\n\
             \n[chapter.ch_0001]\ntitle = \"第一章\"\nscenes = [\"sc_0001\", \"sc_0002\"]\n\
             \n[scene.sc_0001]\ntitle = \"场景一\"\nfragments = [\"f_0001\"]\n\
             \n[scene.sc_0002]\ntitle = \"场景二\"\nfragments = [\"f_0002\"]\n",
        )
        .unwrap();

        // 片段一：frontmatter + 锚点注释都不应计入字数
        std::fs::write(
            paths.fragments_dir().join("f_0001.md"),
            "---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n他没有应声。<!-- @c_0001 -->\n",
        )
        .unwrap();
        // 片段二：纯正文（CJK 按字计，连续 ASCII 词按词计）
        std::fs::write(
            paths.fragments_dir().join("f_0002.md"),
            "窗外落雨。hello world 你好\n",
        )
        .unwrap();

        let p = peek(&root).unwrap();
        assert_eq!(p.name, "测试小说");
        assert_eq!(p.chapter_count, 1);
        assert_eq!(p.scene_count, 2);
        assert_eq!(p.fragment_count, 2);
        // 5（他没有应声）+ 4（窗外落雨）+ 2（你好）+ 2（hello world）
        assert_eq!(p.word_count, 13);
        assert_eq!(p.target_words, None);
        assert_eq!(p.today.total(), 0);
        assert_eq!(p.total.total(), 0);
    }

    #[test]
    fn peek_projects_isolates_failures() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("hao-shu");
        init_project(&root, "好书").unwrap();

        let entries = peek_projects(&[
            root.to_string_lossy().into_owned(),
            "/nonexistent/path".to_string(),
        ]);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].root, root.to_string_lossy());
        assert!(entries[0].peek.is_some());
        assert_eq!(entries[0].peek.as_ref().unwrap().name, "好书");
        assert!(entries[1].peek.is_none());
    }

    #[test]
    fn book_meta_missing_file_is_empty_then_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shou-xie-shu");
        init_project(&root, "手写书").unwrap();

        // 手动项目无 outline.toml → 空值
        let meta = get_book_meta(&root).unwrap();
        assert!(meta.genre.is_none() && meta.target_words.is_none());

        // 保存后读回
        let next = StoryMeta {
            genre: Some("武侠悬疑".into()),
            premise: Some("剑客寻旧案".into()),
            audience: Some("男频".into()),
            pov: Some("第三人称限知".into()),
            style: Some("热血".into()),
            target_words: Some(1_200_000),
            chapter_words: Some(3_000),
            ..Default::default()
        };
        save_book_meta(&root, &next).unwrap();
        let got = get_book_meta(&root).unwrap();
        assert_eq!(got.genre.as_deref(), Some("武侠悬疑"));
        assert_eq!(got.premise.as_deref(), Some("剑客寻旧案"));
        assert_eq!(got.target_words, Some(1_200_000));
        assert_eq!(got.chapter_words, Some(3_000));

        // peek 目标字数从此可用
        assert_eq!(peek(&root).unwrap().target_words, Some(1_200_000));
    }

    #[test]
    fn book_meta_save_preserves_untouched_fields_and_sections() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bao-liu");
        let paths = init_project(&root, "保留").unwrap();
        // 建书流程写过的 outline.toml：title/setting + volumes
        std::fs::write(
            paths.outline_toml(),
            "[story]\ntitle = \"拾灯录\"\nsetting = \"云隐洲\"\n\n[[volumes]]\nname = \"第一卷\"\n",
        )
        .unwrap();

        let next = StoryMeta {
            genre: Some("玄幻".into()),
            target_words: Some(0), // 0 → 未设置
            chapter_words: Some(500), // clamp 到 1000
            ..Default::default()
        };
        save_book_meta(&root, &next).unwrap();

        let file = OutlineFile::from_toml_str(&std::fs::read_to_string(paths.outline_toml()).unwrap())
            .unwrap();
        // 表单字段生效
        assert_eq!(file.story.genre.as_deref(), Some("玄幻"));
        assert_eq!(file.story.target_words, None);
        assert_eq!(file.story.chapter_words, Some(1000));
        // 不管辖的字段与其它段保留
        assert_eq!(file.story.title.as_deref(), Some("拾灯录"));
        assert_eq!(file.story.setting.as_deref(), Some("云隐洲"));
        assert_eq!(file.volumes.len(), 1);
        assert_eq!(file.volumes[0].name, "第一卷");
    }
}
