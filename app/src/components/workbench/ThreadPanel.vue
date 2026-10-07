<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import type { ThreadDto, ThreadKnowledgeDto, VolumeGroupDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

const creating = ref(false)
const form = ref({ title: '', description: '', priority: 'minor', dueVolume: 0 })
const editingId = ref<string | null>(null)
const editDraft = ref({ title: '', description: '', goal: '', priority: 'minor', dueVolume: 0 })

/** 卷列表（计划埋设卷下拉用）；0 = 全书长线 */
const volumes = ref<VolumeGroupDto[]>([])
onMounted(async () => {
  if (!store.root) return
  try {
    const data = await api.getChapterOutline(store.root)
    volumes.value = [...data.volumes].sort((a, b) => a.order - b.order)
  } catch {
    volumes.value = []
  }
})

const volumeOptions = computed(() => {
  const orders = new Set(volumes.value.map((v) => v.order))
  // 线索上已设置的卷号不在卷列表里时也保留选项
  for (const t of store.ws?.threads ?? []) {
    if (t.dueVolume) orders.add(t.dueVolume)
  }
  return [...orders].sort((a, b) => a - b)
})

function dueLabel(t: ThreadDto): string {
  return t.dueVolume ? `第${t.dueVolume}卷` : '全书长线'
}

const threads = computed(() => {
  const list = [...(store.ws?.threads ?? [])]
  return list.sort((a, b) => {
    if (a.priority === b.priority) return a.id.localeCompare(b.id)
    return a.priority === 'main' ? -1 : 1
  })
})

type ThreadStatus = 'planned' | 'planted' | 'revealed' | 'resolved' | 'abandoned'

function statusOf(t: ThreadDto): ThreadStatus {
  if (t.abandoned) return 'abandoned'
  if (t.resolvedAt) return 'resolved'
  if (t.revealedAt) return 'revealed'
  if (t.plantedAt) return 'planted'
  return 'planned'
}

const statusMeta: Record<ThreadStatus, { label: string; cls: string }> = {
  planned: { label: '构想', cls: 'st-planned' },
  planted: { label: '已埋设', cls: 'st-planted' },
  revealed: { label: '已揭示', cls: 'st-revealed' },
  resolved: { label: '已回收', cls: 'st-resolved' },
  abandoned: { label: '弃用', cls: 'st-abandoned' },
}

function anchorScene(id: string | null): string {
  if (!id) return ''
  const scene = store.ws?.chapters.flatMap((c) => c.scenes).find((s) => s.id === id)
  return scene?.title ?? id
}

async function create() {
  const f = form.value
  if (!f.title.trim()) {
    creating.value = false
    return
  }
  await store.createThread(
    f.title.trim(),
    f.description.trim(),
    f.priority,
    f.dueVolume || null,
  )
  creating.value = false
  form.value = { title: '', description: '', priority: 'minor', dueVolume: 0 }
}

function startEdit(t: ThreadDto) {
  editingId.value = t.id
  editDraft.value = {
    title: t.title,
    description: t.description ?? '',
    goal: t.goal ?? '',
    priority: t.priority,
    dueVolume: t.dueVolume ?? 0,
  }
}

async function saveEdit(id: string) {
  const title = editDraft.value.title.trim()
  if (!title) {
    ElMessage.warning('线索标题不能为空')
    return
  }
  await store.updateThread(id, {
    title,
    description: editDraft.value.description.trim(),
    goal: editDraft.value.goal.trim(),
    priority: editDraft.value.priority,
    // 0 = 清除，恢复全书长线
    dueVolume: editDraft.value.dueVolume,
  })
  editingId.value = null
}

// ── 线索可见性（观察者 → 状态时间线） ─────────────────────

const knowAdding = ref<string | null>(null)
const knowForm = ref({ observer: 'reader', state: 'known', sceneId: '' })

const stateMeta: Record<string, { label: string; cls: string }> = {
  unknown: { label: '不知', cls: 'kn-unknown' },
  suspected: { label: '起疑', cls: 'kn-suspected' },
  known: { label: '知晓', cls: 'kn-known' },
}

/** 按阅读顺序的场景选项（章节·场景），并给出排序序号 */
const sceneOptions = computed(() => {
  const out: { id: string; label: string }[] = []
  for (const c of store.ws?.chapters ?? []) {
    for (const s of c.scenes) out.push({ id: s.id, label: `${c.title} · ${s.title}` })
  }
  return out
})
const sceneOrder = computed(() => {
  const m = new Map<string, number>()
  sceneOptions.value.forEach((s, i) => m.set(s.id, i))
  return m
})

/** 观察者下拉：叙述层 + 角色 + 组织 */
const observerOptions = computed(() => {
  const entities = store.ws?.entities ?? []
  return [
    {
      group: '叙述层',
      items: [
        { value: 'reader', label: '读者' },
        { value: 'author', label: '作者' },
        { value: 'omniscient', label: '全知视角' },
      ],
    },
    {
      group: '角色',
      items: entities
        .filter((x) => x.kind === 'character')
        .map((x) => ({ value: x.id, label: x.displayName })),
    },
    {
      group: '组织',
      items: entities
        .filter((x) => x.kind === 'organization')
        .map((x) => ({ value: x.id, label: x.displayName })),
    },
  ]
})

function observerLabel(k: ThreadKnowledgeDto): string {
  if (k.observerKind === 'reader') return '读者'
  if (k.observerKind === 'author') return '作者'
  if (k.observerKind === 'omniscient') return '全知'
  if (k.observerEntity) return store.entityName(k.observerEntity)
  return k.observerKind
}

/** 观察者记录回传后端的原始串：内置用 kind，实体用 ID（c_/o_ 前缀） */
function observerKey(k: ThreadKnowledgeDto): string {
  return k.observerEntity ?? k.observerKind
}

/** 时间线按阅读顺序展示（后端存储按场景 ID 排序） */
function sortedTimeline(k: ThreadKnowledgeDto) {
  const order = sceneOrder.value
  return [...k.stateTimeline].sort(
    (a, b) => (order.get(a.scene) ?? 1e9) - (order.get(b.scene) ?? 1e9),
  )
}

function openKnowledge(t: ThreadDto) {
  knowAdding.value = t.id
  knowForm.value = {
    observer: 'reader',
    state: 'known',
    sceneId: store.scene?.id ?? sceneOptions.value[0]?.id ?? '',
  }
}

async function saveKnowledge(threadId: string) {
  const f = knowForm.value
  if (!f.sceneId) {
    ElMessage.warning('请选择场景')
    return
  }
  const ok = await store.setThreadKnowledge(threadId, f.observer, f.state, f.sceneId)
  if (ok) knowAdding.value = null
}

function removeKnowledge(t: ThreadDto, k: ThreadKnowledgeDto) {
  void store.removeThreadKnowledge(t.id, observerKey(k))
}
</script>

<template>
  <div class="thread-panel">
    <div class="tp-head">
      <span class="text-ink-muted">叙事线索 · {{ threads.length }}</span>
      <button class="ink-btn ink-btn--sm ink-btn--primary" @click="creating = true">
        <i class="fa-solid fa-plus" />
        新线索
      </button>
    </div>

    <div v-if="creating" class="tp-create">
      <input v-model="form.title" placeholder="线索标题（例：玉佩之谜）" autofocus
        @keyup.enter="create" @keyup.escape="creating = false" />
      <textarea v-model="form.description" rows="2" placeholder="描述（可选）" />
      <div class="tp-create-row">
        <select v-model="form.priority">
          <option value="main">主线</option>
          <option value="minor">支线</option>
        </select>
        <select v-model="form.dueVolume" title="计划埋设卷">
          <option :value="0">全书长线</option>
          <option v-for="v in volumeOptions" :key="v" :value="v">第{{ v }}卷埋设</option>
        </select>
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="create">创建</button>
        <button class="ink-btn ink-btn--sm" @click="creating = false">取消</button>
      </div>
    </div>

    <div v-if="threads.length === 0 && !creating" class="tp-empty">
      <i class="fa-solid fa-route" />
      <p>叙事线索是伏笔与回收的骨架。<br />创建第一条，让故事有脉可循。</p>
    </div>

    <div v-for="t in threads" :key="t.id" class="thread-card" :class="{ 'is-main': t.priority === 'main' }">
      <template v-if="editingId === t.id">
        <input v-model="editDraft.title" class="ed-input" />
        <textarea v-model="editDraft.description" class="ed-input" rows="2" placeholder="描述（可选）" />
        <textarea v-model="editDraft.goal" class="ed-input" rows="2" placeholder="叙事目标" />
        <div class="tp-create-row">
          <select v-model="editDraft.priority">
            <option value="main">主线</option>
            <option value="minor">支线</option>
          </select>
          <select v-model="editDraft.dueVolume" title="计划埋设卷；到卷未埋会升级为「本卷该埋未埋」">
            <option :value="0">全书长线</option>
            <option v-for="v in volumeOptions" :key="v" :value="v">第{{ v }}卷埋设</option>
          </select>
          <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveEdit(t.id)">保存</button>
          <button class="ink-btn ink-btn--sm" @click="editingId = null">取消</button>
        </div>
      </template>
      <template v-else>
        <div class="tc-head">
          <span class="tc-priority" :class="t.priority === 'main' ? 'is-main' : 'is-minor'">
            {{ t.priority === 'main' ? '主线' : '支线' }}
          </span>
          <span class="tc-status" :class="statusMeta[statusOf(t)].cls">
            {{ statusMeta[statusOf(t)].label }}
          </span>
          <span
            v-if="statusOf(t) === 'planned' && t.dueVolume"
            class="tc-due"
            title="计划埋设卷；到卷已完稿仍未埋会提示「本卷该埋未埋」"
          >
            <i class="fa-regular fa-clock" /> {{ dueLabel(t) }}
          </span>
          <span style="flex: 1" />
          <button class="tc-act" title="编辑" @click="startEdit(t)">
            <i class="fa-solid fa-pen" />
          </button>
          <button class="tc-act tc-act--danger" title="删除" @click="store.deleteThread(t.id)">
            <i class="fa-regular fa-trash-can" />
          </button>
        </div>
        <div class="tc-title">{{ t.title }}</div>
        <p v-if="t.description" class="tc-desc">{{ t.description }}</p>
        <p v-if="t.goal" class="tc-goal"><i class="fa-solid fa-bullseye" /> {{ t.goal }}</p>
        <div v-if="anchorScene(t.plantedAt) || anchorScene(t.resolvedAt)" class="tc-anchors">
          <span v-if="anchorScene(t.plantedAt)">
            <i class="fa-solid fa-seedling" /> 埋设于「{{ anchorScene(t.plantedAt) }}」
          </span>
          <span v-if="anchorScene(t.resolvedAt)">
            <i class="fa-solid fa-check-double" /> 回收于「{{ anchorScene(t.resolvedAt) }}」
          </span>
        </div>

        <!-- 可见性：观察者对该线索的知晓状态时间线 -->
        <div v-if="t.knowledge.length || knowAdding === t.id" class="tc-know">
          <div class="tk-head">
            <span><i class="fa-regular fa-eye" /> 可见性</span>
            <button class="tk-act" title="记录可见性" @click="openKnowledge(t)">
              <i class="fa-solid fa-plus" />
            </button>
          </div>
          <div
            v-for="k in t.knowledge"
            :key="observerKey(k)"
            class="tk-row"
          >
            <span class="tk-observer">{{ observerLabel(k) }}</span>
            <span class="tk-line">
              <template v-for="(e, i) in sortedTimeline(k)" :key="e.scene">
                <i v-if="i > 0" class="fa-solid fa-arrow-right-long tk-arrow" />
                <span class="tk-point" :class="stateMeta[e.state]?.cls" :title="`${anchorScene(e.scene)} · ${stateMeta[e.state]?.label ?? e.state}`">
                  {{ anchorScene(e.scene) }}·{{ stateMeta[e.state]?.label ?? e.state }}
                </span>
              </template>
            </span>
            <button
              class="tk-act tk-act--danger"
              title="删除该观察者的记录"
              @click="removeKnowledge(t, k)"
            >
              <i class="fa-solid fa-xmark" />
            </button>
          </div>
          <div v-if="knowAdding === t.id" class="tk-form">
            <div class="tp-create-row">
              <select v-model="knowForm.observer" title="观察者">
                <optgroup v-for="g in observerOptions" :key="g.group" :label="g.group">
                  <option v-for="o in g.items" :key="o.value" :value="o.value">
                    {{ o.label }}
                  </option>
                </optgroup>
              </select>
              <select v-model="knowForm.state" title="知晓程度">
                <option value="unknown">不知晓</option>
                <option value="suspected">起疑</option>
                <option value="known">知晓</option>
              </select>
            </div>
            <div class="tp-create-row">
              <select v-model="knowForm.sceneId" title="叙事锚点场景">
                <option value="" disabled>选择场景…</option>
                <option v-for="s in sceneOptions" :key="s.id" :value="s.id">
                  {{ s.label }}
                </option>
              </select>
            </div>
            <div class="tp-create-row">
              <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveKnowledge(t.id)">
                记录
              </button>
              <button class="ink-btn ink-btn--sm" @click="knowAdding = null">取消</button>
            </div>
          </div>
        </div>
        <button
          v-else
          class="tk-entry"
          title="记录各观察者何时知晓这条线索"
          @click="openKnowledge(t)"
        >
          <i class="fa-regular fa-eye" /> 记可见性
        </button>
      </template>
    </div>
  </div>
</template>

<style scoped lang="scss">
.thread-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.tp-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
}

