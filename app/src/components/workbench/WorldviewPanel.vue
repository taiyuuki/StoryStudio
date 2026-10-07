<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useWorkspaceStore } from '@/stores/workspace'
import { api } from '@/api/backend'
import type { ConceptDto, SystemDto, WorldviewDto } from '@/api/backend'

const store = useWorkspaceStore()

const loading = ref(true)
const saving = ref(false)
const wv = reactive<WorldviewDto>({ concepts: [], systems: [], eras: [] })

// —— 分栏：概念 / 体系 / 纪元 ——
type Tab = 'concepts' | 'systems' | 'eras'
const tabs = computed(() => [
  { key: 'concepts' as const, label: '概念', icon: 'fa-solid fa-lightbulb', count: wv.concepts.length },
  { key: 'systems' as const, label: '体系', icon: 'fa-solid fa-bolt', count: wv.systems.length },
  { key: 'eras' as const, label: '纪元', icon: 'fa-solid fa-hourglass-half', count: wv.eras.length },
])
const activeTab = ref<Tab>('concepts')

// —— 基础概念 ——
const conceptCreating = ref(false)
const conceptForm = ref({ name: '', category: '', summary: '', details: '' })
const conceptEditing = ref(-1)
const conceptDraft = ref({ name: '', category: '', summary: '', details: '' })

// —— 力量体系 ——
const SYSTEM_KINDS = ['cultivation', 'magic', 'technology', 'power']
const systemCreating = ref(false)
const systemForm = ref({ name: '', kind: 'cultivation', levels: '', rules: '', summary: '' })
const systemEditing = ref(-1)
const systemDraft = ref({ name: '', kind: 'cultivation', levels: '', rules: '', summary: '' })

// —— 纪元 ——
const eraInput = ref('')

const opt = (s: string) => (s.trim() ? s.trim() : null)

async function load() {
  if (!store.root) return
  try {
    Object.assign(wv, await api.getWorldview(store.root))
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
    await api.saveWorldview(store.root, {
      concepts: wv.concepts,
      systems: wv.systems,
      eras: wv.eras,
    })
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    saving.value = false
  }
}

onMounted(load)

// —— 概念操作 ——

function addConcept() {
  const f = conceptForm.value
  if (!f.name.trim()) {
    conceptCreating.value = false
    return
  }
  wv.concepts.push({
    name: f.name.trim(),
    category: opt(f.category),
    summary: opt(f.summary),
    details: opt(f.details),
  })
  conceptForm.value = { name: '', category: '', summary: '', details: '' }
  conceptCreating.value = false
  void persist()
}

function startEditConcept(i: number) {
  conceptEditing.value = i
  const c = wv.concepts[i]
  conceptDraft.value = {
    name: c.name,
    category: c.category ?? '',
    summary: c.summary ?? '',
    details: c.details ?? '',
  }
}

function saveConcept(i: number) {
  const d = conceptDraft.value
  if (!d.name.trim()) return
  wv.concepts[i] = {
    name: d.name.trim(),
    category: opt(d.category),
    summary: opt(d.summary),
    details: opt(d.details),
  }
  conceptEditing.value = -1
  void persist()
}

function deleteConcept(i: number) {
  wv.concepts.splice(i, 1)
  conceptEditing.value = -1
  void persist()
}

// —— 体系操作 ——

function addSystem() {
  const f = systemForm.value
  if (!f.name.trim()) {
    systemCreating.value = false
    return
  }
  wv.systems.push({
    name: f.name.trim(),
    kind: opt(f.kind),
    levels: opt(f.levels),
    rules: opt(f.rules),
    summary: opt(f.summary),
  })
  systemForm.value = { name: '', kind: 'cultivation', levels: '', rules: '', summary: '' }
  systemCreating.value = false
  void persist()
}

function startEditSystem(i: number) {
  systemEditing.value = i
  const s = wv.systems[i]
  systemDraft.value = {
    name: s.name,
    kind: s.kind ?? 'cultivation',
    levels: s.levels ?? '',
    rules: s.rules ?? '',
    summary: s.summary ?? '',
  }
}

