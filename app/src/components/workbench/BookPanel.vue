<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { api, type ChapterProgressDto, type WorkflowEventDto, type WorkflowIssueDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'
import OutlineBatchDialog from './OutlineBatchDialog.vue'

const store = useWorkspaceStore()

// ── 状态 ──────────────────────────────────────────────────
const phase = ref<'idle' | 'paused' | 'done'>('idle')
const chapters = ref<ChapterProgressDto[]>([])
const targetWords = ref(2000)
const running = ref(false)
const thinking = ref(false)

const current = ref<{ index: number; title: string } | null>(null)
const beats = ref<string[]>([])
const preview = ref('')
const written = ref(0)
const check = ref<{ passed: boolean; issues: WorkflowIssueDto[] } | null>(null)
const paused = ref<{ index: number; title: string; issues: WorkflowIssueDto[] } | null>(null)
const summary = ref('')
const error = ref('')

const previewEl = ref<HTMLElement | null>(null)
let runSeq = 0
const outlineOpen = ref(false)

function onOutlineSaved() {
  void reload()
}

// ── 数据加载 ──────────────────────────────────────────────
async function reload() {
  if (!store.root) return
  try {
    const [state, progress] = await Promise.all([
      api.workflowStatus(store.root),
      api.chapterProgress(store.root),
    ])
    phase.value = (state.phase as 'idle' | 'paused' | 'done') ?? 'idle'
    chapters.value = progress
    if (state.targetWords > 0) targetWords.value = state.targetWords
    paused.value =
      state.pausedIndex != null
        ? {
            index: state.pausedIndex,
            title: state.pausedTitle ?? '',
            issues: state.pausedIssues,
          }
        : null
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err)
  }
}

watch(
  () => store.root,
  () => {
    summary.value = ''
    error.value = ''
    void reload()
  },
  { immediate: true },
)

// ── 事件处理 ──────────────────────────────────────────────
function onEvent(ev: WorkflowEventDto) {
  switch (ev.kind) {
    case 'runStarted':
      thinking.value = true
      summary.value = ''
      error.value = ''
      break
    case 'chapterStarted':
      current.value = { index: ev.index ?? 0, title: ev.title ?? '' }
      beats.value = []
      preview.value = ''
      written.value = 0
      check.value = null
      thinking.value = true
      syncChapter(ev.index ?? 0, 'writing')
      break
    case 'beats':
      beats.value = ev.beats
      break
    case 'delta': {
      thinking.value = false
      preview.value = (preview.value + (ev.text ?? '')).slice(-2400)
      written.value += (ev.text ?? '').replace(/\s/g, '').length
      void nextTick(() => {
        const el = previewEl.value
        if (el) el.scrollTop = el.scrollHeight
      })
      break
    }
    case 'committed':
      written.value = ev.words ?? written.value
      break
    case 'qualityCheck':
      check.value = { passed: ev.passed ?? true, issues: ev.issues }
      break
    case 'chapterCompleted':
      syncChapter(ev.index ?? 0, 'completed')
      break
    case 'paused':
      thinking.value = false
      paused.value = {
        index: ev.index ?? 0,
        title: ev.title ?? '',
        issues: ev.issues,
      }
      phase.value = 'paused'
      break
    case 'cancelled':
      break
    case 'finished':
      finish(ev.completedThisRun ?? 0, ev.remaining ?? 0, ev.text ?? '')
      break
  }
}

function syncChapter(index: number, status: string) {
  const ch = chapters.value.find((c) => c.index === index)
  if (ch) ch.status = status
}

const OUTCOME_TEXT: Record<string, (done: number, left: number) => string> = {
  done: (done) => `全部章节已写成${done > 0 ? `（本次 ${done} 章）` : ''}。`,
  paused: (done) => `有一章需要你过目${done > 0 ? `（此前已完成 ${done} 章）` : ''}。`,
  cancelled: (done, left) =>
    `已停下，进度都保住了${done > 0 ? `，本次写成 ${done} 章` : ''}${left > 0 ? `，还剩 ${left} 章待写` : ''}。`,
  batch_limit: (done, left) => `本批 ${done} 章已写完，还剩 ${left} 章下次继续。`,
}

