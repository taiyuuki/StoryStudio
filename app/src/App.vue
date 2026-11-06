<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { useUpdateCheck } from '@/composables/useUpdateCheck'
import AppTitleBar from '@/components/AppTitleBar.vue'
import UpdateDialog from '@/components/common/UpdateDialog.vue'

const ui = useUiStore()
const store = useWorkspaceStore()
const route = useRoute()
const router = useRouter()

const projectTitle = computed(() => {
  if (!store.ws) return ''
  return route.name === 'workbench' ? store.ws.bookTitle : ''
})

// 启动静默检查更新：仅生产构建（tauri dev 下 endpoint 拿到的版本号
// 与开发版本一致，弹窗只会干扰开发）；失败静默，不打扰
onMounted(() => {
  if (import.meta.env.PROD) void useUpdateCheck().checkForUpdate(false)
})

function backToStart() {
  if (route.name !== 'workbench') return
  store.closeProject()
  router.push('/')
}
</script>

<template>
  <div class="app-shell">
    <div class="paper-texture" />
    <AppTitleBar :title="projectTitle" @back-home="backToStart" />
    <div class="app-body">
      <RouterView />
    </div>
    <UpdateDialog />
  </div>
</template>

<style scoped lang="scss">
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  position: relative;
  background: var(--bg-main);
}

.app-body {
  flex: 1;
  min-height: 0;
}
</style>
