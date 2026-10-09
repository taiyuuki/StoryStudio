//! 项目级提示词自定义 — 项目 `prompts/{id}.md` 覆盖编译期内置提示词。
//!
//! 与 `guides/` 覆盖内置创作指导（`tools/guide.rs`）同一模式：
//! 覆盖文件非空则生效，缺失或为空回退内置；覆盖文件进 Git，是项目资产。
//!
//! 每次调用即时读盘（单个小文件，开销可忽略），换取修改即时生效——
//! 不做缓存，妙笔等长会话在下一轮 / 下一次调用自然拿到新提示词。
//!
//! **只开放输出自由文本的提示词**。输出被程序结构化解析的提示词
//! （AI 味检测 / 档案核对 / 断言抽取的 JSON·YAML 报告、建书草稿、
//! 节拍数组的格式契约）不纳入自定义——改坏输出格式会导致解析失败。
//! 正文类提示词（write_fragment / write_chapter）的 frontmatter 锚点
//! 是可选标注、解析有容错，改坏只丢锚点不报错，因此开放。

use std::path::Path;

use story_project_format::ProjectPaths;

use crate::error::StudioError;

/// 一条可自定义提示词的登记信息
#[derive(Debug)]
pub struct PromptSpec {
    /// id（= 覆盖文件名 stem，如 `agent_project`）
    pub id: &'static str,
    /// 中文显示名
    pub label: &'static str,
    /// 面板分组
    pub group: &'static str,
    /// 用途说明
    pub description: &'static str,
    /// 必需占位符（保存时校验齐全，`render()` 的 debug 断言在 release 不生效）
    pub placeholders: &'static [&'static str],
    /// 内置文本（`src/prompts/` 同名文件）
    pub builtin: &'static str,
}

// —— 妙笔对话 ——

/// 妙笔（项目内 Agent）的 system prompt
pub const AGENT_PROJECT: PromptSpec = PromptSpec {
    id: "agent_project",
    label: "妙笔系统提示",
    group: "妙笔对话",
    description: "项目内 AI 助手「妙笔」的角色设定、文风准则与工具协作铁律。修改后下一轮对话生效。",
    placeholders: &[],
    builtin: crate::prompts::AGENT_PROJECT,
};

// —— 续写与补全 ——

/// 场景续写（AI 续写按钮）的 system prompt
pub const WRITE_FRAGMENT: PromptSpec = PromptSpec {
    id: "write_fragment",
    label: "场景续写",
    group: "续写与补全",
    description: "「AI 续写」生成下一片段时的指令。POV 与场景标题由程序在末尾追加；请保留「正文前用 frontmatter 标注 anchors」的要求，删掉会丢失出场锚点。",
    placeholders: &[],
    builtin: story_ai_runtime::prompts::WRITE_FRAGMENT,
};

/// 行内幽灵补全的 system prompt
pub const COMPLETE_FRAGMENT: PromptSpec = PromptSpec {
    id: "complete_fragment",
    label: "行内补全",
    group: "续写与补全",
    description: "编辑器内幽灵补全（Tab 采纳）的指令，约束补全长度与「不重复已有文字」等行为。",
    placeholders: &[],
    builtin: crate::prompts::COMPLETE_FRAGMENT,
};

// —— 逐章成书 ——

/// 整章正文生成的 system prompt
pub const WRITE_CHAPTER: PromptSpec = PromptSpec {
    id: "write_chapter",
    label: "整章正文",
    group: "逐章成书",
    description: "「逐章成书」按节拍写整章正文的指令；节拍、目标字数等材料由程序拼入用户消息。请保留 frontmatter 锚点标注与「只输出正文」的约束。",
    placeholders: &[],
    builtin: story_ai_runtime::prompts::WRITE_CHAPTER,
};

// —— 回顾摘要 ——

/// 场景摘要生成（get_story_recap 兜底）的 system prompt
pub const SCENE_SUMMARY: PromptSpec = PromptSpec {
    id: "scene_summary",
    label: "场景摘要",
    group: "回顾摘要",
    description: "妙笔回顾全书时为未摘要场景生成梗概的指令。",
    placeholders: &[],
    builtin: crate::prompts::SCENE_SUMMARY,
};

/// 全部可自定义提示词（面板按此分组展示）
pub const PROMPT_SPECS: &[&PromptSpec] = &[
    &AGENT_PROJECT,
    &WRITE_FRAGMENT,
    &COMPLETE_FRAGMENT,
    &WRITE_CHAPTER,
    &SCENE_SUMMARY,
];

/// 面板条目：登记信息 + 当前覆盖内容
#[derive(Debug, Clone)]
pub struct PromptEntry {
    pub id: String,
    pub label: String,
    pub group: String,
    pub description: String,
    pub placeholders: Vec<String>,
    pub builtin: String,
    /// 项目覆盖内容（`prompts/{id}.md` 非空时存在）
    pub custom: Option<String>,
}

/// 按 id 查找登记信息
pub fn spec_by_id(id: &str) -> Option<&'static PromptSpec> {
    PROMPT_SPECS.iter().copied().find(|s| s.id == id)
}

