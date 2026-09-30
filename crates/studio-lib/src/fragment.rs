//! Fragment 文件操作 — 保存草稿 + 更新 structure.toml

use story_ai_runtime::FragmentDraft;
use story_project_format::{ProjectPaths, StructureFile};

use crate::error::StudioError;

// ============================================================================
// API
// ============================================================================

/// 将 AI 生成的 Fragment 草稿保存到 fragments/ 目录，并更新 structure.toml
///
/// 返回新建的 Fragment ID 列表。
pub fn save_drafts(
    paths: &ProjectPaths,
    scene_id: &str,
    drafts: &[FragmentDraft],
) -> Result<Vec<String>, StudioError> {
    // 1. 逐个分配 Fragment ID（统一走 mutations 的十六进制分配器，
    //    维护持久高水位；连续调用单调递增）
    let mut saved_ids = Vec::new();

    for draft in drafts {
        let frag_id = crate::mutations::next_fragment_id(paths);
        saved_ids.push(frag_id.clone());

        // 2. 构建 Fragment 文件内容
        let content = render_fragment_file(&frag_id, draft);

        // 3. 写入文件（回读校验 + 码字记账：AI 来源）
        let file_path = paths.fragments_dir().join(format!("{frag_id}.md"));
        crate::journal::write_verified(paths.root(), "fragment_create", &file_path, &content)?;
        crate::stats::record_change(
            paths.root(),
            None,
            &draft.text,
            crate::stats::WriteSource::Ai,
        );
    }

    // 4. 更新 structure.toml
    update_structure_add_fragments(paths, scene_id, &saved_ids)?;

    Ok(saved_ids)
}

/// 更新 structure.toml，将新 Fragment ID 追加到指定 Scene 的 fragments 数组
pub fn update_structure_add_fragments(
    paths: &ProjectPaths,
    scene_id: &str,
    new_frag_ids: &[String],
) -> Result<(), StudioError> {    let structure_path = paths.structure_toml();
    let content = std::fs::read_to_string(&structure_path)?;

    let mut structure = StructureFile::from_toml_str(&content)
        .map_err(|e| StudioError::Config(format!("解析 structure.toml 失败: {e}")))?;

    let scene = structure.scene.get_mut(scene_id).ok_or_else(|| {
        StudioError::NotFound(format!("Scene '{scene_id}' 在 structure.toml 中不存在"))
    })?;

    for frag_id in new_frag_ids {
        if !scene.fragments.contains(frag_id) {
            scene.fragments.push(frag_id.clone());
        }
    }

    let new_content = structure
        .to_toml_string()
        .map_err(|e| StudioError::Config(format!("序列化 structure.toml 失败: {e}")))?;

    std::fs::write(&structure_path, new_content)?;

    Ok(())
}

/// 定位片段所属的场景 ID（审校报告精确跳转用）——扫 structure.toml 各场景的片段表
pub fn scene_of_fragment(paths: &ProjectPaths, fragment_id: &str) -> Result<String, StudioError> {
    let structure_path = paths.structure_toml();
    let content = std::fs::read_to_string(&structure_path)?;
    let structure = StructureFile::from_toml_str(&content)
        .map_err(|e| StudioError::Config(format!("解析 structure.toml 失败: {e}")))?;

    for (scene_id, scene) in structure.scene.iter() {
        if scene.fragments.iter().any(|f| f == fragment_id) {
            return Ok(scene_id.clone());
        }
    }
    Err(StudioError::NotFound(format!(
        "Fragment '{fragment_id}' 不属于任何场景"
    )))
}

// ============================================================================
// 内部辅助
// ============================================================================

/// 剥离 YAML frontmatter，返回正文部分。
///
/// 统计（码字账本）与版本历史（节流比对）共用：字数只算正文，
/// `id:` / `anchors:` 行不计入，锚点变化也不应视为内容变化。
pub(crate) fn fragment_body(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---") else {
        return content;
    };
    let Some(end) = rest.find("\n---") else {
        return content;
    };
    // 跳过闭合 --- 与其后的换行/空行
    let after = &rest[end + 4..];
    after.trim_start_matches(['\r', '\n'])
}

