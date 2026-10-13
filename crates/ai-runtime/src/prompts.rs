//! AI 提示词模板 — `src/prompts/` 一提示词一文件，编译期 `include_str!` 嵌入。
//!
//! 占位符用双花括号（`{{name}}`），由 [`render`] 填充；
//! 模板正文里的 JSON/YAML 示例可安全使用单花括号。

/// 写片段（自然语言续写）的 system prompt 基底
pub const WRITE_FRAGMENT: &str = include_str!("prompts/write_fragment.md");

/// 整章正文生成（逐章流水线）的 system prompt
pub const WRITE_CHAPTER: &str = include_str!("prompts/write_chapter.md");

/// 知识断言抽取的 system prompt（`{{predicates}}` / `{{known_characters}}`）
pub const EXTRACT_ASSERTIONS: &str = include_str!("prompts/extract_assertions.md");

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
    fn render_fills_placeholders() {
        let out = render(
            "书名《{{title}}》，共 {{n}} 章",
            &[("title", "拾灯录"), ("n", "3")],
        );
        assert_eq!(out, "书名《拾灯录》，共 3 章");
    }

    #[test]
    fn single_braces_in_examples_are_untouched() {
        // 模板里的 JSON 示例用单花括号，不与占位符冲突
        let out = render("输出 {name, age} 与 {{name}}", &[("name", "林风")]);
        assert_eq!(out, "输出 {name, age} 与 林风");
    }
}
