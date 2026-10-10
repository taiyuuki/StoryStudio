//! 全文检索 — 前端「检索」对话框的后端，对齐 AI 的 search_text 工具。
//!
//! 直接遍历 structure.toml + 片段正文（不整本 IR 解析）：检索是显式、
//! 低频的动作，按阅读顺序轻量扫一遍即可，无需实体/关系等旁支数据。
//! 摘录构造与 AI 工具共用 tools::query::build_excerpt（【】标记命中段）。

use std::path::Path;

use serde::Serialize;
use story_project_format::{ProjectPaths, StructureFile};

/// 摘录上下文窗（与 AI 工具的 search_text 同宽）
const CONTEXT_CHARS: usize = 40;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub fragment_id: String,
    pub scene_id: String,
    pub chapter_title: String,
    pub scene_title: String,
    /// 命中上下文摘录，命中段以【】标记（前端高亮渲染与跳转定位复用）
    pub excerpt: String,
    /// 该片段内的出现次数
    pub occurrences: usize,
}

/// 全书正文关键词检索（大小写不敏感，按阅读顺序返回，至多 max_hits 条摘录）。
///
/// 超出上限后停止收集摘录，直接截断（前端提示缩小关键词即可）。
pub fn search_text(dir: &Path, query: &str, max_hits: usize) -> Result<Vec<SearchHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("搜索关键词不能为空".to_string());
    }

    let paths = ProjectPaths::new(dir);
    let content = std::fs::read_to_string(paths.structure_toml())
        .map_err(|e| format!("读取 structure.toml 失败: {e}"))?;
    let structure = StructureFile::from_toml_str(&content)
        .map_err(|e| format!("structure.toml 解析失败: {e}"))?;

    let query_lower = q.to_lowercase();
    let query_len = q.chars().count();
    let mut hits: Vec<SearchHit> = Vec::new();

    'outer: for ch_id in &structure.book.chapters {
        let Some(ch) = structure.chapter.get(ch_id) else { continue };
        for sc_id in &ch.scenes {
            let Some(sc) = structure.scene.get(sc_id) else { continue };
            for fid in &sc.fragments {
                let Some(path) = paths.file_for_id(fid) else { continue };
                let Ok(content) = std::fs::read_to_string(&path) else { continue };
                let body = crate::fragment::fragment_body(&content);

                // 先取首个命中位置构造摘录，再统计次数（两趟都只扫本片段）
                let body_lower = body.to_lowercase();
                let Some((byte_idx, _)) = body_lower.match_indices(&query_lower).next() else {
                    continue;
                };
                let occurrences = body_lower.match_indices(&query_lower).count();
                let excerpt =
                    crate::tools::query::build_excerpt(body, byte_idx, query_len, CONTEXT_CHARS);

                hits.push(SearchHit {
                    fragment_id: fid.clone(),
                    scene_id: sc_id.clone(),
                    chapter_title: ch.title.clone(),
                    scene_title: sc.title.clone().unwrap_or_else(|| "未命名场景".into()),
                    excerpt,
                    occurrences,
                });
                if hits.len() >= max_hits {
                    break 'outer;
                }
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_project_format::init::init_project;

    fn setup_test_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_project(tmp.path(), "test-novel").unwrap();
        tmp
    }

    /// 按阅读顺序返回 + 摘录【】标记 + 大小写不敏感
    #[test]
    fn search_returns_hits_in_reading_order() {
        let tmp = setup_test_project();
        let dir = tmp.path();
        let paths = ProjectPaths::new(dir);
        let ch = crate::mutations::create_chapter(&paths, "第一章").unwrap();
        let sc1 = crate::mutations::create_scene(&paths, &ch, "渡口", None).unwrap();
        let sc2 = crate::mutations::create_scene(&paths, &ch, "客栈", None).unwrap();
        crate::mutations::create_fragment(&paths, &sc2, "他推开客栈的门。").unwrap();
        let f1 = crate::mutations::create_fragment(&paths, &sc1, "槐安渡的老船工等在渡口。").unwrap();

        let hits = search_text(dir, "渡口", 50).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].fragment_id, f1);
        assert_eq!(hits[0].scene_id, sc1);
        assert_eq!(hits[0].chapter_title, "第一章");
        assert_eq!(hits[0].scene_title, "渡口");
        assert_eq!(hits[0].occurrences, 1);
        assert!(hits[0].excerpt.contains("【渡口】"), "{}", hits[0].excerpt);

        // 大小写不敏感（ASCII 关键词）
        crate::mutations::create_fragment(&paths, &sc1, "The Raven landed.").unwrap();
        let hits = search_text(dir, "raven", 50).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].excerpt.contains("【Raven】"), "{}", hits[0].excerpt);

        // 空关键词报错
        assert!(search_text(dir, "  ", 50).is_err());
    }
}
