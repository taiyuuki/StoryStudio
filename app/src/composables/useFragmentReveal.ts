// 片段引文精确定位 — 报告对话框 → 目标 FragmentBlock 编辑器的跨组件总线
// + CodeMirror 命中高亮扩展
//
// 用法：requestFragmentReveal(fragmentId, quote) 发起定位；FragmentBlock
// watch pending，命中自己的 fragmentId 时调用 revealQuote 在编辑器里
// 滚动到引文处并高亮闪烁。
import { ref } from 'vue'
import { EditorView, Decoration, type DecorationSet } from '@codemirror/view'
import { StateEffect, StateField } from '@codemirror/state'

// ── 跨组件定位总线 ────────────────────────────────────────

export interface RevealRequest {
  seq: number
  fragmentId: string
  quote: string
}

const pending = ref<RevealRequest | null>(null)
let seq = 0

export function requestFragmentReveal(fragmentId: string, quote: string) {
  pending.value = { seq: ++seq, fragmentId, quote }
}

export function useFragmentReveal() {
  return { pending }
}

// ── CodeMirror 命中高亮扩展 ──────────────────────────────

const flashHit = StateEffect.define<{ from: number; to: number }>()
const clearHit = StateEffect.define<null>()

const hitFlashField = StateField.define<{ from: number; to: number } | null>({
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
        ? Decoration.set([Decoration.mark({ class: 'cm-hit-flash' }).range(value.from, value.to)])
        : Decoration.none,
    ),
})

/** 挂到编辑器扩展里（标记类名，样式由宿主组件提供以带主题变量） */
export const hitFlash = [hitFlashField]

let clearTimer: ReturnType<typeof setTimeout> | null = null

/**
 * 在编辑器里定位引文：精确匹配 → 空白容错匹配 → 失败回 false。
 * 成功时滚动到命中处（居中）、光标落在命中开头、高亮约 2.4s 后自动消退。
 */
export function revealQuote(view: EditorView, quote: string): boolean {
  const text = view.state.doc.toString()
  const range = quoteRange(text, quote)
  if (!range) return false

  view.dispatch({
    selection: { anchor: range[0] },
    effects: [
      flashHit.of({ from: range[0], to: range[1] }),
      EditorView.scrollIntoView(range[0], { y: 'center' }),
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

/** 引文 → 编辑器文本范围：先精确匹配，再空白容错匹配 */
function quoteRange(text: string, quote: string): [number, number] | null {
  const exact = text.indexOf(quote)
  if (exact >= 0) return [exact, exact + quote.length]

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
  const needle = [...quote].filter((c) => !isWs(c)).join('')
  if (!needle) return null
  const at = chars.join('').indexOf(needle)
  if (at < 0) return null
  const from = offsets[at]
  const to = offsets[at + needle.length - 1] + 1
  return [from, to]
}
