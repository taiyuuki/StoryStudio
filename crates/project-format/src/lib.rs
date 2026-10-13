//! Project Format — 文件读写 (RFC-0003)
//!
//! 定义 Story Studio Project 的文件格式与目录结构。
//! 每种信息只有一个 Source of Truth（§3）。

pub mod anchoring_file;
pub mod atomic;
pub mod chapters_file;
pub mod compiler_toml;
pub mod entity_file;
pub mod fragment_file;
pub mod init;
pub mod knowledge_file;
pub mod outline_file;
pub mod paths;
pub mod project_toml;
pub mod relation_file;
pub mod slug;
pub mod structure_toml;
pub mod threads_file;
pub mod volumes;
pub mod worldview_file;

pub use anchoring_file::{AnchoringFile, AnchoringFileError, UnresolvedRefDef};
pub use chapters_file::{
    is_valid_status, ChapterOutlineDef, ChaptersFile, ChaptersFileError, VolumeGroupDef,
    STATUS_COMPLETED, STATUS_PLANNED, STATUS_WRITING,
};
pub use compiler_toml::CompilerConfigFile;
pub use entity_file::{EntityFile, EntityFileError};
pub use fragment_file::{FragmentFile, FragmentFileError, FragmentFrontmatter};
pub use init::{init_project, InitError};
pub use knowledge_file::{AssertionsFile, EvidenceFile, KnowledgeFileError};
pub use outline_file::{ConflictDef, OutlineFile, OutlineFileError, StoryMeta, VolumeDef};
pub use paths::ProjectPaths;
pub use project_toml::{AiConfig, KnowledgeConfig, ProjectConfig};
pub use relation_file::{
    RelationDef, RelationFileError, RelationsFile, StoryTimeDef as RelationTimeDef, ValidityDef,
};
pub use slug::slugify;
pub use structure_toml::{BookDef, ChapterDef, SceneDef, StructureFile};
pub use threads_file::{ThreadsFile, ThreadsFileError};
pub use volumes::{
    load_chapters_unified, remap_due_volumes, save_outline_volumes, unified_groups, unify_volumes,
    VolumeRemap, VolumeSyncError,
};
pub use worldview_file::{
    ConceptDef, ConceptsFile, SystemDef, SystemsFile, TimelineFile, WorldviewFileError,
    WorldviewSectionFile,
};

/// project-format crate 对应的 RFC 版本
pub const RFC_VERSION: &str = "0.2";
