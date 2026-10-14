// 片段引文精确定位 — 报告对话框 → 目标 FragmentBlock 编辑器的跨组件总线
// + CodeMirror 命中高亮扩展
//
// 用法：requestFragmentReveal(fragmentId, quote) 发起定位；FragmentBlock
// watch pending，命中自己的 fragmentId 时调用 revealQuote 在编辑器里
// 滚动到引文处并高亮闪烁。引文在片段内有多处匹配时，连续请求同一条目
// 会依次跳到下一处（occurrence 递增，超出后循环），全部匹配淡色底纹、
// 当前命中闪烁。
import { ref } from 'vue'
import { EditorView, Decoration, type DecorationSet } from '@codemirror/view'
import { StateEffect, StateField } from '@codemirror/state'

// ── 跨组件定位总线 ────────────────────────────────────────

export interface RevealRequest {
  seq: number
  fragmentId: string
  quote: string
  /** 第几处匹配（0 起）：同一条目连续请求时递增，由 revealQuote 取模循环 */
  occurrence: number
}

const pending = ref<RevealRequest | null>(null)
let seq = 0
// 同一条目连续请求 → 跳下一处匹配；换条目 → 从第一处重新开始
let lastKey = ''
let occurrence = 0

export function requestFragmentReveal(fragmentId: string, quote: string) {
  const key = `${fragmentId}::${quote}`
  occurrence = key === lastKey ? occurrence + 1 : 0
  lastKey = key
  pending.value = { seq: ++seq, fragmentId, quote, occurrence }
}

export function useFragmentReveal() {
  return { pending }
}

// ── CodeMirror 命中高亮扩展 ──────────────────────────────

interface RevealHits {
  /** 全部匹配位置（淡色底纹） */
  hits: [number, number][]
  /** 当前跳转到的匹配（闪烁） */
  current: [number, number]
}

const flashHit = StateEffect.define<RevealHits>()
const clearHit = StateEffect.define<null>()

const hitFlashField = StateField.define<RevealHits | null>({
  create: () => null,
  update(value, tr) {
    // 文档一动高亮即失效（位置已漂移）
    let next = tr.docChanged ? null : value
    for (const eff of tr.effects) {
      if (eff.is(flashHit)) next = eff.value
      else if (eff.is(clearHit)) next = null
    }
    return next
  },
  provide: (field) =>
    EditorView.decorations.from(field, (value): DecorationSet =>
      value
        ? Decoration.set(
            value.hits.map((h) =>
              Decoration.mark({ class: h === value.current ? 'cm-hit-flash' : 'cm-hit-dim' }).range(
                h[0],
                h[1],
              ),
            ),
            true,
          )
        : Decoration.none,
    ),
})

/** 挂到编辑器扩展里（标记类名，样式由宿主组件提供以带主题变量） */
export const hitFlash = [hitFlashField]

let clearTimer: ReturnType<typeof setTimeout> | null = null

/**
 * 在编辑器里定位引文：精确匹配 → 空白容错匹配 → 失败回 false。
 * 成功时滚动到第 occurrence 处命中（取模循环，居中）、光标落在命中开头、
 * 全部命中淡色底纹、当前命中闪烁，约 2.4s 后自动消退。
 */
export function revealQuote(view: EditorView, quote: string, occurrence = 0): boolean {
  const text = view.state.doc.toString()
  const hits = quoteRanges(text, quote)
  if (hits.length === 0) return false
  const current = hits[occurrence % hits.length]

  view.dispatch({
    selection: { anchor: current[0] },
    effects: [
      flashHit.of({ hits, current }),
      EditorView.scrollIntoView(current[0], { y: 'center' }),
    ],
  })

  if (clearTimer) clearTimeout(clearTimer)
  clearTimer = setTimeout(() => {
    clearTimer = null
    try {
      view.dispatch({ effects: clearHit.of(null) })
    } catch {
      // 视图已销毁，无需清理
    }
  }, 2400)
  return true
}

/** 引文 → 首处命中范围（兼容旧用法；导出供测试） */
export function quoteRange(text: string, quote: string): [number, number] | null {
  return quoteRanges(text, quote)[0] ?? null
}

/**
 * 引文 → 全部命中范围（非重叠，按出现顺序）：先收集精确匹配；
 * 一处都没有再走空白容错（压缩掉所有空白后匹配，映射回原始偏移）。
 */
export function quoteRanges(text: string, quote: string): [number, number][] {
  // 空/纯空白引文无意义（精确匹配 indexOf 会命中 0 位）
  if (!quote.trim()) return []

  const exact: [number, number][] = []
  let at = text.indexOf(quote)
  while (at >= 0) {
    exact.push([at, at + quote.length])
    at = text.indexOf(quote, at + quote.length)
  }
  if (exact.length > 0) return exact

  // 空白容错：压缩掉所有空白后匹配，映射回原始偏移
  const isWs = (c: string) => /\s/.test(c)
  const chars: string[] = []
  const offsets: number[] = []
  for (let i = 0; i < text.length; i++) {
    if (!isWs(text[i])) {
      chars.push(text[i])
      offsets.push(i)
    }
  }
  const flat = chars.join('')
  const needle = [...quote].filter((c) => !isWs(c)).join('')
  if (!needle) return []

  const fuzzy: [number, number][] = []
  let hit = flat.indexOf(needle)
  while (hit >= 0) {
    fuzzy.push([offsets[hit], offsets[hit + needle.length - 1] + 1])
    hit = flat.indexOf(needle, hit + needle.length)
  }
  return fuzzy
}
