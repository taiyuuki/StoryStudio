// 妙笔会话状态 — 全局 store（不随面板组件销毁而丢失）
//
// 时间线 / running / sessionId 原先放在 AiPanel 组件里，靠 RightDock 的
// KeepAlive 缓存组件保状态：面板之间切换确实保得住，但关闭右栏时整个
// KeepAlive 会随外层 v-if 一起卸载、缓存清空，AiPanel 被销毁重建——
// running 归零、sessionId 归零，而后台 agent_chat 还在跑；此时再发消息
// 会 ensureSession 另开一个新会话，造成两个 AI 会话并行。
// 状态搬进 store 后与组件生命周期解耦：面板无论怎么切 / 关 / 开，
// 按钮态、时间线与会话绑定都从 store 原样恢复。
import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { api, type AgentEventDto, type ChatMetaDto } from '@/api/backend'
import { useWorkspaceStore } from './workspace'

export interface TimelineItem {
  key: number
  type: 'user' | 'ai' | 'reasoning' | 'tool' | 'ask' | 'error' | 'status' | 'thinking'
  text: string
  streaming?: boolean
  tool?: string
  toolResult?: string
  /** 工具未返回结果即中断（错误/取消），图标显示为失败 */
  toolFailed?: boolean
  toolOpen?: boolean
  options?: string[]
  answered?: boolean
}

// ── 妙笔改动类工具 → 刷新工作台（与 tools/agent.rs 注册的工具对应） ──
const MUTATION_PREFIXES = [
  'create_', 'write_', 'edit_', 'append_', 'rewrite_', 'delete_', 'update_',
  'add_', 'remove_', 'set_', 'move_', 'merge_', 'split_', 'advance_',
  'insert_', 'design_', 'anchor_', 'save_', 'resolve_', 'retire_',
]
const FRAGMENT_TOOLS = new Set([
  'write_fragment', 'edit_fragment', 'append_fragment', 'rewrite_fragment',
  'delete_fragment', 'split_fragment', 'split_fragments', 'merge_fragments',
  'move_fragment', 'set_fragment_anchor', 'remove_fragment_anchor',
  'anchor_fragments',
])