.tp-create {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);
}

.ed-input,
.tp-create input,
.tp-create textarea {
  width: 100%;
  padding: 8px 11px;
  border-radius: 7px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 14px;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }
}

.ed-input + .ed-input,
textarea.ed-input {
  resize: vertical;
  line-height: 1.6;
}

.tp-create textarea {
  resize: vertical;
  line-height: 1.6;
}

.tp-create-row {
  display: flex;
  gap: 8px;

  select {
    flex: 1;
    height: 28px;
    padding: 0 6px;
    border-radius: 6px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12px;
    outline: none;
  }
}

.tp-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 36px 12px;
  text-align: center;
  color: var(--ink-disabled);
  font-size: 12.5px;
  line-height: 2;

  i {
    font-size: 26px;
    opacity: 0.7;
  }
}

.thread-card {
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  display: flex;
  flex-direction: column;
  gap: 7px;
  transition: border-color 0.2s, box-shadow 0.2s;

  &:hover {
    border-color: var(--ui-border-hover);
  }

  &.is-main {
    border-left: 3px solid var(--ink-accent);
  }
}

.tc-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.tc-priority {
  font-size: 10.5px;
  padding: 1px 7px;
  border-radius: 4px;

  &.is-main {
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
  }

  &.is-minor {
    background: var(--surface-2);
    color: var(--ink-muted);
  }
}

