//! 时间线工具 — 纪元加载与锚点序数解析（continuity 基础设施）
//!
//! 设计要点：检查靠拓扑序与间隔，历法只管比较与展示。
//! - 纯数字锚（1023 / 1023-9-15）视为单一纪元（era 0）
//! - 历法标签锚（"灵历3217年"）按 worldview/timeline.toml 的 eras 位置折算序数
//! - 解析不了的标签是合法数据（只展示不比较），返回 None 而非报错

use story_project_format::{ProjectPaths, WorldviewSectionFile};

/// 纪元跨度基数：只要年份数值 < 10_000_000，跨纪元比较恒正确
const ERA_SPAN: i64 = 100_000_000;

/// 从 worldview/timeline.toml 读取有序纪元表（文件缺失/解析失败返回空表）
pub fn load_eras(paths: &ProjectPaths) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(paths.worldview_timeline_toml()) else {
        return Vec::new();
    };
    story_project_format::TimelineFile::from_toml_str(&content)
        .map(|t| t.eras)
        .unwrap_or_default()
}

/// 把时间锚解析为可比较序数。
///
/// - `1023` / `1023-9` / `1023-9-15` → 单纪元数值
/// - `灵历3217` / `灵历3217-9` / `灵历3217年` → 纪元位置 × 跨度 + 年月日
/// - 其余（"某年""很久以前"等模糊描述）→ None（合法，仅展示）
pub fn anchor_ordinal(anchor: &str, eras: &[String]) -> Option<i64> {
    let t = anchor.trim();
    if t.is_empty() {
        return None;
    }

    // 纯数字形态 → 单纪元
    if t.starts_with(|c: char| c.is_ascii_digit()) {
        return numeric_ordinal(t, 0);
    }

    // 纪元前缀形态：最长纪元名优先（防"灵历"与"灵历新"前缀互撞）
    let matched = eras
        .iter()
        .enumerate()
        .filter_map(|(idx, era)| {
            t.strip_prefix(era.as_str()).map(|rest| (idx, era, rest))
        })
        .max_by_key(|(_, era, _)| era.chars().count());

    if let Some((era_idx, _era, rest)) = matched {
        let year_str = leading_num(rest)?;
        let year: i64 = year_str.parse().ok()?;
        let after_year = rest[year_str.len()..]
            .trim_start_matches('年')
            .trim_start_matches('-');
        let month_str = leading_num(after_year);
        let month: i64 = month_str
            .as_ref()
            .and_then(|m| m.parse().ok())
            .unwrap_or(0);
        let after_month = month_str
            .map(|m| after_year[m.len()..].trim_start_matches('-'))
            .unwrap_or(after_year);
        let day: i64 = leading_num(after_month)
            .and_then(|d| d.parse().ok())
            .unwrap_or(0);
        return Some(era_idx as i64 * ERA_SPAN + year * 10_000 + month * 100 + day);
    }

    None
}

/// `year` / `year-month` / `year-month-day` → 序数
fn numeric_ordinal(s: &str, era_idx: i64) -> Option<i64> {
    let mut parts = s.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    let day: i64 = parts.next().and_then(|d| d.parse().ok()).unwrap_or(0);
    Some(era_idx * ERA_SPAN + year * 10_000 + month * 100 + day)
}

/// 开头的连续数字段
fn leading_num(s: &str) -> Option<String> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// 加载项目纪元表并解析锚点（工具层便捷组合）
pub fn resolve_anchor(paths: &ProjectPaths, anchor: &str) -> Option<i64> {
    anchor_ordinal(anchor, &load_eras(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERAS: &[&str] = &["创世", "灵历", "开元"];

    fn eras() -> Vec<String> {
        ERAS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn numeric_anchors_compare() {
        assert!(anchor_ordinal("1023", &eras()).unwrap() < anchor_ordinal("1024", &eras()).unwrap());
        assert!(
            anchor_ordinal("1023-3", &eras()).unwrap()
                < anchor_ordinal("1023-9-15", &eras()).unwrap()
        );
    }

    #[test]
    fn era_anchors_compare_across_eras() {
        let e = eras();
        let a = anchor_ordinal("灵历3217", &e).unwrap();
        let b = anchor_ordinal("灵历3218", &e).unwrap();
        let c = anchor_ordinal("开元1", &e).unwrap();
        let d = anchor_ordinal("创世999", &e).unwrap();
        assert!(a < b, "同纪元按年比");
        assert!(b < c, "后纪元整体晚于前纪元");
        assert!(d < a, "首纪元早于后续纪元");
        assert_eq!(
            anchor_ordinal("灵历3217年", &e),
            Some(a),
            "中文年号后缀容忍"
        );
        assert!(
            anchor_ordinal("灵历3217-9", &e).unwrap() > a,
            "纪元内月日放大"
        );
    }

    #[test]
    fn fuzzy_anchors_are_none_not_error() {
        let e = eras();
        assert_eq!(anchor_ordinal("某年", &e), None);
        assert_eq!(anchor_ordinal("很久以前", &e), None);
        assert_eq!(anchor_ordinal("不存在的纪元5", &e), None);
        assert_eq!(anchor_ordinal("开元三年", &e), None, "中文数字年暂不支持");
    }
}
