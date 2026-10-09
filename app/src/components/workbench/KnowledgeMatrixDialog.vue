<script setup lang="ts">
import { ref, watch } from 'vue'
import type { KnowledgeMatrixDto, MatrixCellDto, MatrixThreadDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()
const model = defineModel<boolean>({ default: false })

const matrix = ref<KnowledgeMatrixDto | null>(null)
const loading = ref(false)

watch(model, async (open) => {
  if (!open || !store.root) return
  loading.value = true
  matrix.value = null
  try {
    matrix.value = await api.threadKnowledgeMatrix(store.root)
  } finally {
    loading.value = false
  }
})

const stateMeta: Record<string, { label: string; cls: string }> = {
  known: { label: '知晓', cls: 'c-known' },
  suspected: { label: '起疑', cls: 'c-suspected' },
  unknown: { label: '未知', cls: 'c-unknown' },
}

function cellTitle(thread: MatrixThreadDto, cell: MatrixCellDto): string {
  const meta = stateMeta[cell.state]
  if (!meta) return `${thread.title}：无记录`
  const where = cell.sceneLabel ? `（${cell.sceneLabel}）` : ''
  return `${thread.title}：${meta.label}${where}`
}

const statusLabel: Record<string, string> = {
  planned: '构想',
  planted: '已埋',
  revealed: '已揭',
  resolved: '已回收',
  abandoned: '弃用',
}

function observerIcon(kind: string): string {
  if (kind === 'reader') return 'fa-solid fa-book-open'
  if (kind === 'organization') return 'fa-solid fa-building-user'
  return 'fa-solid fa-user'
}
</script>

<template>
  <ElDialog
    v-model="model"
    title="认知矩阵 · 谁知道什么"
    width="860px"
    align-center
    append-to-body
    class="matrix-dialog"
  >
    <div v-loading="loading" class="matrix-body">
      <div v-if="matrix && matrix.threads.length === 0 && !loading" class="matrix-empty">
        <i class="fa-solid fa-table-cells" />
        <p>尚无线索记录</p>
        <p class="hint">创建线索并登记各观察者的认知时间线后，这里会摊开成一张信息差全景表</p>
      </div>

      <template v-else-if="matrix">
        <p class="matrix-legend">
          <span class="lg lg-known">知晓</span>
          <span class="lg lg-suspected">起疑</span>
          <span class="lg lg-unknown">未知</span>
          <span class="lg">·</span>
          <span class="hint">格子 = 该观察者对线索的最新认知（悬停看定格场景）；空格 = 未跟踪</span>
        </p>
        <div class="matrix-scroll">
          <table class="matrix-table">
            <thead>
              <tr>
                <th class="col-observer">观察者 ＼ 线索</th>
                <th v-for="t in matrix.threads" :key="t.id" class="col-thread">
                  <span class="t-title">{{ t.title }}</span>
                  <span class="t-meta" :class="`st-${t.status}`">
                    {{ statusLabel[t.status] ?? t.status }}{{ t.priority === 'main' ? ' · 主线' : '' }}
                  </span>
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(obs, r) in matrix.observers" :key="r" :class="{ 'row-reader': obs.kind === 'reader' }">
                <td class="col-observer">
                  <i :class="observerIcon(obs.kind)" />
                  {{ obs.label }}
                </td>
                <td v-for="(cell, c) in matrix.cells[r]" :key="c">
                  <span
                    v-if="cell.state && stateMeta[cell.state]"
                    class="cell"
                    :class="stateMeta[cell.state].cls"
                    :title="cellTitle(matrix.threads[c], cell)"
                  >
                    {{ stateMeta[cell.state].label }}
                  </span>
                  <span v-else class="cell cell-none">·</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </template>
    </div>

    <template #footer>
      <button class="ink-btn ink-btn--sm" @click="model = false">关闭</button>
    </template>
  </ElDialog>
</template>

<style scoped lang="scss">
.matrix-body {
  min-height: 240px;
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.matrix-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--ink-disabled);

  i { font-size: 26px; }
  p { font-size: 13px; }
  .hint { font-size: 11.5px; max-width: 340px; text-align: center; line-height: 1.7; }
}

.matrix-legend {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  font-size: 12px;
  color: var(--ink-muted);

  .lg {
    display: inline-block;
    padding: 1px 8px;
    border-radius: 4px;
    font-size: 11px;
  }
  .lg-known { background: rgba(74, 157, 107, 0.16); color: var(--success, #4a9d6b); }
  .lg-suspected { background: rgba(200, 150, 60, 0.18); color: #b8862d; }
  .lg-unknown { background: var(--surface-2); color: var(--ink-muted); }

  .hint {
    margin-left: 4px;
    font-size: 11px;
    color: var(--ink-disabled);
  }
}

.matrix-scroll {
  flex: 1;
  overflow: auto;
  border: 1px solid var(--divider);
  border-radius: 8px;
}

.matrix-table {
  border-collapse: separate;
  border-spacing: 0;
  width: 100%;
  font-size: 12px;

  th, td {
    border-bottom: 1px solid var(--divider);
    border-right: 1px solid var(--divider);
    padding: 7px 10px;
    text-align: left;
    white-space: nowrap;
  }

  thead th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    z-index: 2;
    font-weight: 500;
    color: var(--ink-muted);
  }

  .col-observer {
    position: sticky;
    left: 0;
    background: var(--surface-1, #fff);
    z-index: 1;
    min-width: 120px;
    color: var(--ink-main);

    i {
      width: 16px;
      color: var(--ink-muted);
      margin-right: 4px;
      font-size: 11px;
    }
  }

  thead .col-observer { z-index: 3; }

  .row-reader .col-observer {
    font-weight: 600;
  }

  .col-thread {
    min-width: 110px;
    max-width: 180px;
    white-space: normal;

    .t-title {
      display: block;
      color: var(--ink-main);
      line-height: 1.5;
    }
    .t-meta {
      display: inline-block;
      margin-top: 2px;
      font-size: 10.5px;
      padding: 0 5px;
      border-radius: 3px;
      background: var(--surface-2);
      color: var(--ink-muted);

      &.st-planted { color: var(--success, #4a9d6b); }
      &.st-revealed { color: #b8862d; }
      &.st-resolved { color: var(--ink-accent); }
      &.st-abandoned { color: var(--danger, #c4554d); }
    }
  }

  tbody tr:hover td {
    background: var(--surface-2);
  }
  tbody tr:hover .col-observer {
    background: var(--surface-2);
  }

  .cell {
    display: inline-block;
    min-width: 40px;
    text-align: center;
    padding: 2px 8px;
    border-radius: 5px;
    font-size: 11.5px;
    cursor: default;
  }
  .c-known { background: rgba(74, 157, 107, 0.16); color: var(--success, #4a9d6b); }
  .c-suspected { background: rgba(200, 150, 60, 0.18); color: #b8862d; }
  .c-unknown { background: var(--surface-2); color: var(--ink-muted); }
  .cell-none { color: var(--ink-disabled); }
}
</style>
