<script setup lang="ts">
import { computed, nextTick, onActivated, onMounted, ref, useTemplateRef, watch } from 'vue'
import { api, type AgentEventDto, type ChatMetaDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

// 显式命名：RightDock 的 <KeepAlive include="AiPanel"> 靠它缓存本面板，
// AI 会话进行中切走再回来不丢状态
defineOptions({ name: 'AiPanel' })

// ── 消息时间线 ────────────────────────────────────────────
interface TimelineItem {
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

const items = ref<TimelineItem[]>([])
const running = ref(false)
const input = ref('')
let seq = 0
/** 当前待回答的提问 key */
let pendingAskKey: number | null = null

const listRef = useTemplateRef<HTMLElement>('list')

async function scrollToBottom() {
  await nextTick()
  const el = listRef.value
  if (el) el.scrollTop = el.scrollHeight
}

function push(item: Omit<TimelineItem, 'key'>): TimelineItem {
  // 新项插入即终结上一段 AI 回复的流式状态（光标消失）
  const prev = items.value[items.value.length - 1]
  if (prev?.type === 'ai') prev.streaming = false
  const full = { key: ++seq, ...item }
  items.value.push(full)
  void scrollToBottom()
  return full
}

function replaceStatus(text: string) {
  // 瞬态状态（进度 / 重试）只保留最后一条
  const last = items.value[items.value.length - 1]
  if (last?.type === 'status') last.text = text
  else push({ type: 'status', text })
}

// ── 思考状态 ──────────────────────────────────────────────
function showThinking() {
  clearThinking()
  push({ type: 'thinking', text: '妙笔正在思考' })
}

function clearThinking() {
  items.value = items.value.filter((it) => it.type !== 'thinking')
}

// ── 工具名 → 用户可读标签（与 tools/agent.rs 注册的工具一一对应） ──
const toolLabels: Record<string, string> = {
  // 实体 / 设定
  create_character: '创建角色',
  create_location: '创建地点',
  create_item: '创建道具',
  create_organization: '创建组织',
  search_entities: '查找角色设定',
  get_entity_detail: '查看角色档案',
  update_entity: '更新设定档案',
  delete_entity: '删除设定',
  merge_entities: '合并设定',
  profile_check: '核对档案一致性',
  list_unresolved_refs: '查看未锚定指代',
  resolve_unresolved_ref: '处置未锚定指代',
  // 结构（章 / 场景）
  get_structure: '查看全书结构',
  create_chapter: '新建章节',
  update_chapter: '调整章节',
  delete_chapter: '删除章节',
  create_scene: '新建场景',
  update_scene: '调整场景',
  delete_scene: '删除场景',
  save_scene_summaries: '批量保存摘要',
  repair_project: '修复数据',
  // 正文（片段）
  write_fragment_text: '撰写正文',
  edit_fragment_text: '修改正文',
  read_fragment_text: '读取正文',
  append_fragment_text: '续写正文',
  rewrite_fragment: '重写正文',
  delete_fragment: '删除正文',
  split_fragment: '拆分段落',
  merge_fragments: '合并段落',
  move_fragment: '调整段落顺序',
  set_fragment_anchor: '锚定实体',
  remove_fragment_anchor: '移除锚点',
  anchor_fragments: '锚定出场',
  // 事实 / 关系
  add_assertion: '记录事实',
  retire_assertion: '废弃事实',
  delete_assertion: '删除事实',
  query_assertions: '查询事实',
  add_relation: '建立关系',
  remove_relation: '移除关系',
  // 叙事线索
  create_thread: '新建线索',
  get_threads: '查看叙事线索',
  get_thread_detail: '查看线索详情',
  update_thread: '更新线索',
  advance_thread: '推进线索',
  delete_thread: '删除线索',
  set_thread_knowledge: '更新线索知情状态',
  // 查询 / 上下文
  get_story_recap: '回顾故事',
  get_relations: '查看关系网',
  get_character_state: '查看角色近况',
  find_mentions: '查找出场位置',
  search_text: '全文检索',
  get_project_stats: '统计作品数据',
  get_scene_context: '查看场景上下文',
  // 大纲 / 章节脉络 / 世界观
  get_outline: '查看大纲',
  set_outline_item: '撰写大纲',
  update_outline_item: '更新大纲',
  delete_outline_item: '删除大纲条目',
  design_chapters: '规划章节',
  get_chapter_outline: '查看章节脉络',
  update_chapter_outline: '更新章节脉络',
  delete_chapter_outline: '删除章节脉络',
  get_worldview: '查看世界观设定',
  set_worldview_item: '撰写世界观',
  update_worldview_item: '更新世界观',
  delete_worldview_item: '删除世界观条目',
  // 检测
  ai_check_fragment: '审校段落',
  ai_check_chapter: '整章审校',
  ai_check_scene: '整场景审校',
  compile_check: '一致性检查',
  // 其他
  get_writing_guide: '查阅写作指南',
  ask_user: '向你提问',
}

function toolLabel(name: string | null | undefined): string {
  if (!name) return '正在操作'
  return toolLabels[name] ?? name.replace(/_/g, ' ')
}

// ── 妙笔改动 → 刷新工作台 ────────────────────────────────
const MUTATION_PREFIXES = [
  'create_', 'write_', 'edit_', 'append_', 'rewrite_', 'delete_', 'update_',
  'add_', 'remove_', 'set_', 'move_', 'merge_', 'split_', 'advance_',
  'insert_', 'design_',
]
const FRAGMENT_TOOLS = new Set([
  'write_fragment', 'edit_fragment', 'append_fragment', 'rewrite_fragment',
  'delete_fragment', 'split_fragment', 'split_fragments', 'merge_fragments',
  'move_fragment',
])

let refreshTimer: ReturnType<typeof setTimeout> | null = null
function scheduleWorkbenchRefresh(reloadScene: boolean) {
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => {
    refreshTimer = null
    void store.refresh()
    if (reloadScene && store.scene) void store.selectScene(store.scene.id)
  }, 600)
}

// ── 会话历史 ──────────────────────────────────────────────
const sessions = ref<ChatMetaDto[]>([])
const historyOpen = ref(false)
/** 后端是否保有本会话记忆（进程内）；从磁盘恢复的会话为 false，首条消息需带上下文重建 */
let backendAlive = false
let sessionId = ''
let sessionTitle = ''

let persistTimer: ReturnType<typeof setTimeout> | null = null
function persist() {
  if (!sessionId || !store.root) return
  if (persistTimer) clearTimeout(persistTimer)
  persistTimer = setTimeout(() => {
    persistTimer = null
    if (!sessionId || !store.root) return
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
      id: sessionId,
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

function ensureSession(firstMessage: string) {
  if (sessionId) return
  sessionId = crypto.randomUUID()
  sessionTitle = firstMessage.trim().slice(0, 24) || '新的话题'
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
  if (sessionId) void api.agentDropSession(sessionId)
  sessionId = ''
  sessionTitle = ''
  backendAlive = false
  items.value = []
  pendingAskKey = null
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
    sessionId = chat.id
    sessionTitle = chat.title
    backendAlive = false
    historyOpen.value = false
    void scrollToBottom()
  } catch (err) {
    items.value = [{ key: ++seq, type: 'error', text: (err as Error).message }]
  }
}

async function removeSession(meta: ChatMetaDto) {
  await api.chatDelete(store.root!, meta.id)
  if (meta.id === sessionId) resetConversation()
  await loadSessions()
}

function newChat() {
  if (running.value) return
  persist()
  resetConversation()
  historyOpen.value = false
}

function fmtSessionTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getMonth() + 1}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

// ── 事件消费 ──────────────────────────────────────────────
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
      void scrollToBottom()
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
      pendingAskKey = item.key
      persist()
      break
    }
    case 'planProgress':
      replaceStatus(
        ev.extra ? `正在执行 ${(ev.extra as { current?: number; total?: number }).current}/${(ev.extra as { current?: number; total?: number }).total}：${ev.text}` : ev.text,
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

/** 本轮中断（错误/取消）时，把仍处于"进行中"的工具项收尾为失败态，
 * 避免转圈图标永久停留 */
function sweepFailedTools() {
  for (const it of items.value) {
    if (it.type === 'tool' && it.toolResult === undefined) {
      it.toolResult = '（未返回结果——本轮已中断）'
      it.toolFailed = true
    }
  }
}

// ── 发送 ──────────────────────────────────────────────────
const answerMode = computed(() => pendingAskKey !== null)

async function send() {
  const text = input.value.trim()
  if (!text || running.value || !store.root) return
  input.value = ''

  if (answerMode.value) {
    // 回答 Agent 的提问
    const askItem = items.value.find((it) => it.key === pendingAskKey)
    if (askItem) askItem.answered = true
    pendingAskKey = null
    push({ type: 'user', text })
    showThinking()
    await api.agentRespond(sessionId, text)
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
    await api.agentChat(store.root, sessionId, outbound, onEvent)
  } catch (err) {
    push({ type: 'error', text: (err as Error).message })
  } finally {
    running.value = false
    clearThinking()
    sweepFailedTools()
    const last = items.value[items.value.length - 1]
    if (last?.type === 'ai') last.streaming = false
    pendingAskKey = null
    persist()
  }
}

async function pickOption(option: string) {
  if (!answerMode.value || running.value) return
  input.value = option
  await send()
}

async function stop() {
  if (!sessionId) return
  pendingAskKey = null
  await api.agentCancel(sessionId)
}

// 换项目 → 清空（会话历史随项目存取）
watch(
  () => store.root,
  () => {
    if (sessionId) void api.agentDropSession(sessionId)
    resetConversation()
    historyOpen.value = false
    void loadSessions()
  },
)

onMounted(() => void loadSessions())

// 从缓存恢复显示（切走再切回）：停用期间流式内容还在增长，回到底部
onActivated(() => void scrollToBottom())
</script>

<template>
  <div class="miao-panel">
    <div ref="list" class="miao-list">
      <div v-if="items.length === 0" class="miao-hero">
        <div class="hero-icon"><i class="fa-solid fa-feather-pointed" /></div>
        <h3>妙笔</h3>
        <p>你的 AI 创作搭档，了解这本书的全部设定。<br />让它续写、查设定、埋伏笔，或只是聊聊剧情。</p>
        <div class="hero-tries">
          <button @click="input = '帮我梳理一下目前的故事线和伏笔'">梳理伏笔</button>
          <button @click="input = '以当前视角续写一段，注意保持文风'">续写一段</button>
          <button @click="input = '给下个场景出三个方向'">出主意</button>
        </div>

        <!-- 历史话题 -->
        <div v-if="sessions.length > 0" class="hero-history">
          <div class="hh-title">继续之前的话题</div>
          <button v-for="s in sessions.slice(0, 5)" :key="s.id" class="hh-item" @click="resume(s)">
            <span class="hh-name">{{ s.title }}</span>
            <span class="hh-time num">{{ fmtSessionTime(s.updatedAt) }}</span>
          </button>
        </div>
      </div>

      <template v-for="it in items" :key="it.key">
        <!-- 用户 -->
        <div v-if="it.type === 'user'" class="msg-user">
          <div class="bubble">{{ it.text }}</div>
        </div>

        <!-- AI 回复 -->
        <div v-else-if="it.type === 'ai'" class="msg-ai">
          <div class="ai-avatar"><i class="fa-solid fa-feather-pointed" /></div>
          <div class="ai-body">
            <span class="ai-text">{{ it.text }}</span>
            <span v-if="it.streaming" class="ai-caret" />
          </div>
        </div>

        <!-- 思考中 -->
        <div v-else-if="it.type === 'thinking'" class="msg-thinking">
          <span class="dots"><i /><i /><i /></span>
          {{ it.text }}
        </div>

        <!-- 思考过程（折叠） -->
        <details v-else-if="it.type === 'reasoning'" class="msg-reasoning">
          <summary><i class="fa-regular fa-lightbulb" /> 想了想</summary>
          <p>{{ it.text }}</p>
        </details>

        <!-- 工具活动 -->
        <details v-else-if="it.type === 'tool'" class="msg-tool" :open="it.toolOpen">
          <summary @click.prevent="it.toolOpen = !it.toolOpen">
            <i
              class="fa-solid"
              :class="
                it.toolFailed
                  ? 'fa-circle-exclamation'
                  : it.toolResult === undefined
                    ? 'fa-spinner fa-spin'
                    : 'fa-circle-check'
              "
              :style="
                it.toolFailed
                  ? 'color: var(--state-danger)'
                  : it.toolResult === undefined
                    ? ''
                    : 'color: var(--state-success)'
              "
            />
            <span class="tool-name">{{ toolLabel(it.tool) }}</span>
            <i class="fa-solid chev" :class="it.toolOpen ? 'fa-angle-up' : 'fa-angle-down'" />
          </summary>
          <pre v-if="it.toolOpen" class="tool-detail">{{ it.toolResult ?? '进行中…' }}</pre>
        </details>

        <!-- 提问卡 -->
        <div v-else-if="it.type === 'ask'" class="msg-ask" :class="{ 'is-answered': it.answered }">
          <div class="ask-q"><i class="fa-regular fa-circle-question" /> {{ it.text }}</div>
          <div v-if="!it.answered && it.options?.length" class="ask-opts">
            <button v-for="opt in it.options" :key="opt" :disabled="running && !answerMode" @click="pickOption(opt)">
              {{ opt }}
            </button>
          </div>
          <div v-else-if="it.answered" class="ask-done">已回答</div>
        </div>

        <!-- 错误 -->
        <div v-else-if="it.type === 'error'" class="msg-error">
          <i class="fa-solid fa-circle-exclamation" />
          <span>{{ it.text }}</span>
        </div>

        <!-- 瞬态状态 -->
        <div v-else class="msg-status">
          <i class="fa-solid fa-circle-notch fa-spin" />
          {{ it.text }}
        </div>
      </template>
    </div>

    <!-- 历史列表浮层 -->
    <Transition name="hist">
      <div v-if="historyOpen" class="miao-history">
        <div class="mh-head">
          <span>历史话题（{{ sessions.length }}）</span>
          <button class="ink-icon-btn" @click="historyOpen = false">
            <i class="fa-solid fa-xmark" />
          </button>
        </div>
        <div class="mh-list">
          <div v-if="sessions.length === 0" class="mh-empty">还没有历史话题</div>
          <div
            v-for="s in sessions"
            :key="s.id"
            class="mh-item"
            :class="{ 'is-active': s.id === sessionId }"
            @click="resume(s)"
          >
            <div class="mh-main">
              <div class="mh-name">{{ s.title }}</div>
              <div class="mh-time num">{{ fmtSessionTime(s.updatedAt) }}</div>
            </div>
            <button class="mh-del" title="删除" @click.stop="removeSession(s)">
              <i class="fa-regular fa-trash-can" />
            </button>
          </div>
        </div>
      </div>
    </Transition>

    <div class="miao-input">
      <div v-if="answerMode" class="input-hint">
        <i class="fa-regular fa-circle-question" />
        妙笔在等你回答，回复后它会继续
      </div>
      <div class="input-row">
        <textarea
          v-model="input"
          :placeholder="answerMode ? '输入你的回答…' : '让妙笔做什么？'"
          rows="2"
          :disabled="running && !answerMode"
          @keydown.enter.exact.prevent="send"
        />
        <button v-if="running && !answerMode" class="ink-icon-btn miao-stop" title="停止" @click="stop">
          <i class="fa-solid fa-stop" />
        </button>
        <button
          v-else
          class="ink-btn ink-btn--primary ink-btn--sm miao-send"
          :disabled="running || !input.trim()"
          @click="send"
        >
          <i class="fa-solid fa-paper-plane" />
        </button>
      </div>
      <div class="input-foot">
        <button class="foot-btn" title="历史话题" @click="((historyOpen = !historyOpen), void loadSessions())">
          <i class="fa-solid fa-clock-rotate-left" />
          历史
        </button>
        <button v-if="items.length > 0" class="foot-btn" @click="newChat">
          <i class="fa-regular fa-plus" />
          新话题
        </button>
        <span style="flex: 1" />
        <span class="text-ink-muted">Enter 发送</span>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.miao-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  margin: -14px -16px -20px;
  position: relative;
}

// ── 消息列表 ──────────────────────────────────────────────
.miao-list {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.miao-hero {
  margin: auto;
  text-align: center;
  padding: 24px 8px;
  width: 100%;

  .hero-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 52px;
    height: 52px;
    border-radius: 14px;
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
    font-size: 22px;
    margin-bottom: 12px;
  }

  h3 {
    font-size: 17px;
    color: var(--ink-main);
    margin-bottom: 8px;
  }

  p {
    font-size: 12.5px;
    line-height: 2;
    color: var(--ink-muted);
  }

  .hero-tries {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 16px;

    button {
      padding: 8px 12px;
      border-radius: 9px;
      border: 1px solid var(--divider);
      background: var(--surface-1);
      color: var(--ink-sec);
      font-family: inherit;
      font-size: 12.5px;
      cursor: pointer;
      transition: all 0.2s;

      &:hover {
        border-color: var(--ink-accent);
        color: var(--ink-accent);
        background: var(--ink-accent-soft);
      }
    }
  }

  .hero-history {
    margin-top: 22px;
    text-align: left;

    .hh-title {
      font-size: 11.5px;
      color: var(--ink-muted);
      letter-spacing: 0.1em;
      margin-bottom: 8px;
      padding: 0 2px;
    }

    .hh-item {
      display: flex;
      align-items: center;
      gap: 8px;
      width: 100%;
      padding: 8px 10px;
      margin-bottom: 4px;
      border: none;
      border-radius: 9px;
      background: transparent;
      color: var(--ink-sec);
      font-family: inherit;
      font-size: 12.5px;
      cursor: pointer;
      transition: background 0.2s;

      &:hover {
        background: var(--surface-2);

        .hh-name {
          color: var(--ink-main);
        }
      }

      .hh-name {
        flex: 1;
        text-align: left;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }

      .hh-time {
        font-size: 10.5px;
        color: var(--ink-disabled);
        flex-shrink: 0;
      }
    }
  }
}

.msg-user {
  align-self: flex-end;
  max-width: 85%;

  .bubble {
    padding: 9px 13px;
    border-radius: 14px 14px 3px 14px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-color);
    font-size: 13px;
    line-height: 1.7;
    white-space: pre-wrap;
    word-break: break-word;
  }
}