function saveSystem(i: number) {
  const d = systemDraft.value
  if (!d.name.trim()) return
  wv.systems[i] = {
    name: d.name.trim(),
    kind: opt(d.kind),
    levels: opt(d.levels),
    rules: opt(d.rules),
    summary: opt(d.summary),
  }
  systemEditing.value = -1
  void persist()
}

function deleteSystem(i: number) {
  wv.systems.splice(i, 1)
  systemEditing.value = -1
  void persist()
}

// —— 纪元操作 ——

function addEras() {
  const parts = eraInput.value
    .split(/[,，、]/)
    .map((s) => s.trim())
    .filter((s) => s && !wv.eras.includes(s))
  if (parts.length) {
    wv.eras.push(...parts)
    void persist()
  }
  eraInput.value = ''
}

function removeEra(i: number) {
  wv.eras.splice(i, 1)
  void persist()
}

function moveEra(i: number, dir: -1 | 1) {
  const j = i + dir
  if (j < 0 || j >= wv.eras.length) return
  ;[wv.eras[i], wv.eras[j]] = [wv.eras[j], wv.eras[i]]
  void persist()
}
</script>

<template>
  <div class="worldview-panel">
    <div v-if="loading" class="wp-loading">
      <i class="fa-solid fa-circle-notch fa-spin" />
    </div>

    <template v-else>
      <p class="wp-hint">
        概念、体系与历法在此维护
        <span v-if="saving" class="wp-saving"><i class="fa-solid fa-floppy-disk" /> 保存中…</span>
      </p>

      <!-- 分栏 tab -->
      <div class="wp-tabs">
        <button
          v-for="t in tabs"
          :key="t.key"
          class="wp-tab"
          :class="{ 'is-active': activeTab === t.key }"
          @click="activeTab = t.key"
        >
          <i class="fa-solid" :class="t.icon" />
          {{ t.label }}
          <span class="num wp-tab-count">{{ t.count }}</span>
        </button>
      </div>

      <!-- 基础概念 -->
      <section v-show="activeTab === 'concepts'">
        <div class="wp-toolbar">
          <span class="wp-count-hint num">世界规则、纪元、种族等基本设定</span>
          <span style="flex: 1" />
          <button class="wp-mini-btn" @click="conceptCreating = true">
            <i class="fa-solid fa-plus" /> 新增概念
          </button>
        </div>

        <div v-if="conceptCreating" class="wp-edit">
          <input v-model="conceptForm.name" placeholder="概念名（如：灵气）" autofocus
            @keyup.enter="addConcept" @keyup.escape="conceptCreating = false" />
          <input v-model="conceptForm.category" placeholder="分类（如：世界规则 / 纪元 / 种族）" />
          <textarea v-model="conceptForm.summary" rows="2" placeholder="一句话概括（可选）" />
          <textarea v-model="conceptForm.details" rows="2" placeholder="详细说明（可选）" />
          <div class="wp-edit-row">
            <span style="flex: 1" />
            <button class="ink-btn ink-btn--sm" @click="conceptCreating = false">取消</button>
            <button class="ink-btn ink-btn--sm ink-btn--primary" @click="addConcept">添加</button>
          </div>
        </div>

        <div v-if="wv.concepts.length === 0 && !conceptCreating" class="wp-empty">
          还没有基础概念——世界规则、纪元、种族都从这里开始。
        </div>

        <div v-for="(c, i) in wv.concepts" :key="`c-${i}-${c.name}`" class="wp-card">
          <template v-if="conceptEditing === i">
            <input v-model="conceptDraft.name" class="wp-input" />
            <input v-model="conceptDraft.category" class="wp-input" placeholder="分类" />
            <textarea v-model="conceptDraft.summary" class="wp-input" rows="2" placeholder="一句话概括" />
            <textarea v-model="conceptDraft.details" class="wp-input" rows="3" placeholder="详细说明" />
            <div class="wp-edit-row">
              <span style="flex: 1" />
              <button class="ink-btn ink-btn--sm" @click="conceptEditing = -1">取消</button>
              <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveConcept(i)">保存</button>
            </div>
          </template>
          <template v-else>
            <div class="wp-card-head">
              <span class="wp-name">{{ c.name }}</span>
              <span v-if="c.category" class="wp-tag">{{ c.category }}</span>
              <span style="flex: 1" />
              <button class="wp-act" title="编辑" @click="startEditConcept(i)">
                <i class="fa-solid fa-pen" />
              </button>
              <button class="wp-act wp-act--danger" title="删除" @click="deleteConcept(i)">
                <i class="fa-regular fa-trash-can" />
              </button>
            </div>
            <p v-if="c.summary" class="wp-summary">{{ c.summary }}</p>
            <p v-if="c.details" class="wp-details">{{ c.details }}</p>
          </template>
        </div>
      </section>

      <!-- 力量体系 -->
      <section v-show="activeTab === 'systems'">
        <div class="wp-toolbar">
          <span class="wp-count-hint num">修炼、魔法、异能的等级与规则</span>
          <span style="flex: 1" />
          <button class="wp-mini-btn" @click="systemCreating = true">
            <i class="fa-solid fa-plus" /> 新增体系
          </button>
        </div>

        <div v-if="systemCreating" class="wp-edit">
          <input v-model="systemForm.name" placeholder="体系名（如：修炼体系）" autofocus
            @keyup.enter="addSystem" @keyup.escape="systemCreating = false" />
          <select v-model="systemForm.kind">
            <option v-for="k in SYSTEM_KINDS" :key="k" :value="k">{{ k }}</option>
          </select>
          <textarea v-model="systemForm.levels" rows="2" placeholder="等级链（如：炼气→筑基→金丹）" />
          <textarea v-model="systemForm.rules" rows="2" placeholder="规则与代价（可选）" />
          <textarea v-model="systemForm.summary" rows="2" placeholder="一句话概括（可选）" />
          <div class="wp-edit-row">
            <span style="flex: 1" />
            <button class="ink-btn ink-btn--sm" @click="systemCreating = false">取消</button>
            <button class="ink-btn ink-btn--sm ink-btn--primary" @click="addSystem">添加</button>
          </div>
        </div>

        <div v-if="wv.systems.length === 0 && !systemCreating" class="wp-empty">
          还没有力量体系——修炼、魔法、异能的等级与规则记在这里。
        </div>

        <div v-for="(s, i) in wv.systems" :key="`s-${i}-${s.name}`" class="wp-card">
          <template v-if="systemEditing === i">
            <input v-model="systemDraft.name" class="wp-input" />
            <select v-model="systemDraft.kind">
              <option v-for="k in SYSTEM_KINDS" :key="k" :value="k">{{ k }}</option>
            </select>
            <textarea v-model="systemDraft.levels" class="wp-input" rows="2" placeholder="等级链" />
            <textarea v-model="systemDraft.rules" class="wp-input" rows="2" placeholder="规则与代价" />
            <textarea v-model="systemDraft.summary" class="wp-input" rows="2" placeholder="一句话概括" />
            <div class="wp-edit-row">
              <span style="flex: 1" />
              <button class="ink-btn ink-btn--sm" @click="systemEditing = -1">取消</button>
              <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveSystem(i)">保存</button>
            </div>
          </template>
          <template v-else>
            <div class="wp-card-head">
              <span class="wp-name">{{ s.name }}</span>
              <span v-if="s.kind" class="wp-tag">{{ s.kind }}</span>
              <span style="flex: 1" />
              <button class="wp-act" title="编辑" @click="startEditSystem(i)">
                <i class="fa-solid fa-pen" />
              </button>
              <button class="wp-act wp-act--danger" title="删除" @click="deleteSystem(i)">
                <i class="fa-regular fa-trash-can" />
              </button>
            </div>
            <p v-if="s.levels" class="wp-levels"><i class="fa-solid fa-stairs" /> {{ s.levels }}</p>
            <p v-if="s.rules" class="wp-details">{{ s.rules }}</p>
            <p v-if="s.summary" class="wp-summary">{{ s.summary }}</p>
          </template>
        </div>
      </section>

      <!-- 纪元 -->
      <section v-show="activeTab === 'eras'">
        <p class="wp-era-hint">先后顺序即时间顺序；场景时间锚（如「灵历3217年」）按此折算比较。</p>

        <div v-if="wv.eras.length" class="wp-eras">
          <div v-for="(era, i) in wv.eras" :key="`${era}-${i}`" class="wp-era">
            <span class="wp-era-idx num">{{ i + 1 }}</span>
            <span class="wp-era-name">{{ era }}</span>
            <button class="wp-act" title="前移" :disabled="i === 0" @click="moveEra(i, -1)">
              <i class="fa-solid fa-arrow-up" />
            </button>
            <button class="wp-act" title="后移" :disabled="i === wv.eras.length - 1" @click="moveEra(i, 1)">
              <i class="fa-solid fa-arrow-down" />
            </button>
            <button class="wp-act wp-act--danger" title="删除" @click="removeEra(i)">
              <i class="fa-regular fa-trash-can" />
            </button>
          </div>
        </div>

        <div class="wp-era-add">
          <input
            v-model="eraInput"
            placeholder="纪元名，可用逗号一次添加多个（如：创世，灵历，开元）"
            @keyup.enter="addEras"
          />
          <button class="ink-btn ink-btn--sm ink-btn--primary" @click="addEras">添加</button>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped lang="scss">
