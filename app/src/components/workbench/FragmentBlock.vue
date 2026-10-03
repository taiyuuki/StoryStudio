<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import type { FragmentDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { useCodeMirror, EditorView } from '@/composables/useCodeMirror'
import { useAiCheck } from '@/composables/useAiCheck'
import { revealQuote, useFragmentReveal } from '@/composables/useFragmentReveal'
import FragmentHistoryDialog from './FragmentHistoryDialog.vue'

const props = defineProps<{
  fragment: FragmentDto
  index: number
  total: number
}>()

const store = useWorkspaceStore()
const ui = useUiStore()

// 编辑中的文本（延迟同步到 store，避免每键触发全树刷新）
const doc = ref(props.fragment.text)

const { container, view, create } = useCodeMirror({
  doc,
  onChange: (text) => {
    doc.value = text
    scheduleSave(text)
  },
  placeholder: '落笔…',
  aiComplete: {
    // 幽灵补全：输入停顿后预测下文，Tab 采纳 / Esc 关闭
    enabled: () => ui.aiGhost,
    fetch: (before, after, onDelta) =>
      api
        .aiCompleteFragment(store.root, props.fragment.id, before, after, onDelta)
        .then((r) => r.text),
    onCancel: () => void api.aiCompleteCancel(),
  },
})

// 审校报告的精确定位：命中本段时滚动到引文处并高亮
const { pending: revealPending } = useFragmentReveal()
watch(revealPending, (req) => {
  if (!req || req.fragmentId !== props.fragment.id) return
  const v = view.value
  if (!v) return
  if (!revealQuote(v, req.quote)) {
    // 引文已改写匹配不上：退回段首定位
    v.dispatch({ selection: { anchor: 0 }, effects: EditorView.scrollIntoView(0, { y: 'center' }) })
  }
})

onMounted(() => {
  if (container.value) create(container.value)
})

// 外部数据变化（切场景回来 / AI 生成后 / 版本恢复后）→ 同步进编辑器
watch(
  () => props.fragment.text,
  (next) => {
    if (next !== doc.value) doc.value = next
  },
)

// ── 自动保存（防抖 800ms）──────────────────────────────
let saveTimer: ReturnType<typeof setTimeout> | null = null
function scheduleSave(text: string) {
  store.fragmentSave[props.fragment.id] = 'dirty'
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => void store.saveFragment(props.fragment.id, text), 800)
}

const saveState = computed(() => store.fragmentSave[props.fragment.id] ?? 'saved')
const saveLabel = computed(() => {
  switch (saveState.value) {
    case 'dirty':
      return '未保存'
    case 'saving':
      return '保存中'
    case 'error':
      return '保存失败，点击重试'
    default:
      return '已保存'
  }
})

const anchors = computed(() =>
  (props.fragment.anchors ?? []).map((a) => ({
    name: store.entityName(a.entity),
    role: a.role,
  })),
)

const historyOpen = ref(false)

// AI 审校本段：先冲刷未落盘编辑（检测读磁盘），结果填入妙笔改进
const { run: runAiCheck } = useAiCheck()

async function checkFragment() {
  await store.saveFragment(props.fragment.id, doc.value)
  const head = doc.value.trim().slice(0, 10)
  void runAiCheck('fragment', props.fragment.id, `片段 ${head ? `「${head}…」` : props.fragment.id}`)
}

async function insertAfter() {
  await store.insertFragment(props.fragment.id)
}

async function moveUp() {
  await store.moveFragment(props.fragment.id, -1)
}

async function moveDown() {
  await store.moveFragment(props.fragment.id, 1)
}
</script>

