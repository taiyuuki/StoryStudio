<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useWorkspaceStore } from '@/stores/workspace'
import { api } from '@/api/backend'
import type {
  ChapterOutlineItemDto,
  VolumeGroupDto,
} from '@/api/backend'

const store = useWorkspaceStore()

const loading = ref(true)
const saving = ref(false)
const outline = reactive<{ volumes: VolumeGroupDto[]; chapters: ChapterOutlineItemDto[] }>({
  volumes: [],
  chapters: [],
})

const STATUS_META: Record<string, { label: string; cls: string }> = {
  planned: { label: '待写', cls: 'st-planned' },
  writing: { label: '写作中', cls: 'st-writing' },
  completed: { label: '已完成', cls: 'st-completed' },
}

const statusMeta = (s: string) => STATUS_META[s] ?? { label: s, cls: 'st-planned' }

// ── 筛选（长篇定位用：关键词 / 状态） ────────────────────────
const query = ref('')
const statusFilter = ref('')

const filtering = computed(() => query.value.trim() !== '' || statusFilter.value !== '')

function chapterMatches(c: ChapterOutlineItemDto): boolean {
  if (statusFilter.value && c.status !== statusFilter.value) return false
  const q = query.value.trim().toLowerCase()
  if (!q) return true
  return [c.title, c.summary, c.goal, c.hook, c.notes, ...c.beats]
    .filter(Boolean)
    .some((s) => s!.toLowerCase().includes(q))
}

// ── 分组视图（卷按 order 升序，未分卷殿后） ─────────────────
interface Group {
  key: string
  order: number
  volume: VolumeGroupDto | null
  items: { gi: number; ch: ChapterOutlineItemDto }[]
}

const groups = computed<Group[]>(() => {
  const vols = [...outline.volumes].sort((a, b) => a.order - b.order)
  const result: Group[] = vols.map((v) => ({
    key: `v-${v.order}`,
    order: v.order,
    volume: v,
    items: [],
  }))
  const loose: Group = { key: 'v-0', order: 0, volume: null, items: [] }
  outline.chapters.forEach((ch, gi) => {
    if (!chapterMatches(ch)) return
    const g = result.find((x) => x.order === ch.volumeOrder) ?? loose
    g.items.push({ gi, ch })
  })
  if (loose.items.length > 0) result.push(loose)
  return filtering.value ? result.filter((g) => g.items.length > 0) : result
})

const completedCount = computed(
  () => outline.chapters.filter((c) => c.status === 'completed').length,
)

const volumeLabel = (v: VolumeGroupDto | null) =>
  v ? `第${v.order}卷 · ${v.name}` : '未分卷'

/** 空项目手动加章时默认落入第一卷 */
const firstVolumeOrder = computed(
  () => [...outline.volumes].sort((a, b) => a.order - b.order)[0]?.order ?? 0,
)

// ── 卷折叠（长篇只渲染展开的卷） ─────────────────────────────
const expanded = ref<Set<number>>(new Set())

const isExpanded = (order: number) => filtering.value || expanded.value.has(order)

function toggleGroup(order: number) {
  const next = new Set(expanded.value)
  if (next.has(order)) next.delete(order)
  else next.add(order)
  expanded.value = next
}

/** 展开当前写作位置所在的卷（首个未完成章）；全部完成则收起 */
function expandWritingVolume() {
  const target = outline.chapters.find((c) => c.status !== 'completed')
  expanded.value = target ? new Set([target.volumeOrder]) : new Set()
}

// ── 角色与线索选项 ──────────────────────────────────────────
const characterOptions = computed(() =>
  (store.ws?.entities ?? [])
    .filter((e) => e.kind === 'character')
    .map((e) => ({ value: e.id, label: `${e.displayName}（${e.id}）` })),
)

const threadOptions = computed(() =>
  (store.ws?.threads ?? []).map((t) => ({ value: t.id, label: t.title })),
)

const nameOfCharacter = (id: string) =>
  (store.ws?.entities ?? []).find((e) => e.id === id)?.displayName ?? id

// ── 加载 / 保存 ─────────────────────────────────────────────
async function load() {
  if (!store.root) return
  try {
    const data = await api.getChapterOutline(store.root)
    outline.volumes = data.volumes
    outline.chapters = data.chapters
    expandWritingVolume()
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    loading.value = false
  }
}