.tc-status {
  font-size: 10.5px;
  padding: 1px 7px;
  border-radius: 4px;
  color: var(--ink-sec);

  &.st-planned { background: var(--surface-2); color: var(--ink-muted); }
  &.st-planted { background: color-mix(in srgb, var(--ink-positive) 14%, transparent); color: var(--ink-positive); }
  &.st-revealed { background: color-mix(in srgb, var(--ink-warning) 15%, transparent); color: var(--ink-warning); }
  &.st-resolved { background: var(--ink-accent-soft); color: var(--ink-accent); }
  &.st-abandoned { background: color-mix(in srgb, var(--state-danger) 12%, transparent); color: var(--state-danger); }}

.tc-due {
  font-size: 10.5px;
  color: var(--ink-muted);
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.tc-act {
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 24px;
  height: 24px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 11px;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &--danger:hover {
    background: var(--state-danger);
    color: #fff;
  }
}

.tc-title {
  font-size: 14px;
  color: var(--ink-main);
  font-weight: 600;
}

.tc-desc {
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-sec);
}

.tc-goal {
  display: flex;
  gap: 6px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-accent);

  i {
    margin-top: 4px;
    font-size: 10px;
  }
}

.tc-anchors {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding-top: 6px;
  border-top: 1px dashed var(--divider);
  font-size: 11px;
  color: var(--ink-muted);

  span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
}