.msg-ai {
  display: flex;
  gap: 9px;
  max-width: 100%;

  .ai-avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 8px;
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
    font-size: 11px;
    flex-shrink: 0;
    margin-top: 2px;
  }

  .ai-body {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    line-height: 1.9;
    color: var(--ink-main);
    white-space: pre-wrap;
    word-break: break-word;
    padding-top: 3px;
  }
}

.ai-caret {
  display: inline-block;
  width: 2px;
  height: 1em;
  margin-left: 2px;
  vertical-align: -0.15em;
  background: var(--caret-color);
  animation: blink 0.9s steps(1) infinite;
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

// 思考中
.msg-thinking {
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 12px;
  color: var(--ink-muted);
  padding: 2px 4px;

  .dots {
    display: inline-flex;
    gap: 4px;

    i {
      width: 5px;
      height: 5px;
      border-radius: 50%;
      background: var(--ink-accent);
      animation: dot-pulse 1.2s ease-in-out infinite;

      &:nth-child(2) {
        animation-delay: 0.15s;
      }

      &:nth-child(3) {
        animation-delay: 0.3s;
      }
    }
  }
}

@keyframes dot-pulse {
  0%,
  100% {
    opacity: 0.25;
    transform: translateY(0);
  }
  50% {
    opacity: 1;
    transform: translateY(-2px);
  }
}

.msg-reasoning {
  font-size: 12px;

  summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--ink-muted);
    cursor: pointer;
    user-select: none;
  }

  p {
    margin-top: 6px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--ink-sec);
    line-height: 1.8;
    white-space: pre-wrap;
  }
}

