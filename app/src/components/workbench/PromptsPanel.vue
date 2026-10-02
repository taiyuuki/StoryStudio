<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useWorkspaceStore } from '@/stores/workspace'
import { api } from '@/api/backend'
import type { PromptDto } from '@/api/backend'

const store = useWorkspaceStore()

const loading = ref(true)
const saving = ref(false)
const prompts = ref<PromptDto[]>([])
const selectedId = ref('')
/** 编辑中的文本（未自定义时以内置内容为起点，保存即创建覆盖） */
const draft = ref('')
/** 已自定义条目展开内置原文对照 */
const showBuiltin = ref(false)

const selected = computed(() => prompts.value.find((p) => p.id === selectedId.value) ?? null)

/** 下拉按分组聚合（保持后端给定顺序） */
const groups = computed(() => {
  const map: { group: string; items: PromptDto[] }[] = []
  for (const p of prompts.value) {
    let g = map.find((x) => x.group === p.group)
    if (!g) {
      g = { group: p.group, items: [] }
      map.push(g)
    }
    g.items.push(p)
  }
  return map
})

/** 覆盖生效的基线：自定义内容或内置内容 */
const baseline = computed(() => selected.value?.custom ?? selected.value?.builtin ?? '')

const dirty = computed(() => draft.value !== baseline.value)

/** 必需占位符缺失（保存前双向校验，前端红灯后端兜底） */
const missingPlaceholders = computed(() => {
  const p = selected.value
  if (!p) return []
  return p.placeholders.filter((name) => !draft.value.includes(`{{${name}}}`))
})

/** 占位符 chip 展示文本（含花括号，避免在模板插值里写字面量） */
function placeholderChip(name: string) {
  return `{{${name}}}`
}

const missingChips = computed(() => missingPlaceholders.value.map(placeholderChip).join(' '))

const blank = computed(() => !draft.value.trim())

const customizedCount = computed(() => prompts.value.filter((p) => p.custom).length)

async function load() {
  if (!store.root) return
  try {
    const list = await api.listPrompts(store.root)
    prompts.value = list
    // 面板每次挂载都重新加载：默认选第一项并把编辑区填充为生效内容
    if (list.length) {
      selectedId.value = list[0].id
      syncDraft()
    }
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    loading.value = false
  }
}

function syncDraft() {
  draft.value = baseline.value
  showBuiltin.value = false
}

/** 切换条目：有未保存改动先确认丢弃 */
async function onSelectChange(e: Event) {
  const el = e.target as HTMLSelectElement
  const id = el.value
  if (id === selectedId.value) return
  if (dirty.value) {
    const ok = await ElMessageBox.confirm(
      `「${selected.value?.label}」的修改尚未保存，切换后将丢失。`,
      '放弃未保存的修改？',
      { confirmButtonText: '放弃修改', cancelButtonText: '留下', type: 'warning' },
    ).then(() => true).catch(() => false)
    if (!ok) {
      el.value = selectedId.value // 绑定值未变，手动拉回原生 select 显示
      return
    }
  }
  selectedId.value = id
  syncDraft()
}

async function save() {
  const p = selected.value
  if (!p || !store.root || saving.value) return
  if (blank.value) {
    ElMessage.warning('提示词内容不能为空；恢复内置请用「恢复默认」')
    return
  }
  if (missingPlaceholders.value.length) {
    ElMessage.warning(`缺少必需占位符：${missingChips.value}`)
    return
  }
  saving.value = true
  try {
    await api.savePrompt(store.root, p.id, draft.value)
    p.custom = draft.value
    ElMessage.success(`「${p.label}」已保存，下一次调用生效`)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    saving.value = false
  }
}

async function reset() {
  const p = selected.value
  if (!p || !store.root || saving.value) return
  const ok = await ElMessageBox.confirm(
    `删除「${p.label}」的自定义内容，恢复内置提示词？`,
    '恢复默认',
    { confirmButtonText: '恢复默认', cancelButtonText: '先不', type: 'warning' },
  ).then(() => true).catch(() => false)
  if (!ok) return
  saving.value = true
  try {
    await api.resetPrompt(store.root, p.id)
    p.custom = null
    syncDraft()
    ElMessage.success(`「${p.label}」已恢复内置`)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    saving.value = false
  }
}

onMounted(load)

// Ctrl/Cmd + S 保存
function onKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
    e.preventDefault()
    if (dirty.value && !blank.value && !missingPlaceholders.value.length) void save()
  }
}
</script>

