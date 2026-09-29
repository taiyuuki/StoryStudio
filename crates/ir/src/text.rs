//! 文本统计 — 全工作区统一的字数口径
//!
//! CJK 字符每字计 1，连续的 ASCII 字母数字串计 1（与网文平台口径接近），
//! 标点与空白不计。exporter（项目统计）、studio-lib（工具回执/流水线质检）、
//! 桌面端（界面字数）共用本实现，避免"同一片段三处字数不同"。

/// 是否为 CJK 表意字符
pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF   // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 基本
        | 0xF900..=0xFAFF // CJK 兼容
        | 0x20000..=0x2A6DF
    )
}

/// 字数统计：CJK 每字 1，连续 ASCII 字母数字串计 1
pub fn count_words(text: &str) -> u32 {
    let mut total = 0u32;
    let mut in_ascii_word = false;
    for ch in text.chars() {
        if is_cjk(ch) {
            total += 1;
            in_ascii_word = false;
        } else if ch.is_ascii_alphanumeric() {
            if !in_ascii_word {
                total += 1;
                in_ascii_word = true;
            }
        } else {
            in_ascii_word = false;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_cjk_and_ascii_words() {
        assert_eq!(count_words("林风推开门"), 5);
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("他说 ok 了"), 4);
        assert_eq!(count_words("，，，"), 0);
        assert_eq!(count_words("他推开门。"), 4, "标点不计");
    }
}
