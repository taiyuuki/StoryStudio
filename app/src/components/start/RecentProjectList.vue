<script setup lang="ts">
import { computed } from 'vue'
import { useUiStore } from '@/stores/ui'
import { open } from '@tauri-apps/plugin-dialog'

const emit = defineEmits<{ open: [root: string] }>()

const ui = useUiStore()

const projects = computed(() => ui.recentProjects)

async function pickAndOpen() {
  const dir = await open({
    directory: true,
    title: '选择 StoryStudio 项目目录（含 project.toml）',
  })
  if (typeof dir === 'string') emit('open', dir)
}

function fmtTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}/${pad(d.getMonth() + 1)}/${pad(d.getDate())}`
}

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}
</script>

<template>
  <div class="recent-list">
    <div v-if="projects.length === 0" class="empty">
      <i class="fa-regular fa-folder-open" />
      <p>还没有打开过的项目</p>
    </div>

    <template v-else>
      <div
        v-for="p in projects"
        :key="p.root"
        class="recent-item"
        :title="p.root"
        @click="emit('open', p.root)"
      >
        <i class="fa-solid fa-book" />
        <div class="ri-main">
          <div class="ri-name">{{ p.name }}</div>
          <div class="ri-path">{{ baseName(p.root) }} · {{ fmtTime(p.openedAt) }}</div>
        </div>
        <button
          class="ri-remove"
          title="从列表移除"
          @click.stop="ui.forgetProject(p.root)"
        >
          <i class="fa-solid fa-xmark" />
        </button>
      </div>
    </template>

    <button class="ink-btn open-btn" @click="pickAndOpen">
      <i class="fa-solid fa-folder-tree" />
      打开其他项目…
    </button>
  </div>
</template>

<style scoped lang="scss">
.recent-list {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding-top: 6px;
  overflow-y: auto;
}

.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--ink-disabled);
  font-size: 13px;

  i {
    font-size: 30px;
    opacity: 0.6;
  }
}

.recent-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 10px;
  cursor: pointer;
  border: 1px solid transparent;
  transition: all 0.2s;

  > i {
    color: var(--ink-accent);
    font-size: 15px;
  }

  &:hover {
    background: var(--surface-2);
    border-color: var(--divider);

    .ri-remove {
      opacity: 1;
    }
  }
}

.ri-main {
  flex: 1;
  min-width: 0;
}

.ri-name {
  font-size: 13.5px;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ri-path {
  margin-top: 2px;
  font-size: 11.5px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ri-remove {
  opacity: 0;
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 24px;
  height: 24px;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s;

  &:hover {
    background: var(--ink-accent-soft);
    color: var(--state-danger);
  }
}

.open-btn {
  margin-top: 10px;
  justify-content: center;
  width: 100%;
}
</style>
