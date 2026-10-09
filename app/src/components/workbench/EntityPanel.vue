<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import type { EntityDto, EntityKind, RelationStoryTimeDto, RelationValidityDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

type Tab = EntityKind
const tabs: { key: Tab; label: string; icon: string }[] = [
  { key: 'character', label: '角色', icon: 'fa-solid fa-user' },
  { key: 'location', label: '地点', icon: 'fa-solid fa-location-dot' },
  { key: 'item', label: '道具', icon: 'fa-solid fa-gem' },
  { key: 'organization', label: '组织', icon: 'fa-solid fa-flag' },
]

const activeTab = ref<Tab>('character')
const selectedId = ref<string | null>(null)
const creating = ref(false)
const newName = ref('')

const entities = computed(() =>
  (store.ws?.entities ?? []).filter((e) => e.kind === activeTab.value),
)

function countOf(kind: EntityKind): number {
  return (store.ws?.entities ?? []).filter((e) => e.kind === kind).length
}

const selected = computed(() =>
  entities.value.find((e) => e.id === selectedId.value) ?? null,
)

// 编辑副本（保存时整体提交）
const draft = reactive<EntityDto>({
  id: '',
  kind: 'character',
  displayName: '',
  symbol: null,
  aliases: [],
  secretName: null,
  nameRevealedIn: null,
  profile: null,
  geography: null,
  item: null,
  org: null,
  attributes: {},
})

const concealed = computed(() => Boolean(draft.secretName?.trim()))

// 揭露真名：display_name 换为预定真名，原代称转入别名（继续锚定历史指代），
// 清遮蔽状态。与 AI 侧 reveal_character_name 工具同一语义。
function revealName() {
  const trueName = draft.secretName?.trim()
  if (!trueName) return
  const placeholder = draft.displayName.trim()
  if (placeholder && placeholder !== trueName && !draft.aliases.includes(placeholder)) {
    draft.aliases.unshift(placeholder)
  }
  draft.displayName = trueName
  draft.secretName = null
  draft.nameRevealedIn = '工作台手动揭露'
}

function edit(e: EntityDto | null) {
  selectedId.value = e?.id ?? null
  if (!e) return
  Object.assign(draft, JSON.parse(JSON.stringify(e)) as EntityDto)
  // 老文件可能缺专属段，补上空段让表单可编辑
  if (draft.kind === 'location' && !draft.geography) {
    draft.geography = { parent: null, kind: null, position: null, features: null, summary: null }
  }
  if (draft.kind === 'organization' && !draft.org) {
    draft.org = { kind: null, leader: null, territory: null, ideology: null, summary: null, location: null }
  }
}

const aliasText = computed({
  get: () => draft.aliases.join('、'),
  set: (v: string) => {
    draft.aliases = v
      .split(/[,，、\s]+/)
      .map((s) => s.trim())
      .filter(Boolean)
  },
})

// ── 描述性别名警示 ────────────────────────────────────────
// 锚定层对别名做全文精确子串匹配：描述性短语（银发女/红衣人/摆的人）会把
// 正文中任何字面相同的路人也锚到本实体。编辑时即时提示，不阻断保存。
const DESCRIPTIVE_ALIAS_MARKERS = [
  '银发', '白发', '黑发', '红衣', '白衣', '黑衣', '青衣', '灰袍', '白袍', '红袍',
  '高个', '矮个', '独眼', '驼背', '胖子', '瘦子', '胡子',
  '老者', '老妇', '老头', '妇人', '少女', '少年', '男子', '女子', '男人', '女人',
  '男孩', '女孩', '婴儿', '中年', '年轻', '年迈',
]

const riskyAliases = computed(() => {
  const flagged = new Set<string>()
  for (const a of draft.aliases) {
    if (DESCRIPTIVE_ALIAS_MARKERS.some((m) => a.includes(m))) {
      flagged.add(a)
    } else if (a.length <= 4 && /[女男人者汉]$/.test(a)) {
      // 短且以泛称收尾（银发女/红衣人/摆的人/黑衣者）：无专名信号的别名
      flagged.add(a)
    }
  }
  return [...flagged]
})

async function create() {
  const name = newName.value.trim()
  if (!name) {
    creating.value = false
    return
  }
  const e = await store.createEntity(activeTab.value, name)
  creating.value = false
  newName.value = ''
  if (e) {
    edit(e)
    ElMessage.success(`已创建「${e.displayName}」`)
  }
}

async function save() {
  await store.saveEntity(JSON.parse(JSON.stringify(draft)) as EntityDto)
  if (selected.value) Object.assign(selected.value, JSON.parse(JSON.stringify(draft)))
}

async function remove(e: EntityDto) {
  await store.deleteEntity(e.id)
  if (selectedId.value === e.id) selectedId.value = null
}

// ── 合并（匿名占位实体真名揭晓后并入正式档案） ──────────────
const merging = ref(false)
const mergeTargetId = ref('')

const mergeCandidates = computed(() =>
  entities.value.filter((e) => e.id !== selectedId.value),
)

async function doMerge() {
  if (!selected.value || !mergeTargetId.value) return
  const ok = await store.mergeEntities(selected.value.id, mergeTargetId.value)
  if (ok) {
    merging.value = false
    selectedId.value = null
  }
}

const presetRelations = [
  'mentor_of', 'family_of', 'lover_of', 'enemy_of', 'rival_of', 'subordinate_of', 'knows',
]
const relForm = reactive({ from: '', to: '', relation: 'knows' })
const relCreating = ref(false)

const relations = computed(() =>
  store.ws?.relations.map((r) => ({
    ...r,
    fromName: store.entityName(r.from),
    toName: store.entityName(r.to),
  })) ?? [],
)

// ── 时变关系：生效区间标签 ─────────────────────────────────
// 场景锚优先显示「章 · 场景」标题（比裸 ID 可读），历法边界显示年月
const sceneLabels = computed(() => {
  const map = new Map<string, string>()
  for (const ch of store.ws?.chapters ?? []) {
    for (const sc of ch.scenes) {
      map.set(sc.id, `${ch.title}·${sc.title || '未命名场景'}`)
    }
  }
  return map
})

function boundLabel(t: RelationStoryTimeDto): string {
  if (t.scene) return sceneLabels.value.get(t.scene) ?? t.scene
  let s = `${t.year ?? '?'}年`
  if (t.month) {
    s += `${t.month}月`
    if (t.day) s += `${t.day}日`
  }
  return s
}

function validityLabel(valid?: RelationValidityDto | null): string {
  if (!valid?.from && !valid?.until) return ''
  const parts: string[] = []
  if (valid?.from) parts.push(`自 ${boundLabel(valid.from)}`)
  if (valid?.until) parts.push(`至 ${boundLabel(valid.until)}前`)
  return parts.join(' · ')
}

// ── 时变关系：窗口编辑（两个场景选择器，空 = 无界） ─────────
type RelRow = (typeof relations.value)[number]

const relEditingId = ref<string | null>(null)
const relValidForm = reactive({ from: '', until: '' })

const sceneOptions = computed(() => {
  const opts: { id: string; label: string }[] = []
  for (const ch of store.ws?.chapters ?? []) {
    for (const sc of ch.scenes) {
      opts.push({ id: sc.id, label: `${ch.title}·${sc.title || '未命名场景'}` })
    }
  }
  return opts
})

function toggleRelationWindow(r: RelRow) {
  if (relEditingId.value === r.id) {
    relEditingId.value = null
    return
  }
  relEditingId.value = r.id
  relValidForm.from = r.valid?.from?.scene ?? ''
  relValidForm.until = r.valid?.until?.scene ?? ''
}

async function saveRelationWindow(relationId: string) {
  if (relValidForm.from && relValidForm.until && relValidForm.from === relValidForm.until) {
    ElMessage.warning('生效与失效不能是同一场景（窗口为空区间）')
    return
  }
  await store.updateRelation(relationId, relValidForm.from || null, relValidForm.until || null)
  relEditingId.value = null
}

async function addRelation() {
  if (!relForm.from || !relForm.to) {
    ElMessage.warning('选择关系的双方')
    return
  }
  if (relForm.from === relForm.to) {
    ElMessage.warning('不能与自己建立关系')
    return
  }
  await store.addRelation(relForm.from, relForm.to, relForm.relation)
  relCreating.value = false
}

function label(r: string): string {
  const map: Record<string, string> = {
    mentor_of: '师徒',
    family_of: '亲缘',
    lover_of: '恋人',
    enemy_of: '敌对',
    rival_of: '竞争',
    subordinate_of: '从属',
    knows: '相识',
  }
  return map[r] ?? r
}
</script>

<template>
  <div class="entity-panel">
    <div class="kind-tabs">
      <button
        v-for="t in tabs"
        :key="t.key"
        class="kind-tab"
        :class="{ 'is-active': activeTab === t.key }"
        @click="((activeTab = t.key), (selectedId = null))"
      >
        <i :class="t.icon" />
        {{ t.label }}
        <span class="num count">{{ countOf(t.key) }}</span>
      </button>
    </div>

    <div v-if="!selected" class="list-view">
      <div class="list-head">
        <span class="text-ink-muted">共 {{ entities.length }} 项</span>
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="((creating = true), (newName = ''))">
          <i class="fa-solid fa-plus" />
          新建
        </button>
      </div>

      <div v-if="creating" class="create-row">
        <input
          v-model="newName"
          :placeholder="`新${tabs.find((t) => t.key === activeTab)?.label}名称`"
          autofocus
          @keyup.enter="create"
          @keyup.escape="creating = false"
          @blur="create"
        />
      </div>

      <div v-if="entities.length === 0 && !creating" class="empty">
        <i class="fa-regular fa-circle" />
        <p>还没有{{ tabs.find((t) => t.key === activeTab)?.label }}设定</p>
      </div>

      <div
        v-for="e in entities"
        :key="e.id"
        class="entity-item"
        role="button"
        tabindex="0"
        @click="edit(e)"
        @keyup.enter="edit(e)"
        @keyup.space="edit(e)"
      >
        <span class="ei-avatar" :data-kind="e.kind">{{ e.displayName.slice(0, 1) }}</span>
        <span class="ei-main">
          <span class="ei-name">
            {{ e.displayName }}
            <i
              v-if="e.secretName"
              class="fa-solid fa-user-secret conceal-mark"
              :title="`真名遮蔽中（预定真名：${e.secretName}）——正文只能用代称，详情页可揭露`"
            />
          </span>
          <span class="ei-sub">{{ e.aliases.length ? e.aliases.join('、') : '　' }}</span>
        </span>
        <button class="ei-del" title="删除" @click.stop="remove(e)">
          <i class="fa-regular fa-trash-can" />
        </button>
      </div>

      <!-- 关系（任意 tab 下都可管理） -->
      <div class="rel-section">
        <div class="rel-head">
          <i class="fa-solid fa-diagram-project" />
          <span>关系网</span>
          <span class="num text-ink-muted">{{ relations.length }}</span>
          <span style="flex: 1" />
          <button class="ink-btn ink-btn--sm" @click="((relCreating = !relCreating), ((relForm.from = ''), (relForm.to = ''), (relForm.relation = 'knows')))">
            <i class="fa-solid fa-plus" />
            添加
          </button>
        </div>

        <div v-if="relCreating" class="rel-create">
          <select v-model="relForm.from">
            <option value="" disabled>甲方</option>
            <option v-for="e in store.ws?.entities" :key="e.id" :value="e.id">{{ e.displayName }}</option>
          </select>
          <select v-model="relForm.relation">
            <option v-for="r in presetRelations" :key="r" :value="r">{{ label(r) }}</option>
          </select>
          <select v-model="relForm.to">
            <option value="" disabled>乙方</option>
            <option v-for="e in store.ws?.entities" :key="e.id" :value="e.id">{{ e.displayName }}</option>
          </select>
          <button class="ink-btn ink-btn--sm ink-btn--primary" @click="addRelation">确定</button>
        </div>

        <template v-for="r in relations" :key="r.id">
          <div class="rel-item" :class="{ 'is-editing': relEditingId === r.id }">
            <span class="ri-main">
              <span class="ri-line">
                <b>{{ r.fromName }}</b>
                <i class="fa-solid fa-arrow-right-long" />
                <span class="rel-label">{{ label(r.relation) }}</span>
                <i class="fa-solid fa-arrow-right-long" />
                <b>{{ r.toName }}</b>
              </span>
              <span
                v-if="validityLabel(r.valid)"
                class="rel-validity"
                title="点击编辑生效区间"
                @click="toggleRelationWindow(r)"
              >
                <i class="fa-regular fa-clock" />
                {{ validityLabel(r.valid) }}
              </span>
            </span>
            <button
              class="ri-clock"
              :title="r.valid ? '编辑生效区间' : '添加生效区间（时变关系）'"
              @click="toggleRelationWindow(r)"
            >
              <i class="fa-regular fa-clock" />
            </button>
            <button class="ri-del" @click="store.removeRelation(r.id)">
              <i class="fa-solid fa-xmark" />
            </button>
          </div>

          <div v-if="relEditingId === r.id" class="rel-valid-edit">
            <select v-model="relValidForm.from">
              <option value="">自故事开始</option>
              <option v-for="s in sceneOptions" :key="s.id" :value="s.id">自 {{ s.label }}</option>
            </select>
            <select v-model="relValidForm.until">
              <option value="">直到故事结束</option>
              <option v-for="s in sceneOptions" :key="s.id" :value="s.id">至 {{ s.label }} 前</option>
            </select>
            <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveRelationWindow(r.id)">保存</button>
            <p v-if="sceneOptions.length === 0" class="rv-empty">尚无场景可锚定——先在结构树中创建场景</p>
          </div>
        </template>
        <p v-if="relations.length === 0" class="empty empty--thin">尚无关系</p>
      </div>
    </div>

    <!-- 详情编辑 -->
    <div v-else class="detail-view">
      <button class="back-btn" @click="selectedId = null">
        <i class="fa-solid fa-arrow-left" />
        返回列表
      </button>

      <div class="d-row">
        <label>名称</label>
        <input v-model="draft.displayName" />
      </div>
      <p v-if="concealed" class="conceal-banner">
        <i class="fa-solid fa-user-secret" />
        真名遮蔽中——正文与 AI 上下文只能使用代称「{{ draft.displayName }}」，
        预定真名「{{ draft.secretName }}」已隐藏。正文正式揭露后点右侧按钮切换：
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="revealName">揭露真名</button>
      </p>
      <p v-else-if="draft.nameRevealedIn" class="conceal-banner conceal-banner--done">
        <i class="fa-solid fa-check" />
        真名已揭露（{{ draft.nameRevealedIn }}），原代称已转入别名。
      </p>
      <div class="d-row">
        <label>别名</label>
        <input v-model="aliasText" placeholder="多个别名用、分隔" />
      </div>
      <p v-if="riskyAliases.length" class="alias-warn">
        <i class="fa-solid fa-triangle-exclamation" />
        「{{ riskyAliases.join('、') }}」是描述性或泛称别的写法：锚定按字面全文匹配，
        正文里路人角色的同类描述也会被算作本角色的出场。若非专属称呼，建议删去。
      </p>

      <template v-if="activeTab === 'character' && draft.profile">
        <div class="d-row d-grid">
          <div>
            <label>性别</label>
            <input v-model="draft.profile.gender" />
          </div>
          <div>
            <label>职业</label>
            <input v-model="draft.profile.occupation" />
          </div>
        </div>
        <div class="d-row d-grid">
          <div>
            <label>种族</label>
            <input v-model="draft.profile.race" />
          </div>
          <div>
            <label>出生年份</label>
            <input v-model.number="draft.profile.birthYear" type="number" class="num" />
          </div>
        </div>
        <div class="d-row d-row--col">
          <label>外貌</label>
          <textarea v-model="draft.profile.appearance" rows="2" />
        </div>
        <div class="d-row d-row--col">
          <label>性格</label>
          <textarea v-model="draft.profile.personality" rows="2" />
        </div>
        <div class="d-row d-row--col">
          <label>背景</label>
          <textarea v-model="draft.profile.background" rows="3" />
        </div>
        <div class="d-row d-row--col">
          <label>习性（口头禅 / 小动作）</label>
          <textarea v-model="draft.profile.habits" rows="2" />
        </div>
      </template>

      <template v-if="activeTab === 'location' && draft.geography">
        <div class="d-row">
          <label>类型</label>
          <input v-model="draft.geography.kind" placeholder="大陆 / 国家 / 城市 / 秘境…" />
        </div>
        <div class="d-row d-row--col">
          <label>方位描述</label>
          <textarea v-model="draft.geography.position" rows="2" />
        </div>
        <div class="d-row d-row--col">
          <label>特征</label>
          <textarea v-model="draft.geography.features" rows="2" placeholder="风貌、遗迹、气候等" />
        </div>
        <div class="d-row d-row--col">
          <label>简介</label>
          <textarea v-model="draft.geography.summary" rows="2" placeholder="地点综述" />
        </div>
      </template>

      <template v-if="activeTab === 'organization' && draft.org">
        <div class="d-row">
          <label>类型</label>
          <input v-model="draft.org.kind" placeholder="宗门 / 国家 / 家族 / 公会…" />
        </div>
        <div class="d-row">
          <label>首领</label>
          <input v-model="draft.org.leader" />
        </div>
        <div class="d-row">
          <label>势力范围</label>
          <input v-model="draft.org.territory" />
        </div>
        <div class="d-row d-row--col">
          <label>理念</label>
          <textarea v-model="draft.org.ideology" rows="2" placeholder="理念 / 宗旨（可选）" />
        </div>
        <div class="d-row d-row--col">
          <label>简介</label>
          <textarea v-model="draft.org.summary" rows="2" />
        </div>
      </template>

      <template v-if="activeTab === 'item' && draft.item">
        <div class="d-row d-grid">
          <div>
            <label>类型</label>
            <input v-model="draft.item.kind" />
          </div>
          <div>
            <label>品阶</label>
            <input v-model="draft.item.grade" />
          </div>
        </div>
        <div class="d-row d-row--col">
          <label>描述</label>
          <textarea v-model="draft.item.description" rows="2" />
        </div>
        <div class="d-row d-row--col">
          <label>能力</label>
          <textarea v-model="draft.item.abilities" rows="2" />
        </div>
        <div class="d-row">
          <label>唯一道具</label>
          <ElSwitch v-model="draft.item.unique" />
        </div>
      </template>

      <div class="d-actions">
        <button class="ink-btn ink-btn--sm ink-btn--primary" @click="save">
          <i class="fa-solid fa-check" />
          保存
        </button>
        <button
          class="ink-btn ink-btn--sm"
          title="把本实体并入另一同类型实体（锚点/关系/POV 等引用自动重定向）——匿名占位实体真名揭晓后用它"
          @click="merging = !merging"
        >
          <i class="fa-solid fa-code-merge" />
          合并到…
        </button>
        <button class="ink-btn ink-btn--sm ink-btn--danger" @click="remove(selected); selectedId = null">
          <i class="fa-regular fa-trash-can" />
          删除
        </button>
      </div>

      <!-- 合并目标选择 -->
      <div v-if="merging" class="merge-box">
        <select v-model="mergeTargetId">
          <option value="" disabled>选择保留的目标实体（本实体将并入它）</option>
          <option v-for="e in mergeCandidates" :key="e.id" :value="e.id">
            {{ e.displayName }}（{{ e.id }}）
          </option>
        </select>
        <button
          class="ink-btn ink-btn--sm ink-btn--primary"
          :disabled="!mergeTargetId"
          @click="doMerge"
        >
          合并
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.entity-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.kind-tabs {
  display: flex;
  gap: 6px;
  padding: 4px;
  border-radius: 10px;
  background: var(--surface-2);
}

.kind-tab {
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  height: 30px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    color: var(--ink-main);
  }

  &.is-active {
    background: var(--surface-solid);
    color: var(--ink-accent);
    box-shadow: var(--ui-shadow);
  }

  .count {
    font-size: 10.5px;
    opacity: 0.7;
  }
}

