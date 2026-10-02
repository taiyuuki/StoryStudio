<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import type { AiLedgerSummaryDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const store = useWorkspaceStore()

const goalEditing = ref(false)
const goalDraft = ref(2000)
const ledger = ref<AiLedgerSummaryDto | null>(null)

// AI 账本分页（后端返回全量保留条目）
const PAGE_SIZE = 10
const page = ref(1)

onMounted(() => {
  void store.refreshStats()
  if (store.root) {
    api.aiLedgerSummary(store.root).then(
      (v) => {
        ledger.value = v
        page.value = 1
      },
      () => {},
    )
  }
  goalDraft.value = store.stats?.dailyGoal ?? 2000
})

const totalPages = computed(() =>
  ledger.value ? Math.max(1, Math.ceil(ledger.value.recent.length / PAGE_SIZE)) : 1,
)
const pagedRecords = computed(() => {
  const list = ledger.value?.recent ?? []
  return list.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE)
})

function turnPage(delta: number) {
  page.value = Math.min(totalPages.value, Math.max(1, page.value + delta))
}

const stats = computed(() => store.stats)
const todayTotal = computed(() => (stats.value?.today.manual ?? 0) + (stats.value?.today.ai ?? 0))
const goalPercent = computed(() =>
  stats.value?.dailyGoal ? Math.min(100, Math.round((todayTotal.value / stats.value.dailyGoal) * 100)) : 0,
)
const maxDayWords = computed(() =>
  Math.max(1, ...(stats.value?.recentDays ?? []).map((d) => d.count.manual + d.count.ai)),
)

function weekday(dateStr: string): string {
  return ['日', '一', '二', '三', '四', '五', '六'][new Date(dateStr + 'T00:00:00').getDay()]
}
function shortDate(dateStr: string): string {
  return dateStr.slice(5).replace('-', '/')
}

