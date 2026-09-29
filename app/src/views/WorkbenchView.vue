<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useWorkspaceStore } from '@/stores/workspace'
import StructureTree from '@/components/workbench/StructureTree.vue'
import SceneEditor from '@/components/workbench/SceneEditor.vue'
import RightDock from '@/components/workbench/RightDock.vue'

const store = useWorkspaceStore()
const router = useRouter()

onMounted(() => {
  // 直接访问 /workbench（无项目）→ 回起始页
  if (!store.root) {
    router.replace('/')
    return
  }
  // 刷新恢复：用持久化的 root 重新拉取
  if (!store.ws) void store.openRoot(store.root)
})
</script>

<template>
  <div v-if="store.ws" class="ink-bg-container workbench">
    <div class="paper-texture" />
    <div class="wb-columns">
      <StructureTree class="wb-tree" />
      <SceneEditor class="wb-editor" />
      <RightDock class="wb-dock" />
    </div>
  </div>
  <div v-else class="ink-bg-container wb-loading">
    <i class="fa-solid fa-circle-notch fa-spin" />
    <span>正在研墨…</span>
  </div>
</template>

<style scoped lang="scss">
.workbench {
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.wb-columns {
  position: relative;
  z-index: 1;
  flex: 1;
  display: flex;
  min-height: 0;
}

.wb-tree {
  flex-shrink: 0;
}

.wb-editor {
  flex: 1;
  min-width: 0;
}

.wb-dock {
  flex-shrink: 0;
}

.wb-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--ink-muted);
  font-size: 14px;
  letter-spacing: 0.2em;
}
</style>
