// 书籍信息共用常量 — 建书向导与手动新建 / 书籍信息编辑共用
import type { BookMetaDto } from '@/api/backend'

/** 题材预设（「自动」仅建书向导用，交给 AI 判断；表单组件从「玄幻」起显示） */
export const GENRES: { name: string; icon: string }[] = [
  { name: '自动', icon: 'fa-wand-magic-sparkles' },
  { name: '玄幻', icon: 'fa-hat-wizard' },
  { name: '仙侠', icon: 'fa-yin-yang' },
  { name: '武侠', icon: 'fa-khanda' },
  { name: '奇幻', icon: 'fa-dragon' },
  { name: '科幻', icon: 'fa-rocket' },
  { name: '都市', icon: 'fa-city' },
  { name: '言情', icon: 'fa-heart' },
  { name: '悬疑', icon: 'fa-magnifying-glass' },
  { name: '历史', icon: 'fa-landmark' },
  { name: '军事', icon: 'fa-shield-halved' },
  { name: '游戏', icon: 'fa-gamepad' },
  { name: '竞技', icon: 'fa-trophy' },
  { name: '灵异', icon: 'fa-ghost' },
  { name: '轻小说', icon: 'fa-feather' },
  { name: '现实', icon: 'fa-seedling' },
  { name: '职场', icon: 'fa-briefcase' },
]

export const AUDIENCES = ['男频', '女频', '全年龄']
export const POVS = ['第三人称限知', '第三人称全知', '第一人称']
export const STYLES = ['热血', '轻松', '暗黑', '细腻', '快节奏爽文', '慢热文艺']

/** 体量档位（目标字数） */
export const SCALES: [string, number][] = [
  ['短篇（30 万字内）', 300_000],
  ['标准（100 万字）', 1_000_000],
  ['长跑（200 万字+）', 2_000_000],
]

/** 单章字数档位 */
export const WORD_OPTIONS = [1500, 2000, 2500, 3000, 4000, 5000, 6000, 8000, 10000]

/** 全部字段都未填写（决定新建项目后是否需要补写 outline.toml） */
export function isBookMetaEmpty(meta: BookMetaDto): boolean {
  return !(
    meta.genre?.trim() ||
    meta.premise?.trim() ||
    meta.theme?.trim() ||
    meta.audience?.trim() ||
    meta.pov?.trim() ||
    meta.style?.trim() ||
    meta.targetWords ||
    meta.chapterWords
  )
}

/** 保存前归一：文本去空白、空串归 null */
export function normalizeBookMeta(meta: BookMetaDto): BookMetaDto {
  const t = (v: string | null) => {
    const s = v?.trim()
    return s ? s : null
  }
  return {
    genre: t(meta.genre),
    premise: t(meta.premise),
    theme: t(meta.theme),
    audience: t(meta.audience),
    pov: t(meta.pov),
    style: t(meta.style),
    targetWords: meta.targetWords ?? null,
    chapterWords: meta.chapterWords ?? null,
  }
}
