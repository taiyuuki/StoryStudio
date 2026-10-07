<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import type { AssertionDto, KnowledgeDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

const knowledge = ref<KnowledgeDto | null>(null)
const loading = ref(false)
const creating = ref(false)

// ── 过滤 ─────────────────────────────────────────────
type StatusFilter = 'all' | 'active' | 'superseded' | 'contradicted'
const filterStatus = ref<StatusFilter>('active')
const filterSubject = ref('') // '' = 全部主体

const statusChips: { value: StatusFilter; label: string }[] = [
  { value: 'active', label: '生效中' },
  { value: 'superseded', label: '已废弃' },
  { value: 'contradicted', label: '矛盾' },
  { value: 'all', label: '全部' },
]

const statusMeta: Record<string, { label: string; cls: string }> = {
  active: { label: '生效中', cls: 'as-active' },
  superseded: { label: '已废弃', cls: 'as-superseded' },
  contradicted: { label: '矛盾', cls: 'as-contradicted' },
}

const subjectOptions = computed(() => store.ws?.entities ?? [])

const assertions = computed(() => {
  let list = knowledge.value?.assertions ?? []
  if (filterStatus.value !== 'all') list = list.filter((a) => a.status === filterStatus.value)
  if (filterSubject.value) list = list.filter((a) => a.subject === filterSubject.value)
  return list
})

const activeCount = computed(
  () => knowledge.value?.assertions.filter((a) => a.status === 'active').length ?? 0,
)

// ── 谓词元数据（与后端 coerce_object_value / AUTO_SUPERSEDE_PREDICATES 对齐）──
interface PredicateMeta {
  value: string
  label: string
  /** 值输入控件类型 */
  kind: 'bool' | 'item' | 'location' | 'org' | 'text'
  /** 时变谓词：新值自动废弃同主体同谓词旧值 */
  autoSupersede: boolean
}

const PREDICATES: PredicateMeta[] = [
  { value: 'alive', label: '存活', kind: 'bool', autoSupersede: true },
  { value: 'age', label: '年龄', kind: 'text', autoSupersede: true },
  { value: 'located_at', label: '所在', kind: 'location', autoSupersede: true },
  { value: 'status', label: '状态', kind: 'text', autoSupersede: true },
  { value: 'holds', label: '持有', kind: 'item', autoSupersede: false },
  { value: 'member_of', label: '隶属', kind: 'org', autoSupersede: false },
  { value: 'identity', label: '身份', kind: 'text', autoSupersede: false },
]

const CUSTOM = '__custom__'

const form = ref({
  subject: '',
  predicate: 'alive',
  customPredicate: '',
  object: '',
  evidenceFragment: '',
})

const predicateMeta = computed<PredicateMeta | null>(() => {
  if (form.value.predicate === CUSTOM) return null
  return PREDICATES.find((p) => p.value === form.value.predicate) ?? null
})

/** 值控件候选：按谓词联动实体类型 */
const objectEntities = computed(() => {
  const kind = predicateMeta.value?.kind
  if (kind === 'item') return store.ws?.entities.filter((e) => e.kind === 'item') ?? []
  if (kind === 'location') return store.ws?.entities.filter((e) => e.kind === 'location') ?? []
  if (kind === 'org') return store.ws?.entities.filter((e) => e.kind === 'organization') ?? []
  return []
})

// 切换谓词时清空值，避免控件类型与残留值不符
watch(
  () => form.value.predicate,
  () => {
    form.value.object = ''
  },
)

function predicateLabel(p: string): string {
  return PREDICATES.find((x) => x.value === p)?.label ?? p
}

/** 值展示：实体 ID → 名称；布尔 → 存活/已故 */
function objectDisplay(a: AssertionDto): string {
  if (a.predicate === 'alive') return a.object === 'true' ? '存活' : '已故'
  const entity = store.ws?.entities.find((e) => e.id === a.object)
  if (entity) return entity.displayName
  return a.object
}

/** 来源标签：AI 抽取的显示模型名；手动记录的不显示 */
function sourceLabel(a: AssertionDto): string {
  if (a.extractedBy.startsWith('ai:')) return `AI · ${a.extractedBy.slice(3)}`
  return ''
}

// ── 加载与操作 ────────────────────────────────────────
async function reload() {
  if (!store.root) return
  loading.value = true
  try {
    knowledge.value = await api.listKnowledge(store.root)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    loading.value = false
  }
}

onMounted(reload)
watch(
  () => store.root,
  () => {
    knowledge.value = null
    creating.value = false
    void reload()
  },
)

function openCreate(subject?: string, predicate?: string) {
  const isCustom = predicate && !PREDICATES.some((p) => p.value === predicate)
  form.value = {
    subject: subject ?? store.ws?.entities.find((e) => e.kind === 'character')?.id ?? '',
    predicate: isCustom ? CUSTOM : (predicate ?? 'alive'),
    customPredicate: isCustom ? (predicate ?? '') : '',
    object: '',
    evidenceFragment: '',
  }
  creating.value = true
}

async function save() {
  if (!store.root) return
  const f = form.value
  const predicate = f.predicate === CUSTOM ? f.customPredicate.trim() : f.predicate
  if (!f.subject) {
    ElMessage.warning('请选择断言主体')
    return
  }
  if (!predicate) {
    ElMessage.warning('请填写谓词')
    return
  }
  if (!f.object.trim() && f.object !== 'true' && f.object !== 'false') {
    ElMessage.warning('请填写断言值')
    return
  }
  try {
    const outcome = await api.addAssertion(
      store.root,
      f.subject,
      predicate,
      f.object.trim(),
      f.evidenceFragment || undefined,
    )
    if (outcome.deduplicated) {
      ElMessage.info('已存在相同断言，未重复创建')
    } else if (outcome.superseded.length > 0) {
      ElMessage.success(`已记录，自动废弃 ${outcome.superseded.length} 条旧值`)
    } else if (outcome.coexisting.length > 0) {
      ElMessage.warning(
        `已记录；同谓词仍有旧值并存（${outcome.coexisting.map(objectText).join('、')}），如属演进请废弃旧值`,
      )
    } else {
      ElMessage.success('断言已记录')
    }
    creating.value = false
    await reload()
    void store.scheduleCompile()
  } catch (err) {
    ElMessage.error((err as Error).message)
  }
}

function objectText(v: string): string {
  if (v === 'true') return '存活'
  if (v === 'false') return '已故'
  return store.ws?.entities.find((e) => e.id === v)?.displayName ?? v
}

async function setStatus(a: AssertionDto, status: string) {
  if (!store.root) return
  try {
    await api.setAssertionStatus(store.root, a.id, status)
    await reload()
    void store.scheduleCompile()
  } catch (err) {
    ElMessage.error((err as Error).message)
  }
}

async function remove(a: AssertionDto) {
  if (!store.root) return
  try {
    await api.deleteAssertion(store.root, a.id)
    await reload()
    void store.scheduleCompile()
  } catch (err) {
    ElMessage.error((err as Error).message)
  }
}
</script>

<template>
  <div class="assertion-panel">
    <div class="ap-head">
      <span class="text-ink-muted">知识断言 · {{ activeCount }} 生效</span>
      <button class="ink-btn ink-btn--sm ink-btn--primary" @click="openCreate()">
        <i class="fa-solid fa-plus" />
        新断言
      </button>
    </div>

    <p class="ap-note">
      断言记录上帝视角的事实真相（假死也算活着）；正文表象与角色认知差请用「叙事线索」的可见性体现。
    </p>

    <!-- 过滤 -->
    <div class="ap-filter">
      <div class="ap-chips">
        <button
          v-for="c in statusChips"
          :key="c.value"
          class="ap-chip"
          :class="{ 'is-on': filterStatus === c.value }"
          @click="filterStatus = c.value"
        >
          {{ c.label }}
        </button>
      </div>
      <select v-model="filterSubject" class="ap-subject">
        <option value="">全部主体</option>
        <option v-for="e in subjectOptions" :key="e.id" :value="e.id">
          {{ e.displayName }}
        </option>
      </select>
    </div>

    <!-- 添加表单 -->
    <div v-if="creating" class="ap-create">
      <div class="tp-row">
        <select v-model="form.subject" title="断言主体">
          <optgroup label="角色">
            <option
              v-for="e in store.ws?.entities.filter((x) => x.kind === 'character')"
              :key="e.id"
              :value="e.id"
            >
              {{ e.displayName }}
            </option>
          </optgroup>
          <optgroup label="组织">
            <option
              v-for="e in store.ws?.entities.filter((x) => x.kind === 'organization')"
              :key="e.id"
              :value="e.id"
            >
              {{ e.displayName }}
            </option>
          </optgroup>
          <optgroup label="地点">
            <option
              v-for="e in store.ws?.entities.filter((x) => x.kind === 'location')"
              :key="e.id"
              :value="e.id"
            >
              {{ e.displayName }}
            </option>
          </optgroup>
        </select>
        <select v-model="form.predicate" title="谓词">
          <option v-for="p in PREDICATES" :key="p.value" :value="p.value">
            {{ p.label }}
          </option>
          <option :value="CUSTOM">自定义…</option>
        </select>
      </div>
      <input
        v-if="form.predicate === CUSTOM"
        v-model="form.customPredicate"
        class="tp-input"
        placeholder="自定义谓词（如 death_cause）"
      />
      <!-- 值输入：按谓词联动 -->
      <select v-if="predicateMeta?.kind === 'bool'" v-model="form.object" title="断言值">
        <option value="true">存活</option>
        <option value="false">已故</option>
      </select>
      <select v-else-if="objectEntities.length" v-model="form.object" title="断言值">
        <option value="" disabled>选择{{ predicateMeta?.label }}…</option>
        <option v-for="e in objectEntities" :key="e.id" :value="e.id">
          {{ e.displayName }}
        </option>
      </select>
      <input
        v-else
        v-model="form.object"
        class="tp-input"
        :placeholder="form.predicate === 'age' ? '年龄（整数）' : '断言值（文本）'"
      />
      <select v-model="form.evidenceFragment" title="证据片段（可选）">
        <option value="">无证据</option>
        <option v-for="f in knowledge?.fragments ?? []" :key="f.id" :value="f.id">
          {{ f.label }}（{{ f.id }}）
        </option>
      </select>
      <p v-if="predicateMeta?.autoSupersede" class="ap-hint">
        时变谓词：保存后同主体旧值自动废弃
      </p>
      <div class="tp-row">
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="save">记录</button>
        <button class="ink-btn ink-btn--sm" @click="creating = false">取消</button>
      </div>
    </div>

    <!-- 空状态 -->
    <div v-if="!loading && assertions.length === 0 && !creating" class="ap-empty">
      <i class="fa-solid fa-database" />
      <p>
        知识断言是设定事实的台账：生死、年龄、持有、位置……<br />
        编译器据此检查矛盾（死亡角色复活、年龄错位等）。
      </p>
    </div>

    <!-- 断言卡片 -->
    <div v-for="a in assertions" :key="a.id" class="as-card" :class="[statusMeta[a.status]?.cls]">
      <div class="as-head">
        <span class="as-status">{{ statusMeta[a.status]?.label ?? a.status }}</span>
        <span v-if="sourceLabel(a)" class="as-source">{{ sourceLabel(a) }}</span>
        <span style="flex: 1" />
        <template v-if="a.status === 'active'">
          <button
            v-if="PREDICATES.some((p) => p.value === a.predicate && p.autoSupersede)"
            class="as-act"
            title="记录新值（旧值自动废弃）"
            @click="openCreate(a.subject, a.predicate)"
          >
            <i class="fa-solid fa-pen" />
          </button>
          <button
            class="as-act"
            title="废弃（superseded：不再参与检查）"
            @click="setStatus(a, 'superseded')"
          >
            <i class="fa-solid fa-box-archive" />
          </button>
          <button
            class="as-act"
            title="标记矛盾（contradicted）"
            @click="setStatus(a, 'contradicted')"
          >
            <i class="fa-solid fa-triangle-exclamation" />
          </button>
        </template>
        <button
          v-else
          class="as-act"
          title="恢复生效"
          @click="setStatus(a, 'active')"
        >
          <i class="fa-solid fa-rotate-left" />
        </button>
        <button class="as-act as-act--danger" title="删除" @click="remove(a)">
          <i class="fa-regular fa-trash-can" />
        </button>
      </div>
      <div class="as-body">
        <span class="as-subject">{{ store.entityName(a.subject) }}</span>
        <span class="as-pred">{{ predicateLabel(a.predicate) }}</span>
        <span class="as-object">{{ objectDisplay(a) }}</span>
      </div>
      <div v-if="a.validFrom || a.validUntil" class="as-valid">
        <i class="fa-regular fa-clock" />
        {{ a.validFrom ?? '…' }} → {{ a.validUntil ?? '至今' }}
      </div>
      <div v-if="a.evidence.length" class="as-evidence">
        <span v-for="ev in a.evidence" :key="ev.id" :title="`${ev.fragment}（${ev.id}）`">
          <i class="fa-solid fa-quote-left" /> {{ ev.label || ev.fragment }}
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.assertion-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.ap-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
}

