<script setup lang="ts">
import { ref, watch } from 'vue'
import { ElMessageBox } from 'element-plus'
import type { FragmentVersionDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const props = defineProps<{
  fragmentId: string
}>()

const model = defineModel<boolean>({ default: false })

const store = useWorkspaceStore()
const versions = ref<FragmentVersionDto[]>([])
const selected = ref<FragmentVersionDto | null>(null)
const preview = ref('')
const loading = ref(false)
const restoring = ref(false)

watch(model, async (open) => {
  if (!open || !props.fragmentId) return
  loading.value = true
  selected.value = null
  preview.value = ''
  try {
    versions.value = await store.loadVersions(props.fragmentId)
  } finally {
    loading.value = false
  }
})

async function pick(v: FragmentVersionDto) {
  selected.value = v
  preview.value = await store.readVersion(props.fragmentId, v.timestamp)
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleString('zh-CN', { hour12: false })
}

async function restore() {
  if (!selected.value) return
  try {
    await ElMessageBox.confirm(
      '恢复前会先把现在的内容存进历史，换错了随时能再换回来。要恢复到这一版吗？',
      '恢复版本',
      { type: 'warning', confirmButtonText: '恢复', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  restoring.value = true
  try {
    if (await store.restoreVersion(props.fragmentId, selected.value.timestamp)) {
      model.value = false
    }
  } finally {
    restoring.value = false
  }
}
</script>

<template>
  <ElDialog
    v-model="model"
    title="版本历史"
    width="720px"
    align-center
    append-to-body
    class="history-dialog"
  >
    <div v-loading="loading" class="history-body">
      <div v-if="versions.length === 0 && !loading" class="history-empty">
        <i class="fa-regular fa-clock" />
        <p>暂无历史版本</p>
        <p class="hint">写作时会自动留底：停笔一会儿，或一次改动比较大，就会存下一版</p>
      </div>

      <template v-else>
        <ul class="version-list">
          <li
            v-for="v in versions"
            :key="v.timestamp"
            :class="{ 'is-active': selected?.timestamp === v.timestamp }"
            @click="pick(v)"
          >
            <i class="fa-regular fa-clock" />
            <div class="v-meta">
              <span class="v-time">{{ formatTime(v.timestamp) }}</span>
              <span class="v-words num">{{ v.words.toLocaleString() }} 字</span>
            </div>
          </li>
        </ul>
        <div class="version-preview">
          <pre v-if="preview">{{ preview }}</pre>
          <p v-else class="hint">选择左侧版本查看内容</p>
        </div>
      </template>
    </div>

    <template #footer>
      <button class="ink-btn ink-btn--sm" @click="model = false">关闭</button>
      <button
        class="ink-btn ink-btn--sm ink-btn--primary"
        :disabled="!selected || restoring"
        @click="restore"
      >
        <i :class="restoring ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-rotate-left'" />
        恢复此版本
      </button>
    </template>
  </ElDialog>
</template>

<style scoped lang="scss">
.history-body {
  display: flex;
  gap: 14px;
  min-height: 320px;
  max-height: 56vh;
}

.history-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--ink-disabled);

  i {
    font-size: 26px;
  }

  p {
    font-size: 13px;
  }

  .hint {
    font-size: 11.5px;
    max-width: 300px;
    text-align: center;
    line-height: 1.7;
  }
}

.version-list {
  width: 220px;
  flex-shrink: 0;
  margin: 0;
  padding: 0;
  list-style: none;
  overflow-y: auto;
  border-right: 1px solid var(--divider);

  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 10px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--ink-muted);
    font-size: 12px;
    transition: background 0.15s;

    &:hover {
      background: var(--surface-2);
    }

    &.is-active {
      background: var(--ink-accent-soft);
      color: var(--ink-accent);
    }
  }

  .v-meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .v-time {
    color: inherit;
  }

  .v-words {
    font-size: 11px;
    opacity: 0.7;
  }
}

.version-preview {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  background: var(--surface-2);
  border-radius: 8px;
  padding: 12px 14px;

  pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: 'Noto Serif SC', 'Songti SC', serif;
    font-size: 13.5px;
    line-height: 2;
    color: var(--ink-main);
  }

  .hint {
    color: var(--ink-disabled);
    font-size: 12px;
    text-align: center;
    padding-top: 120px;
  }
}
</style>
