// AI 幽灵补全 — CodeMirror 6 行内补全扩展（类似 VSCode Copilot 的 inline suggestion）
//
// 输入停顿后异步请求补全，流式增量渲染为光标处的灰色幽灵文字；
// Tab 采纳、Esc 关闭、继续输入 / 光标移动自动消失。
// 任何文档 / 选区变化都会让补全锚点失效（陈旧响应一律丢弃）。
import {
  EditorView,
  Decoration,
  keymap,
  ViewPlugin,
  WidgetType,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view'
import { StateEffect, StateField, Transaction, type Extension } from '@codemirror/state'

/** 拉取一次补全：onDelta 推流式增量，resolve 完整文本 */
export type GhostCompletionFetch = (
  before: string,
  after: string,
  onDelta: (delta: string) => void,
) => Promise<string>

export interface AiGhostCompletionConfig {
  fetch: GhostCompletionFetch
  /** 是否启用（每次触发时求值；关闭时不请求） */
  enabled?: () => boolean
  /** 有请求在途时被顶掉 / 用户关闭幽灵时调用（用于取消后端请求） */
  onCancel?: () => void
  /** 触发防抖（毫秒），默认 600 */
  delay?: number
  /** 幽灵文字长度上限，默认 120 字 */
  maxLength?: number
}

/** 从 update 里取 userEvent 注解；混入无注解的事务视为程序性写入 */
function userEventOf(u: ViewUpdate): string | undefined {
  let event: string | undefined
  for (const tr of u.transactions) {
    const annotated = tr.annotation(Transaction.userEvent)
    if (annotated === undefined) return undefined
    event = annotated
  }
  return event
}

// ── 状态：幽灵文字锚点 + 内容 ─────────────────────────────

interface GhostState {
  pos: number
  text: string
}

const showGhost = StateEffect.define<GhostState>()
const clearGhost = StateEffect.define<null>()

const ghostField = StateField.define<GhostState | null>({
  create: () => null,
  update(value, tr) {
    for (const eff of tr.effects) {
      if (eff.is(showGhost)) return eff.value
      if (eff.is(clearGhost)) return null
    }
    // 文档或选区一动，幽灵锚点即失效（采纳走正常编辑链路，也被这里清掉）
    if (tr.docChanged || tr.selection) return null
    return value
  },
  provide: (field) =>
    EditorView.decorations.from(field, (value): DecorationSet =>
      value && value.text
        ? Decoration.set([
            Decoration.widget({
              widget: new GhostWidget(value.text),
              side: 1, // 渲染在光标之后
            }).range(value.pos),
          ])
        : Decoration.none,
    ),
})

class GhostWidget extends WidgetType {
  constructor(readonly text: string) {
    super()
  }

  eq(other: GhostWidget) {
    return other.text === this.text
  }

  toDOM() {
    const span = document.createElement('span')
    span.className = 'cm-ghost-text'
    span.textContent = this.text
    return span
  }

  /** 点击幽灵文字落到其后文本位置，不吞事件 */
  ignoreEvent() {
    return true
  }
}

const ghostTheme = EditorView.theme({
  '.cm-ghost-text': {
    color: 'color-mix(in srgb, var(--ink-main) 42%, transparent)',
    'user-select': 'none',
    'pointer-events': 'none',
  },
})

// ── 补全文本清洗：echo 去重 / 截断 / 句读收尾 ──────────────

/** 中途可收尾的标点（幽灵补全不必非到句号，逗号处收掉也自然） */
const CUT_PUNCT = ['。', '！', '？', '…', '；', '，', '：', '”', '"']

/**
 * 清洗模型输出为可展示的幽灵文字。
 * - 去掉前导空白；遇到换行即止（片段内不换行，换段是作者的事）
 * - 剥掉模型把光标前最后几个字重抄一遍的 echo（≥2 字才算 echo，避免误伤）
 * - 超长时优先在句读处收尾（覆盖上限四成以上才认），否则硬截
 */
export function sanitizeGhostText(raw: string, before: string, maxLength: number): string {
  let s = raw.replace(/^\s+/, '')
  const nl = s.indexOf('\n')
  if (nl >= 0) s = s.slice(0, nl)
  const maxEcho = Math.min(before.length, 24)
  for (let n = maxEcho; n >= 2; n--) {
    if (s.startsWith(before.slice(before.length - n))) {
      s = s.slice(n)
      break
    }
  }
  if (s.length > maxLength) {
    const head = s.slice(0, maxLength)
    let cut = -1
    for (const p of CUT_PUNCT) {
      const i = head.lastIndexOf(p)
      if (i > cut) cut = i
    }
    s = cut + 1 >= maxLength * 0.4 ? head.slice(0, cut + 1) : head
  }
  return s.trimEnd()
}

// ── 触发控制器：防抖 / 请求 / 陈旧丢弃 / 取消 ──────────────

/** 这些 userEvent 视为作者主动编辑，停顿后值得补全 */
function isAuthorEdit(userEvent: string | undefined): boolean {
  if (!userEvent) return false // 外部程序写入（切场景同步、版本恢复等）
  if (userEvent === 'input.complete') return false // 采纳补全本身不再触发
  if (userEvent.includes('compose')) return false // IME 组词中间态
  return userEvent.startsWith('input.') || userEvent.startsWith('delete.')
}

class GhostController {
  private timer: ReturnType<typeof setTimeout> | null = null
  /** 请求序号：每次触发 / 取消自增，旧响应据此作废 */
  private seq = 0
  private composing = false
  /** 视图已销毁（销毁后异步回调不得再 dispatch） */
  private alive = true
  /** 有请求在途（只在真有时才发取消 IPC，避免每个按键都打后端） */
  private inFlight = false

  constructor(
    private readonly view: EditorView,
    private readonly conf: AiGhostCompletionConfig,
  ) {
    view.dom.addEventListener('compositionstart', this.onCompositionStart)
    view.dom.addEventListener('compositionend', this.onCompositionEnd)
  }

  destroy() {
    this.cancelPending()
    this.alive = false
    this.view.dom.removeEventListener('compositionstart', this.onCompositionStart)
    this.view.dom.removeEventListener('compositionend', this.onCompositionEnd)
  }

  private onCompositionStart = () => {
    this.composing = true
  }

  private onCompositionEnd = () => {
    this.composing = false
    // 组词落定相当于一次输入，停顿后同样给补全（中文写作主要靠 IME）
    if (this.conf.enabled?.() !== false) this.schedule()
  }

  update(u: ViewUpdate) {
    if (!u.docChanged) return
    if (this.composing) return
    // 外部写入 / IME 收尾事务不主动触发；也不在这里取消挂起请求——
    // compositionend 后 CM 会补发带 compose 标记的事务，取消会误杀
    // 刚排定的补全。陈旧响应由 anchored() 校验丢弃，无泄漏风险。
    if (!isAuthorEdit(userEventOf(u))) return
    if (this.conf.enabled?.() === false) return
    this.schedule()
  }

  /** 停顿后触发（防抖窗口内的新输入会重置） */
  private schedule() {
    this.cancelPending()
    this.timer = setTimeout(() => {
      this.timer = null
      void this.request()
    }, this.conf.delay ?? 600)
  }

  /** 作废挂起的防抖与在途请求（含后端取消） */
  private cancelPending() {
    this.seq++
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
    if (this.inFlight) this.conf.onCancel?.()
  }

  /** Esc 关闭：清幽灵并作废在途请求 */
  dismiss() {
    this.cancelPending()
    if (this.alive && this.view.state.field(ghostField, false)) {
      this.view.dispatch({ effects: clearGhost.of(null) })
    }
  }

  /** Alt-\ 手动触发（不打字也要得到一条补全） */
  requestNow() {
    if (this.composing || this.conf.enabled?.() === false) return
    this.cancelPending()
    void this.request()
  }

  private async request() {
    const view = this.view
    const seq = ++this.seq
    if (!this.alive || this.conf.enabled?.() === false) return

    const sel = view.state.selection.main
    if (sel.anchor !== sel.head) return // 有选区不补
    const pos = sel.head
    const before = view.state.doc.sliceString(0, pos)
    const after = view.state.doc.sliceString(pos)
    const maxLength = this.conf.maxLength ?? 120

    this.inFlight = true
    let shown = ''
    try {
      const full = await this.conf.fetch(before, after, (delta) => {
        if (seq !== this.seq || !this.alive) return
        if (!this.anchored(pos, before)) {
          this.cancelPending()
          return
        }
        shown += delta
        this.show(pos, sanitizeGhostText(shown, before, maxLength))
      })
      if (seq !== this.seq || !this.alive) return
      if (!this.anchored(pos, before)) return
      this.show(pos, sanitizeGhostText(full, before, maxLength))
    } catch {
      // 补全失败保持安静——它是背景能力，不该弹窗打断写作
      if (seq === this.seq && this.alive) {
        this.view.dispatch({ effects: clearGhost.of(null) })
      }
    } finally {
      if (seq === this.seq) this.inFlight = false
    }
  }

  private show(pos: number, text: string) {
    this.view.dispatch({
      effects: text ? showGhost.of({ pos, text }) : clearGhost.of(null),
    })
  }

  /** 请求发出后文档前缀与光标都没动，响应才有效 */
  private anchored(pos: number, before: string): boolean {
    const sel = this.view.state.selection.main
    return (
      sel.head === pos &&
      sel.anchor === pos &&
      this.view.state.doc.sliceString(0, pos) === before
    )
  }
}

// ── 扩展入口 ───────────────────────────────────────────────

export function aiGhostCompletion(conf: AiGhostCompletionConfig): Extension {
  const ghostPlugin = ViewPlugin.fromClass(
    class extends GhostController {
      constructor(view: EditorView) {
        super(view, conf)
      }
    },
  )

  const ghostKeymap = keymap.of([
    {
      key: 'Tab',
      run: (view) => {
        const ghost = view.state.field(ghostField, false)
        if (!ghost || !ghost.text) return false
        view.dispatch({
          changes: { from: ghost.pos, insert: ghost.text },
          selection: { anchor: ghost.pos + ghost.text.length },
          scrollIntoView: true,
          userEvent: 'input.complete',
        })
        return true
      },
    },
    {
      key: 'Escape',
      run: (view) => {
        if (!view.state.field(ghostField, false)) return false
        view.plugin(ghostPlugin)?.dismiss()
        return true
      },
    },
    {
      key: 'Alt-\\',
      run: (view) => {
        view.plugin(ghostPlugin)?.requestNow()
        return true
      },
    },
  ])

  return [ghostField, ghostPlugin, ghostKeymap, ghostTheme]
}