async function persist() {
  if (!store.root) return
  saving.value = true
  try {
    await api.saveChapterOutline(store.root, {
      volumes: outline.volumes,
      chapters: outline.chapters,
    })
    // 后端会做清理（空标题丢弃、卷引用校正），保存后重新拉齐
    const data = await api.getChapterOutline(store.root)
    outline.volumes = data.volumes
    outline.chapters = data.chapters
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    saving.value = false
  }
}

onMounted(load)

// ── 卷操作 ──────────────────────────────────────────────────
const volEditing = ref(-1)
const volDraft = ref({ name: '', theme: '', summary: '' })

function volIndex(order: number) {
  return outline.volumes.findIndex((v) => v.order === order)
}

function startEditVolume(order: number) {
  const v = outline.volumes[volIndex(order)]
  if (!v) return
  volEditing.value = order
  volDraft.value = { name: v.name, theme: v.theme ?? '', summary: v.summary ?? '' }
}

function saveVolume(order: number) {
  const i = volIndex(order)
  if (i < 0) return
  const d = volDraft.value
  if (!d.name.trim()) return
  outline.volumes[i] = {
    ...outline.volumes[i],
    name: d.name.trim(),
    theme: d.theme.trim() || null,
    summary: d.summary.trim() || null,
  }
  volEditing.value = -1
  void persist()
}

function addVolume() {
  const next = outline.volumes.reduce((m, v) => Math.max(m, v.order), 0) + 1
  outline.volumes.push({ name: `第${next}卷`, order: next, theme: null, summary: null })
  expanded.value = new Set([...expanded.value, next])
  void persist()
}

/** 卷上移/下移 = 与相邻卷交换 order，并把两组章节的卷引用对调 */
function moveVolume(order: number, dir: -1 | 1) {
  const sorted = [...outline.volumes].sort((a, b) => a.order - b.order)
  const idx = sorted.findIndex((v) => v.order === order)
  const j = idx + dir
  if (idx < 0 || j < 0 || j >= sorted.length) return
  const a = sorted[idx]
  const b = sorted[j]
  const ao = a.order
  const bo = b.order
  a.order = bo
  b.order = ao
  for (const c of outline.chapters) {
    if (c.volumeOrder === ao) c.volumeOrder = bo
    else if (c.volumeOrder === bo) c.volumeOrder = ao
  }
  volEditing.value = -1
  void persist()
}

/** 删卷不删章：该卷章节归入未分卷 */
function deleteVolume(order: number) {
  const i = volIndex(order)
  if (i < 0) return
  outline.volumes.splice(i, 1)
  for (const c of outline.chapters) {
    if (c.volumeOrder === order) c.volumeOrder = 0
  }
  volEditing.value = -1
  void persist()
}

// ── 章节操作 ────────────────────────────────────────────────
const chEditing = ref(-1)
const chDraft = ref({
  title: '',
  volumeOrder: 0,
  status: 'planned',
  summary: '',
  goal: '',
  beats: '',
  characters: [] as string[],
  threads: [] as string[],
  hook: '',
  notes: '',
})

function startEditChapter(gi: number) {
  const c = outline.chapters[gi]
  if (!c) return
  chEditing.value = gi
  chDraft.value = {
    title: c.title,
    volumeOrder: c.volumeOrder,
    status: c.status,
    summary: c.summary ?? '',
    goal: c.goal ?? '',
    beats: c.beats.join('\n'),
    characters: [...c.characters],
    threads: [...c.threads],
    hook: c.hook ?? '',
    notes: c.notes ?? '',
  }
}

function saveChapter(gi: number) {
  const d = chDraft.value
  if (!d.title.trim()) return
  outline.chapters[gi] = {
    ...outline.chapters[gi],
    title: d.title.trim(),
    volumeOrder: d.volumeOrder,
    status: d.status,
    summary: d.summary.trim() || null,
    goal: d.goal.trim() || null,
    beats: d.beats
      .split('\n')
      .map((s) => s.trim())
      .filter(Boolean),
    characters: [...d.characters],
    threads: [...d.threads],
    hook: d.hook.trim() || null,
    notes: d.notes.trim() || null,
  }
  chEditing.value = -1
  void persist()
}

