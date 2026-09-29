//! 文件路径约定 (RFC-0003 §7)
//!
//! StableId → 文件路径的确定性映射，无需额外索引。

use std::path::{Path, PathBuf};

use story_ir::IdPrefix;

/// 项目路径助手——封装目录结构约定
#[derive(Debug, Clone)]
pub struct ProjectPaths {
    root: PathBuf,
}

impl ProjectPaths {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    // —— 根级文件 ——

    pub fn project_toml(&self) -> PathBuf {
        self.root.join("project.toml")
    }

    pub fn structure_toml(&self) -> PathBuf {
        self.root.join("structure.toml")
    }

    pub fn gitignore(&self) -> PathBuf {
        self.root.join(".gitignore")
    }

    pub fn threads_toml(&self) -> PathBuf {
        self.root.join("threads.toml")
    }

    pub fn outline_toml(&self) -> PathBuf {
        self.root.join("outline.toml")
    }

    pub fn chapters_toml(&self) -> PathBuf {
        self.root.join("chapters.toml")
    }

    pub fn worldview_toml(&self) -> PathBuf {
        self.root.join("worldview.toml")
    }

    pub fn relations_toml(&self) -> PathBuf {
        self.root.join("relations.toml")
    }

    pub fn assertions_toml(&self) -> PathBuf {
        self.knowledge_dir().join("assertions.toml")
    }

    /// 创作指导目录（项目级覆盖内置指导用）
    ///
    /// 项目可在 `guides/` 放置同名 `.md` 文件覆盖内置的类型创作指导，
    /// 为系列定制专属写作要求。
    pub fn guides_dir(&self) -> PathBuf {
        self.root.join("guides")
    }

    /// 指定名称的创作指导文件（`guides/{name}.md`）
    pub fn guide_file(&self, name: &str) -> PathBuf {
        self.guides_dir().join(format!("{name}.md"))
    }

    // —— 目录 ——

    pub fn fragments_dir(&self) -> PathBuf {
        self.root.join("fragments")
    }

    pub fn entities_dir(&self) -> PathBuf {
        self.root.join("entities")
    }

    pub fn characters_dir(&self) -> PathBuf {
        self.entities_dir().join("characters")
    }

    pub fn locations_dir(&self) -> PathBuf {
        self.entities_dir().join("locations")
    }

    pub fn items_dir(&self) -> PathBuf {
        self.entities_dir().join("items")
    }

    pub fn organizations_dir(&self) -> PathBuf {
        self.entities_dir().join("organizations")
    }

    pub fn knowledge_dir(&self) -> PathBuf {
        self.root.join("knowledge")
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(".novelstudio").join("cache")
    }

    // —— StableId → 文件路径 ——

    /// 根据 StableId 的前缀推导文件路径
    pub fn file_for_id(&self, id_str: &str) -> Option<PathBuf> {
        let prefix = story_ir::StableId::new(id_str).ok()?.prefix();
        match prefix {
            IdPrefix::Fragment => Some(self.fragments_dir().join(format!("{id_str}.md"))),
            IdPrefix::Character => Some(self.characters_dir().join(format!("{id_str}.toml"))),
            IdPrefix::Location => Some(self.locations_dir().join(format!("{id_str}.toml"))),
            IdPrefix::Item => Some(self.items_dir().join(format!("{id_str}.toml"))),
            IdPrefix::Organization => Some(self.organizations_dir().join(format!("{id_str}.toml"))),
            _ => None,
        }
    }

    /// 从文件路径提取 StableId（文件名不含扩展名）
    pub fn id_from_path(path: &Path) -> Option<String> {
        path.file_stem()?.to_str().map(|s| s.to_string())
    }

    /// 根目录引用
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_for_fragment() {
        let paths = ProjectPaths::new("/tmp/novel");
        let file = paths.file_for_id("f_0001").unwrap();
        assert_eq!(file, PathBuf::from("/tmp/novel/fragments/f_0001.md"));
    }

    #[test]
    fn paths_for_character() {
        let paths = ProjectPaths::new("/tmp/novel");
        let file = paths.file_for_id("c_8A91D4").unwrap();
        assert_eq!(
            file,
            PathBuf::from("/tmp/novel/entities/characters/c_8A91D4.toml")
        );
    }

    #[test]
    fn paths_for_item() {
        let paths = ProjectPaths::new("/tmp/novel");
        let file = paths.file_for_id("i_A7E902").unwrap();
        assert_eq!(
            file,
            PathBuf::from("/tmp/novel/entities/items/i_A7E902.toml")
        );
    }

    #[test]
    fn id_from_filename() {
        let path = PathBuf::from("/tmp/novel/fragments/f_0001.md");
        assert_eq!(
            ProjectPaths::id_from_path(&path),
            Some("f_0001".to_string())
        );
    }
}
