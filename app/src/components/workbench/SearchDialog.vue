<script setup lang="ts">
import { nextTick, onUnmounted, ref, useTemplateRef, watch } from 'vue'
import { ElMessage } from 'element-plus'
import type { SearchHitDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { requestFragmentReveal } from '@/composables/useFragmentReveal'

const ui = useUiStore()
const store = useWorkspaceStore()

// 与后端 command 的 max_hits 一致（达到即截断摘录）
const MAX_HITS = 200

const query = ref('')
/** null = 尚未搜索（区分空结果） */
const hits = ref<SearchHitDto[] | null>(null)
const searching = ref(false)
const inputEl = useTemplateRef<HTMLInputElement>('queryInput')

// Cmd/Ctrl+F 打开（Tauri webview 没有原生查找栏，接过来做全书检索）
function onKeydown(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
    e.preventDefault()
    ui.searchOpen = true
  }
}
window.addEventListener('keydown', onKeydown)
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

watch(
  () => ui.searchOpen,
  (open) => {
    if (open) void nextTick(() => inputEl.value?.focus())
  },
)

async function run() {
  const q = query.value.trim()
  if (!q || searching.value || !store.root) return
  searching.value = true
  try {
    hits.value = await api.searchText(store.root, q)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    searching.value = false
  }
}

/** 摘录按【】分段：命中段渲染 <mark>（不用 v-html——正文可能含任意字符） */
function excerptParts(excerpt: string): { text: string; hit: boolean }[] {
  const out: { text: string; hit: boolean }[] = []
  const re = /【([^】]*)】/g
  let last = 0
  let m: RegExpExecArray | null
  while ((m = re.exec(excerpt))) {
    if (m.index > last) out.push({ text: excerpt.slice(last, m.index), hit: false })
    out.push({ text: m[1], hit: true })
    last = m.index + m[0].length
  }
  if (last < excerpt.length) out.push({ text: excerpt.slice(last), hit: false })
  return out
}

/** 跳到命中处：切场景 → 编辑器滚动到命中段并高亮（引文取【】标记的原文） */
async function jumpTo(hit: SearchHitDto) {
  const quote = excerptParts(hit.excerpt).find((p) => p.hit)?.text ?? ''
  if (store.scene?.id !== hit.sceneId) {
    await store.selectScene(hit.sceneId)
    await nextTick()
  }
  ui.searchOpen = false
  requestFragmentReveal(hit.fragmentId, quote)
}
</script>

<template>
  <ElDialog v-model="ui.searchOpen" title="全文检索" width="560px" align-center append-to-body>
    <div class="sd-box">
      <div class="sd-bar">
        <input
          ref="queryInput"
          v-model="query"
          placeholder="搜索全书正文关键词…"
          @keyup.enter="run"
        />
        <button
          class="ink-btn ink-btn--sm ink-btn--primary"
          :disabled="searching || !query.trim()"
          @click="run"
        >
          <i :class="searching ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-magnifying-glass'" />
          检索
        </button>
      </div>

      <div v-if="hits !== null" class="sd-meta">
        {{ hits.length > 0 ? `${hits.length} 个片段命中` : '没有匹配的正文' }}
        <template v-if="hits.length >= MAX_HITS">（已达上限，试试更具体的关键词）</template>
      </div>

      <div v-if="hits !== null && hits.length === 0" class="sd-empty">
        <i class="fa-solid fa-feather" />
        <p>全书正文没有出现这个关键词。</p>
      </div>

      <div v-else-if="hits !== null" class="sd-results">
        <button v-for="h in hits" :key="h.fragmentId" class="sd-hit" @click="jumpTo(h)">
          <span class="sd-loc">
            {{ h.chapterTitle }} · {{ h.sceneTitle }}
            <template v-if="h.occurrences > 1">×{{ h.occurrences }}</template>
          </span>
          <span class="sd-excerpt">
            <template v-for="(p, i) in excerptParts(h.excerpt)" :key="i">
              <mark v-if="p.hit">{{ p.text }}</mark>
              <template v-else>{{ p.text }}</template>
            </template>
          </span>
        </button>
      </div>
    </div>
  </ElDialog>
</template>

<style scoped lang="scss">
.sd-box {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.sd-bar {
  display: flex;
  gap: 8px;

  input {
    flex: 1;
    min-width: 0;
    padding: 8px 12px;
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

.sd-meta {
  font-size: 12px;
  color: var(--ink-muted);
}

.sd-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 28px 0;
  color: var(--ink-disabled);
  font-size: 12.5px;

  i {
    font-size: 24px;
    opacity: 0.7;
  }
}

.sd-results {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 52vh;
  overflow-y: auto;
  padding-right: 2px;
}

.sd-hit {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  text-align: left;
  font-family: inherit;
  cursor: pointer;
  transition: border-color 0.15s;

  &:hover {
    border-color: var(--ink-accent);
  }
}

.sd-loc {
  font-size: 11px;
  color: var(--ink-muted);
}

.sd-excerpt {
  font-size: 13px;
  line-height: 1.8;
  color: var(--ink-main);
  word-break: break-all;

  mark {
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
    padding: 0 1px;
    border-radius: 2px;
  }
}
</style>
