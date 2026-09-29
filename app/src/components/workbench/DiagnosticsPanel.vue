<script setup lang="ts">
import { computed, ref } from 'vue'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

type Sev = 'error' | 'warning' | 'all'
const filter = ref<Sev>('all')

const result = computed(() => store.compileResult)
const visible = computed(() => {
  if (!result.value?.ok) return []
  if (filter.value === 'all') return result.value.diagnostics
  return result.value.diagnostics.filter((d) => d.severity === filter.value)
})

const sevMeta: Record<string, { label: string; cls: string; icon: string }> = {
  error: { label: '错误', cls: 'sev-error', icon: 'fa-solid fa-circle-xmark' },
  warning: { label: '警告', cls: 'sev-warning', icon: 'fa-solid fa-triangle-exclamation' },
  hint: { label: '提示', cls: 'sev-hint', icon: 'fa-solid fa-lightbulb' },
  info: { label: '信息', cls: 'sev-info', icon: 'fa-solid fa-circle-info' },
}

const stats = computed(() => result.value?.stats)

function locText(entity: string | null, fragment: string | null): string {
  if (entity) return `涉及：${store.entityName(entity)}`
  if (fragment) return '涉及正文段落'
  return ''
}
</script>

<template>
  <div class="diag-panel">
    <div class="dp-head">
      <div class="dp-filters">
        <button class="dp-filter" :class="{ 'is-active': filter === 'all' }" @click="filter = 'all'">
          全部 <span class="num">{{ result?.diagnostics.length ?? 0 }}</span>
        </button>
        <button class="dp-filter" :class="{ 'is-active': filter === 'error' }" @click="filter = 'error'">
          错误 <span class="num">{{ store.errorCount }}</span>
        </button>
        <button class="dp-filter" :class="{ 'is-active': filter === 'warning' }" @click="filter = 'warning'">
          警告 <span class="num">{{ store.warningCount }}</span>
        </button>
      </div>
      <button class="ink-btn ink-btn--sm" :disabled="store.loading" @click="store.runCompile()">
        <i class="fa-solid fa-rotate" />
        重查
      </button>
    </div>

    <!-- 解析失败 -->
    <div v-if="result && !result.ok" class="dp-fatal">
      <i class="fa-solid fa-circle-xmark" />
      <p>暂时无法完成检查，请检查最近修改的内容</p>
      <pre>{{ result.error }}</pre>
    </div>

    <!-- 通过 -->
    <div v-else-if="visible.length === 0" class="dp-clean">
      <i class="fa-solid fa-circle-check" />
      <p>很好，没有发现{{ filter === 'error' ? '错误' : filter === 'warning' ? '警告' : '问题' }}</p>
    </div>

    <!-- 列表 -->
    <div
      v-for="(d, i) in visible"
      :key="i"
      class="diag-item"
      :class="sevMeta[d.severity]?.cls"
    >
      <i :class="sevMeta[d.severity]?.icon ?? 'fa-solid fa-circle-info'" />
      <div class="di-main">
        <div class="di-msg">{{ d.message }}</div>
        <div v-if="locText(d.entity, d.fragment)" class="di-meta">
          <span>{{ locText(d.entity, d.fragment) }}</span>
        </div>
      </div>
    </div>

    <!-- 统计 -->
    <div v-if="stats" class="dp-stats">
      <div class="stat">
        <span class="num">{{ stats.fragmentCount }}</span>
        <label>段落</label>
      </div>
      <div class="stat">
        <span class="num">{{ stats.entityCount }}</span>
        <label>设定</label>
      </div>
      <div class="stat">
        <span class="num">{{ stats.edgeCount }}</span>
        <label>关系</label>
      </div>
      <div class="stat">
        <span class="num">{{ stats.assertionCount }}</span>
        <label>事实</label>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.diag-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.dp-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.dp-filters {
  display: flex;
  gap: 4px;
  padding: 3px;
  border-radius: 8px;
  background: var(--surface-2);
}

.dp-filter {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 9px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 11.5px;
  cursor: pointer;
  transition: all 0.2s;

  .num {
    font-size: 10.5px;
    opacity: 0.8;
  }

  &.is-active {
    background: var(--surface-solid);
    color: var(--ink-accent);
    box-shadow: var(--ui-shadow);
  }
}

.dp-fatal {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 16px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--state-danger) 8%, transparent);
  color: var(--state-danger);
  font-size: 12.5px;

  i {
    font-size: 20px;
  }

  pre {
    padding: 8px;
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--ink-sec);
    font-size: 11px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 160px;
    overflow-y: auto;
  }
}

.dp-clean {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 36px 0;
  color: var(--ink-disabled);
  font-size: 12.5px;

  i {
    font-size: 26px;
    color: var(--state-success);
    opacity: 0.8;
  }
}

.diag-item {
  display: flex;
  gap: 10px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--divider);
  background: var(--surface-1);

  > i {
    font-size: 13px;
    margin-top: 2px;
  }

  &.sev-error > i {
    color: var(--state-danger);
  }

  &.sev-warning > i {
    color: var(--state-warning);
  }

  &.sev-hint > i,
  &.sev-info > i {
    color: var(--ink-muted);
  }
}

.di-main {
  flex: 1;
  min-width: 0;
}

.di-msg {
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--ink-main);
}

.di-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 4px;
  font-size: 10.5px;
  color: var(--ink-muted);

  .di-code {
    background: var(--surface-2);
    padding: 0 5px;
    border-radius: 4px;
  }
}

.dp-stats {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 8px;
  margin-top: 4px;
  padding-top: 12px;
  border-top: 1px solid var(--divider);
}

.stat {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;

  span {
    font-size: 17px;
    color: var(--ink-main);
  }

  label {
    font-size: 10.5px;
    color: var(--ink-muted);
    letter-spacing: 0.1em;
  }
}
</style>