.msg-tool {
  font-size: 12px;

  summary {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--ink-sec);
    cursor: pointer;
    user-select: none;
    list-style: none;

    .tool-name {
      flex: 1;
    }

    .chev {
      font-size: 10px;
      color: var(--ink-disabled);
    }
  }

  .tool-detail {
    margin: 6px 0 0;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--ink-muted);
    font-size: 11px;
    line-height: 1.7;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 180px;
    overflow-y: auto;
  }
}

.msg-ask {
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px solid var(--ink-accent);
  background: var(--ink-accent-soft);

  &.is-answered {
    border-color: var(--divider);
    background: var(--surface-2);
    opacity: 0.75;
  }

  .ask-q {
    display: flex;
    gap: 8px;
    font-size: 13px;
    line-height: 1.7;
    color: var(--ink-main);

    i {
      color: var(--ink-accent);
      margin-top: 4px;
    }
  }

  .ask-opts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;

    button {
      padding: 6px 12px;
      border-radius: 18px;
      border: 1px solid var(--ink-accent);
      background: transparent;
      color: var(--ink-accent);
      font-family: inherit;
      font-size: 12px;
      cursor: pointer;
      transition: all 0.2s;

      &:hover:not(:disabled) {
        background: var(--ink-accent);
        color: var(--btn-primary-color);
      }

      &:disabled {
        opacity: 0.5;
        cursor: default;
      }
    }
  }

  .ask-done {
    margin-top: 8px;
    font-size: 11.5px;
    color: var(--ink-muted);
  }
}