.list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
}

.create-row {
  input {
    width: 100%;
    height: 34px;
    padding: 0 10px;
    border-radius: 8px;
    border: 1px dashed var(--ink-accent);
    background: var(--ink-accent-soft);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 13px;
    outline: none;
  }
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 26px 0;
  color: var(--ink-disabled);
  font-size: 12.5px;

  &--thin {
    padding: 10px 0;
  }
}

.entity-item {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 9px 10px;
  border: 1px solid transparent;
  border-radius: 10px;
  background: transparent;
  cursor: pointer;
  transition: all 0.2s;
  text-align: left;

  &:hover {
    background: var(--surface-2);
    border-color: var(--divider);

    .ei-del {
      opacity: 1;
    }
  }
}

.ei-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 9px;
  font-size: 14px;
  flex-shrink: 0;
  background: var(--ink-accent-soft);
  color: var(--ink-accent);

  &[data-kind='location'] {
    background: color-mix(in srgb, var(--ink-positive) 14%, transparent);
    color: var(--ink-positive);
  }

  &[data-kind='item'] {
    background: color-mix(in srgb, var(--ink-warning) 15%, transparent);
    color: var(--ink-warning);
  }

  &[data-kind='organization'] {
    background: var(--surface-2);
    color: var(--ink-sec);
  }
}