function addChapter(order: number) {
  const last = outline.chapters[outline.chapters.length - 1]
  const volumeOrder =
    order !== 0 && outline.volumes.some((v) => v.order === order)
      ? order
      : (last?.volumeOrder ?? 0)
  outline.chapters.push({
    chapterId: null,
    title: '',
    volumeOrder,
    summary: null,
    goal: null,
    beats: [],
    characters: [],
    threads: [],
    scenes: [],
    hook: null,
    notes: null,
    status: 'planned',
  })
  expanded.value = new Set([...expanded.value, volumeOrder])
  chEditing.value = outline.chapters.length - 1
  startEditChapter(chEditing.value)
}

async function deleteChapter(gi: number) {
  const ch = outline.chapters[gi]
  if (!ch) return
  // 已写成正文的章节：强确认（误删会丢失整章规划，正文也会失去对应）
  const written = ch.chapterId != null || ch.status === 'completed'
  const ok = await ElMessageBox.confirm(
    written
      ? `《${ch.title || '未命名'}》已经写成正文了。删除后正文内容仍在，但这一章的目标、节拍、出场角色等规划会一并丢失，正文章节也会失去对应。`
      : `删除《${ch.title || '未命名'}》的章节脉络？之后可以随时重新添加。`,
    written ? '删除已完成的章节？' : '删除章节脉络',
    {
      confirmButtonText: written ? '仍然删除' : '删除',
      cancelButtonText: '先不删',
      type: written ? 'warning' : 'info',
    },
  )
    .then(() => true)
    .catch(() => false)
  if (!ok) return
  outline.chapters.splice(gi, 1)
  chEditing.value = -1
  void persist()
}

function moveChapter(gi: number, dir: -1 | 1) {
  const j = gi + dir
  if (j < 0 || j >= outline.chapters.length) return
  ;[outline.chapters[gi], outline.chapters[j]] = [outline.chapters[j], outline.chapters[gi]]
  chEditing.value = -1
  void persist()
}
</script>

