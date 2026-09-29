// CodeMirror 6 实例管理 — 水墨纸面主题 + 变更回调
import { onBeforeUnmount, shallowRef, watch, type Ref } from 'vue'
import { EditorView, keymap } from '@codemirror/view'
import { EditorState } from '@codemirror/state'
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { markdown } from '@codemirror/lang-markdown'

/** 水墨纸面主题 — 衬线、大行距、段距、暖纸底 */
const inkTheme = EditorView.theme({
  '&': {
    color: 'var(--ink-main)',
    backgroundColor: 'transparent',
    fontSize: '17px',
    height: 'auto',
  },
  '.cm-scroller': {
    fontFamily:
      '"Noto Serif SC", "Source Han Serif SC", "Songti SC", "STSong", "SimSun", serif',
    lineHeight: '2',
    padding: '2px 0',
  },
  '.cm-content': {
    caretColor: 'var(--caret-color)',
    padding: '4px 0 28px',
  },
  '.cm-line': {
    padding: '0 2px',
  },
  // 段落间空行弱化（prose 节奏）
  '.cm-line:has(+ .cm-line:empty)': {
    marginBottom: '0.45em',
  },
  '.cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--selection-bg-color) !important',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-cursor': {
    borderLeftColor: 'var(--caret-color)',
    borderLeftWidth: '2px',
  },
})

interface UseCodeMirrorOptions {
  doc: Ref<string>
  /** 本地输入回调（外部写入不触发） */
  onChange: (text: string) => void
  placeholder?: string
  readonly?: boolean
}

export function useCodeMirror(options: UseCodeMirrorOptions) {
  const container = shallowRef<HTMLElement | null>(null)
  const view = shallowRef<EditorView | null>(null)
  let applyingExternal = false
  // IME 组词保护：拼音组合期间 doc 已含中间态，但不触发 onChange
  // （避免把半截拼音落盘 / 计入版本历史），compositionend 后补发最终文本
  let composing = false
  let lastEmitted = ''

  function create(parent: HTMLElement) {
    const placeholderExt = options.placeholder
      ? EditorView.contentAttributes.of({ 'data-placeholder': options.placeholder })
      : []

    const state = EditorState.create({
      doc: options.doc.value,
      extensions: [
        history(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        markdown(),
        inkTheme,
        placeholderExt,
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged && !applyingExternal) {
            const text = update.state.doc.toString()
            if (composing) return // 组词中间态不外发
            lastEmitted = text
            options.onChange(text)
          }
        }),
        EditorState.readOnly.of(options.readonly ?? false),
      ],
    })
    const editor = new EditorView({ state, parent })
    view.value = editor
    lastEmitted = options.doc.value

    editor.dom.addEventListener('compositionstart', () => {
      composing = true
    })
    editor.dom.addEventListener('compositionend', () => {
      composing = false
      // 组词结束：若最终文本与上次外发的不同，补发一次
      const text = editor.state.doc.toString()
      if (text !== lastEmitted) {
        lastEmitted = text
        options.onChange(text)
      }
    })
    return editor
  }

  watch(
    () => options.doc.value,
    (next) => {
      const v = view.value
      if (!v) return
      const current = v.state.doc.toString()
      if (next === current) return
      applyingExternal = true
      v.dispatch({
        changes: { from: 0, to: current.length, insert: next },
        selection: { anchor: Math.min(v.state.selection.main.anchor, next.length) },
      })
      applyingExternal = false
      lastEmitted = next
    },
  )

  onBeforeUnmount(() => {
    view.value?.destroy()
    view.value = null
  })

  return { container, view, create }
}

export { EditorView }