function finish(done: number, remaining: number, outcome: string) {
  if (!running.value) return // finished 事件与命令返回双通路，只结算一次
  running.value = false
  thinking.value = false
  current.value = null
  summary.value = (OUTCOME_TEXT[outcome] ?? OUTCOME_TEXT.done)(done, remaining)
  if (outcome === 'done') phase.value = 'done'
  void store.refresh()
  void reload()
}

// ── 动作 ──────────────────────────────────────────────────
async function runAction(action: 'run' | 'approve' | 'rewrite' | 'skip') {
  if (!store.root || running.value) return
  running.value = true
  error.value = ''
  paused.value = null
  summary.value = ''
  if (action === 'run') thinking.value = true
  const seq = ++runSeq

  try {
    const report = await api.workflowRun(
      store.root,
      action,
      action === 'run' ? targetWords.value : null,
      null,
      onEvent,
    )
    if (seq !== runSeq) return
    finish(report.completedThisRun, report.remaining, report.outcome)
  } catch (err) {
    if (seq !== runSeq) return
    running.value = false
    thinking.value = false
    error.value = err instanceof Error ? err.message : String(err)
    void reload()
  }
}

async function stopRun() {
  await api.workflowCancel()
}

onBeforeUnmount(() => {
  // 面板关掉不停流水线；切项目时取消，避免写到旧目录
  if (running.value && store.root) void api.workflowCancel()
})

// ── 展示 ──────────────────────────────────────────────────
const pendingCount = computed(
  () => chapters.value.filter((c) => c.status !== 'completed').length,
)
const hasChapters = computed(() => chapters.value.length > 0)
const canStart = computed(() => !running.value && hasChapters.value && phase.value !== 'paused')

const STATUS_TEXT: Record<string, string> = {
  planned: '待写',
  writing: '写作中',
  completed: '已完成',
}

const ISSUE_ICON: Record<string, string> = {
  word_count: 'fa-solid fa-align-left',
  truncated: 'fa-solid fa-scissors',
  dup_paragraph: 'fa-solid fa-clone',
  compile_error: 'fa-solid fa-triangle-exclamation',
  compile_warning: 'fa-solid fa-circle-exclamation',
}
</script>