<template>
  <div class="chapters-panel">
    <div v-if="loading" class="cp-loading">
      <i class="fa-solid fa-circle-notch fa-spin" />
    </div>

    <template v-else>
      <p class="cp-hint">
        整理分卷与每一章的脉络——「逐章成书」会按这里的顺序动笔。
        共 {{ outline.chapters.length }} 章 · 已完成 {{ completedCount }}。
        <span v-if="saving" class="cp-saving"><i class="fa-solid fa-floppy-disk" /> 保存中…</span>
      </p>

      <!-- 筛选工具条 -->
      <div class="cp-toolbar">
        <input
          v-model="query"
          class="cp-input"
          placeholder="搜标题 / 梗概 / 节拍…"
        />
        <select v-model="statusFilter" class="cp-status-filter">
          <option value="">全部状态</option>
          <option value="planned">待写</option>
          <option value="writing">写作中</option>
          <option value="completed">已完成</option>
        </select>
      </div>

      <div v-for="g in groups" :key="g.key" class="cp-group">
        <!-- 组头：点击整行折叠/展开；管理按钮悬停显示 -->
        <div class="cp-group-head" @click="toggleGroup(g.order)">
          <i
            class="fa-solid fa-chevron-right cp-chevron"
            :class="{ 'is-open': isExpanded(g.order) }"
          />
          <span class="cp-group-title">{{ volumeLabel(g.volume) }}</span>
          <span v-if="g.volume?.theme" class="cp-vol-theme" :title="g.volume.theme">
            {{ g.volume.theme }}
          </span>
          <span class="num">{{ g.items.length }} 章</span>
          <div v-if="g.volume" class="cp-head-actions" @click.stop>
            <button class="cp-act" title="编辑卷" @click="startEditVolume(g.volume.order)">
              <i class="fa-solid fa-pen" />
            </button>
            <button class="cp-act" title="卷上移" @click="moveVolume(g.volume.order, -1)">
              <i class="fa-solid fa-arrow-up" />
            </button>
            <button class="cp-act" title="卷下移" @click="moveVolume(g.volume.order, 1)">
              <i class="fa-solid fa-arrow-down" />
            </button>
            <button
              class="cp-act cp-act--danger"
              title="删除这一卷（章节保留，归入未分卷）"
              @click="deleteVolume(g.volume.order)"
            >
              <i class="fa-regular fa-trash-can" />
            </button>
          </div>
        </div>

        <!-- 卷编辑 -->
        <div v-if="g.volume && volEditing === g.volume.order" class="cp-edit">
          <input v-model="volDraft.name" class="cp-input" placeholder="卷名" />
          <input v-model="volDraft.theme" class="cp-input" placeholder="主题（可选）" />
          <textarea v-model="volDraft.summary" class="cp-input" rows="2" placeholder="本卷梗概（可选）" />
          <div class="cp-edit-row">
            <span style="flex: 1" />
            <button class="ink-btn ink-btn--sm" @click="volEditing = -1">取消</button>
            <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveVolume(g.volume.order)">保存</button>
          </div>
        </div>

        <!-- 章节列表（卷折叠时不渲染，长篇也能保持轻快） -->
        <template v-if="isExpanded(g.order)">
          <div
            v-for="{ gi, ch } in g.items"
            :key="gi"
            class="cp-card"
            :class="{ 'is-done': ch.status === 'completed' }"
          >
            <template v-if="chEditing === gi">
              <div class="cp-edit">
                <div class="cp-edit-grid">
                  <input v-model="chDraft.title" class="cp-input" placeholder="章节标题" />
                  <select v-model.number="chDraft.volumeOrder" class="cp-input">
                    <option :value="0">未分卷</option>
                    <option v-for="v in [...outline.volumes].sort((a, b) => a.order - b.order)" :key="v.order" :value="v.order">
                      第{{ v.order }}卷 · {{ v.name }}
                    </option>
                  </select>
                  <select v-model="chDraft.status" class="cp-input">
                    <option value="planned">待写</option>
                    <option value="writing">写作中</option>
                    <option value="completed">已完成</option>
                  </select>
                </div>
                <textarea v-model="chDraft.summary" class="cp-input" rows="2" placeholder="章节梗概" />
                <textarea v-model="chDraft.goal" class="cp-input" rows="2" placeholder="本章目标（推进什么剧情/角色弧线）" />
                <textarea v-model="chDraft.beats" class="cp-input" rows="3" placeholder="叙事节拍（一行一个）" />
                <div class="cp-edit-grid">
                  <div>
                    <label class="cp-label">出场角色</label>
                    <ElSelect v-model="chDraft.characters" multiple collapse-tags filterable placeholder="选择角色" size="small" class="cp-select">
                      <ElOption v-for="o in characterOptions" :key="o.value" :value="o.value" :label="o.label" />
                    </ElSelect>
                  </div>
                  <div>
                    <label class="cp-label">涉及线索</label>
                    <ElSelect v-model="chDraft.threads" multiple collapse-tags filterable placeholder="选择线索" size="small" class="cp-select">
                      <ElOption v-for="o in threadOptions" :key="o.value" :value="o.value" :label="o.label" />
                    </ElSelect>
                  </div>
                </div>
                <input v-model="chDraft.hook" class="cp-input" placeholder="章末钩子（让读者翻页的理由）" />
                <textarea v-model="chDraft.notes" class="cp-input" rows="2" placeholder="备注（可选）" />
                <div class="cp-edit-row">
                  <span style="flex: 1" />
                  <button class="ink-btn ink-btn--sm" @click="chEditing = -1">取消</button>
                  <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveChapter(gi)">保存</button>
                </div>
              </div>
            </template>
            <template v-else>
              <div class="cp-card-head">
                <span class="cp-idx num">{{ gi + 1 }}</span>
                <span class="cp-title" :title="ch.title">{{ ch.title || '（未命名）' }}</span>
                <span class="cp-status" :class="statusMeta(ch.status).cls">
                  {{ statusMeta(ch.status).label }}
                </span>
                <div class="cp-card-actions">
                  <button class="cp-act" title="编辑" @click="startEditChapter(gi)">
                    <i class="fa-solid fa-pen" />
                  </button>
                  <button class="cp-act" title="上移" :disabled="gi === 0" @click="moveChapter(gi, -1)">
                    <i class="fa-solid fa-arrow-up" />
                  </button>
                  <button class="cp-act" title="下移" :disabled="gi === outline.chapters.length - 1" @click="moveChapter(gi, 1)">
                    <i class="fa-solid fa-arrow-down" />
                  </button>
                  <button class="cp-act cp-act--danger" title="删除" @click="deleteChapter(gi)">
                    <i class="fa-regular fa-trash-can" />
                  </button>
                </div>
              </div>
              <p v-if="ch.summary" class="cp-summary">{{ ch.summary }}</p>
              <div class="cp-meta">
                <span v-if="ch.goal"><i class="fa-solid fa-bullseye" /> {{ ch.goal }}</span>
                <span v-if="ch.hook"><i class="fa-solid fa-anchor" /> {{ ch.hook }}</span>
                <span v-if="ch.beats.length"><i class="fa-solid fa-stairs" /> {{ ch.beats.length }} 拍</span>
                <span v-if="ch.characters.length">
                  <i class="fa-solid fa-user" /> {{ ch.characters.map(nameOfCharacter).join('、') }}
                </span>
                <span v-if="ch.chapterId || ch.scenes.length" class="cp-bound-info">
                  <i class="fa-solid fa-link" />
                  已写成正文{{ ch.scenes.length ? ` · ${ch.scenes.length} 个场景` : '' }}
                </span>
              </div>
            </template>
          </div>

          <div v-if="g.items.length === 0" class="cp-empty">
            {{ filtering ? '这一卷没有符合筛选的章节。' : '这一卷还没有章节。' }}
          </div>

          <button class="cp-add-chapter" @click="addChapter(g.order)">
            <i class="fa-solid fa-plus" /> 加一章
          </button>
        </template>
      </div>

      <div v-if="outline.chapters.length === 0" class="cp-empty cp-empty--hero">
        <i class="fa-regular fa-map" />
        <p>这本书还没有章节脉络。<br />先手动加一章，或去「逐章成书」让 AI 排一批。</p>
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="addChapter(firstVolumeOrder)">
          <i class="fa-solid fa-plus" />
          加一章
        </button>
      </div>

      <div v-if="filtering && groups.length === 0 && outline.chapters.length > 0" class="cp-empty">
        没有找到符合「{{ query }}」的章节。
      </div>

      <div class="cp-foot">
        <button class="cp-mini-btn" @click="addVolume">
          <i class="fa-solid fa-plus" /> 加一卷
        </button>
      </div>
    </template>
  </div>
