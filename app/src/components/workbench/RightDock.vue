<script setup lang="ts">
import type { Component } from 'vue'
import { computed } from 'vue'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import AiPanel from './AiPanel.vue'
import BookPanel from './BookPanel.vue'
import EntityPanel from './EntityPanel.vue'
import ThreadPanel from './ThreadPanel.vue'
import DiagnosticsPanel from './DiagnosticsPanel.vue'
import ExportPanel from './ExportPanel.vue'
import StatsPanel from './StatsPanel.vue'
import WorldviewPanel from './WorldviewPanel.vue'
import ChaptersPanel from './ChaptersPanel.vue'

const ui = useUiStore()
const store = useWorkspaceStore()

const panels = computed(
  () =>
    new Map<string, { label: string; icon: string; comp: Component; badge?: number }>([
      ['miao', { label: '妙笔', icon: 'fa-solid fa-feather-pointed', comp: AiPanel }],
      ['stats', { label: '码字统计', icon: 'fa-solid fa-chart-simple', comp: StatsPanel }],
      ['book', { label: '逐章成书', icon: 'fa-solid fa-book-open', comp: BookPanel }],
      ['chapters', { label: '章节脉络', icon: 'fa-solid fa-list-ol', comp: ChaptersPanel }],
      ['entities', { label: '设定', icon: 'fa-solid fa-users', comp: EntityPanel }],
      ['worldview', { label: '世界观', icon: 'fa-solid fa-globe', comp: WorldviewPanel }],
      ['threads', { label: '叙事线索', icon: 'fa-solid fa-route', comp: ThreadPanel }],
      [
        'diagnostics',
        {
          label: '一致性检查',
          icon: 'fa-solid fa-stethoscope',
          comp: DiagnosticsPanel,
          badge: store.errorCount + store.warningCount,
        },
      ],
      ['export', { label: '导出成书', icon: 'fa-solid fa-file-export', comp: ExportPanel }],
    ]),
)

const activeComp = computed(() => {
  if (!ui.activePanel) return null
  return panels.value.get(ui.activePanel)?.comp ?? null
})

const activeMeta = computed(() => {
  if (!ui.activePanel) return null
  return panels.value.get(ui.activePanel) ?? null
})
</script>

<template>
  <aside v-if="store.ws" class="dock">
    <div class="dock-rail">
      <button
        v-for="[key, p] in panels"
        :key="key"
        class="rail-btn"
        :class="{ 'is-active': ui.activePanel === key }"
        :title="p.label"
        @click="ui.togglePanel(key as any)"
      >
        <i :class="p.icon" />
        <span v-if="p.badge" class="rail-badge num">{{ p.badge > 99 ? '99+' : p.badge }}</span>
      </button>
    </div>

    <Transition name="dock-panel">
      <div v-if="activeComp && activeMeta" class="dock-panel">
        <div class="dock-head">
          <i :class="activeMeta.icon" />
          <span>{{ activeMeta.label }}</span>
          <button class="ink-icon-btn dock-close" @click="ui.activePanel = null">
            <i class="fa-solid fa-xmark" />
          </button>
        </div>
        <div class="dock-body">
          <component :is="activeComp" />
        </div>
      </div>
    </Transition>
  </aside>
</template>

<style scoped lang="scss">
.dock {
  display: flex;
  min-height: 0;
  border-left: 1px solid var(--divider);
  background: var(--surface-1);
  backdrop-filter: blur(12px);
}

.dock-rail {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 12px 0;
  width: 48px;
  flex-shrink: 0;
  border-right: 1px solid var(--divider);
}

.rail-btn {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: var(--ink-muted);
  font-size: 14px;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &.is-active {
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
  }
}

.rail-badge {
  position: absolute;
  top: -2px;
  right: -2px;
  min-width: 15px;
  height: 15px;
  padding: 0 3px;
  border-radius: 8px;
  background: var(--state-danger);
  color: #fff;
  font-size: 9.5px;
  line-height: 15px;
  text-align: center;
}

.dock-panel {
  display: flex;
  flex-direction: column;
  width: v-bind('ui.dockWidth + "px"');
  min-height: 0;
}

.dock-head {
  display: flex;
  align-items: center;
  gap: 9px;
  height: 46px;
  padding: 0 8px 0 16px;
  flex-shrink: 0;
  font-size: 13px;
  color: var(--ink-main);
  border-bottom: 1px solid var(--divider);

  i {
    color: var(--ink-accent);
    font-size: 13px;
  }

  span {
    flex: 1;
  }
}

.dock-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 14px 16px 20px;
}

// 面板开合动画
.dock-panel-enter-active,
.dock-panel-leave-active {
  transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
}

.dock-panel-enter-from,
.dock-panel-leave-to {
  opacity: 0;
  margin-right: -20px;
}
</style>
