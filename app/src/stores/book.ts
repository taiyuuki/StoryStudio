// 逐章成书状态 — 全局 store（不随面板组件销毁而丢失）
//
// 状态原先放在 BookPanel 组件里，面板一切走 / 一关组件就销毁：运行态归零、
// 进度丢失，后台流水线的事件没人接。搬进 store 后与组件生命周期解耦——
// AI 正在写时切走面板甚至关闭右栏，再打开时运行态、本章预览、暂停待审
// 都从 store 原样恢复；仅在切换项目时取消流水线（避免写进旧目录）。
import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import {
  api,
  type ChapterProgressDto,
  type WorkflowEventDto,
  type WorkflowIssueDto,
} from '@/api/backend'
import { useWorkspaceStore } from './workspace'

export const useBookStore = defineStore('book', () => {
  const store = useWorkspaceStore()

  // ── 状态 ────────────────────────────────────────────────
  const phase = ref<'idle' | 'paused' | 'done'>('idle')
  const chapters = ref<ChapterProgressDto[]>([])
  const targetWords = ref(2000)
  /** 本次最多连续写几章（0 = 不限，写完全部）；控制单次成书批量 */
  const maxChapters = ref(0)
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
  /** 运行序号：重置 / 新一轮启动后，旧 promise 的结果据此作废 */
  let runSeq = 0

  const pendingCount = computed(
    () => chapters.value.filter((c) => c.status !== 'completed').length,
  )
  const hasChapters = computed(() => chapters.value.length > 0)
  const canStart = computed(() => !running.value && hasChapters.value && phase.value !== 'paused')

  // ── 数据加载 ────────────────────────────────────────────
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

  /** 清空一轮运行的瞬态（保留章节进度 / 设置） */
  function resetTransients() {
    runSeq++
    running.value = false
    thinking.value = false
    current.value = null
    beats.value = []
    preview.value = ''
    written.value = 0
    check.value = null
    paused.value = null
    summary.value = ''
    error.value = ''
  }

  // 换项目：取消在途流水线（避免写进旧目录）→ 重置 → 重新加载
  watch(
    () => store.root,
    async (root) => {
      if (running.value) void api.workflowCancel()
      resetTransients()
      void reload()
      if (!root) return
      // 目标字数仍是默认值（无运行状态恢复、用户也没改过）时，
      // 采用书籍信息的单章字数（outline.toml [story] chapter_words）
      try {
        const meta = await api.getBookMeta(root)
        if (meta.chapterWords && targetWords.value === 2000) {
          targetWords.value = meta.chapterWords
        }
      } catch {
        // 拉不到书籍信息就保持默认
      }
    },
    { immediate: true },
  )

  // ── 事件处理（流水线事件 → 状态） ────────────────────────
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
      case 'delta':
        thinking.value = false
        preview.value = (preview.value + (ev.text ?? '')).slice(-2400)
        written.value += (ev.text ?? '').replace(/\s/g, '').length
        break
      case 'materialized':
        // 场景骨架先落盘：结构树尽早出现新章节/场景
        scheduleWorkbenchSync()
        break
      case 'committed':
        written.value = ev.words ?? written.value
        // 落盘即同步工作台：结构树 / 字数 / （安全的）编辑器内容随写随现，
        // 不必等整轮结束或用户手动刷新
        scheduleWorkbenchSync()
        break
      case 'qualityCheck':
        check.value = { passed: ev.passed ?? true, issues: ev.issues }
        break
      case 'chapterCompleted':
        syncChapter(ev.index ?? 0, 'completed')
        // 每章完成自动存为 git 快照（未启用版本档案时静默跳过）
        scheduleAutoArchive(ev.index ?? 0, ev.title ?? '')
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

  // ── 每章自动存档（防抖合并短时间内的多次完成事件） ────────
  let archiveTimer: ReturnType<typeof setTimeout> | null = null
  let archivePending: { index: number; title: string } | null = null
  function scheduleAutoArchive(index: number, title: string) {
    archivePending = { index, title }
    if (archiveTimer) clearTimeout(archiveTimer)
    archiveTimer = setTimeout(() => {
      archiveTimer = null
      const job = archivePending
      archivePending = null
      if (!job || !store.root) return
      void api
        .gitCommit(store.root, `逐章成书：第 ${job.index} 章《${job.title}》完成`)
        .catch(() => {})
    }, 1500)
  }

  // ── 工作台同步（落盘后随写随现） ────────────────────────
  // 流水线每提交一段就防抖刷新：结构树 / 全书字数总是更新；当前打开的
  // 场景仅在用户没有未保存编辑、且磁盘片段数确实变化时重载——绝不
  // 抢占正在打字的编辑器。
  let workbenchTimer: ReturnType<typeof setTimeout> | null = null
  function scheduleWorkbenchSync() {
    if (workbenchTimer) clearTimeout(workbenchTimer)
    workbenchTimer = setTimeout(() => {
      workbenchTimer = null
      void store.refresh().then(() => {
        const sceneId = store.scene?.id
        if (!sceneId || !store.scene) return
        const busy = Object.values(store.fragmentSave).some(
          (s) => s === 'dirty' || s === 'saving',
        )
        if (busy) return
        const treeScene = store.ws?.chapters
          .flatMap((c) => c.scenes)
          .find((s) => s.id === sceneId)
        if (treeScene && treeScene.fragmentCount !== store.scene.fragments.length) {
          void store.selectScene(sceneId)
        }
      })
    }, 800)
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

  // ── 动作 ────────────────────────────────────────────────
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
        action === 'run' && maxChapters.value > 0 ? maxChapters.value : null,
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

  return {
    phase,
    chapters,
    targetWords,
    maxChapters,
    running,
    thinking,
    current,
    beats,
    preview,
    written,
    check,
    paused,
    summary,
    error,
    pendingCount,
    hasChapters,
    canStart,
    reload,
    runAction,
    stopRun,
  }
})