/// 读取项目覆盖（非空才生效）
fn read_override(root: &Path, id: &str) -> Option<String> {
    let path = ProjectPaths::new(root).prompt_file(id);
    let text = std::fs::read_to_string(path).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 解析提示词生效文本：项目覆盖优先，否则内置
pub fn resolve(spec: &PromptSpec, root: Option<&Path>) -> String {
    root.and_then(|r| read_override(r, spec.id))
        .unwrap_or_else(|| spec.builtin.to_string())
}

/// 仅当项目覆盖存在时返回（供 Task 注入，避免无谓 clone）
pub fn resolve_opt(spec: &PromptSpec, root: Option<&Path>) -> Option<String> {
    root.and_then(|r| read_override(r, spec.id))
}

/// 列出全部可自定义提示词及覆盖状态
pub fn list(root: &Path) -> Vec<PromptEntry> {
    PROMPT_SPECS
        .iter()
        .map(|spec| PromptEntry {
            id: spec.id.to_string(),
            label: spec.label.to_string(),
            group: spec.group.to_string(),
            description: spec.description.to_string(),
            placeholders: spec.placeholders.iter().map(|s| s.to_string()).collect(),
            builtin: spec.builtin.to_string(),
            custom: read_override(root, spec.id),
        })
        .collect()
}

/// 保存项目覆盖
///
/// 校验：id 在册、内容非空白（恢复默认请用 [`reset`]）、必需占位符齐全
/// （`render()` 的 debug 断言在 release 不生效，缺失的占位符会被原样发给模型）。
pub fn save(root: &Path, id: &str, content: &str) -> Result<(), StudioError> {
    let spec = spec_by_id(id)
        .ok_or_else(|| StudioError::Prompt(format!("未知提示词：{id}")))?;
    if content.trim().is_empty() {
        return Err(StudioError::Prompt(
            "提示词内容不能为空；恢复内置请用「重置」".into(),
        ));
    }
    for p in spec.placeholders {
        if !content.contains(&format!("{{{{{p}}}}}")) {
            return Err(StudioError::Prompt(format!(
                "缺少必需占位符 {{{{{p}}}}}——程序运行时会向其中填充内容，删除会导致生成失败"
            )));
        }
    }
    let target = ProjectPaths::new(root).prompt_file(id);
    crate::journal::write_verified(root, "save_prompt", &target, content)
}

/// 删除项目覆盖，恢复内置提示词；返回覆盖文件是否原本存在
pub fn reset(root: &Path, id: &str) -> Result<bool, StudioError> {
    if spec_by_id(id).is_none() {
        return Err(StudioError::Prompt(format!("未知提示词：{id}")));
    }
    let path = ProjectPaths::new(root).prompt_file(id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(StudioError::Io(format!("删除提示词覆盖失败: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_falls_back_to_builtin_without_root_or_file() {
        assert_eq!(resolve(&AGENT_PROJECT, None), AGENT_PROJECT.builtin);

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve(&SCENE_SUMMARY, Some(dir.path())),
            SCENE_SUMMARY.builtin
        );
        assert_eq!(resolve_opt(&SCENE_SUMMARY, Some(dir.path())), None);
    }

    #[test]
    fn override_file_wins_and_empty_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let prompts_dir = dir.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();

        std::fs::write(prompts_dir.join("scene_summary.md"), "自定义摘要指令").unwrap();
        assert_eq!(
            resolve(&SCENE_SUMMARY, Some(dir.path())),
            "自定义摘要指令"
        );
        assert_eq!(
            resolve_opt(&SCENE_SUMMARY, Some(dir.path())),
            Some("自定义摘要指令".to_string())
        );

        std::fs::write(prompts_dir.join("scene_summary.md"), "   \n\t").unwrap();
        assert_eq!(
            resolve(&SCENE_SUMMARY, Some(dir.path())),
            SCENE_SUMMARY.builtin
        );
    }

    #[test]
    fn save_validates_id_and_content() {
        let dir = tempfile::tempdir().unwrap();

        assert!(matches!(
            save(dir.path(), "no_such_prompt", "x"),
            Err(StudioError::Prompt(_))
        ));
        // 格式契约型提示词（如 ai_check）不在册，不可自定义
        assert!(matches!(
            save(dir.path(), "ai_check", "自定义检测"),
            Err(StudioError::Prompt(_))
        ));
        assert!(matches!(
            save(dir.path(), "scene_summary", "  "),
            Err(StudioError::Prompt(_))
        ));

        save(dir.path(), "scene_summary", "自定义摘要指令").unwrap();
        assert_eq!(
            resolve(&SCENE_SUMMARY, Some(dir.path())),
            "自定义摘要指令"
        );
    }

    #[test]
    fn reset_removes_override_and_tolerates_missing() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "write_chapter", "自定义整章").unwrap();
        assert!(reset(dir.path(), "write_chapter").unwrap());
        assert_eq!(resolve(&WRITE_CHAPTER, Some(dir.path())), WRITE_CHAPTER.builtin);
        assert!(!reset(dir.path(), "write_chapter").unwrap());
    }

    #[test]
    fn list_reports_custom_state() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "agent_project", "我的妙笔").unwrap();

        let entries = list(dir.path());
        assert_eq!(entries.len(), PROMPT_SPECS.len());
        let agent = entries.iter().find(|e| e.id == "agent_project").unwrap();
        assert_eq!(agent.custom.as_deref(), Some("我的妙笔"));
        assert_eq!(agent.builtin, AGENT_PROJECT.builtin);
        let summary = entries.iter().find(|e| e.id == "scene_summary").unwrap();
        assert!(summary.custom.is_none());
    }

    /// 开放清单只含输出自由文本的提示词；
    /// 输出被程序结构化解析的一律不开放（改坏格式会导致解析失败）
    #[test]
    fn open_list_excludes_structured_output_prompts() {
        let ids: Vec<&str> = PROMPT_SPECS.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec![
                "agent_project",
                "write_fragment",
                "complete_fragment",
                "write_chapter",
                "scene_summary",
            ]
        );
        for excluded in [
            "ai_check",
            "profile_check",
            "extract_assertions",
            "book_world",
            "book_outline",
            "chapter_beats_system",
            "chapter_beats_task",
            "agent_global",
            "book_concepts",
        ] {
            assert!(spec_by_id(excluded).is_none(), "{excluded} 不应开放自定义");
        }
    }
}
