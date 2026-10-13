//! 卷结构统一 — outline.toml 是卷的唯一真相源。
//!
//! 历史上卷定义存了两份：outline.toml `[[volumes]]`（大纲层，建书向导写入）
//! 与 chapters.toml `[[volumes]]`（脉络分组层，向导保存批次时复制了一份），
//! 两边缺少同步机制，极易漂移：AI 只改一份，用户在脉络面板看到的是另一份。
//!
//! 统一规则：
//! - 卷定义只存 outline.toml，数组序即卷序号（1-based，连续）
//! - chapters.toml 的 `[[volumes]]` 是遗留副本：读取时按卷名对齐合并进大纲，
//!   章节的 volume_order 与线索的 due_volume 一并重映射，然后清空副本
//! - 合并幂等：迁移完成后再次调用不做任何事

use std::collections::HashMap;

use thiserror::Error;

use crate::chapters_file::{ChaptersFile, VolumeGroupDef};
use crate::outline_file::{OutlineFile, VolumeDef};
use crate::paths::ProjectPaths;
use crate::threads_file::ThreadsFile;

/// 旧卷序号 → 统一卷序号的映射表
pub type VolumeRemap = HashMap<u32, u32>;

#[derive(Debug, Error)]
pub enum VolumeSyncError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML 序列化失败: {0}")]
    Toml(#[from] toml::ser::Error),
    #[error("outline.toml 解析失败: {0}")]
    Outline(#[from] crate::outline_file::OutlineFileError),
}

/// 把 chapters.toml 的遗留卷副本合并进 outline.toml 的卷结构。
/// 就地修改：outline 追加独有卷、章节 volume_order 重映射、副本清空。
/// 返回重映射表（chapters 无遗留副本时返回空表 = 无需迁移）。
///
/// 对齐规则：同名卷视为同一卷，名称/主题/梗概以 outline 侧为准；
/// chapters 独有的卷按 order 排序追加到 outline 末尾、序号顺延；
/// 章节映射不到的卷引用归入未分卷（0）。
pub fn unify_volumes(outline: &mut OutlineFile, chapters: &mut ChaptersFile) -> VolumeRemap {
    let mut remap: VolumeRemap = VolumeRemap::new();
    if chapters.volumes.is_empty() {
        return remap;
    }

    let mut legacy: Vec<&VolumeGroupDef> = chapters.volumes.iter().collect();
    legacy.sort_by_key(|v| v.order);

    // 追加的卷先攒在一边：统一卷序号 = outline 原有数组序 + 追加位置
    let base = outline.volumes.len();
    let mut appended: Vec<VolumeDef> = Vec::new();
    for cv in legacy {
        if let Some(pos) = outline
            .volumes
            .iter()
            .chain(appended.iter())
            .position(|v| v.name == cv.name)
        {
            remap.entry(cv.order).or_insert((pos + 1) as u32);
        } else {
            appended.push(VolumeDef {
                name: cv.name.clone(),
                summary: cv.summary.clone(),
                theme: cv.theme.clone(),
            });
            let order = (base + appended.len()) as u32;
            remap.entry(cv.order).or_insert(order);
        }
    }
    outline.volumes.append(&mut appended);

    for ch in &mut chapters.chapters {
        if ch.volume_order > 0 {
            ch.volume_order = remap.get(&ch.volume_order).copied().unwrap_or(0);
        }
    }
    chapters.volumes.clear();
    remap
}

/// 按映射表重写线索的 due_volume（引用了被重编号的旧卷序号时）。
/// 返回是否有改动。
pub fn remap_due_volumes(threads: &mut ThreadsFile, remap: &VolumeRemap) -> bool {
    let mut changed = false;
    for t in &mut threads.threads {
        if let Some(dv) = t.due_volume {
            if let Some(&nv) = remap.get(&dv) {
                t.due_volume = Some(nv);
                changed = true;
            }
        }
    }
    changed
}

/// outline 卷 → 统一卷视图（order = 数组序 1-based）
pub fn unified_groups(outline: &OutlineFile) -> Vec<VolumeGroupDef> {
    outline
        .volumes
        .iter()
        .enumerate()
        .map(|(i, v)| VolumeGroupDef {
            name: v.name.clone(),
            order: (i + 1) as u32,
            theme: v.theme.clone(),
            summary: v.summary.clone(),
        })
        .collect()
}

/// 读取 chapters.toml 并完成卷结构迁移，返回 (章节脉络, 统一卷列表)。
///
/// 遗留副本存在时一次性落盘（outline.toml 合并卷、chapters.toml 清空副本、
/// threads.toml 重映射 due_volume）；迁移写盘 best-effort，失败时保留原状
/// 下次再试，本次读取仍返回内存合并后的结果。
/// 文件缺失或损坏按空处理（与各调用方原有宽容语义一致）。
pub fn load_chapters_unified(paths: &ProjectPaths) -> (ChaptersFile, Vec<VolumeGroupDef>) {
    let mut chapters = std::fs::read_to_string(paths.chapters_toml())
        .ok()
        .and_then(|c| ChaptersFile::from_toml_str(&c).ok())
        .unwrap_or_default();
    let mut outline = std::fs::read_to_string(paths.outline_toml())
        .ok()
        .and_then(|c| OutlineFile::from_toml_str(&c).ok())
        .unwrap_or_default();

    let remap = unify_volumes(&mut outline, &mut chapters);
    if remap.is_empty() {
        return (chapters, unified_groups(&outline));
    }

    let mut threads = std::fs::read_to_string(paths.threads_toml())
        .ok()
        .and_then(|c| ThreadsFile::from_toml_str(&c).ok())
        .unwrap_or_default();
    let threads_dirty = remap_due_volumes(&mut threads, &remap);

    if let (Ok(c), Ok(o)) = (chapters.to_toml_string(), outline.to_toml_string()) {
        let written = crate::atomic::write(paths.chapters_toml(), &c).is_ok()
            && crate::atomic::write(paths.outline_toml(), &o).is_ok();
        if written && threads_dirty {
            if let Ok(t) = threads.to_toml_string() {
                let _ = crate::atomic::write(paths.threads_toml(), &t);
            }
        }
    }
    (chapters, unified_groups(&outline))
}

/// 全量写 outline.toml 的卷结构（读-改-写，只动 [[volumes]] 段，
/// 其余大纲内容原样保留）。卷按 order 排序后数组序即新序号。
pub fn save_outline_volumes(
    paths: &ProjectPaths,
    volumes: &[VolumeGroupDef],
) -> Result<(), VolumeSyncError> {
    let mut outline = if paths.outline_toml().exists() {
        let content = std::fs::read_to_string(paths.outline_toml())?;
        OutlineFile::from_toml_str(&content)?
    } else {
        OutlineFile::default()
    };

    let mut sorted: Vec<&VolumeGroupDef> = volumes.iter().collect();
    sorted.sort_by_key(|v| v.order);
    outline.volumes = sorted
        .iter()
        .map(|v| VolumeDef {
            name: v.name.clone(),
            summary: v.summary.clone(),
            theme: v.theme.clone(),
        })
        .collect();

    crate::atomic::write(paths.outline_toml(), &outline.to_toml_string()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol_group(name: &str, order: u32) -> VolumeGroupDef {
        VolumeGroupDef {
            name: name.to_string(),
            order,
            theme: None,
            summary: None,
        }
    }

    fn vol_def(name: &str) -> VolumeDef {
        VolumeDef {
            name: name.to_string(),
            summary: None,
            theme: None,
        }
    }

    fn chapter(title: &str, volume_order: u32) -> crate::chapters_file::ChapterOutlineDef {
        crate::chapters_file::ChapterOutlineDef {
            title: title.to_string(),
            volume_order,
            ..Default::default()
        }
    }

    #[test]
    fn legacy_volumes_migrate_when_outline_empty() {
        let mut outline = OutlineFile::default();
        let mut chapters = ChaptersFile {
            volumes: vec![vol_group("风起", 1), vol_group("潮生", 2)],
            chapters: vec![chapter("雨夜", 1), chapter("渡河", 2), chapter("散章", 0)],
        };

        let remap = unify_volumes(&mut outline, &mut chapters);

        assert_eq!(remap.get(&1), Some(&1));
        assert_eq!(remap.get(&2), Some(&2));
        assert_eq!(outline.volumes.len(), 2);
        assert_eq!(outline.volumes[1].name, "潮生");
        assert!(chapters.volumes.is_empty(), "遗留副本应清空");
        assert_eq!(chapters.chapters[0].volume_order, 1);
        assert_eq!(chapters.chapters[1].volume_order, 2);
        assert_eq!(chapters.chapters[2].volume_order, 0, "未分卷不动");
    }

    #[test]
    fn same_name_aligns_outline_wins() {
        let mut outline = OutlineFile {
            volumes: vec![vol_def("风起"), vol_def("潮生")],
            ..Default::default()
        };
        let mut chapters = ChaptersFile {
            volumes: vec![vol_group("潮生", 2), vol_group("孤卷", 5)],
            chapters: vec![chapter("渡河", 2), chapter("孤行", 5), chapter("悬空", 9)],
        };

        let remap = unify_volumes(&mut outline, &mut chapters);

        // 同名对齐既有序号，独有卷追加为第 3 卷
        assert_eq!(remap.get(&2), Some(&2));
        assert_eq!(remap.get(&5), Some(&3));
        assert_eq!(outline.volumes.len(), 3);
        assert_eq!(outline.volumes[2].name, "孤卷");
        // 悬空引用归未分卷
        assert_eq!(chapters.chapters[2].volume_order, 0);
    }

    #[test]
    fn unify_is_idempotent() {
        let mut outline = OutlineFile::default();
        let mut chapters = ChaptersFile {
            volumes: vec![vol_group("风起", 1)],
            chapters: vec![chapter("雨夜", 1)],
        };
        unify_volumes(&mut outline, &mut chapters);

        let remap = unify_volumes(&mut outline, &mut chapters);
        assert!(remap.is_empty(), "迁移后再次调用应为空映射");
        assert_eq!(chapters.chapters[0].volume_order, 1);
    }

    #[test]
    fn due_volume_remap_only_hits_known() {
        // serde 字段名是单数 [[thread]]
        let mut threads = ThreadsFile::from_toml_str(
            r#"
[[thread]]
id = "t_001"
title = "灯渣"
due_volume = 2

[[thread]]
id = "t_002"
title = "旧案"
due_volume = 9
"#,
        )
        .unwrap();

        let mut remap = VolumeRemap::new();
        remap.insert(2, 3);
        assert!(remap_due_volumes(&mut threads, &remap));
        assert_eq!(threads.threads[0].due_volume, Some(3));
        assert_eq!(threads.threads[1].due_volume, Some(9), "未命中映射保持原值");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(tmp.path());

        save_outline_volumes(
            &paths,
            &[
                VolumeGroupDef {
                    name: "第二卷 上山".into(),
                    order: 2,
                    theme: Some("对峙".into()),
                    summary: None,
                },
                vol_group("第一卷 灯下", 1),
            ],
        )
        .unwrap();

        // order 排序落盘为数组序
        let outline =
            OutlineFile::from_toml_str(&std::fs::read_to_string(paths.outline_toml()).unwrap())
                .unwrap();
        let groups = unified_groups(&outline);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "第一卷 灯下");
        assert_eq!(groups[1].name, "第二卷 上山");
        assert_eq!(groups[1].theme.as_deref(), Some("对峙"));

        // 再写入时保留 outline 其他段
        let with_story = "title = \"灯\"\n\n[[volumes]]\nname = \"旧卷\"\n";
        std::fs::write(paths.outline_toml(), with_story).unwrap();
        save_outline_volumes(&paths, &[vol_group("新卷", 1)]).unwrap();
        let content = std::fs::read_to_string(paths.outline_toml()).unwrap();
        assert!(content.contains("新卷"), "{content}");
        assert!(!content.contains("旧卷"), "卷段应被全量替换: {content}");
    }
}