.worldview-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.wp-loading {
  display: flex;
  justify-content: center;
  padding: 30px 0;
  color: var(--ink-muted);
}

.wp-hint {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.8;
  color: var(--ink-muted);
}

.wp-saving {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: 6px;
  color: var(--ink-accent);
}

// ── 分栏 tab（与实体面板同款分段样式） ────────────────────
.wp-tabs {
  display: flex;
  gap: 6px;
  padding: 4px;
  border-radius: 10px;
  background: var(--surface-2);
}

.wp-tab {
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 30px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s;

  i {
    font-size: 11.5px;
  }

  &:hover {
    color: var(--ink-main);
  }

  &.is-active {
    background: var(--surface-solid);
    color: var(--ink-accent);
    box-shadow: var(--ui-shadow);
  }
}

.wp-tab-count {
  font-size: 10.5px;
  color: var(--ink-disabled);

  .is-active & {
    color: var(--ink-accent);
    opacity: 0.75;
  }
}

.wp-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 9px;
}

.wp-count-hint {
  font-size: 11px;
  color: var(--ink-disabled);
  letter-spacing: 0.03em;
}

.wp-mini-btn {
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

.wp-edit {
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 12px;
  margin-bottom: 10px;
  border-radius: 10px;
  border: 1px dashed var(--ui-border);
  background: var(--ink-accent-soft);
}

.wp-edit-row {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.wp-input,
.wp-edit input,
.wp-edit textarea,
.wp-edit select,
.wp-era-add input {
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

.wp-edit textarea {
  resize: vertical;
  line-height: 1.6;
}

.wp-empty {
  padding: 16px 12px;
  border-radius: 10px;
  border: 1px dashed var(--divider);
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-disabled);
  text-align: center;
}

.wp-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 11px 13px;
  margin-bottom: 8px;
  border-radius: 12px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  transition: border-color 0.2s;

  &:hover {
    border-color: var(--ui-border-hover);
  }
}

.wp-card-head {
  display: flex;
  align-items: center;
  gap: 7px;
}

.wp-name {
  font-size: 13.5px;
  font-weight: 600;
  color: var(--ink-main);
}

.wp-tag {
  padding: 1px 7px;
  border-radius: 4px;
  background: var(--ink-accent-soft);
  color: var(--ink-accent);
  font-size: 10.5px;
}

.wp-summary {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-sec);
}

.wp-details {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-muted);
  white-space: pre-wrap;
}

.wp-levels {
  display: flex;
  gap: 6px;
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-accent);

  i {
    margin-top: 4px;
    font-size: 10px;
  }
}

.wp-act {
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 22px;
  height: 22px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 10.5px;

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

.wp-era-hint {
  margin: 0 0 10px;
  font-size: 11px;
  line-height: 1.7;
  color: var(--ink-muted);
}

.wp-eras {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 8px;
}

.wp-era {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px 6px 10px;
  border-radius: 9px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
}

.wp-era-idx {
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.wp-era-name {
  flex: 1;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--ink-main);
}

.wp-era-add {
  display: flex;
  gap: 8px;

  input {
    flex: 1;
    height: 30px;
  }
}
</style>
