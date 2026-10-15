// 工作区 — 项目数据编排（单一数据源，所有面板从这里读写）
import { defineStore } from 'pinia'
import { computed, reactive, ref, shallowRef, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type {
  ChapterDto,
  CompileDto,
  EntityDto,
  EntityKind,
  FragmentDto,
  FragmentDiffLineDto,
  FragmentVersionDto,
  RelationDto,
  SceneDetailDto,
  ScenePatch,
  ThreadDto,
  WatchStatusDto,
  WordStatsSummaryDto,
  WorkspaceDto,
} from '@/api/backend'
import { api, onFsChanged, onWatchState } from '@/api/backend'
import { useUiStore } from './ui'
// 循环依赖安全：useBookStore 只在本 store 的事件回调里延迟调用，
// book store 初始化也只在函数体内解引用本 store——两个 setup 都不会
// 在初始化路径上触发对方（见 busyWriting / watch(busyWriting)）
import { useBookStore } from './book'

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
      // 优先回到上次编辑位置（首页「继续写作」直达）；场景已被删则回退首个场景
      const last = ui.lastScene[data.root]
      const target =
        last && data.chapters.some((ch) => ch.scenes.some((sc) => sc.id === last.sceneId))
          ? last.sceneId
          : (data.chapters[0]?.scenes[0]?.id ?? null)
      if (target) await selectScene(target)
      void runCompile()
      void refreshStats()
      // MCP 服务跟随项目（工具注册表绑定项目目录）：已运行则重启指向新书，
      // 未运行但偏好开启则随项目自动启动；失败静默，不阻塞项目打开
      void mcpFollowProject(data.root)
      // 文件监听同样跟随项目（RFC-0019 §6.3），失败静默
      void watchFollowProject(data.root)
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    } finally {
      loading.value = false
    }
  }

  // MCP 偏好键（设置页写入，与 SettingsView 保持一致）
  const MCP_PREF = { enabled: 'mcp.autoStart', port: 'mcp.port', readOnly: 'mcp.readOnly' } as const

  async function mcpFollowProject(newRoot: string): Promise<void> {
    try {
      const st = await api.mcpServerStatus()
      if (st.running) {
        await api.mcpServerStart(newRoot, st.port, st.readOnly)
      } else if (localStorage.getItem(MCP_PREF.enabled) === 'on') {
        const port = Number(localStorage.getItem(MCP_PREF.port) ?? 27520)
        const readOnly = localStorage.getItem(MCP_PREF.readOnly) === '1'
        await api.mcpServerStart(newRoot, port, readOnly)
      }
    } catch {
      // MCP 服务不阻塞项目打开
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
    // MCP 服务跟随「当前打开的项目」：关闭项目即停服务，
    // 避免外部 Agent 仍能读写已关闭的书；偏好保留，重开项目自动恢复
    void api.mcpServerStop().catch(() => {})
    // 文件监听同停（RFC-0019：监听生命周期 = 当前打开的项目）
    void api.watchStop().catch(() => {})
    watchState.value = null
  }

  // ── 场景 / 片段 ────────────────────────────────────────
  async function selectScene(sceneId: string): Promise<void> {
    if (!root.value) return
    try {
      scene.value = await api.getScene(root.value, sceneId)
      for (const f of scene.value.fragments) fragmentSave[f.id] = 'saved'
      // 记住上次位置（首页「继续写作」直达）
      ui.rememberScene(root.value, {
        sceneId: scene.value.id,
        chapterTitle: scene.value.chapterTitle,
        sceneTitle: scene.value.title || '未命名场景',
        ts: Date.now(),
      })
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
  /** 保存单飞期间该片段正在落盘的文本（watcher 冲突检测要排除自己的在途写入） */
  const savingText = new Map<string, string>()

  function readSaveState(fragmentId: string): SaveState {
    return fragmentSave[fragmentId] ?? 'saved'
  }

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
    savingText.set(fragmentId, text)
    try {
      await api.writeFragment(root.value, fragmentId, text)
      // 途中有新编辑（防抖计时器又武装了，或已暂存待补跑）则本次落盘文本已过时：
      // 回写 f.text 会经 props.watch 把编辑器整体替换回旧文本——文字闪回、
      // CodeMirror 跨文档测量失稳还会把纸面滚动甩到顶部。新文本由补跑保存追平。
      // （经函数读取保存状态：await 期间 scheduleSave 会把它改回 'dirty'，
      //   直接读属性会被 TS 按赋值路径窄化成 'saving' 而报错）
      const newerEdits =
        readSaveState(fragmentId) === 'dirty' || pendingSaveText.has(fragmentId)
      if (!newerEdits) {
        const f = scene.value?.fragments.find((x) => x.id === fragmentId)
        if (f) {
          f.text = text
          f.wordCount = countWords(text)
        }
      }
      // 途中有新编辑则保持 dirty，不误报已保存（计时器武装也算，不只看暂存）
      fragmentSave[fragmentId] = newerEdits ? 'dirty' : 'saved'
      // 保存成功即冲突解除（用户版本已落盘成为最新真相）
      conflictedNotified.delete(fragmentId)
      scheduleCompile()
      void refreshStats()
    } catch (err) {
      fragmentSave[fragmentId] = 'error'
      ElMessage.error((err as Error).message)
    } finally {
      saveInFlight.delete(fragmentId)
      savingText.delete(fragmentId)
      const pending = pendingSaveText.get(fragmentId)
      if (pending !== undefined) {
        pendingSaveText.delete(fragmentId)
        // 途中有新编辑 → 用最新文本补跑
        void saveFragment(fragmentId, pending)
      }
    }
  }

  /** 保存失败后的手动重试（FragmentBlock 的错误指示可点击）；必须传编辑器当前文本，fragment.text 可能已过时 */
  function retrySave(fragmentId: string, text: string): void {
    void saveFragment(fragmentId, text)
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

  // ── 片段锚点（手动编辑，对齐 AI 的 set/remove_fragment_anchor 工具）──
  // 只回写 f.anchors（不替换整个 fragment），避免 f.text 变化把编辑器里
  // 未保存的正文整体替换回去（文字闪回，见 saveFragment 的同款注释）

  async function setFragmentAnchor(
    fragmentId: string,
    entityId: string,
    role: string,
  ): Promise<void> {
    if (!root.value) return
    try {
      const anchors = await api.setFragmentAnchor(root.value, fragmentId, entityId, role)
      const f = scene.value?.fragments.find((x) => x.id === fragmentId)
      if (f) f.anchors = anchors
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function removeFragmentAnchor(fragmentId: string, entityId: string): Promise<void> {
    if (!root.value) return
    try {
      const anchors = await api.removeFragmentAnchor(root.value, fragmentId, entityId)
      const f = scene.value?.fragments.find((x) => x.id === fragmentId)
      if (f) f.anchors = anchors
      scheduleCompile()
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  /** AI 理解锚定本段（按当前落盘正文做指代消解补锚，保留手动锚点） */
  async function aiAnchorFragment(fragmentId: string): Promise<boolean> {
    if (!root.value) return false
    try {
      const r = await api.aiAnchorFragment(root.value, fragmentId)
      const f = scene.value?.fragments.find((x) => x.id === fragmentId)
      if (f) f.anchors = r.anchors
      // 台账（anchoring.toml）可能新增待处置指代，右栏待审面板的角标跟新
      void refresh()
      scheduleCompile()
      const pending = r.unresolved.length
        ? `；${r.unresolved.length} 条待处置指代已列入「锚定待审」面板`
        : ''
      ElMessage.success(
        r.applied > 0 ? `AI 已锚定 ${r.applied} 处出场${pending}` : `本段无需补锚${pending}`,
      )
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
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

  /** 合并实体（from 并入 into：锚点/关系/POV 等引用自动重定向），成功后全量刷新 */
  async function mergeEntities(fromId: string, intoId: string): Promise<boolean> {
    if (!root.value) return false
    const fromName = ws.value?.entities.find((x) => x.id === fromId)?.displayName ?? fromId
    const intoName = ws.value?.entities.find((x) => x.id === intoId)?.displayName ?? intoId
    try {
      await ElMessageBox.confirm(
        `将「${fromName}」并入「${intoName}」：前者的档案名与别名转入后者的别名，锚点/关系/POV/出场表等引用自动重定向，随后删除「${fromName}」。确定合并？`,
        '合并设定',
        { type: 'warning', confirmButtonText: '合并', cancelButtonText: '取消' },
      )
    } catch {
      return false
    }
    try {
      const summary = await api.mergeEntities(root.value, fromId, intoId)
      ElMessage.success(summary)
      await refresh()
      scheduleCompile()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  /** 处置一条未锚定指代（建档 / 归拢 / 忽略），成功后刷新台账 */
  async function resolveUnresolved(
    fragmentId: string,
    quote: string,
    action: 'create' | 'merge' | 'ignore',
    opts?: { targetEntityId?: string; name?: string; kind?: EntityKind },
  ): Promise<boolean> {
    if (!root.value) return false
    try {
      const summary = await api.resolveUnresolved(root.value, fragmentId, quote, action, opts)
      ElMessage.success(summary)
      // 台账与实体列表都可能变化（create 建档），全量刷新最稳
      await refresh()
      scheduleCompile()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  /** 批量忽略未锚定指代（背景人物等一次性代称） */
  async function ignoreUnresolvedRefs(refs: [string, string][]): Promise<boolean> {
    if (!root.value || refs.length === 0) return false
    try {
      const summary = await api.ignoreUnresolvedRefs(root.value, refs)
      ElMessage.success(summary)
      await refresh()
      scheduleCompile()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  /** 批量删除未锚定指代记录（仅清台账；正文仍含该 quote 时下次锚定会重新出现） */
  async function deleteUnresolvedRefs(refs: [string, string][]): Promise<boolean> {
    if (!root.value || refs.length === 0) return false
    try {
      const summary = await api.deleteUnresolvedRefs(root.value, refs)
      ElMessage.success(summary)
      await refresh()
      scheduleCompile()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
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

  /** 更新关系生效区间（时变窗口）；null = 清除该边界 */
  async function updateRelation(
    relationId: string,
    validFromScene: string | null,
    validUntilScene: string | null,
  ): Promise<void> {
    if (!root.value) return
    try {
      const r = await api.updateRelation(root.value, relationId, validFromScene, validUntilScene)
      if (ws.value) {
        const i = ws.value.relations.findIndex((x) => x.id === relationId)
        if (i >= 0) ws.value.relations[i] = r
      }
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  // ── 叙事线索 ──────────────────────────────────────────
  async function createThread(
    title: string,
    description: string,
    priority: string,
    dueVolume?: number | null,
  ): Promise<void> {
    if (!root.value) return
    try {
      const t = await api.createThread(root.value, title, description, priority, dueVolume ?? null)
      ws.value?.threads.push(t)
    } catch (err) {
      ElMessage.error((err as Error).message)
    }
  }

  async function updateThread(
    threadId: string,
    patch: {
      title?: string
      description?: string
      goal?: string
      priority?: string
      /** reader（明线）/ pov（暗线） */
      scope?: string
      /** 0 = 清除（恢复全书长线） */
      dueVolume?: number
    },
  ): Promise<void> {
    if (!root.value) return
    try {
      await api.updateThread(root.value, threadId, patch)
      const t = ws.value?.threads.find((x) => x.id === threadId)
      if (t) {
        Object.assign(t, patch)
        if (patch.dueVolume !== undefined) t.dueVolume = patch.dueVolume || null
      }
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

  /** 推进线索叙事状态（埋下/揭示/回收/放弃）；揭示/回收的读者知情后端自动补记 */
  async function advanceThread(
    threadId: string,
    status: string,
    sceneId?: string | null,
  ): Promise<boolean> {
    if (!root.value) return false
    try {
      const t = await api.advanceThread(root.value, threadId, status, sceneId ?? null)
      const list = ws.value?.threads
      if (list) {
        const i = list.findIndex((x) => x.id === threadId)
        if (i >= 0) list[i] = t
      }
      scheduleCompile()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  /** 记录线索可见性（同观察者同场景覆盖）；成功后刷新线索数据 */
  async function setThreadKnowledge(
    threadId: string,
    observer: string,
    state: string,
    sceneId: string,
  ): Promise<boolean> {
    if (!root.value) return false
    try {
      await api.setThreadKnowledge(root.value, threadId, observer, state, sceneId)
      await refresh()
      return true
    } catch (err) {
      ElMessage.error((err as Error).message)
      return false
    }
  }

  /** 删除线索可见性记录（sceneId 省略 = 删整条观察者记录） */
  async function removeThreadKnowledge(
    threadId: string,
    observer: string,
    sceneId?: string,
  ): Promise<void> {
    if (!root.value) return
    try {
      await api.removeThreadKnowledge(root.value, threadId, observer, sceneId)
      await refresh()
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

  /** 版本差异；oldTs/newTs 传 null 表示当前磁盘内容 */
  async function diffVersion(
    fragmentId: string,
    oldTs: number | null,
    newTs: number | null,
  ): Promise<FragmentDiffLineDto[]> {
    try {
      return await api.fragmentVersionDiff(root.value!, fragmentId, oldTs, newTs)
    } catch (err) {
      ElMessage.error((err as Error).message)
      return []
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

  // ── 文件监听（RFC-0019）：磁盘 → 前端实时同步 ────────────
  //
  // 磁盘是真相源，界面是镜像：MCP 写入与外部编辑器改 TOML 经 watcher
  // 事件秒级同步到 store。去重不问来源，只问"前端是否已知这份内容"——
  // 内容相等即 no-op，正确性建立在内容合并（L2）上，UI 自己保存的回推
  // 因此对用户不可见。

  /** 监听状态（watch_start 返回值 / project-watch-state 事件） */
  const watchState = ref<WatchStatusDto | null>(null)

  /** 章节脉络数据的变更代数（fs 事件驱动，只增不减）。
   *
   * chapters.toml / structure.toml 可能被不同执行体落盘（妙笔
   * update_chapter_outline、外部 MCP 客户端、脉络面板），watcher 事件经
   * routeFsBatch 分流时在此计代。逐章成书（book store）等跨面板消费方
   * watch 它感知脉络状态过期并自行重拉——workspace 只发布事实，何时
   * 重载、要不要跳过（如流水线运行中）由消费方自己的策略决定。 */
  const chaptersVersion = ref(0)

  /** 批处理间隔：固定间隔而非 trailing 防抖——持续写入流下最坏延迟有界（§3 Goal 1） */
  const FS_BATCH_MS = 300
  let fsPending = new Set<string>()
  let fsBatchTimer: ReturnType<typeof setTimeout> | null = null

  /** AI 续写 / 逐章流水线运行中（§10.1 暂存触发） */
  function busyWriting(): boolean {
    return aiWriting.value || useBookStore().running
  }

  function onFsFiles(files: string[]): void {
    if (!root.value || files.length === 0) return
    if (busyWriting()) {
      stashFsFiles(files)
      return
    }
    for (const f of files) fsPending.add(f)
    if (!fsBatchTimer) {
      fsBatchTimer = setTimeout(() => {
        fsBatchTimer = null
        const batch = fsPending
        fsPending = new Set()
        routeFsBatch(batch)
      }, FS_BATCH_MS)
    }
  }

  /** §8.2 分流：场景级变更重拉当前场景，其余整树刷新；编译与统计总跟随 */
  function routeFsBatch(files: Set<string>): void {
    const sceneScope = [...files].some(
      (f) => f.startsWith('fragments/') || f === 'structure.toml' || f === 'chapters.toml',
    )
    if (sceneScope) {
      void refreshSceneFromDisk()
      // 结构文件决定章节树形态，树也要跟（ws 整树替换是本 store 既有惯例）
      if (files.has('structure.toml') || files.has('chapters.toml')) {
        chaptersVersion.value++
        void refresh()
      }
    } else {
      void refresh()
    }
    scheduleCompile()
    void refreshStats()
  }

  /** 重拉当前场景并按 §8.1 合并；场景在磁盘上消失则清空并提示（§10） */
  async function refreshSceneFromDisk(): Promise<void> {
    const sceneId = scene.value?.id
    if (!root.value || !sceneId) return
    let fresh: SceneDetailDto
    try {
      fresh = await api.getScene(root.value, sceneId)
    } catch {
      scene.value = null
      void refresh()
      ElMessage.warning('当前场景已被外部删除')
      return
    }
    if (scene.value?.id !== sceneId) return // 拉取期间用户已切换场景，放弃合并
    mergeSceneFromDisk(fresh)
  }

  /** 冲突提示去重：同一段提示过就不再重复（保存成功后清除） */
  const conflictedNotified = new Set<string>()

  function notifyConflict(fragmentId: string): void {
    if (conflictedNotified.has(fragmentId)) return
    conflictedNotified.add(fragmentId)
    ElMessage.warning('本段在磁盘上已被外部修改，保存时将以你正在编辑的内容为准')
  }

  /**
   * §8.1 片段合并——与 saveFragment 的防闪回守卫同源：
   * saved 且 text 相同 → 不赋值（CodeMirror 不换文档，滚动/选区不动）；
   * dirty / saving / error → 绝不回写 text，盘面内容变化即冲突提示。
   */
  function mergeSceneFromDisk(fresh: SceneDetailDto): void {
    const cur = scene.value
    if (!cur || cur.id !== fresh.id) return
    // 场景元数据不在编辑器保护范围，照单跟随
    cur.title = fresh.title
    cur.pov = fresh.pov
    cur.mood = fresh.mood
    cur.function = fresh.function
    cur.summary = fresh.summary
    cur.storyTime = fresh.storyTime

    const stateOf = (id: string) => fragmentSave[id] ?? 'saved'
    const freshById = new Map(fresh.fragments.map((f) => [f.id, f]))
    const anyUnsaved = cur.fragments.some((f) => stateOf(f.id) !== 'saved')

    for (const f of cur.fragments) {
      const nf = freshById.get(f.id)
      if (!nf) {
        // 磁盘已删除：dirty 片段保留待用户处理（提示冲突），saved 随盘移除
        if (stateOf(f.id) !== 'saved') notifyConflict(f.id)
        continue
      }
      f.anchors = nf.anchors
      if (stateOf(f.id) === 'saved') {
        if (nf.text !== f.text) {
          f.text = nf.text
          f.wordCount = nf.wordCount
        } else if (nf.wordCount !== f.wordCount) {
          f.wordCount = nf.wordCount
        }
      } else if (nf.text !== f.text && nf.text !== savingText.get(f.id)) {
        // f.text 是最近一次落盘内容，盘面与它不同即外部改写；
        // saving 态排除自己刚写进去的那份在途文本
        notifyConflict(f.id)
      }
    }

    const removedSaved = new Set(
      cur.fragments
        .filter((f) => !freshById.has(f.id) && stateOf(f.id) === 'saved')
        .map((f) => f.id),
    )
    const added = fresh.fragments.filter((nf) => !cur.fragments.some((f) => f.id === nf.id))
    for (const nf of added) fragmentSave[nf.id] = 'saved'

    if (!anyUnsaved) {
      // 全部已保存：完全跟随盘面（顺序 / 增 / 删）
      cur.fragments = fresh.fragments.map((nf) => cur.fragments.find((f) => f.id === nf.id) ?? nf)
    } else {
      // 有未保存编辑：保持现有顺序、尾部追加新增，重排等保存后自然对齐
      cur.fragments = cur.fragments.filter((f) => !removedSaved.has(f.id)).concat(added)
    }
  }

  /** §10.1 暂存：AI 流式 / 流水线批量写期间不实时分流，结束瞬间合成大批次补推 */
  const STASH_TIMEOUT_MS = 5 * 60_000
  let stashActive = false
  let stashFiles = new Set<string>()
  let stashTimer: ReturnType<typeof setTimeout> | null = null

  function stashFsFiles(files: string[]): void {
    if (!stashActive) {
      stashActive = true
      stashTimer = setTimeout(() => {
        // 超时降级：流水线异常挂住——全量兜底并恢复实时处理
        exitStash(false)
        void refresh()
        scheduleCompile()
      }, STASH_TIMEOUT_MS)
    }
    for (const f of files) stashFiles.add(f)
  }

  /** 退出暂存；flush=true 把累积路径作为合成大批次走 §8.2 分流 */
  function exitStash(flush: boolean): void {
    if (stashTimer) {
      clearTimeout(stashTimer)
      stashTimer = null
    }
    const batch = stashFiles
    stashFiles = new Set()
    stashActive = false
    if (flush && batch.size > 0 && root.value) routeFsBatch(batch)
  }

  // 流结束（busyWriting 回落）即补推暂存批次
  watch(busyWriting, (busy, was) => {
    if (was && !busy && stashActive) exitStash(true)
  })

  /** 事件订阅（store 单例，只注册一次；非 Tauri 环境静默失败） */
  async function setupFsWatch(): Promise<void> {
    try {
      await onFsChanged(onFsFiles)
      await onWatchState((st) => {
        watchState.value = st
        if (st.state === 'stopped' && st.reason) {
          ElMessage.warning(`文件监听已停止：${st.reason}`)
        }
      })
    } catch {
      // 纯浏览器环境无 Tauri 事件可订阅
    }
  }
  void setupFsWatch()

  /** 文件监听跟随项目（对偶 mcpFollowProject）：随打开启动，切书自动重启 */
  async function watchFollowProject(newRoot: string): Promise<void> {
    try {
      watchState.value = await api.watchStart(newRoot)
    } catch {
      // 监听不可用不阻塞项目打开
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
    chaptersVersion,
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
    setFragmentAnchor,
    removeFragmentAnchor,
    aiAnchorFragment,
    createChapter,
    renameChapter,
    deleteChapter,
    createScene,
    deleteScene,
    createEntity,
    saveEntity,
    deleteEntity,
    mergeEntities,
    resolveUnresolved,
    ignoreUnresolvedRefs,
    deleteUnresolvedRefs,
    addRelation,
    removeRelation,
    updateRelation,
    createThread,
    updateThread,
    advanceThread,
    deleteThread,
    setThreadKnowledge,
    removeThreadKnowledge,
    scheduleCompile,
    retrySave,
    loadVersions,
    readVersion,
    restoreVersion,
    diffVersion,
    stats,
    refreshStats,
    setDailyGoal,
    watchState,
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