.ap-note {
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-disabled);
  margin: -6px 0 0;
}

.ap-filter {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.ap-chips {
  display: flex;
  gap: 6px;
}

.ap-chip {
  padding: 2px 10px;
  border-radius: 10px;
  border: 1px solid var(--ui-border);
  background: transparent;
  color: var(--ink-muted);
  font-family: inherit;
  font-size: 11.5px;
  cursor: pointer;

  &.is-on {
    border-color: var(--ink-accent);
    color: var(--ink-accent);
    background: var(--ink-accent-soft);
  }
}

.ap-subject,
.ap-create select,
.tp-input {
  width: 100%;
  padding: 7px 10px;
  border-radius: 7px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 13px;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }
}

.ap-create {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);
}

.tp-row {
  display: flex;
  gap: 8px;

  select {
    flex: 1;
    min-width: 0;
  }
}

.ap-hint {
  margin: 0;
  font-size: 11px;
  color: var(--ink-disabled);
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

.as-card {
  padding: 11px 13px;
  border-radius: 12px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  display: flex;
  flex-direction: column;
  gap: 6px;

  &:hover {
    border-color: var(--ui-border-hover);
  }

  &.as-superseded,
  &.as-contradicted {
    opacity: 0.72;

    .as-body {
      text-decoration: line-through;
      text-decoration-color: color-mix(in srgb, currentColor 40%, transparent);
    }
  }
}

.as-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.as-status {
  font-size: 10.5px;
  padding: 1px 7px;
  border-radius: 4px;
  color: var(--ink-sec);

  .as-active & {
    background: color-mix(in srgb, var(--ink-positive) 14%, transparent);
    color: var(--ink-positive);
  }

  .as-superseded & {
    background: var(--surface-2);
    color: var(--ink-muted);
  }

  .as-contradicted & {
    background: color-mix(in srgb, var(--state-danger) 12%, transparent);
    color: var(--state-danger);
  }
}

.as-source {
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.as-act {
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

.as-body {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 6px;
  font-size: 13.5px;
}

.as-subject {
  color: var(--ink-main);
  font-weight: 600;
}

.as-pred {
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 4px;
  background: var(--surface-2);
  color: var(--ink-sec);
}

.as-object {
  color: var(--ink-sec);
}

.as-valid {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: var(--ink-muted);
}

.as-evidence {
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

    i {
      font-size: 9px;
      opacity: 0.7;
    }
  }
}
</style>
