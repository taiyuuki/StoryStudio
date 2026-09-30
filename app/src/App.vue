<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import AppTitleBar from '@/components/AppTitleBar.vue'

const ui = useUiStore()
const store = useWorkspaceStore()
const route = useRoute()
const router = useRouter()

const projectTitle = computed(() => {
  if (!store.ws) return ''
  return route.name === 'workbench' ? store.ws.bookTitle : ''
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
