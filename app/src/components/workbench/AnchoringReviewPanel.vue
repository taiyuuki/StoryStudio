<script setup lang="ts">
import { computed, nextTick, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import type { EntityKind, UnresolvedRefDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'
import { requestFragmentReveal } from '@/composables/useFragmentReveal'

const store = useWorkspaceStore()

/** 只看待处置，或看全部（含已忽略/已归拢） */
const showAll = ref(false)

const refs = computed(() => store.ws?.unresolvedRefs ?? [])
const pending = computed(() => refs.value.filter((r) => r.status === 'pending'))
const visible = computed(() =>
  showAll.value ? refs.value.filter((r) => r.status !== 'pending') : pending.value,
)

/** 批量勾选（仅在待处置视图下可用），key = fragmentId::quote */
const selected = ref(new Set<string>())
const selectedCount = computed(() => selected.value.size)
const allSelected = computed(
  () => pending.value.length > 0 && pending.value.every((r) => selected.value.has(keyOf(r))),
)

/** 勾选走 change 事件：WebKit 里 label 转发的合成 click 会让 label 上的 click handler 双触发，勾选被来回翻转 */
function onSelectToggle(r: UnresolvedRefDto, e: Event) {
  const checked = (e.target as HTMLInputElement).checked
  const next = new Set(selected.value)
  if (checked) next.add(keyOf(r))
  else next.delete(keyOf(r))
  selected.value = next
}

function toggleSelectAll() {
  selected.value = allSelected.value
    ? new Set()
    : new Set(pending.value.map((r) => keyOf(r)))
}

async function ignoreSelected() {
  const items = pending.value.filter((r) => selected.value.has(keyOf(r)))
  if (items.length === 0) return
  try {
    await ElMessageBox.confirm(
      `将忽略 ${items.length} 条指代，后续锚定不再提示。确定？`,
      '批量忽略',
      { confirmButtonText: '忽略', cancelButtonText: '取消', type: 'warning' },
    )
  } catch {
    return
  }
  const ok = await store.ignoreUnresolvedRefs(items.map((r) => [r.fragmentId, r.quote]))
  if (ok) selected.value = new Set()
}

async function deleteSelected() {
  const items = pending.value.filter((r) => selected.value.has(keyOf(r)))
  if (items.length === 0) return
  try {
    await ElMessageBox.confirm(
      `将从台账删除 ${items.length} 条记录。正文仍含该指代时，下次锚定会重新出现。`,
      '批量删除',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
  } catch {
    return
  }
  const ok = await store.deleteUnresolvedRefs(items.map((r) => [r.fragmentId, r.quote]))
  if (ok) selected.value = new Set()
}

/** 定位到正文：跳到指代所在片段，滚动到引文处并高亮（多处匹配时连续点击逐次跳转） */
async function jumpToRef(r: UnresolvedRefDto) {
  if (!store.root) return
  let sceneId: string
  try {
    sceneId = await api.sceneOfFragment(store.root, r.fragmentId)
  } catch (err) {
    ElMessage.error(`定位失败：${(err as Error).message}`)
    return
  }
  // 片段不在当前场景：先切过去，等编辑器挂载完再定位
  if (store.scene?.id !== sceneId) {
    await store.selectScene(sceneId)
    await nextTick()
  }
  requestFragmentReveal(r.fragmentId, r.quote)
}

/** 引文区点击定位（勾选框的点击不触发） */
function onQuoteClick(r: UnresolvedRefDto, e: MouseEvent) {
  if ((e.target as HTMLElement).closest('.ap-check')) return
  void jumpToRef(r)
}

/** 从台账删除单条记录（不撤销已处置写入的锚点） */
async function deleteRef(r: UnresolvedRefDto) {
  try {
    await ElMessageBox.confirm(
      `将从台账删除「${r.quote}」的记录。${
        r.status === 'resolved'
          ? '仅清台账，不会撤销已补的锚点。'
          : '正文仍含该指代时，下次锚定会重新出现。'
      }`,
      '删除记录',
      { confirmButtonText: '删除', cancelButtonText: '取消', type: 'warning' },
    )
  } catch {
    return
  }
  await store.deleteUnresolvedRefs([[r.fragmentId, r.quote]])
}

const entityOptions = computed(() =>
  (store.ws?.entities ?? []).map((e) => ({ id: e.id, label: `${e.displayName}（${e.id}）` })),
)

/** 展开中的处置行：quote 维度唯一（同片段 quote 唯一） */
const expanded = reactive<{
  key: string | null
  action: 'create' | 'merge' | null
  name: string
  kind: EntityKind
  targetId: string
}>({ key: null, action: null, name: '', kind: 'character', targetId: '' })

function keyOf(r: UnresolvedRefDto): string {
  return `${r.fragmentId}::${r.quote}`
}

function toggle(r: UnresolvedRefDto, action: 'create' | 'merge') {
  const key = keyOf(r)
  if (expanded.key === key && expanded.action === action) {
    expanded.key = null
    expanded.action = null
    return
  }
  expanded.key = key
  expanded.action = action
  expanded.name = ''
  expanded.kind = 'character'
  expanded.targetId = ''
}

async function submitCreate(r: UnresolvedRefDto) {
  const name = expanded.name.trim()
  if (!name) return
  await store.resolveUnresolved(r.fragmentId, r.quote, 'create', {
    name,
    kind: expanded.kind,
  })
  resetExpanded()
}

async function submitMerge(r: UnresolvedRefDto) {
  if (!expanded.targetId) return
  await store.resolveUnresolved(r.fragmentId, r.quote, 'merge', {
    targetEntityId: expanded.targetId,
  })
  resetExpanded()
}

async function ignore(r: UnresolvedRefDto) {
  await store.resolveUnresolved(r.fragmentId, r.quote, 'ignore')
  resetExpanded()
}

function resetExpanded() {
  expanded.key = null
  expanded.action = null
  expanded.name = ''
  expanded.targetId = ''
}

function statusLabel(r: UnresolvedRefDto): string {
  if (r.status === 'ignored') return '已忽略'
  if (r.status === 'resolved') {
    const name = store.ws?.entities.find((e) => e.id === r.target)?.displayName ?? r.target
    return `已归拢到「${name}」`
  }
  return ''
}

const kindOptions: { value: EntityKind; label: string }[] = [
  { value: 'character', label: '角色' },
  { value: 'location', label: '地点' },
  { value: 'item', label: '道具' },
  { value: 'organization', label: '组织' },
]
</script>

<template>
  <div class="anchor-panel">
    <div class="ap-head">
      <span class="text-ink-muted">
        锚定待审 · {{ pending.length }} 条
        <template v-if="refs.length > pending.length">（已处置 {{ refs.length - pending.length }}）</template>
      </span>
      <div class="ap-head-actions">
        <template v-if="!showAll && pending.length > 0">
          <button class="ink-btn ink-btn--sm" @click="toggleSelectAll">
            {{ allSelected ? '取消全选' : '全选' }}
          </button>
          <button
            class="ink-btn ink-btn--sm"
            :disabled="selectedCount === 0"
            title="忽略所选的一次性背景指代，后续锚定不再提示"
            @click="ignoreSelected"
          >
            <i class="fa-solid fa-eye-slash" /> 忽略所选{{ selectedCount > 0 ? `（${selectedCount}）` : '' }}
          </button>
          <button
            class="ink-btn ink-btn--sm"
            :disabled="selectedCount === 0"
            title="从台账移除所选记录；正文仍含该指代时下次锚定会重新出现"
            @click="deleteSelected"
          >
            <i class="fa-solid fa-trash-can" /> 删除所选
          </button>
        </template>
        <button
          v-if="refs.length > pending.length"
          class="ink-btn ink-btn--sm"
          @click="showAll = !showAll"
        >
          {{ showAll ? '只看待处置' : '看已处置' }}
        </button>
      </div>
    </div>

    <div v-if="visible.length === 0" class="ap-empty">
      <i class="fa-solid fa-anchor" />
      <p v-if="pending.length === 0">
        没有待处置的未锚定指代。<br />写作时理解锚定发现的名册外指代会汇集到这里。
      </p>
      <p v-else>没有已处置记录。</p>
    </div>

    <div
      v-for="r in visible"
      :key="keyOf(r)"
      class="ap-card"
      :class="{ 'is-done': r.status !== 'pending', 'is-selected': selected.has(keyOf(r)) }"
    >
      <div
        class="ap-quote"
        title="定位到正文（多处命中时逐次跳转）"
        @click="onQuoteClick(r, $event)"
      >
        <label v-if="r.status === 'pending'" class="ap-check">
          <input
            type="checkbox"
            :checked="selected.has(keyOf(r))"
            @change="onSelectToggle(r, $event)"
          />
        </label>
        <i class="fa-solid fa-quote-left" />
        {{ r.quote }}
      </div>
      <div class="ap-meta">
        <span class="ap-hint">{{ r.hint || '名册中无匹配' }}</span>
        <button
          class="ap-frag"
          title="定位到正文（多处命中时逐次跳转）"
          @click="jumpToRef(r)"
        >
          {{ r.fragmentId }}
        </button>
        <span v-if="r.status !== 'pending'" class="ap-status">{{ statusLabel(r) }}</span>
      </div>

      <div v-if="r.status === 'pending'" class="ap-actions">
        <button
          class="ink-btn ink-btn--sm ink-btn--primary"
          :class="{ 'is-active': expanded.key === keyOf(r) && expanded.action === 'create' }"
          @click="toggle(r, 'create')"
        >
          <i class="fa-solid fa-user-plus" /> 建档
        </button>
        <button
          class="ink-btn ink-btn--sm"
          :class="{ 'is-active': expanded.key === keyOf(r) && expanded.action === 'merge' }"
          @click="toggle(r, 'merge')"
        >
          <i class="fa-solid fa-code-merge" /> 归拢到…
        </button>
        <button class="ink-btn ink-btn--sm" title="忽略，后续锚定不再提示" @click="ignore(r)">
          <i class="fa-solid fa-eye-slash" /> 忽略
        </button>
        <button
          class="ink-btn ink-btn--sm"
          title="从台账删除该记录；正文仍含此指代时下次锚定会重新出现"
          @click="deleteRef(r)"
        >
          <i class="fa-solid fa-trash-can" /> 删除
        </button>
      </div>
      <div v-else class="ap-actions">
        <button
          class="ink-btn ink-btn--sm"
          title="从台账删除该记录（不撤销已补的锚点）"
          @click="deleteRef(r)"
        >
          <i class="fa-solid fa-trash-can" /> 删除
        </button>
      </div>

      <!-- 建档（匿名指代可用占位名） -->
      <div v-if="expanded.key === keyOf(r) && expanded.action === 'create'" class="ap-form">
        <input
          v-model="expanded.name"
          placeholder="档案名（匿名指代可用占位名，如：九一真凶）"
          autofocus
          @keyup.enter="submitCreate(r)"
        />
        <div class="ap-form-row">
          <select v-model="expanded.kind">
            <option v-for="k in kindOptions" :key="k.value" :value="k.value">{{ k.label }}</option>
          </select>
          <button class="ink-btn ink-btn--sm ink-btn--primary" @click="submitCreate(r)">
            创建并归拢
          </button>
        </div>
      </div>

      <!-- 归拢到已有实体 -->
      <div v-if="expanded.key === keyOf(r) && expanded.action === 'merge'" class="ap-form">
        <select v-model="expanded.targetId">
          <option value="" disabled>选择归拢到的实体</option>
          <option v-for="e in entityOptions" :key="e.id" :value="e.id">{{ e.label }}</option>
        </select>
        <div class="ap-form-row">
          <span class="text-ink-muted">
            悬疑里的凶手指代（摆的人/写的人…）可先建占位实体归拢，真名揭晓后合并
          </span>
          <button
            class="ink-btn ink-btn--sm ink-btn--primary"
            :disabled="!expanded.targetId"
            @click="submitMerge(r)"
          >
            归拢
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.anchor-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.ap-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
}

.ap-head-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ap-check {
  display: inline-flex;
  align-items: center;
  cursor: pointer;

  input {
    width: 15px;
    height: 15px;
    accent-color: var(--ink-accent);
    cursor: pointer;
  }
}

.ap-card.is-selected {
  border-color: var(--ink-accent);
  background: var(--ink-accent-soft);
}

.ap-empty {
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

.ap-card {
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  display: flex;
  flex-direction: column;
  gap: 8px;

  &:hover {
    border-color: var(--ui-border-hover);
  }

  &.is-done {
    opacity: 0.65;
  }
}

.ap-quote {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13.5px;
  font-weight: 600;
  color: var(--ink-main);
  cursor: pointer;

  i {
    font-size: 11px;
    color: var(--ink-accent);
  }
}

.ap-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 11.5px;
  color: var(--ink-muted);
  flex-wrap: wrap;
}

// 片段徽标 = 定位按钮：跳到该片段并高亮引文
.ap-frag {
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 5px;
  border: none;
  background: var(--ink-accent-soft);
  color: inherit;
  cursor: pointer;

  &:hover {
    background: color-mix(in srgb, var(--ink-accent) 25%, transparent);
  }
}

.ap-status {
  color: var(--ink-muted);
}

.ap-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.ap-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);

  input,
  select {
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
}

.ap-form-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11.5px;

  select {
    width: auto;
    flex: 0 0 auto;
  }

  span {
    flex: 1;
    line-height: 1.5;
  }
}
</style>