<template>
  <div class="book-panel">
    <!-- 出错 -->
    <div v-if="error" class="bk-error">
      <i class="fa-solid fa-circle-exclamation" />
      <span>{{ error }}</span>
    </div>

    <!-- 本次结果 -->
    <div v-if="summary && !running" class="bk-summary">
      <i class="fa-regular fa-circle-check" />
      <span>{{ summary }}</span>
    </div>

    <!-- 运行中 -->
    <div v-if="running" class="bk-live">
      <div class="bk-live-head">
        <template v-if="current">
          <span class="bk-ch-title">第 {{ current.index }} 章《{{ current.title }}》</span>
          <span class="num">{{ written }} 字</span>
        </template>
        <span v-else class="text-ink-muted">准备中…</span>
        <button class="ink-btn ink-btn--sm bk-stop" @click="stopRun">
          <i class="fa-solid fa-stop" />
          停止
        </button>
      </div>

      <div v-if="thinking" class="bk-thinking">
        <span class="dots"><i /><i /><i /></span>
        正在构思本章…
      </div>

      <div v-if="beats.length" class="bk-beats">
        <div class="bk-sub">本章节拍</div>
        <ol>
          <li v-for="(b, i) in beats" :key="i">{{ b }}</li>
        </ol>
      </div>

      <div v-if="preview" ref="previewEl" class="bk-preview">{{ preview }}</div>

      <div v-if="check" class="bk-check" :class="{ 'is-pass': check.passed }">
        <template v-if="check.passed">
          <i class="fa-regular fa-circle-check" />
          质检通过
        </template>
        <template v-else>
          <i class="fa-solid fa-circle-exclamation" />
          <span>质检发现 {{ check.issues.length }} 项问题</span>
          <ul>
            <li v-for="(iss, i) in check.issues" :key="i">
              <i :class="ISSUE_ICON[iss.code] ?? 'fa-solid fa-circle-info'" />
              {{ iss.message }}
            </li>
          </ul>
        </template>
      </div>
    </div>

    <!-- 暂停待审 -->
    <div v-if="paused && !running" class="bk-paused">
      <div class="bk-paused-head">
        <i class="fa-regular fa-hand" />
        第 {{ paused.index }} 章《{{ paused.title }}》需要你看一眼
      </div>
      <ul class="bk-issues">
        <li v-for="(iss, i) in paused.issues" :key="i" :class="{ 'is-hard': iss.hard }">
          <i :class="ISSUE_ICON[iss.code] ?? 'fa-solid fa-circle-info'" />
          {{ iss.message }}
        </li>
      </ul>
      <div class="bk-paused-actions">
        <button class="ink-btn ink-btn--primary ink-btn--sm" @click="runAction('rewrite')">
          <i class="fa-solid fa-rotate-right" />
          重写本章
        </button>
        <button class="ink-btn ink-btn--sm" @click="runAction('approve')">
          <i class="fa-solid fa-check" />
          接受，继续写
        </button>
        <button class="ink-btn ink-btn--sm" @click="runAction('skip')">
          <i class="fa-solid fa-forward" />
          跳过本章
        </button>
      </div>
    </div>

    <!-- 设置 + 启动 -->
    <div v-if="canStart" class="bk-setup">
      <div v-if="phase === 'done' && pendingCount === 0" class="bk-done-banner">
        <i class="fa-regular fa-circle-check" />
        全部章节已完成
      </div>
      <label class="bk-field">
        <span>每章目标字数</span>
        <input v-model.number="targetWords" type="number" min="500" step="100" :disabled="running" />
      </label>
      <button class="ink-btn ink-btn--primary bk-start" @click="runAction('run')">
        <i class="fa-solid fa-play" />
        {{ pendingCount === chapters.length ? '开始成书' : '继续成书' }}
      </button>
    </div>

    <!-- 章节脉络 -->
    <div class="bk-chapters">
      <div class="bk-sub">
        章节脉络
        <span class="num text-ink-muted">
          {{ chapters.length - pendingCount }}/{{ chapters.length }}
        </span>
        <button
          v-if="!running"
          class="ink-btn ink-btn--sm bk-outline-add"
          title="让 AI 读前文，接着排后面的章节"
          @click="outlineOpen = true"
        >
          <i class="fa-solid fa-plus" />
          续排
        </button>
      </div>
      <ul v-if="hasChapters" class="bk-ch-list">
        <li
          v-for="ch in chapters"
          :key="ch.index"
          :class="{
            'is-current': running && current?.index === ch.index,
            'is-paused': paused?.index === ch.index,
          }"
        >
          <span class="bk-ch-idx num">{{ ch.index }}</span>
          <span class="bk-ch-name">{{ ch.title }}</span>
          <span class="bk-ch-status" :data-s="ch.status">
            {{ STATUS_TEXT[ch.status] ?? ch.status }}
          </span>
        </li>
      </ul>
      <div v-else class="bk-empty">
        <i class="fa-regular fa-map" />
        <p>这本书还没有章节脉络。</p>
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="outlineOpen = true">
          <i class="fa-solid fa-wand-magic-sparkles" />
          让 AI 排一批
        </button>
      </div>
      <div v-if="hasChapters && pendingCount === 0 && !running" class="bk-all-done-note">
        <i class="fa-solid fa-circle-info" />
        待写章节用完了——写长篇不必一次排完，随时可以续排下一批。
      </div>
    </div>

    <OutlineBatchDialog v-model="outlineOpen" :existing-count="chapters.length" @saved="onOutlineSaved" />
  </div>
</template>

<style scoped lang="scss">
.book-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin: -14px -16px -20px;
  padding: 14px 16px 20px;
  height: 100%;
  overflow-y: auto;
}

.bk-sub {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--ink-muted);
  margin-bottom: 8px;
}

// 错误 / 结果横幅
.bk-error,
.bk-summary {
  display: flex;
  gap: 8px;
  padding: 10px 12px;
  border-radius: 10px;
  font-size: 12.5px;
  line-height: 1.6;
}

.bk-error {
  background: var(--state-danger-soft, rgba(192, 57, 43, 0.1));
  color: var(--state-danger);

  i {
    margin-top: 3px;
  }
}

.bk-summary {
  background: var(--surface-2);
  color: var(--ink-main);

  i {
    color: var(--ink-accent);
    margin-top: 3px;
  }
}

