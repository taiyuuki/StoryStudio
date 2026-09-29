//! 目录名 slug 化
//!
//! 将书名（可含中文）转为 git 友好的 ASCII 目录名。
//!
//! 规则：
//! - 中文 → 拼音（无声调），如「雨夜行」→ `yuyexing`
//! - ASCII 字母/数字 → 保留并小写
//! - 其余字符（空格、标点、符号）→ 折叠为单个 `-`
//! - 去除首尾 `-`，折叠连续 `-`
//! - 结果为空时回退 `untitled`

use pinyin::ToPinyin;

/// 将任意书名转为 git 友好的 ASCII slug。
///
/// # 示例
/// ```
/// use story_project_format::slugify;
/// assert_eq!(slugify("雨夜行"), "yuyexing");
/// assert_eq!(slugify("My Story 2!"), "my-story-2");
/// assert_eq!(slugify("剑来Vol.2"), "jianlai-vol-2");
/// assert_eq!(slugify("《！！！》"), "untitled");
/// ```
pub fn slugify(name: &str) -> String {
    use ChunkKind::*;

    let mut out = String::with_capacity(name.len());

    // 上一个写入的字符块类型，用于在拼音↔ASCII 边界插入分隔符
    // （避免「剑来Vol」→「jianlaivol」这种歧义连写）
    let mut prev_kind: Option<ChunkKind> = None;

    for ch in name.chars() {
        if let Some(py) = ch.to_pinyin() {
            if prev_kind == Some(Ascii) {
                out.push('-');
            }
            out.push_str(py.plain());
            prev_kind = Some(Pinyin);
        } else if ch.is_ascii_alphanumeric() {
            if prev_kind == Some(Pinyin) {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
            prev_kind = Some(Ascii);
        } else {
            // 非字母数字、非中文 → 分隔符；折叠连续
            if prev_kind.is_some() {
                out.push('-');
                prev_kind = None;
            }
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ChunkKind {
    Pinyin,
    Ascii,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_chinese() {
        assert_eq!(slugify("雨夜行"), "yuyexing");
        assert_eq!(slugify("三体"), "santi");
        assert_eq!(slugify("剑来"), "jianlai");
    }

    #[test]
    fn mixed_chinese_ascii() {
        assert_eq!(slugify("剑来Vol.2"), "jianlai-vol-2");
        assert_eq!(slugify("三体 III"), "santi-iii");
    }

    #[test]
    fn pure_ascii() {
        assert_eq!(slugify("My Story 2!"), "my-story-2");
        assert_eq!(slugify("HelloWorld"), "helloworld");
        assert_eq!(slugify(" Already-Clean "), "already-clean");
    }

    #[test]
    fn collapses_separators() {
        // 连续符号折叠为单个 `-`
        assert_eq!(slugify("a!!!b"), "a-b");
        assert_eq!(slugify("雨 夜  行"), "yu-ye-xing");
    }

    #[test]
    fn all_symbols_fallback() {
        assert_eq!(slugify("《！！！》"), "untitled");
        assert_eq!(slugify("   "), "untitled");
        assert_eq!(slugify(""), "untitled");
    }

    #[test]
    fn no_leading_trailing_dash() {
        assert_eq!(slugify("...雨夜行..."), "yuyexing");
        assert_eq!(slugify("---abc"), "abc");
        assert_eq!(slugify("abc---"), "abc");
    }

    #[test]
    fn idempotent() {
        let s = slugify("雨夜行 Vol.2");
        assert_eq!(slugify(&s), s);
    }
}