/// 渲染单个 Fragment 文件内容（frontmatter + 正文）
fn render_fragment_file(frag_id: &str, draft: &FragmentDraft) -> String {
    let mut content = format!("---\nid: {frag_id}\n");

    if !draft.anchors.is_empty() {
        content.push_str("anchors:\n");
        for anchor in &draft.anchors {
            let role = match anchor.role {
                story_ir::AnchorRole::Appears => "appears",
                story_ir::AnchorRole::Mentioned => "mentioned",
                story_ir::AnchorRole::Referenced => "referenced",
            };
            content.push_str(&format!(
                "  - entity: {}\n    role: {role}\n",
                anchor.entity
            ));
        }
    }
    content.push_str("---\n\n");
    content.push_str(strip_leading_frontmatter(&draft.text));
    if !content.ends_with('\n') {
        content.push('\n');
    }

    content
}

/// 剥离模型复读的 YAML frontmatter。
///
/// 续写任务的输出格式与片段文件相似，模型偶尔会在正文开头模仿一段
/// `---\nanchors:\n  - ...\n---`。此处按行识别"首行 `---` 且后续存在
/// 独立 `---` 行"的块并将其丢弃，避免与 render 出的 frontmatter 叠加成双层。
/// 剥离模型复读的 YAML frontmatter。
///
/// 续写任务的输出格式与片段文件相似，模型偶尔会在正文开头模仿一段
/// `---\nanchors:\n  - ...\n---`。此处按行识别"首行 `---` 且后续存在
/// 独立 `---` 行"的块并将其丢弃，避免与 render 出的 frontmatter 叠加成双层。
/// 容忍块前的空行/空白（模型输出常见的排版）。
pub(crate) fn strip_leading_frontmatter(text: &str) -> &str {
    let text = text.trim_start_matches(['\r', '\n', ' ', '\t']);
    let mut iter = text.split_inclusive('\n');
    let first = iter.next();
    if first.map(str::trim) != Some("---") {
        return text;
    }
    // 收集到闭合 --- 的块；须为 id/anchors 残块形态才剥离——
    // 以 --- 起笔的文学文本（块内是散文）原样保留
    let mut consumed = first.map_or(0, str::len);
    let mut block: Vec<&str> = Vec::new();
    for line in iter {
        consumed += line.len();
        if line.trim() == "---" {
            if is_residue_block(&block) {
                return text[consumed.min(text.len())..].trim_start_matches(['\r', '\n']);
            }
            return text;
        }
        block.push(line);
        // frontmatter 块不会太长，超过阈值视作误判（正文本身的分隔线）
        if consumed > 4096 {
            break;
        }
    }
    text
}

pub(crate) fn strip_all_leading_frontmatter(mut text: &str) -> &str {
    for _ in 0..4 {
        let stripped = strip_leading_frontmatter(text);
        if stripped.len() == text.len() {
            return text;
        }
        text = stripped;
    }
    text
}

/// 判断 `---` 围栏块的内容是否为"id/anchors 残块"形态：
/// 非空行仅含 `id: f_XXXX`、`anchors:` 或列表项（`- xxx`），
/// 且至少含 `id:` 或 `anchors:` 之一。
///
/// 形态守卫：正文中合法的分隔线块（内含叙事文字）不会被误判。
pub(crate) fn is_residue_block(block_lines: &[&str]) -> bool {
    let mut has_marker = false;
    let mut non_empty = 0usize;
    for line in block_lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_empty += 1;
        if t.starts_with("anchors:")
            || (t.starts_with("id:") && t["id:".len()..].trim_start().starts_with("f_"))
        {
            has_marker = true;
        } else if t.starts_with('-') {
            // 列表项（anchors 条目）
        } else if t.starts_with("role:") {
            // 锚点条目的续行（role: appears/mentioned/referenced）
        } else {
            return false;
        }
    }
    non_empty > 0 && has_marker
}