// ── 可见性 ────────────────────────────────────────────
.tk-entry {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--ink-disabled);
  font-family: inherit;
  font-size: 11px;
  cursor: pointer;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-muted);
  }
}

.tc-know {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: 6px;
  border-top: 1px dashed var(--divider);
}

.tk-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  color: var(--ink-muted);

  span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
}

.tk-row {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  font-size: 11px;
}

.tk-observer {
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 4px;
  background: var(--surface-2);
  color: var(--ink-sec);
}

.tk-line {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 0;
}

.tk-arrow {
  font-size: 8px;
  color: var(--ink-disabled);
}

.tk-point {
  padding: 1px 7px;
  border-radius: 4px;

  &.kn-unknown {
    background: var(--surface-2);
    color: var(--ink-muted);
  }

  &.kn-suspected {
    background: color-mix(in srgb, var(--ink-warning) 15%, transparent);
    color: var(--ink-warning);
  }

  &.kn-known {
    background: color-mix(in srgb, var(--ink-positive) 14%, transparent);
    color: var(--ink-positive);
  }
}

.tk-act {
  border: none;
  background: transparent;
  color: var(--ink-disabled);
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  border-radius: 5px;
  cursor: pointer;
  font-size: 10px;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &--danger:hover {
    background: var(--state-danger);
    color: #fff;
  }
}

.tk-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px;
  border-radius: 8px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);

  select {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 6px;
    border-radius: 6px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12px;
    outline: none;
  }
}
</style>