<template>
  <div class="prompts-panel" @keydown="onKeydown">
    <div v-if="loading" class="pp-loading">
      <i class="fa-solid fa-circle-notch fa-spin" />
    </div>

    <template v-else-if="selected">
      <p class="pp-hint">
        项目级覆盖：保存后写入 prompts/ 目录，随项目进版本档案
        <span v-if="saving" class="pp-saving"><i class="fa-solid fa-floppy-disk" /> 保存中…</span>
        <span v-else-if="customizedCount" class="pp-count num">已自定义 {{ customizedCount }} 项</span>
      </p>

      <!-- 按分组下拉选择提示词 -->
      <select :value="selectedId" class="pp-select" @change="onSelectChange">
        <optgroup v-for="g in groups" :key="g.group" :label="g.group">
          <option v-for="p in g.items" :key="p.id" :value="p.id">
            {{ p.label }}{{ p.custom ? '（已自定义）' : '' }}
          </option>
        </optgroup>
      </select>

      <!-- 当前条目说明 -->
      <div class="pp-meta">
        <p class="pp-desc">{{ selected.description }}</p>
        <div v-if="selected.placeholders.length" class="pp-slots">
          <span class="pp-slots-label">必需占位符</span>
          <code
            v-for="name in selected.placeholders"
            :key="name"
            class="pp-slot num"
            :class="{ 'is-missing': missingPlaceholders.includes(name) }"
          >{{ placeholderChip(name) }}</code>
        </div>
        <p v-if="missingPlaceholders.length" class="pp-slot-warn">
          缺少 {{ missingChips }}——程序运行时会向其中填充内容，删除会导致生成失败
        </p>
      </div>

      <!-- 编辑区：未自定义时以内置为起点 -->
      <textarea
        v-model="draft"
        class="pp-editor num"
        rows="18"
        spellcheck="false"
        :class="{ 'is-dirty': dirty }"
      />

      <!-- 已自定义时展开内置原文对照 -->
      <div v-if="selected.custom" class="pp-builtin">
        <button class="pp-builtin-toggle" @click="showBuiltin = !showBuiltin">
          <i class="fa-solid" :class="showBuiltin ? 'fa-chevron-down' : 'fa-chevron-right'" />
          {{ showBuiltin ? '收起内置原文' : '对照内置原文' }}
        </button>
        <textarea v-if="showBuiltin" class="pp-editor num pp-editor--readonly" :value="selected.builtin" readonly rows="10" />
      </div>

      <!-- 操作行 -->
      <div class="pp-actions">
        <span class="pp-state" :class="{ 'is-dirty': dirty }">
          <template v-if="dirty">未保存<template v-if="blank || missingPlaceholders.length">（内容不合规）</template></template>
          <template v-else-if="selected.custom">已自定义，保存于项目 prompts/</template>
          <template v-else>未自定义，当前为内置提示词</template>
        </span>
        <span style="flex: 1" />
        <button
          v-if="selected.custom"
          class="ink-btn ink-btn--sm"
          :disabled="saving || dirty"
          title="删除自定义，恢复内置提示词"
          @click="reset"
        >恢复默认</button>
        <button
          class="ink-btn ink-btn--sm ink-btn--primary"
          :disabled="saving || !dirty || blank || missingPlaceholders.length > 0"
          title="Ctrl+S"
          @click="save"
        >保存</button>
      </div>

      <p class="pp-foot">修改对下一次 AI 调用生效；妙笔对话为下一轮生效。仅开放输出自由文本的提示词——检测报告、断言抽取、建书草稿、节拍设计等输出被程序解析的提示词不开放自定义，以免改坏输出格式导致解析失败。</p>
    </template>

    <div v-else class="pp-empty">未能加载提示词清单。</div>
  </div>
</template>

<style scoped lang="scss">
.prompts-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.pp-loading {
  display: flex;
  justify-content: center;
  padding: 30px 0;
  color: var(--ink-muted);
}

.pp-hint {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 11.5px;
  line-height: 1.8;
  color: var(--ink-muted);
}

.pp-saving {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--ink-accent);
}

.pp-count {
  margin-left: auto;
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.pp-select {
  width: 100%;
  height: 32px;
  padding: 0 9px;
  border-radius: 8px;
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

.pp-meta {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.pp-desc {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-sec);
}

.pp-slots {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
}

.pp-slots-label {
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.pp-slot {
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--surface-2);
  color: var(--ink-sec);
  font-size: 10.5px;

  &.is-missing {
    background: var(--state-danger);
    color: #fff;
  }
}

.pp-slot-warn {
  margin: 0;
  font-size: 11px;
  line-height: 1.6;
  color: var(--state-danger);
}

.pp-editor {
  width: 100%;
  min-height: 260px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-size: 12px;
  line-height: 1.7;
  resize: vertical;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }

  &.is-dirty {
    border-color: var(--ink-accent-soft);
  }

  &--readonly {
    min-height: 0;
    opacity: 0.75;
    cursor: default;
  }
}

.pp-builtin-toggle {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border: none;
  background: transparent;
  padding: 0;
  color: var(--ink-muted);
  font-family: inherit;
  font-size: 11px;
  cursor: pointer;

  &:hover {
    color: var(--ink-main);
  }
}

.pp-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.pp-state {
  font-size: 11px;
  color: var(--ink-disabled);

  &.is-dirty {
    color: var(--ink-accent);
  }
}

.pp-foot {
  margin: 0;
  font-size: 10.5px;
  line-height: 1.7;
  color: var(--ink-disabled);
}

.pp-empty {
  padding: 16px 12px;
  border-radius: 10px;
  border: 1px dashed var(--divider);
  font-size: 12px;
  color: var(--ink-disabled);
  text-align: center;
}
</style>