.ei-main {
  flex: 1;
  min-width: 0;
}

.ei-name {
  display: block;
  font-size: 13.5px;
  color: var(--ink-main);
}

.ei-sub {
  display: block;
  margin-top: 1px;
  font-size: 11px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ei-del {
  opacity: 0;
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 26px;
  height: 26px;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s;

  &:hover {
    background: var(--state-danger);
    color: #fff;
  }
}

// ── 关系 ──────────────────────────────────────────────
.rel-section {
  margin-top: 8px;
  padding-top: 12px;
  border-top: 1px solid var(--divider);
}

.rel-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-main);

  i {
    color: var(--ink-accent);
    font-size: 12px;
  }

  span:nth-child(3) {
    font-size: 11px;
  }
}

.rel-create {
  display: flex;
  gap: 6px;
  margin: 10px 0 6px;

  select {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 6px;
    border-radius: 7px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12px;
    outline: none;
  }
}

.rel-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 8px;
  transition: background 0.2s;

  &:hover {
    background: var(--surface-2);

    .ri-del,
    .ri-clock {
      opacity: 1;
    }
  }

  &.is-editing {
    background: var(--surface-2);
    border-radius: 8px 8px 0 0;
  }
}

.ri-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

// 时变关系的生效区间（次行弱化显示，点击进入编辑）
.rel-validity {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  width: fit-content;
  font-size: 10.5px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;

  &:hover {
    color: var(--ink-accent);
  }

  i {
    font-size: 9.5px;
    flex-shrink: 0;
  }
}