</template>

<style scoped lang="scss">
// 面板自身作为滚动容器（顶缘无 padding，粘性卷头 top:0 在任何引擎下都严格贴边）
.chapters-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow-y: auto;
  margin: -14px -16px -20px;
  padding: 0 16px 20px;
}

.cp-loading {
  display: flex;
  justify-content: center;
  padding: 30px 0;
  color: var(--ink-muted);
}

.cp-hint {
  margin: 0;
  padding: 14px 0 0;
  font-size: 11.5px;
  line-height: 1.8;
  color: var(--ink-muted);
}

.cp-saving {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: 6px;
  color: var(--ink-accent);
}

// 筛选工具条（搜索框弹性伸展，状态下拉固定宽度不挤压）
.cp-toolbar {
  display: flex;
  gap: 8px;
  margin: 12px 0 0;

  .cp-input {
    flex: 1;
    min-width: 0;
  }
}

.cp-toolbar .cp-status-filter {
  flex: 0 0 88px;
  width: auto;
}

.cp-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 14px 0 0;
}

// 组头：滚动时通栏吸附在滚动区顶部——负边距吃掉 .cp-group 的左右留白
.cp-group-head {
  position: sticky;
  top: 0;
  z-index: 2;
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0 -16px;
  padding: 6px 10px 6px 16px;
  border-radius: 0;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--ink-main);
  cursor: pointer;
  user-select: none;
  background: var(--surface-1);
  backdrop-filter: blur(8px);
  box-shadow: 0 1px 0 var(--divider);

  &:hover {
    background: var(--surface-2);
  }

  .num {
    font-weight: 400;
    font-size: 11px;
    color: var(--ink-disabled);
    flex-shrink: 0;
  }
}

.cp-chevron {
  font-size: 10px;
  color: var(--ink-muted);
  transition: transform 0.2s;

  &.is-open {
    transform: rotate(90deg);
  }
}

.cp-group-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cp-vol-theme {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 400;
  font-size: 11px;
  color: var(--ink-muted);
}

// 组头管理按钮：悬停浮现，不与标题抢空间
.cp-head-actions {
  position: absolute;
  right: 4px;
  top: 50%;
  transform: translateY(-50%);
  display: flex;
  gap: 2px;
  padding: 0 4px;
  border-radius: 7px;
  background: linear-gradient(90deg, transparent, var(--surface-2) 18%);
  opacity: 0;
  transition: opacity 0.15s;

  .cp-group-head:hover & {
    opacity: 1;
  }
}

