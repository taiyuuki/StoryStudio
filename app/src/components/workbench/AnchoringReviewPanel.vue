<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import type { EntityKind, UnresolvedRefDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

/** 只看待处置，或看全部（含已忽略/已归拢） */
const showAll = ref(false)

const refs = computed(() => store.ws?.unresolvedRefs ?? [])
const pending = computed(() => refs.value.filter((r) => r.status === 'pending'))
const visible = computed(() =>
  showAll.value ? refs.value.filter((r) => r.status !== 'pending') : pending.value,
)

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
      <button
        v-if="refs.length > pending.length"
        class="ink-btn ink-btn--sm"
        @click="showAll = !showAll"
      >
        {{ showAll ? '只看待处置' : '看已处置' }}
      </button>
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
      :class="{ 'is-done': r.status !== 'pending' }"
    >
      <div class="ap-quote">
        <i class="fa-solid fa-quote-left" />
        {{ r.quote }}
      </div>
      <div class="ap-meta">
        <span class="ap-hint">{{ r.hint || '名册中无匹配' }}</span>
        <span class="ap-frag">{{ r.fragmentId }}</span>
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

.ap-frag {
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 5px;
  background: var(--ink-accent-soft);
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