// 时钟按钮：hover 浮现，点击编辑/添加生效区间
.ri-clock {
  opacity: 0;
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 22px;
  height: 22px;
  border-radius: 5px;
  cursor: pointer;
  flex-shrink: 0;

  &:hover {
    color: var(--ink-accent);
  }
}

// 生效区间编辑行（两个场景选择器 + 保存）
.rel-valid-edit {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  margin: -1px 0 4px;
  padding: 8px 10px;
  border-radius: 0 0 8px 8px;
  background: var(--surface-2);

  select {
    flex: 1;
    min-width: 120px;
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

  .rv-empty {
    width: 100%;
    margin: 0;
    font-size: 11px;
    color: var(--state-warning);
  }
}

.ri-line {
  flex: 1;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  font-size: 12.5px;
  color: var(--ink-sec);
  white-space: nowrap;
  overflow: hidden;

  b {
    color: var(--ink-main);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  i {
    font-size: 9px;
    color: var(--ink-disabled);
    flex-shrink: 0;
  }
}

.rel-label {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--ink-accent);
  background: var(--ink-accent-soft);
  padding: 1px 7px;
  border-radius: 4px;
}

.ri-del {
  opacity: 0;
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 22px;
  height: 22px;
  border-radius: 5px;
  cursor: pointer;

  &:hover {
    color: var(--state-danger);
  }
}

// ── 详情 ──────────────────────────────────────────────
.detail-view {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.back-btn {
  align-self: flex-start;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: none;
  background: transparent;
  color: var(--ink-accent);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  padding: 2px 0;

  &:hover {
    text-decoration: underline;
  }
}

// 描述性别名警示（不阻断保存，只提示误锚风险）
// 真名遮蔽：列表角标 + 详情横幅
.conceal-mark {
  margin-left: 5px;
  font-size: 10px;
  color: var(--state-warning);
  vertical-align: 1px;
}

.conceal-banner {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
  align-items: center;
  margin: -6px 0 0;
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--state-warning);

  i {
    flex-shrink: 0;
    font-size: 11px;
  }
  button {
    margin-left: auto;
  }

  &--done {
    color: var(--state-success);
  }
}

.alias-warn {
  display: flex;
  gap: 7px;
  align-items: flex-start;
  margin: -8px 0 0;
  font-size: 11.5px;
  line-height: 1.65;
  color: var(--state-warning);

  i {
    flex-shrink: 0;
    margin-top: 3px;
    font-size: 11px;
  }
}

.d-row {
  display: flex;
  flex-direction: column;
  gap: 5px;

  label {
    font-size: 12px;
    color: var(--ink-sec);
    letter-spacing: 0.06em;
  }
  input,
  textarea,
  select {
    padding: 8px 11px;
    border-radius: 7px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 14px;
    outline: none;
    transition: border-color 0.2s;
    width: 100%;

    &:focus {
      border-color: var(--ink-accent);
    }
  }

  textarea {
    resize: vertical;
    line-height: 1.7;
  }

  &--col {
    label {
      margin-top: 2px;
    }
  }
}

.d-grid {
  flex-direction: row;
  gap: 10px;

  > div {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
}

.d-actions {
  display: flex;
  justify-content: space-between;
  margin-top: 6px;
  padding-top: 12px;
  border-top: 1px solid var(--divider);
}

.merge-box {
  display: flex;
  gap: 8px;
  margin-top: 10px;
  padding: 10px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);

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
</style>
