// 工作区 — 项目数据编排（单一数据源，所有面板从这里读写）
import { defineStore } from 'pinia'
import { computed, reactive, ref, shallowRef } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type {
  ChapterDto,
  CompileDto,
  EntityDto,
  EntityKind,
  FragmentDto,
  FragmentVersionDto,
  RelationDto,
  SceneDetailDto,
  ScenePatch,
  ThreadDto,
  WordStatsSummaryDto,
  WorkspaceDto,
} from '@/api/backend'
import { api } from '@/api/backend'
import { useUiStore } from './ui'

export type SaveState = 'dirty' | 'saving' | 'saved' | 'error'

export const useWorkspaceStore = defineStore('workspace', () => {
  const ui = useUiStore()

  // ── 状态 ────────────────────────────────────────────────
  const root = ref('')
  const ws = ref<WorkspaceDto | null>(null)
  const scene = ref<SceneDetailDto | null>(null)
  const compileResult = shallowRef<CompileDto | null>(null)
  const loading = ref(false)
  const aiWriting = ref(false)

  /** fragmentId → 保存状态 */
  const fragmentSave = reactive<Record<string, SaveState>>({})

  // ── 派生 ────────────────────────────────────────────────
  const characters = computed(() => ws.value?.entities.filter((e) => e.kind === 'character') ?? [])
  const entityName = computed(() => {
    const map = new Map<string, string>()
    for (const e of ws.value?.entities ?? []) map.set(e.id, e.displayName)
    return (id: string) => map.get(id) ?? id
  })
  const sceneWordCount = computed(
    // 与片段 foot 的字数同口径（Rust count_words），不混用前端全字符统计
    () => scene.value?.fragments.reduce((sum, f) => sum + (f.wordCount ?? 0), 0) ?? 0,
  )
  const errorCount = computed(() => {
    const d = compileResult.value
    if (!d?.ok) return 0
    return d.diagnostics.filter((x) => x.severity === 'error').length
  })
  const warningCount = computed(() => {
    const d = compileResult.value
    if (!d?.ok) return 0
    return d.diagnostics.filter((x) => x.severity === 'warning').length
  })

  // ── 项目 ────────────────────────────────────────────────
  async function openRoot(dir: string): Promise<boolean> {
    loading.value = true
    try {
      const data = await api.openProject(dir)
      root.value = data.root
      ws.value = data
      scene.value = null
      compileResult.value = null
      ui.rememberProject(data.root, data.name)
      // 同步本地时区（码字统计按本地日切日）；失败不影响打开
      api.statsSyncClock(data.root, new Date().getTimezoneOffset()).catch(() => {})
      // 默认选中第一个场景
      const first = data.chapters[0]?.scenes[0]
      if (first) await selectScene(first.id)
      void runCompile()
      void refreshStats()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    } finally {
      loading.value = false
    }
  }

  async function refresh(): Promise<void> {
    if (!root.value) return
    try {
      ws.value = await api.refreshWorkspace(root.value)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  function closeProject() {
    root.value = ''
    ws.value = null
    scene.value = null
    compileResult.value = null
  }

  // ── 场景 / 片段 ────────────────────────────────────────
  async function selectScene(sceneId: string): Promise<void> {
    if (!root.value) return
    try {
      scene.value = await api.getScene(root.value, sceneId)
      for (const f of scene.value.fragments) fragmentSave[f.id] = 'saved'
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  function patchSceneLocal(sceneId: string, patch: ScenePatch) {
    // 本地 tree 同步（标题/POV 立即反映）
    for (const ch of ws.value?.chapters ?? []) {
      for (const sc of ch.scenes) {
        if (sc.id !== sceneId) continue
        if (patch.title !== undefined) sc.title = patch.title
        if (patch.pov !== undefined) sc.pov = patch.pov
        if (patch.mood !== undefined) sc.mood = patch.mood
        if (patch.summary !== undefined) sc.summary = patch.summary
      }
    }
    if (scene.value?.id === sceneId) {
      if (patch.title !== undefined) scene.value.title = patch.title
      if (patch.pov !== undefined) scene.value.pov = patch.pov
      if (patch.mood !== undefined) scene.value.mood = patch.mood
      if (patch.function !== undefined) scene.value.function = patch.function
      if (patch.summary !== undefined) scene.value.summary = patch.summary
      if (patch.storyTime !== undefined) scene.value.storyTime = patch.storyTime
    }
  }

  async function updateScene(sceneId: string, patch: ScenePatch): Promise<void> {
    if (!root.value) return
    try {
      await api.updateScene(root.value, sceneId, patch)
      patchSceneLocal(sceneId, patch)
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 保存（单飞互斥：同一片段同时只有一次写请求，期间的新版本补跑）──
  const saveInFlight = new Set<string>()
  const pendingSaveText = new Map<string, string>()

  async function saveFragment(fragmentId: string, text: string): Promise<void> {
    if (!root.value) return
    if (saveInFlight.has(fragmentId)) {
      // 已有请求在途：暂存最新文本，等它完成后补跑（防旧响应覆盖新内容）
      pendingSaveText.set(fragmentId, text)
      fragmentSave[fragmentId] = 'dirty'
      return
    }
    saveInFlight.add(fragmentId)
    fragmentSave[fragmentId] = 'saving'
    try {
      await api.writeFragment(root.value, fragmentId, text)
      const f = scene.value?.fragments.find((x) => x.id === fragmentId)
      if (f) {
        f.text = text
        f.wordCount = countWords(text)
      }
      // 途中有新编辑（已暂存待补跑）则保持 dirty，不误报已保存
      fragmentSave[fragmentId] = pendingSaveText.has(fragmentId) ? 'dirty' : 'saved'
      scheduleCompile()
      void refreshStats()
    } catch (err) {
      fragmentSave[fragmentId] = 'error'
      ElMessage.error((err as Error).message)
    } finally {
      saveInFlight.delete(fragmentId)
      const pending = pendingSaveText.get(fragmentId)
      if (pending !== undefined) {
        pendingSaveText.delete(fragmentId)
        // 途中有新编辑 → 用最新文本补跑
        void saveFragment(fragmentId, pending)
      }
    }
  }

  /** 保存失败后的手动重试（FragmentBlock 的错误指示可点击） */
  function retrySave(fragmentId: string): void {
    const f = scene.value?.fragments.find((x) => x.id === fragmentId)
    if (f) void saveFragment(fragmentId, f.text)
  }

  async function insertFragment(afterId: string | null): Promise<void> {
    if (!root.value || !scene.value) return
    try {
      const frag = await api.insertFragment(root.value, scene.value.id, afterId, '')
      scene.value.fragments.push(frag)
      fragmentSave[frag.id] = 'saved'
      void refresh()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function removeFragment(fragmentId: string): Promise<void> {
    if (!root.value || !scene.value) return
    try {
      await ElMessageBox.confirm('删除后无法恢复，确定删除这个段落？', '删除段落', {
        type: 'warning',
        confirmButtonText: '删除',
        cancelButtonText: '取消',
      })
    } catch {
      return
    }
    try {
      await api.deleteFragment(root.value, scene.value.id, fragmentId)
      scene.value.fragments = scene.value.fragments.filter((f) => f.id !== fragmentId)
      delete fragmentSave[fragmentId]
      void refresh()
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function moveFragment(fragmentId: string, direction: -1 | 1): Promise<void> {
    if (!root.value || !scene.value) return
    const frags = scene.value.fragments
    const pos = frags.findIndex((f) => f.id === fragmentId)
    if (pos < 0) return
    const target = pos + direction
    if (target < 0 || target >= frags.length) return
    const ids = frags.map((f) => f.id)
    ids.splice(target, 0, ids.splice(pos, 1)[0])
    try {
      await api.moveFragment(root.value, scene.value.id, fragmentId, target)
      const reordered = ids.map((id) => frags.find((f) => f.id === id)!)
      scene.value.fragments = reordered
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 章节 ────────────────────────────────────────────────
  async function createChapter(title: string): Promise<ChapterDto | null> {
    if (!root.value) return null
    try {
      const ch = await api.createChapter(root.value, title)
      ws.value?.chapters.push(ch)
      return ch
    } catch (err) {
      ElMessage.error((err as Error).message)
      return null
    }
  }

  async function renameChapter(chapterId: string, title: string): Promise<void> {
    if (!root.value) return
    try {
      await api.renameChapter(root.value, chapterId, title)
      const ch = ws.value?.chapters.find((c) => c.id === chapterId)
      if (ch) ch.title = title
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function deleteChapter(chapterId: string): Promise<void> {
    if (!root.value) return
    try {
      await ElMessageBox.confirm(
        '删除章节会连同里面的所有场景和正文一起删除，无法恢复。确定吗？',
        '删除章节',
        { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
      )
    } catch {
      return
    }
    try {
      await api.deleteChapter(root.value, chapterId)
      if (ws.value) ws.value.chapters = ws.value.chapters.filter((c) => c.id !== chapterId)
      if (scene.value?.chapterId === chapterId) scene.value = null
      void refresh()
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function createScene(chapterId: string, title: string): Promise<void> {
    if (!root.value) return
    try {
      const detail = await api.createScene(root.value, chapterId, title)
      const ch = ws.value?.chapters.find((c) => c.id === chapterId)
      if (ch && ws.value) {
        ch.scenes.push({
          id: detail.id,
          chapterId,
          title,
          pov: null,
          mood: null,
          summary: null,
          fragmentCount: 0,
          wordCount: 0,
        })
      }
      scene.value = detail
      void refresh()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function deleteScene(sceneId: string): Promise<void> {
    if (!root.value) return
    try {
      await ElMessageBox.confirm('删除场景会连同里面的所有正文一起删除，确定吗？', '删除场景', {
        type: 'warning',
        confirmButtonText: '删除',
        cancelButtonText: '取消',
      })
    } catch {
      return
    }
    try {
      await api.deleteScene(root.value, sceneId)
      if (ws.value) {
        for (const ch of ws.value.chapters) {
          ch.scenes = ch.scenes.filter((s) => s.id !== sceneId)
        }
      }
      if (scene.value?.id === sceneId) scene.value = null
      void refresh()
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 实体 / 关系 ────────────────────────────────────────
  async function createEntity(kind: EntityKind, name: string): Promise<EntityDto | null> {
    if (!root.value) return null
    try {
      const e = await api.createEntity(root.value, kind, name)
      ws.value?.entities.push(e)
      return e
    } catch (err) {
      ElMessage.error((err as Error).message)
      return null
    }
  }

  async function saveEntity(entity: EntityDto): Promise<void> {
    if (!root.value) return
    try {
      await api.saveEntity(root.value, entity)
      const list = ws.value?.entities ?? []
      const idx = list.findIndex((x) => x.id === entity.id)
      if (idx >= 0) list[idx] = entity
      ElMessage.success('已保存')
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function deleteEntity(entityId: string): Promise<void> {
    if (!root.value) return
    try {
      await ElMessageBox.confirm(
        `删除「${ws.value?.entities.find((x) => x.id === entityId)?.displayName ?? '该设定'}」后，正文里提到它的地方不会被自动清理（一致性检查会提示残留）。确定删除？`,
        '删除设定',
        { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
      )
    } catch {
      return
    }
    try {
      await api.deleteEntity(root.value, entityId)
      if (ws.value) {
        ws.value.entities = ws.value.entities.filter((e) => e.id !== entityId)
        ws.value.relations = ws.value.relations.filter((r) => r.from !== entityId && r.to !== entityId)
      }
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function addRelation(from: string, to: string, relation: string): Promise<void> {
    if (!root.value) return
    try {
      const r = await api.addRelation(root.value, from, to, relation)
      ws.value?.relations.push(r)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function removeRelation(relationId: string): Promise<void> {
    if (!root.value) return
    try {
      await api.removeRelation(root.value, relationId)
      if (ws.value) {
        ws.value.relations = ws.value.relations.filter((r) => r.id !== relationId)
      }
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 叙事线索 ──────────────────────────────────────────
  async function createThread(title: string, description: string, priority: string): Promise<void> {
    if (!root.value) return
    try {
      const t = await api.createThread(root.value, title, description, priority)
      ws.value?.threads.push(t)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function updateThread(
    threadId: string,
    patch: { title?: string; description?: string; goal?: string; priority?: string },
  ): Promise<void> {
    if (!root.value) return
    try {
      await api.updateThread(root.value, threadId, patch)
      const t = ws.value?.threads.find((x) => x.id === threadId)
      if (t) Object.assign(t, patch)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function deleteThread(threadId: string): Promise<void> {
    if (!root.value) return
    try {
      await api.deleteThread(root.value, threadId)
      if (ws.value) ws.value.threads = ws.value.threads.filter((t) => t.id !== threadId)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 片段版本历史 ────────────────────────────────────────
  async function loadVersions(fragmentId: string): Promise<FragmentVersionDto[]> {
    if (!root.value) return []
    try {
      return await api.fragmentVersions(root.value, fragmentId)
    } catch (err) {
      ElMessage.error((err as Error).message)
      return []
    }
  }

  async function readVersion(fragmentId: string, timestamp: number): Promise<string> {
    try {
      const content = await api.fragmentVersionRead(root.value!, fragmentId, timestamp)
      // 剥掉 frontmatter 只展示正文
      const m = content.match(/^---\n[\s\S]*?\n---\n\n?/)
      return m ? content.slice(m[0].length) : content
    } catch (err) {
      ElMessage.error((err as Error).message)
      return ''
    }
  }

  async function restoreVersion(fragmentId: string, timestamp: number): Promise<boolean> {
    if (!root.value) return false
    try {
      const frag = await api.fragmentRestoreVersion(root.value, fragmentId, timestamp)
      if (scene.value) {
        const idx = scene.value.fragments.findIndex((x) => x.id === fragmentId)
        if (idx >= 0) scene.value.fragments[idx] = frag
      }
      scheduleCompile()
      ElMessage.success('已恢复到所选版本；之前的内容还在历史里，随时可以换回来')
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  // ── 码字统计 ────────────────────────────────────────────
  const stats = ref<WordStatsSummaryDto | null>(null)

  async function refreshStats(): Promise<void> {
    if (!root.value) return
    try {
      stats.value = await api.statsSummary(root.value)
    } catch {
      // 统计不可得不打扰写作
    }
  }

  async function setDailyGoal(goal: number): Promise<void> {
    if (!root.value) return
    try {
      await api.statsSetGoal(root.value, goal)
      await refreshStats()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 编译 ────────────────────────────────────────────────
  let compileTimer: ReturnType<typeof setTimeout> | null = null
  function scheduleCompile() {
    if (compileTimer) clearTimeout(compileTimer)
    compileTimer = setTimeout(() => void runCompile(), 1200)
  }

  async function runCompile(): Promise<void> {
    if (!root.value) return
    try {
      compileResult.value = await api.compileProject(root.value)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── AI 续写（流式）────────────────────────────────────
  const aiStreamText = ref('')

  async function aiWriteScene(instruction: string | null): Promise<void> {
    if (!root.value || !scene.value) return
    aiWriting.value = true
    aiStreamText.value = ''
    const sceneId = scene.value.id

    // 完成处理幂等：done 消息与命令返回值两条通路谁先到谁触发
    let finished = false
    const finish = async (ids: string[]) => {
      if (finished) return
      finished = true
      try {
        if (ids.length > 0) {
          await selectScene(sceneId)
          await refresh()
          ElMessage.success('续写完成，已加入本场景')
          void refreshStats()
        } else {
          ElMessage.warning('没有生成任何内容')
        }
      } finally {
        aiWriting.value = false
        aiStreamText.value = ''
      }
    }

    try {
      const result = await api.aiWriteScene(root.value, sceneId, instruction, (msg) => {
        if (msg.kind === 'delta') {
          aiStreamText.value += msg.text
        } else if (msg.kind === 'done') {
          void finish(msg.text ? msg.text.split(',') : [])
        }
      })
      await finish(result.fragmentIds)
    } catch (err) {
      if (!finished) {
        ElMessage.error(`续写失败：${(err as Error).message}`)
        aiWriting.value = false
        aiStreamText.value = ''
      }
    }
  }

  /** 用户主动放弃等待：结束 UI 等待态；后台若仍在生成，结果会经 done 消息自动出现 */
  function aiAbandonWait() {
    if (!aiWriting.value) return
    aiWriting.value = false
    aiStreamText.value = ''
    ElMessage.info('已停止等待；若后台仍在生成，完成后会自动出现')
  }

  return {
    root,
    ws,
    scene,
    compileResult,
    loading,
    aiWriting,
    fragmentSave,
    characters,
    entityName,
    sceneWordCount,
    errorCount,
    warningCount,
    openRoot,
    refresh,
    closeProject,
    selectScene,
    updateScene,
    saveFragment,
    insertFragment,
    removeFragment,
    moveFragment,
    createChapter,
    renameChapter,
    deleteChapter,
    createScene,
    deleteScene,
    createEntity,
    saveEntity,
    deleteEntity,
    addRelation,
    removeRelation,
    createThread,
    updateThread,
    deleteThread,
    retrySave,
    loadVersions,
    readVersion,
    restoreVersion,
    stats,
    refreshStats,
    setDailyGoal,
    runCompile,
    aiStreamText,
    aiWriteScene,
    aiAbandonWait,
  }
})

/**
 * 字数统计 — 与 Rust 端 story_ir::count_words 同口径：
 * CJK 每字计 1，连续 ASCII 字母数字串计 1，标点与空白不计。
 * 保持两端口径一致，避免保存后界面字数跳变。
 */
function countWords(text: string): number {
  let n = 0
  let inAsciiWord = false
  for (const ch of text) {
    if (isCjk(ch)) {
      n++
      inAsciiWord = false
    } else if (/[a-zA-Z0-9]/.test(ch)) {
      if (!inAsciiWord) {
        n++
        inAsciiWord = true
      }
    } else {
      inAsciiWord = false
    }
  }
  return n
}

function isCjk(ch: string): boolean {
  const c = ch.codePointAt(0)!
  return (
    (c >= 0x3400 && c <= 0x4dbf) ||
    (c >= 0x4e00 && c <= 0x9fff) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0x20000 && c <= 0x2a6df)
  )
}

export type { FragmentDto, RelationDto, ThreadDto }