.msg-error {
  display: flex;
  gap: 8px;
  padding: 10px 12px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--state-danger) 9%, transparent);
  color: var(--state-danger);
  font-size: 12.5px;
  line-height: 1.7;

  i {
    margin-top: 4px;
    flex-shrink: 0;
  }
}

.msg-status {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--ink-muted);
  padding: 2px 4px;
}

// ── 历史浮层 ──────────────────────────────────────────────
.miao-history {
  position: absolute;
  left: 10px;
  right: 10px;
  bottom: 108px;
  max-height: 60%;
  z-index: 20;
  display: flex;
  flex-direction: column;
  border-radius: 14px;
  border: 1px solid var(--ui-border);
  background: var(--popover-bg);
  box-shadow: var(--ui-shadow-modal);
  overflow: hidden;
}

.mh-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 8px 10px 14px;
  font-size: 12.5px;
  color: var(--ink-main);
  border-bottom: 1px solid var(--divider);
}

.mh-list {
  overflow-y: auto;
  padding: 6px;
}

.mh-empty {
  padding: 22px 0;
  text-align: center;
  font-size: 12px;
  color: var(--ink-disabled);
}

.mh-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 10px;
  border-radius: 9px;
  cursor: pointer;
  transition: background 0.2s;

  &:hover {
    background: var(--surface-2);

    .mh-del {
      opacity: 1;
    }
  }

  &.is-active {
    background: var(--ink-accent-soft);
  }

  .mh-main {
    flex: 1;
    min-width: 0;
  }

  .mh-name {
    font-size: 12.5px;
    color: var(--ink-main);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mh-time {
    margin-top: 1px;
    font-size: 10.5px;
    color: var(--ink-disabled);
  }

  .mh-del {
    opacity: 0;
    border: none;
    background: transparent;
    color: var(--ink-muted);
    width: 24px;
    height: 24px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 11px;

    &:hover {
      background: var(--state-danger);
      color: #fff;
    }
  }
}