/// 移除正文中**任意位置**的 id/anchors 残块（repair 用）。
///
/// 与 [`strip_all_leading_frontmatter`] 只处理开头不同，本函数全文扫描：
/// 历史缺陷产生的残块可能叠在正文中部。返回 (清理后文本, 是否有变化)。
pub(crate) fn strip_residue_blocks(body: &str) -> (String, bool) {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut changed = false;
    let mut i = 0usize;

    while i < lines.len() {
        if lines[i].trim() == "---" {
            if let Some(rel) = (i + 1..lines.len()).position(|j| lines[j].trim() == "---") {
                let end = i + 1 + rel;
                if is_residue_block(&lines[i + 1..end]) {
                    // 丢弃整个块（含两侧 ---）；吞掉块后紧邻的空行，避免连续空段
                    changed = true;
                    i = end + 1;
                    while i < lines.len() && lines[i].trim().is_empty() {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        kept.push(lines[i]);
        i += 1;
    }

    if !changed {
        return (body.to_string(), false);
    }

    let mut text = kept.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.push('\n');
    (text, true)
}

#[cfg(test)]
mod fragment_body_tests {
    use super::*;

    #[test]
    fn body_strips_frontmatter() {
        let content = "---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n\n正文内容\n";
        assert_eq!(fragment_body(content), "正文内容\n");
    }

    #[test]
    fn body_keeps_content_without_frontmatter() {
        assert_eq!(fragment_body("直接是正文"), "直接是正文");
        // 未闭合的 --- 不视为 frontmatter
        assert_eq!(fragment_body("---\n散文起笔"), "---\n散文起笔");
    }
}

#[cfg(test)]
mod strip_tests {
    use super::*;

    #[test]
    fn strips_model_echoed_frontmatter() {
        let text = "---\nanchors:\n  - 沈砚之\n---\n\n他没有应声。";
        assert_eq!(strip_leading_frontmatter(text), "他没有应声。");
    }

    #[test]
    fn strips_frontmatter_after_leading_blank_lines() {
        // 模型输出在 YAML 块前留了空行（曾绕过首行判断导致双层 frontmatter）
        let text = "\n\n---\nanchors:\n  - 沈砚之\n---\n他没有应声。";
        assert_eq!(strip_leading_frontmatter(text), "他没有应声。");
    }

    #[test]
    fn strips_multiple_leading_frontmatter_blocks() {
        // 历史脏数据：正文开头叠了两个残留块
        let text = "---\nanchors:\n  - 甲\n---\n---\nanchors:\n  - 乙\n---\n正文。";
        assert_eq!(strip_all_leading_frontmatter(text), "正文。");
    }

    #[test]
    fn keeps_normal_text() {
        let text = "雨下了整夜，青石巷的路灯在水洼里碎成一片昏黄。";
        assert_eq!(strip_leading_frontmatter(text), text);
    }

    #[test]
    fn keeps_unclosed_delimiter() {
        // 只有首行 --- 而无闭合行：正文本身的分隔线，不动
        let text = "---\n他走了。";
        assert_eq!(strip_leading_frontmatter(text), text);
    }

    #[test]
    fn strip_residue_blocks_removes_mid_body_blocks() {
        // 正文中部的残块（显示名形态）—— repair 盲区场景
        let body = "第一段。\n\n---\nanchors:\n  - 沈砚之\n---\n\n第二段。";
        let (cleaned, changed) = strip_residue_blocks(body);
        assert!(changed);
        assert!(cleaned.contains("第一段。"));
        assert!(cleaned.contains("第二段。"));
        assert!(!cleaned.contains("anchors:"));

        // 实体 ID 形态（规范锚点格式）同样清除
        let body2 = "开头。\n---\nid: f_0001\nanchors:\n  - entity: c_0001\n    role: appears\n---\n结尾。";
        let (cleaned2, changed2) = strip_residue_blocks(body2);
        assert!(changed2);
        assert!(!cleaned2.contains("entity:"));
        assert!(cleaned2.contains("开头。"));
        assert!(cleaned2.contains("结尾。"));
    }

    #[test]
    fn strip_residue_blocks_keeps_prose_dividers() {
        // 叙事分隔线（单行 --- 或含文字的块）不动
        let body = "上半天。\n\n---\n\n下半天。";
        let (cleaned, changed) = strip_residue_blocks(body);
        assert!(!changed);
        assert_eq!(cleaned, body);

        let body2 = "前。\n---\n天气转晴。\n---\n后。";
        let (cleaned2, changed2) = strip_residue_blocks(body2);
        assert!(!changed2, "含叙事文字的块不应清除: {cleaned2}");
    }

    #[test]
    fn keeps_literary_dash_opening() {
        // 以 --- 起笔的文学文本（块内是散文，非 id/anchors 形态）不剥离
        let text = "---\n\n清晨的雾还没散。\n\n他推门进去。\n---\n\n午后。";
        assert_eq!(strip_leading_frontmatter(text), text);
    }
}