async function saveGoal() {
  await store.setDailyGoal(goalDraft.value)
  goalEditing.value = false
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s}s`
  return `${Math.floor(s / 60)}m${s % 60}s`
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleString('zh-CN', { hour12: false })
}
</script>

<template>
  <div class="stats-panel">
    <!-- 今日 -->
    <section v-if="stats" class="today-card">
      <div class="today-head">
        <span class="today-label">今日码字</span>
        <span v-if="stats.streakDays > 0" class="streak" title="连续码字天数">
          <i class="fa-solid fa-fire" />
          连续 {{ stats.streakDays }} 天
        </span>
      </div>
      <div class="today-nums num">
        <span class="today-total">{{ todayTotal.toLocaleString() }}</span>
        <span class="today-unit">字</span>
      </div>
      <div class="today-split">
        <span>手写 {{ stats.today.manual.toLocaleString() }}</span>
        <span>AI {{ stats.today.ai.toLocaleString() }}</span>
      </div>
      <ElProgress
        :percentage="goalPercent"
        :stroke-width="8"
        :show-text="false"
        :color="'var(--ink-accent)'"
      />
      <div class="goal-row">
        <template v-if="!goalEditing">
          <span class="goal-text">
            每日目标 <span class="num">{{ stats.dailyGoal.toLocaleString() }}</span> 字（{{ goalPercent }}%）
          </span>
          <button class="ink-icon-btn" title="修改目标" @click="goalDraft = stats.dailyGoal; goalEditing = true">
            <i class="fa-solid fa-pen" />
          </button>
        </template>
        <template v-else>
          <ElInputNumber v-model="goalDraft" :min="100" :max="50000" :step="100" size="small" />
          <button class="ink-btn ink-btn--sm ink-btn--primary" @click="saveGoal">保存</button>
          <button class="ink-btn ink-btn--sm" @click="goalEditing = false">取消</button>
        </template>
      </div>
    </section>

    <!-- 近 7 天 -->
    <section v-if="stats" class="recent-card">
      <h4 class="card-title">近 7 天</h4>
      <div class="bar-chart">
        <div v-for="d in stats.recentDays" :key="d.date" class="bar-col" :title="`${d.date}：手写 ${d.count.manual} / AI ${d.count.ai} 字`">
          <div class="bar-stack">
            <div class="bar ai" :style="{ height: `${(d.count.ai / maxDayWords) * 100}%` }" />
            <div class="bar manual" :style="{ height: `${(d.count.manual / maxDayWords) * 100}%` }" />
          </div>
          <span class="bar-label">{{ weekday(d.date) }}</span>
        </div>
      </div>
      <div class="chart-legend">
        <span><i class="dot manual" />手写</span>
        <span><i class="dot ai" />AI</span>
        <span class="chart-total">
          累计 <span class="num">{{ (stats.total.manual + stats.total.ai).toLocaleString() }}</span> 字
        </span>
      </div>
    </section>

    <!-- AI 调用账本 -->
    <section class="ledger-card">
      <h4 class="card-title">AI 调用</h4>
      <div v-if="ledger && ledger.totalCalls > 0" class="ledger-body">
        <div class="ledger-totals">
          <div class="lt-item">
            <span class="num">{{ ledger.totalCalls.toLocaleString() }}</span>
            <label>次调用</label>
          </div>
          <div class="lt-item">
            <span class="num">{{ ledger.totalTokens.toLocaleString() }}</span>
            <label>tokens</label>
          </div>
          <div class="lt-item">
            <span class="num">{{ formatDuration(ledger.totalDurationMs) }}</span>
            <label>累计耗时</label>
          </div>
        </div>
        <ul class="ledger-list">
          <li v-for="(r, i) in pagedRecords" :key="`${page}-${i}`">
            <i
              class="fa-solid"
              :class="r.ok ? 'fa-circle-check text-success' : 'fa-circle-exclamation text-danger'"
            />
            <div class="lr-main">
              <span class="lr-scene">{{ r.scene }}</span>
              <span class="lr-meta num">
                {{ r.tokens.toLocaleString() }} tok · {{ formatDuration(r.durationMs) }} · {{ r.model || r.provider }}
              </span>
            </div>
            <span class="lr-time">{{ formatTime(r.ts) }}</span>
          </li>
        </ul>
        <div v-if="totalPages > 1" class="ledger-pager">
          <button
            class="ink-icon-btn"
            :disabled="page <= 1"
            title="上一页"
            @click="turnPage(-1)"
          >
            <i class="fa-solid fa-angle-left" />
          </button>
          <span class="pager-ind num">{{ page }} / {{ totalPages }}</span>
          <button
            class="ink-icon-btn"
            :disabled="page >= totalPages"
            title="下一页"
            @click="turnPage(1)"
          >
            <i class="fa-solid fa-angle-right" />
          </button>
        </div>
      </div>
      <p v-else class="ledger-empty">还没有 AI 调用记录；续写、妙笔对话、逐章成书的花销会记在这里</p>
    </section>
  </div>
</template>

<style scoped lang="scss">
.stats-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.card-title {
  margin: 0 0 10px;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.1em;
  color: var(--ink-muted);
}

// ── 今日 ────────────────────────────────────────────────
.today-card {
  padding: 14px 16px;
  border: 1px solid var(--divider);
  border-radius: 12px;
  background: var(--surface-1);
}

.today-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
}

.today-label {
  font-size: 12px;
  color: var(--ink-muted);
  letter-spacing: 0.1em;
}

.streak {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--state-warning, #e6a23c) 14%, transparent);
  color: var(--state-warning, #e6a23c);
  font-size: 11px;
}

.today-nums {
  display: flex;
  align-items: baseline;
  gap: 5px;
  margin-bottom: 4px;
}

.today-total {
  font-size: 30px;
  font-weight: 600;
  color: var(--ink-main);
}

.today-unit {
  font-size: 12px;
  color: var(--ink-disabled);
}

.today-split {
  display: flex;
  gap: 12px;
  margin-bottom: 12px;
  font-size: 11.5px;
  color: var(--ink-muted);
}

.goal-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 8px;

  .goal-text {
    flex: 1;
    font-size: 11.5px;
    color: var(--ink-muted);
  }
}

// ── 近 7 天 ──────────────────────────────────────────────
.recent-card {
  padding: 14px 16px;
  border: 1px solid var(--divider);
  border-radius: 12px;
  background: var(--surface-1);
}

.bar-chart {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  height: 96px;
  padding: 0 2px;
}

.bar-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 5px;
  height: 100%;
}

.bar-stack {
  flex: 1;
  width: 100%;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  border-radius: 4px 4px 0 0;
  overflow: hidden;

  .bar {
    width: 100%;
    min-height: 0;
    transition: height 0.3s ease;

    &.manual {
      background: var(--ink-accent);
      opacity: 0.85;
      border-radius: 4px 4px 0 0;
    }

    &.ai {
      background: var(--ink-accent-soft);
      border: 1px solid color-mix(in srgb, var(--ink-accent) 35%, transparent);
      border-bottom: none;
    }
  }
}

.bar-label {
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.chart-legend {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 10px;
  font-size: 11px;
  color: var(--ink-muted);

  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 2px;
    margin-right: 4px;
    vertical-align: -1px;

    &.manual {
      background: var(--ink-accent);
    }

    &.ai {
      background: var(--ink-accent-soft);
      border: 1px solid var(--ink-accent);
    }
  }

  .chart-total {
    margin-left: auto;
    color: var(--ink-disabled);
  }
}

// ── AI 账本 ──────────────────────────────────────────────
.ledger-card {
  padding: 14px 16px;
  border: 1px solid var(--divider);
  border-radius: 12px;
  background: var(--surface-1);
}

.ledger-totals {
  display: flex;
  gap: 8px;
  margin-bottom: 12px;
}

.lt-item {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  padding: 8px 4px;
  border-radius: 8px;
  background: var(--surface-2);

  span {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-main);
  }

  label {
    font-size: 10.5px;
    color: var(--ink-disabled);
  }
}

.ledger-list {
  margin: 0;
  padding: 0;
  list-style: none;

  li {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    padding: 7px 0;
    border-bottom: 1px dashed var(--divider);
    font-size: 11.5px;

    &:last-child {
      border-bottom: none;
    }

    > i {
      margin-top: 3px;
      font-size: 10px;
    }
  }

  .lr-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .lr-scene {
    color: var(--ink-main);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .lr-meta {
    color: var(--ink-disabled);
    font-size: 10.5px;
  }

  .lr-time {
    flex-shrink: 0;
    color: var(--ink-disabled);
    font-size: 10.5px;
  }
}

.ledger-pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  margin-top: 10px;

  .ink-icon-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
}

.pager-ind {
  font-size: 11px;
  color: var(--ink-muted);
  letter-spacing: 0.06em;
}

.ledger-empty {
  margin: 6px 0 2px;
  font-size: 11.5px;
  color: var(--ink-disabled);
  line-height: 1.8;
}
</style>