<template>
  <!-- id 供审校报告「跳到该片段」定位滚动 -->
  <article :id="`frag-${fragment.id}`" class="frag-block" :class="{ 'is-dirty': saveState === 'dirty' }">
    <div class="frag-gutter" :class="{ 'is-visible': false }">
      <span class="frag-no num">{{ String(index + 1).padStart(2, '0') }}</span>
      <div class="gutter-actions">
        <button :disabled="index === 0" title="上移" @click="moveUp">
          <i class="fa-solid fa-arrow-up" />
        </button>
        <button :disabled="index === total - 1" title="下移" @click="moveDown">
          <i class="fa-solid fa-arrow-down" />
        </button>
        <button title="在此后插入段落" @click="insertAfter">
          <i class="fa-solid fa-plus" />
        </button>
        <button title="历史版本" @click="historyOpen = true">
          <i class="fa-solid fa-clock-rotate-left" />
        </button>
        <button title="AI 审校本段（结果送妙笔改进）" @click="checkFragment">
          <i class="fa-solid fa-magnifying-glass" />
        </button>
        <button class="danger" title="删除段落" @click="store.removeFragment(fragment.id)">
          <i class="fa-regular fa-trash-can" />
        </button>
      </div>
    </div>

    <div ref="container" class="frag-editor" />

    <div v-if="anchors.length" class="frag-anchors">
      <span v-for="(a, i) in anchors" :key="i" class="anchor-chip" :title="`${a.name} 在本段出场`">
        <i class="fa-solid fa-link" />
        {{ a.name }}
      </span>
    </div>

    <div class="frag-foot">
      <span
        class="frag-save"
        :class="`state-${saveState}`"
        :title="saveState === 'error' ? '点击重试保存' : undefined"
        :role="saveState === 'error' ? 'button' : undefined"
        @click="saveState === 'error' && store.retrySave(fragment.id, doc)"
      >
        <i
          class="fa-solid"
          :class="{
            'fa-circle-check': saveState === 'saved',
            'fa-circle-notch fa-spin': saveState === 'saving',
            'fa-circle-exclamation': saveState === 'error',
            'fa-circle': saveState === 'dirty',
          }"
        />
        {{ saveLabel }}
      </span>
      <span class="num frag-words">{{ fragment.wordCount.toLocaleString() }} 字</span>
    </div>

    <FragmentHistoryDialog v-model="historyOpen" :fragment-id="fragment.id" />
  </article>
</template>

<style scoped lang="scss">
.frag-block {
  position: relative;
  padding: 10px 8px 6px 40px;
  border-radius: 10px;
  transition: background 0.2s;

  &:hover {
    background: color-mix(in srgb, var(--surface-2) 55%, transparent);

    .frag-gutter .gutter-actions {
      opacity: 1;
    }
  }
}

// ── 左侧装订线 ──────────────────────────────────────────
.frag-gutter {
  position: absolute;
  left: 0;
  top: 14px;
  width: 34px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
}

.frag-no {
  font-size: 11px;
  color: var(--ink-disabled);
}

.gutter-actions {
  opacity: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  transition: opacity 0.2s;

  button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--ink-muted);
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s;

    &:hover:not(:disabled) {
      background: var(--divider);
      color: var(--ink-main);
    }

    &:disabled {
      opacity: 0.3;
      cursor: default;
    }

    &.danger:hover {
      background: var(--state-danger);
      color: #fff;
    }
  }
}

.frag-editor {
  min-height: 60px;
  cursor: text;
}

.frag-editor :deep(.cm-editor) {
  background: transparent;
}

.frag-editor :deep(.cm-editor.cm-focused) {
  outline: none;
}

.frag-editor :deep(.cm-scroller) {
  overflow: hidden;
}

// 审校报告定位的命中高亮：亮起 → 保持 → 渐隐（周期与清除定时同步）
.frag-editor :deep(.cm-hit-flash) {
  border-radius: 3px;
  animation: hit-flash-cycle 2.4s ease-out;
}

@keyframes hit-flash-cycle {
  0% {
    background: color-mix(in srgb, var(--state-danger) 45%, transparent);
  }
  12%,
  70% {
    background: color-mix(in srgb, var(--state-warning) 30%, transparent);
  }
  100% {
    background: transparent;
  }
}

.frag-anchors {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 2px 4px 4px;
}

.anchor-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border-radius: 4px;
  background: var(--ink-accent-soft);
  color: var(--ink-accent);
  font-size: 11px;

  i {
    font-size: 9px;
  }
}

.frag-foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  padding: 2px 6px 4px;
  font-size: 11px;
  color: var(--ink-disabled);
}

.frag-save {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  transition: color 0.2s;

  &.state-error {
    color: var(--state-danger);
    cursor: pointer;
  }
  &.state-saved {
    color: var(--state-success);
    opacity: 0.7;
  }
}
</style>
