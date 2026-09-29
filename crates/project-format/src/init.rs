//! 项目初始化 (RFC-0003 §4 目录结构)
//!
//! 创建新项目的目录骨架与默认文件。

use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::paths::ProjectPaths;
use crate::project_toml::ProjectConfig;
use crate::structure_toml::StructureFile;

#[derive(Debug, Error)]
pub enum InitError {
    #[error("目录已存在且非空: {0}")]
    DirNotEmpty(String),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML 序列化失败: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

/// 初始化新项目
///
/// 创建目录结构 + 默认 project.toml / structure.toml / .gitignore。
/// 如果目标目录已存在且非空，返回错误。
pub fn init_project(root: &Path, project_name: &str) -> Result<ProjectPaths, InitError> {
    // 检查目录
    if root.exists() && root.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(InitError::DirNotEmpty(root.display().to_string()));
    }

    let paths = ProjectPaths::new(root);

    // 创建目录
    let dirs = [
        paths.fragments_dir(),
        paths.characters_dir(),
        paths.locations_dir(),
        paths.items_dir(),
        paths.organizations_dir(),
        paths.knowledge_dir(),
        paths.config_dir(),
    ];
    for dir in &dirs {
        fs::create_dir_all(dir)?;
    }

    // project.toml
    let config = ProjectConfig::new(project_name);
    fs::write(paths.project_toml(), config.to_toml_string()?)?;

    // structure.toml（空结构，只有 Book）
    let structure = StructureFile::new(project_name);
    fs::write(paths.structure_toml(), structure.to_toml_string()?)?;

    // chapters.toml（空章节脉络，附引导注释）
    let chapters_header = "# 章节脉络 (Chapter Outline)\n\
        #\n\
        # 设计层文件：把全局大纲展开成逐章的叙事脉络。\n\
        # 通常由 AI 调用 design_chapters 工具生成，也可手动编辑。\n\
        # 不参与编译检查。\n\
        # 用 get_chapter_outline / update_chapter_outline / delete_chapter_outline 工具维护。\n\n";
    fs::write(paths.chapters_toml(), chapters_header)?;

    // config/compiler.toml（默认规则）
    let compiler_config = crate::compiler_toml::CompilerConfigFile::default();
    fs::write(
        paths.config_dir().join("compiler.toml"),
        compiler_config.to_toml_string()?,
    )?;

    // .gitignore
    let gitignore = "# Story Studio Project\n\n\
        # 工具私有目录（派生缓存）\n\
        /.novelstudio/\n\n\
        # 临时文件\n\
        *.tmp\n*.bak\n";
    fs::write(paths.gitignore(), gitignore)?;

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_structure() {
        let temp = tempfile_dir();
        let project_root = temp.join("test-novel");

        let paths = init_project(&project_root, "测试小说").unwrap();

        assert!(paths.project_toml().exists());
        assert!(paths.structure_toml().exists());
        assert!(paths.chapters_toml().exists());
        assert!(paths.gitignore().exists());
        assert!(paths.fragments_dir().is_dir());
        assert!(paths.characters_dir().is_dir());
        assert!(paths.config_dir().join("compiler.toml").exists());

        // 验证 project.toml 内容
        let toml_str = fs::read_to_string(paths.project_toml()).unwrap();
        let config = ProjectConfig::from_toml_str(&toml_str).unwrap();
        assert_eq!(config.project.name, "测试小说");
    }

    #[test]
    fn init_rejects_non_empty() {
        let temp = tempfile_dir();
        let project_root = temp.join("existing");
        fs::create_dir_all(&project_root).unwrap();
        fs::write(project_root.join("some_file.txt"), "hello").unwrap();

        let result = init_project(&project_root, "test");
        assert!(matches!(result, Err(InitError::DirNotEmpty(_))));
    }

    /// 创建临时目录（测试辅助）
    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "story-studio-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    use std::path::PathBuf;
}