// 运行区
.bk-live {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px;
  border: 1px solid var(--divider);
  border-radius: 12px;
  background: var(--surface-2);
}

.bk-live-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;

  .num {
    margin-left: auto;
    color: var(--ink-muted);
    font-size: 12px;
  }

  .bk-stop {
    margin-left: 8px;
  }
}

.bk-ch-title {
  font-weight: 600;
}

.bk-thinking {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-muted);

  .dots {
    display: inline-flex;
    gap: 3px;

    i {
      width: 4px;
      height: 4px;
      border-radius: 50%;
      background: var(--ink-accent);
      animation: bk-pulse 1.2s ease-in-out infinite;

      &:nth-child(2) {
        animation-delay: 0.2s;
      }

      &:nth-child(3) {
        animation-delay: 0.4s;
      }
    }
  }
}

@keyframes bk-pulse {
  0%,
  60%,
  100% {
    opacity: 0.25;
  }

  30% {
    opacity: 1;
  }
}

.bk-beats ol {
  margin: 0;
  padding-left: 18px;
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-main);

  li::marker {
    color: var(--ink-accent);
  }
}

.bk-preview {
  max-height: 260px;
  overflow-y: auto;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--surface-1);
  border: 1px solid var(--divider);
  font-size: 12.5px;
  line-height: 1.9;
  white-space: pre-wrap;
  word-break: break-all;
}

.bk-check {
  font-size: 12.5px;
  color: var(--ink-main);

  ul {
    margin: 6px 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  li {
    display: flex;
    gap: 6px;
    font-size: 12px;
    color: var(--ink-muted);

    i {
      margin-top: 3px;
      color: var(--state-danger);
      font-size: 11px;
    }
  }

  &.is-pass {
    color: var(--ink-accent);
  }
}

// 暂停待审
.bk-paused {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px;
  border: 1px solid var(--ink-accent);
  border-radius: 12px;
  background: var(--ink-accent-soft);
}

.bk-paused-head {
  display: flex;
  gap: 8px;
  font-size: 13px;
  font-weight: 600;

  i {
    color: var(--ink-accent);
    margin-top: 2px;
  }
}

.bk-issues {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 5px;

  li {
    display: flex;
    gap: 6px;
    font-size: 12px;
    line-height: 1.6;
    color: var(--ink-muted);

    i {
      margin-top: 3px;
      font-size: 11px;
      color: var(--ink-muted);
    }

    &.is-hard i {
      color: var(--state-danger);
    }
  }
}

.bk-paused-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

// 设置
.bk-setup {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.bk-done-banner {
  display: flex;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-accent);
}

.bk-field {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12.5px;
  color: var(--ink-muted);

  input {
    width: 90px;
    padding: 5px 8px;
    border: 1px solid var(--divider);
    border-radius: 8px;
    background: var(--surface-1);
    color: var(--ink-main);
    font-size: 12.5px;

    &:focus {
      outline: none;
      border-color: var(--ink-accent);
    }
  }
}

.bk-start {
  justify-content: center;
}

// 章节列表
.bk-ch-list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 8px;
    border-radius: 8px;
    font-size: 12.5px;

    &.is-current {
      background: var(--ink-accent-soft);
    }

    &.is-paused {
      background: var(--ink-accent-soft);
    }
  }
}

.bk-ch-idx {
  width: 18px;
  color: var(--ink-muted);
  font-size: 11px;
  text-align: right;
}

.bk-ch-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bk-ch-status {
  font-size: 11px;
  color: var(--ink-muted);

  &[data-s='completed'] {
    color: var(--ink-accent);
  }

  &[data-s='writing'] {
    color: var(--ink-accent);
  }
}

// 空状态
.bk-outline-add {
  margin-left: auto;
  padding: 2px 8px;
  font-size: 11px;
}

.bk-all-done-note {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  margin-top: 8px;
  padding: 8px 10px;
  border-radius: 8px;
  background: var(--surface-2);
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    margin-top: 3px;
    flex-shrink: 0;
  }
}

.bk-empty {
  padding: 22px 12px;
  text-align: center;
  font-size: 12.5px;
  line-height: 1.8;
  color: var(--ink-muted);

  i {
    font-size: 22px;
    margin-bottom: 8px;
    opacity: 0.5;
  }

  p {
    margin: 0;
  }
}
</style>