.hist-enter-active,
.hist-leave-active {
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.hist-enter-from,
.hist-leave-to {
  opacity: 0;
  transform: translateY(8px);
}

// ── 输入区 ──────────────────────────────────────────────
.miao-input {
  flex-shrink: 0;
  border-top: 1px solid var(--divider);
  background: var(--surface-1);
  padding: 10px 12px 8px;
}

.input-hint {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 8px;
  font-size: 12px;
  color: var(--ink-accent);

  i {
    font-size: 11px;
  }
}

.input-row {
  display: flex;
  gap: 8px;
  align-items: flex-end;

  textarea {
    flex: 1;
    resize: none;
    padding: 9px 12px;
    border-radius: 10px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.7;
    outline: none;
    transition: border-color 0.2s;

    &:focus {
      border-color: var(--ink-accent);
    }

    &::placeholder {
      color: var(--ink-disabled);
    }

    &:disabled {
      opacity: 0.6;
    }
  }

  .miao-send {
    height: 38px;
    width: 42px;
    padding: 0;
  }

  .miao-stop {
    width: 42px;
    height: 38px;
    color: var(--state-danger);

    &:hover {
      background: color-mix(in srgb, var(--state-danger) 10%, transparent);
    }
  }
}

.input-foot {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 6px;
  font-size: 11px;

  .foot-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: none;
    background: transparent;
    color: var(--ink-muted);
    font-family: inherit;
    font-size: 11px;
    cursor: pointer;

    &:hover {
      color: var(--ink-accent);
    }
  }

  .text-ink-muted {
    font-size: 11px;
  }
}
</style>
