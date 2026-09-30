//! AI 提示词模板 — `src/prompts/` 一提示词一文件，编译期 `include_str!` 嵌入。
//!
//! 与 `tools/guides/`（创作指导）同一模式：文件即文档，改提示词不动代码。
//! 占位符用双花括号（`{{title}}`），由 [`render`] 填充；
//! 模板正文里的 JSON/YAML 示例可安全使用单花括号。

/// 项目内 Agent（妙笔）的 system prompt
pub(crate) const AGENT_PROJECT: &str = include_str!("prompts/agent_project.md");

/// 全局 Agent（未打开项目时的创意顾问）的 system prompt
pub(crate) const AGENT_GLOBAL: &str = include_str!("prompts/agent_global.md");

/// AI 味检测（ai_check_fragment）的 system prompt
pub(crate) const AI_CHECK: &str = include_str!("prompts/ai_check.md");

/// 档案与正文一致性核对（profile_check）的 system prompt
pub(crate) const PROFILE_CHECK: &str = include_str!("prompts/profile_check.md");

/// 开书第 1 步：概念候选生成。
/// 占位符：`{{idea}}` / `{{genre_line}}` / `{{extras}}` / `{{guide}}` / `{{candidates}}`
pub(crate) const BOOK_CONCEPTS: &str = include_str!("prompts/book_concepts.md");

/// 开书第 3 步：世界观与人物设定草稿。
/// 占位符：`{{title}}` / `{{genre_line}}` / `{{extras}}` / `{{premise}}` / `{{theme}}` / `{{n_chars}}`
pub(crate) const BOOK_WORLD: &str = include_str!("prompts/book_world.md");

/// 开书第 4 步 / 写作中续排：章节脉络批次。
/// 占位符：`{{premise}}` / `{{head}}` / `{{existing}}` / `{{characters}}` / `{{threads}}` / `{{next_index}}` / `{{count}}`
pub(crate) const BOOK_OUTLINE: &str = include_str!("prompts/book_outline.md");

/// 场景摘要生成（get_story_recap 兜底）的 system prompt
pub(crate) const SCENE_SUMMARY: &str = include_str!("prompts/scene_summary.md");

/// 行内补全（编辑器幽灵补全）的 system prompt
pub(crate) const COMPLETE_FRAGMENT: &str = include_str!("prompts/complete_fragment.md");

/// 逐章成书：节拍设计的 system prompt
pub(crate) const CHAPTER_BEATS_SYSTEM: &str = include_str!("prompts/chapter_beats_system.md");

/// 逐章成书：节拍设计的任务指令（拼在章节上下文之后）
pub(crate) const CHAPTER_BEATS_TASK: &str = include_str!("prompts/chapter_beats_task.md");

/// 用 `{{name}}` 占位符填充模板
///
/// debug 构建下断言没有遗留占位符（防止模板与调用点失配悄悄把 `{{…}}` 发给模型）。
pub(crate) fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    debug_assert!(!out.contains("{{"), "提示词模板存在未填充的占位符:\n{out}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_fills_placeholders_and_keeps_single_braces() {
        let out = render(
            "输出 {name, age}；书名《{{title}}》共 {{n}} 章",
            &[("title", "拾灯录"), ("n", "3")],
        );
        assert_eq!(out, "输出 {name, age}；书名《拾灯录》共 3 章");
    }

    /// 所有模板都应非空且不以多余空白开头
    #[test]
    fn all_templates_nonempty() {
        for (name, tpl) in [
            ("AGENT_PROJECT", AGENT_PROJECT),
            ("AGENT_GLOBAL", AGENT_GLOBAL),
            ("AI_CHECK", AI_CHECK),
            ("PROFILE_CHECK", PROFILE_CHECK),
            ("BOOK_CONCEPTS", BOOK_CONCEPTS),
            ("BOOK_WORLD", BOOK_WORLD),
            ("BOOK_OUTLINE", BOOK_OUTLINE),
            ("SCENE_SUMMARY", SCENE_SUMMARY),
            ("COMPLETE_FRAGMENT", COMPLETE_FRAGMENT),
            ("CHAPTER_BEATS_SYSTEM", CHAPTER_BEATS_SYSTEM),
            ("CHAPTER_BEATS_TASK", CHAPTER_BEATS_TASK),
        ] {
            assert!(!tpl.trim().is_empty(), "{name} 模板为空");
        }
    }
}
