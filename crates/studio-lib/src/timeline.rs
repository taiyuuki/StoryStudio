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
        return ymd_ordinal(t, 0);
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
        return ymd_ordinal(rest, era_idx as i64);
    }

    None
}

/// `year[-month[-day]]` → 序数。容忍「年」字与无数字后缀
/// （"1025年春"按 1025 年计，季节词暂不参与比较）
fn ymd_ordinal(s: &str, era_idx: i64) -> Option<i64> {
    let year_str = leading_num(s)?;
    let year: i64 = year_str.parse().ok()?;
    let after_year = s[year_str.len()..]
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

/// 场景时间连续性校验（compile_check 附加报告）。
///
/// 按阅读序（章→场景）遍历，可比序数（绝对历法或可解析的历法标签锚）
/// 出现倒退 → 返回警告行。模糊标签（「某个雨夜」）按既有设计跳过；
/// gap 为自由文本暂不参与。返回给 compile_check 拼接的 Warning 行。
pub fn continuity_warnings(paths: &ProjectPaths) -> Vec<String> {
    let eras = load_eras(paths);
    let Ok(content) = std::fs::read_to_string(paths.structure_toml()) else {
        return Vec::new();
    };
    let Ok(structure) = story_project_format::StructureFile::from_toml_str(&content) else {
        return Vec::new();
    };

    let mut warnings = Vec::new();
    // (scene_id, 展示锚文本, 序数)：记录最近一个可比场景
    let mut last: Option<(String, String, i64)> = None;
    for ch_id in &structure.book.chapters {
        let Some(ch) = structure.chapter.get(ch_id) else { continue };
        for sc_id in &ch.scenes {
            let Some(sc) = structure.scene.get(sc_id) else { continue };
            // 绝对历法优先（拼成数字锚走同一折算），否则用历法标签锚
            // 数值锚优先（拼成数字形态走同一折算），否则用历法标签锚
            let anchor = sc.story_time.as_ref().and_then(|t| {
                if t.year.is_some() {
                    let year = t.year.unwrap_or(0);
                    Some(match (&t.month, &t.day) {
                        (Some(m), Some(d)) => format!("{year}-{m}-{d}"),
                        (Some(m), None) => format!("{year}-{m}"),
                        _ => year.to_string(),
                    })
                } else {
                    t.anchor.clone()
                }
            });
            let Some(anchor) = anchor else { continue };
            let Some(ord) = anchor_ordinal(&anchor, &eras) else {
                continue; // 模糊锚：合法，仅展示不比较
            };
            if let Some((last_sc, last_anchor, last_ord)) = &last {
                if ord < *last_ord {
                    warnings.push(format!(
                        "场景 {sc_id}「{}」的时间锚（{anchor}）早于前一可比较场景 {last_sc}（{last_anchor}）——叙事时间疑似倒流，请核对 story_time / story_time_anchor",
                        sc.title.as_deref().unwrap_or("无标题")
                    ));
                }
            }
            last = Some((sc_id.clone(), anchor, ord));
        }
    }
    warnings
}

#[cfg(test)]
mod continuity_tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, ProjectPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(tmp.path());
        std::fs::create_dir_all(paths.fragments_dir()).unwrap();
        (tmp, paths)
    }

    fn write_structure(paths: &ProjectPaths, toml: &str) {
        std::fs::write(paths.structure_toml(), toml).unwrap();
    }

    #[test]
    fn anchor_ordinals_cover_numeric_and_era_labels() {
        let eras = vec!["创世".to_string(), "灵历".to_string()];
        assert!(anchor_ordinal("1024-5-3", &eras).unwrap() < anchor_ordinal("1024-5-10", &eras).unwrap());
        assert!(anchor_ordinal("灵历3217", &eras).unwrap() > anchor_ordinal("创世9999", &eras).unwrap());
        assert_eq!(anchor_ordinal("某个雨夜", &eras), None, "模糊锚不比较");
    }

    #[test]
    fn continuity_flags_regression_and_skips_ambiguous() {
        let (_tmp, paths) = setup();
        write_structure(
            &paths,
            r#"[book]
id = "b_root"
title = "t"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_1", "sc_2", "sc_3", "sc_4"]

[scene.sc_1]
title = "起"
fragments = []
story_time = { year = 1024, month = 5, day = 1 }

[scene.sc_2]
title = "承"
fragments = []
story_time = { year = 1024, month = 5, day = 3 }

[scene.sc_3]
title = "转"
fragments = []
story_time = { year = 1024, month = 5, day = 2 }

[scene.sc_4]
title = "模糊"
fragments = []
story_time = { anchor = "某个雨夜" }
"#,
        );
        let ws = continuity_warnings(&paths);
        assert_eq!(ws.len(), 1, "{ws:?}");
        assert!(ws[0].contains("sc_3"), "{}", ws[0]);
        assert!(ws[0].contains("倒流"), "{}", ws[0]);
    }

    #[test]
    fn continuity_allows_forward_and_era_labels() {
        let (_tmp, paths) = setup();
        std::fs::create_dir_all(paths.root().join("worldview")).unwrap();
        std::fs::write(
            paths.worldview_timeline_toml(),
            "eras = [\"创世\", \"灵历\"]",
        )
        .unwrap();
        write_structure(
            &paths,
            r#"[book]
id = "b_root"
title = "t"
chapters = ["ch_01"]

[chapter.ch_01]
title = "第一章"
scenes = ["sc_1", "sc_2"]

[scene.sc_1]
title = "旧纪元"
fragments = []
story_time_anchor = "创世9998年"

[scene.sc_2]
title = "新纪元"
fragments = []
story_time_anchor = "灵历3年"
"#,
        );
        assert!(continuity_warnings(&paths).is_empty(), "跨纪元前进合法");
    }
}