.cp-mini-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 9px;
  border: 1px solid var(--ui-border);
  border-radius: 7px;
  background: transparent;
  color: var(--ink-muted);
  font-family: inherit;
  font-size: 11.5px;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    border-color: var(--ink-accent);
    color: var(--ink-accent);
  }
}

.cp-edit {
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 12px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);
}

.cp-edit-grid {
  display: flex;
  gap: 8px;

  > * {
    flex: 1;
    min-width: 0;
  }
}

.cp-edit-row {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.cp-input {
  width: 100%;
  padding: 7px 10px;
  border-radius: 7px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 12.5px;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }
}

.cp-edit textarea {
  resize: vertical;
  line-height: 1.6;
}

.cp-label {
  display: block;
  margin: 2px 0 4px;
  font-size: 11px;
  color: var(--ink-muted);
}

.cp-select {
  width: 100%;
}

// 章节卡片：标题行不换行，操作悬停浮现
.cp-card {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 9px 12px;
  border-radius: 11px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  transition: border-color 0.2s;

  &:hover {
    border-color: var(--ui-border-hover);
  }

  &.is-done {
    opacity: 0.78;
  }
}

.cp-card-head {
  position: relative;
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.cp-idx {
  width: 20px;
  flex-shrink: 0;
  font-size: 11px;
  color: var(--ink-disabled);
  text-align: right;
}

.cp-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
  font-weight: 600;
  color: var(--ink-main);
}

.cp-status {
  flex-shrink: 0;
  padding: 1.5px 7px;
  border-radius: 4px;
  font-size: 10.5px;
  white-space: nowrap;

  &.st-planned {
    background: var(--surface-2);
    color: var(--ink-muted);
  }

  &.st-writing {
    background: color-mix(in srgb, var(--ink-warning) 15%, transparent);
    color: var(--ink-warning);
  }

  &.st-completed {
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
  }
}

// 卡片操作按钮：悬停浮现（渐变底保证盖住标题尾部）
.cp-card-actions {
  position: absolute;
  right: -4px;
  top: 50%;
  transform: translateY(-50%);
  display: flex;
  gap: 2px;
  padding: 0 4px;
  border-radius: 7px;
  background: linear-gradient(90deg, transparent, var(--surface-1) 22%);
  opacity: 0;
  transition: opacity 0.15s;

  .cp-card:hover & {
    opacity: 1;
  }
}

.cp-summary {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-sec);
}

.cp-meta {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 11px;
  line-height: 1.6;
  color: var(--ink-muted);

  span {
    display: inline-flex;
    align-items: baseline;
    gap: 5px;
    min-width: 0;

    i {
      font-size: 9.5px;
      margin-top: 2px;
      flex-shrink: 0;
    }
  }
}

.cp-bound-info {
  color: var(--ink-disabled);
}

.cp-act {
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 22px;
  height: 22px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 10.5px;
  flex-shrink: 0;

  &:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &:disabled {
    opacity: 0.3;
    cursor: default;
  }

  &--danger:hover {
    background: var(--state-danger);
    color: #fff;
  }
}

// 组内加章：显眼的虚线按钮，不藏在悬停操作里
.cp-add-chapter {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 30px;
  border: 1px dashed var(--ui-border);
  border-radius: 9px;
  background: transparent;
  color: var(--ink-muted);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    border-color: var(--ink-accent);
    color: var(--ink-accent);
    background: var(--ink-accent-soft);
  }
}

.cp-empty {
  padding: 12px;
  border-radius: 10px;
  border: 1px dashed var(--divider);
  font-size: 12px;
  color: var(--ink-disabled);
  text-align: center;

  &--hero {
    padding: 26px 12px;
    line-height: 2;

    i {
      font-size: 22px;
      opacity: 0.5;
    }

    p {
      margin: 8px 0 10px;
    }

    .ink-btn {
      margin: 0 auto;
    }
  }
}

// 面板直挂的空态（卷外）：侧边距已由面板 padding 提供
.chapters-panel > .cp-empty {
  margin: 12px 0 0;
}

.cp-foot {
  display: flex;
  justify-content: center;
  margin-top: 12px;
  padding-top: 2px;
}
</style>