export const useMiaoStore = defineStore('miao', () => {
  const store = useWorkspaceStore()

  // ── 时间线与运行态 ──────────────────────────────────────
  const items = ref<TimelineItem[]>([])
  const running = ref(false)
  const input = ref('')
  let seq = 0
  /** 当前待回答的提问 key（必须是响应式：answerMode 依赖它） */
  const pendingAskKey = ref<number | null>(null)

  const answerMode = computed(() => pendingAskKey.value !== null)

  function push(item: Omit<TimelineItem, 'key'>): TimelineItem {
    // 新项插入即终结上一段 AI 回复的流式状态（光标消失）
    const prev = items.value[items.value.length - 1]
    if (prev?.type === 'ai') prev.streaming = false
    const full = { key: ++seq, ...item }
    items.value.push(full)
    return full
  }

  function replaceStatus(text: string) {
    // 瞬态状态（进度 / 重试）只保留最后一条
    const last = items.value[items.value.length - 1]
    if (last?.type === 'status') last.text = text
    else push({ type: 'status', text })
  }

  function showThinking() {
    clearThinking()
    push({ type: 'thinking', text: '妙笔正在思考' })
  }

  function clearThinking() {
    items.value = items.value.filter((it) => it.type !== 'thinking')
  }

  /** 本轮中断（错误/取消）时，把仍处于"进行中"的工具项收尾为失败态 */
  function sweepFailedTools() {
    for (const it of items.value) {
      if (it.type === 'tool' && it.toolResult === undefined) {
        it.toolResult = '（未返回结果——本轮已中断）'
        it.toolFailed = true
      }
    }
  }

  let refreshTimer: ReturnType<typeof setTimeout> | null = null
  /** 防抖窗口内挂起的重载标志：只累积不清——后续非片段类工具不能把片段工具排定的重载冲掉 */
  let pendingSceneReload = false
  function scheduleWorkbenchRefresh(reloadScene: boolean) {
    pendingSceneReload = pendingSceneReload || reloadScene
    if (refreshTimer) clearTimeout(refreshTimer)
    refreshTimer = setTimeout(() => {
      refreshTimer = null
      const reload = pendingSceneReload
      pendingSceneReload = false
      void store.refresh().then(() => {
        if (!reload || !store.scene) return
        // 用户正在编辑当前场景时不重载（会把编辑器内容覆盖回磁盘版）；
        // 改动仍体现在结构树与字数里，停笔后的下一次刷新即可见
        const busy = Object.values(store.fragmentSave).some(
          (s) => s === 'dirty' || s === 'saving',
        )
        if (busy) return
        void store.selectScene(store.scene.id)
      })
    }, 600)
  }

  // ── 会话历史 ────────────────────────────────────────────
  const sessions = ref<ChatMetaDto[]>([])
  const sessionId = ref('')
  let sessionTitle = ''
  /** 会话标题预设：外部（如审校送改）在发起前设置，建会话时优先采用 */
  let pendingTitle: string | null = null
  /** 后端是否保有本会话记忆（进程内）；从磁盘恢复的会话为 false，首条消息需带上下文重建 */
  let backendAlive = false

  let persistTimer: ReturnType<typeof setTimeout> | null = null
  function persist() {
    if (!sessionId.value || !store.root) return
    if (persistTimer) clearTimeout(persistTimer)
    persistTimer = setTimeout(() => {
      persistTimer = null
      if (!sessionId.value || !store.root) return
      const persistable = items.value
        .filter((it) => it.type !== 'thinking' && it.type !== 'status')
        .map((it) => ({
          type: it.type,
          text: it.text,
          tool: it.tool,
          toolResult: it.toolResult,
          options: it.options,
          answered: it.answered,
        }))
      void api.chatSave(store.root, {
        id: sessionId.value,
        title: sessionTitle,
        updatedAt: Date.now(),
        items: persistable as unknown as Record<string, unknown>[],
      })
      void loadSessions()
    }, 800)
  }

  async function loadSessions() {
    if (!store.root) {
      sessions.value = []
      return
    }
    try {
      sessions.value = await api.chatList(store.root)
    } catch {
      sessions.value = []
    }
  }

  /** 预设下一个新会话的标题（如「AI 审校改进」），避免拿长指令前缀当标题 */
  function presetSessionTitle(title: string) {
    pendingTitle = title
  }

  function ensureSession(firstMessage: string) {
    if (sessionId.value) {
      pendingTitle = null // 已有会话：预设作废，避免泄漏到之后的会话
      return
    }
    sessionId.value = crypto.randomUUID()
    sessionTitle = pendingTitle ?? (firstMessage.trim().slice(0, 24) || '新的话题')
    pendingTitle = null
  }

  /** 恢复的会话首条消息：把之前的对话记录带给后端重建上下文 */
  function reseedMessage(message: string): string {
    const turns = items.value
      .filter((it) => it.type === 'user' || it.type === 'ai')
      .map((it) => (it.type === 'user' ? `用户：${it.text}` : `妙笔：${it.text}`))
    const history = turns.join('\n').slice(-6000)
    return [
      '（会话恢复）以下是我们之前的对话记录，请据此恢复上下文，不要重复已完成的操作：',
      '……' + history,
      `（用户继续说）${message}`,
    ].join('\n')
  }

  function resetConversation() {
    if (sessionId.value) void api.agentDropSession(sessionId.value)
    sessionId.value = ''
    sessionTitle = ''
    backendAlive = false
    items.value = []
    pendingAskKey.value = null
    running.value = false
  }

  async function resume(meta: ChatMetaDto) {
    if (!store.root) return
    if (running.value) return
    persist()
    try {
      const chat = await api.chatLoad(store.root, meta.id)
      items.value = (chat.items as unknown as Omit<TimelineItem, 'key'>[]).map((it) => ({
        ...it,
        key: ++seq,
        streaming: false,
        toolOpen: false,
      }))
      sessionId.value = chat.id
      sessionTitle = chat.title
      backendAlive = false
    } catch (err) {
      items.value = [{ key: ++seq, type: 'error', text: (err as Error).message }]
    }
  }

  async function removeSession(meta: ChatMetaDto) {
    await api.chatDelete(store.root!, meta.id)
    if (meta.id === sessionId.value) resetConversation()
    await loadSessions()
  }

  function newChat() {
    if (running.value) return
    persist()
    resetConversation()
  }

  // ── 事件消费（Agent 流式事件 → 时间线） ─────────────────
  function onEvent(ev: AgentEventDto) {
    const last = items.value[items.value.length - 1]

    switch (ev.kind) {
      case 'ai': {
        // 增量：续接到最后一条流式回复。
        // 回合结束时 agent 会再推送一次完整全文（记入轨迹用），
        // 表现为"以已累积内容为前缀的更长文本"——此时替换而非追加。
        if (last?.type === 'ai' && last.streaming) {
          if (ev.text.length >= last.text.length && ev.text.startsWith(last.text)) {
            last.text = ev.text
          } else {
            last.text += ev.text
          }
        } else {
          push({ type: 'ai', text: ev.text, streaming: true })
        }
        clearThinking()
        persist()
        break
      }
      case 'toolCall': {
        push({ type: 'tool', text: '', tool: ev.tool ?? undefined })
        persist()
        break
      }
      case 'toolResult': {
        // 回填最近一个未完成的同名工具项
        for (let i = items.value.length - 1; i >= 0; i--) {
          const it = items.value[i]
          if (it.type === 'tool' && it.tool === ev.tool && it.toolResult === undefined) {
            it.toolResult = ev.detail ?? ''
            break
          }
        }
        // 改动类工具完成 → 刷新工作台（结构树/字数/编辑器）
        const tool = ev.tool ?? ''
        if (MUTATION_PREFIXES.some((p) => tool.startsWith(p))) {
          scheduleWorkbenchRefresh(FRAGMENT_TOOLS.has(tool))
        }
        // 工具执行完 → 下一轮思考
        showThinking()
        persist()
        break
      }
      case 'reasoning': {
        if (last?.type === 'reasoning') last.text += ev.text
        else push({ type: 'reasoning', text: ev.text })
        break
      }
      case 'ask': {
        clearThinking()
        const item = push({ type: 'ask', text: ev.text, options: ev.options ?? [], answered: false })
        pendingAskKey.value = item.key
        persist()
        break
      }
      case 'planProgress':
        replaceStatus(
          ev.extra
            ? `正在执行 ${(ev.extra as { current?: number; total?: number }).current}/${(ev.extra as { current?: number; total?: number }).total}：${ev.text}`
            : ev.text,
        )
        break
      case 'retrying':
        replaceStatus(`网络波动，正在重试（第 ${(ev.extra as { attempt?: number })?.attempt ?? '?'} 次）…`)
        break
      case 'compacted':
        replaceStatus('对话较长，已自动整理上下文')
        break
      case 'error':
        clearThinking()
        sweepFailedTools()
        push({ type: 'error', text: ev.text })
        persist()
        break
    }
  }

  // ── 发送 ────────────────────────────────────────────────
  async function send() {
    const text = input.value.trim()
    if (!text || !store.root) return
    // 进行中不允许发起新消息；回答 Agent 提问除外（那正是它在等的输入）
    if (running.value && !answerMode.value) return
    input.value = ''

    if (answerMode.value) {
      // 回答 Agent 的提问
      const askItem = items.value.find((it) => it.key === pendingAskKey.value)
      if (askItem) askItem.answered = true
      pendingAskKey.value = null
      push({ type: 'user', text })
      showThinking()
      await api.agentRespond(sessionId.value, text)
      persist()
      return
    }

    ensureSession(text)
    push({ type: 'user', text })
    showThinking()

    // 恢复的会话：后端无记忆，首条消息携带历史记录重建上下文
    const outbound = backendAlive ? text : reseedMessage(text)
    backendAlive = true
    persist()

    running.value = true
    try {
      await api.agentChat(store.root, sessionId.value, outbound, onEvent)
    } catch (err) {
      push({ type: 'error', text: (err as Error).message })
    } finally {
      running.value = false
      clearThinking()
      sweepFailedTools()
      const last = items.value[items.value.length - 1]
      if (last?.type === 'ai') last.streaming = false
      pendingAskKey.value = null
      persist()
      // AI 会话改动自动存为 git 快照（无变更 / 未启用版本档案时静默跳过）
      void api
        .gitCommit(store.root, `妙笔改动：${text.slice(0, 30)}`)
        .catch(() => {})
    }
  }

  async function pickOption(option: string) {
    // 提问选项恰恰是 running 中等待回答的场景，只看 answerMode
    if (!answerMode.value) return
    input.value = option
    await send()
  }

  async function stop() {
    if (!sessionId.value) return
    pendingAskKey.value = null
    await api.agentCancel(sessionId.value)
  }

  // 换项目 → 清空（会话历史随项目存取）
  watch(
    () => store.root,
    () => {
      resetConversation()
      void loadSessions()
    },
  )

  return {
    items,
    running,
    input,
    sessionId,
    sessions,
    answerMode,
    send,
    pickOption,
    stop,
    resume,
    removeSession,
    newChat,
    loadSessions,
    presetSessionTitle,
  }
})
