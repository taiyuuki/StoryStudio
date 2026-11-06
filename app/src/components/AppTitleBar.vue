<script setup lang="ts">
import { useRouter } from 'vue-router'

const router = useRouter()
import { computed, onMounted, shallowRef } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { useUpdateCheck } from '@/composables/useUpdateCheck'

defineProps<{ title?: string }>()
const emit = defineEmits<{ backHome: [] }>()

const ui = useUiStore()
const workspace = useWorkspaceStore()
const updateCheck = useUpdateCheck()
const isMac = shallowRef(true)
const maximized = shallowRef(false)

const isTauri = computed(() => !!window.__TAURI_INTERNALS__)
const needsControls = computed(() => isTauri.value && !isMac.value)

const themeIcon = computed(() => (ui.theme === 'theme-ink' ? 'fa-regular fa-moon' : 'fa-solid fa-sun'))

/** 文件监听断开警示（RFC-0019 §6.4）：仅失败停止时显示，点击重启 */
const watchBroken = computed(
  () =>
    !!workspace.root &&
    workspace.watchState?.state === 'stopped' &&
    !!workspace.watchState.reason,
)

async function restartWatch() {
  if (!workspace.root) return
  try {
    workspace.watchState = await api.watchStart(workspace.root)
  } catch (err) {
    ElMessage.error(`重启文件监听失败：${(err as Error).message}`)
  }
}

onMounted(async () => {
  isMac.value = /mac/i.test(navigator.userAgent)
  if (!isTauri.value || isMac.value) return
  const win = getCurrentWindow()
  maximized.value = await win.isMaximized()
  await win.onResized(async () => {
    maximized.value = await win.isMaximized()
  })
})

async function minimize() {
  await getCurrentWindow().minimize()
}

async function toggleMaximize() {
  await getCurrentWindow().toggleMaximize()
}

async function close() {
  await getCurrentWindow().close()
}
</script>

<template>
  <header class="title-bar" :class="{ 'title-bar--mac': isMac }" data-tauri-drag-region>
    <div class="tb-left">
      <span class="tb-logo seal-font" data-tauri-drag-region>书</span>
      <!-- 显式返回主页入口（与设置 / AI 开书页头的返回按钮同模式）；
           书名只作展示 + 拖拽区，不再承担返回职责 -->
      <button
        v-if="title"
        class="ink-icon-btn tb-back"
        title="返回主页"
        @click="emit('backHome')"
      >
        <i class="fa-solid fa-arrow-left" />
      </button>
      <span v-if="title" class="tb-title" data-tauri-drag-region>
        <i class="fa-solid fa-book-open" />
        {{ title }}
      </span>
    </div>

    <div class="tb-right">
      <button
        v-if="watchBroken"
        class="ink-icon-btn tb-watch-warn"
        title="文件监听已断开，外部修改不会自动同步；点击重启"
        @click="restartWatch"
      >
        <i class="fa-solid fa-tower-broadcast" />
      </button>
      <!-- 有可用更新时常驻提醒（「以后再说」后仍可见），点击打开更新弹窗 -->
      <button
        v-if="updateCheck.update.value && !updateCheck.downloading.value"
        class="ink-icon-btn tb-update-hint"
        title="发现新版本，点击查看"
        @click="updateCheck.openDialog()"
      >
        <i class="fa-solid fa-circle-arrow-down" />
      </button>
      <button
        class="ink-icon-btn"
        :title="ui.theme === 'theme-ink' ? '切到暗夜水墨' : '切到素宣墨白'"
        @click="ui.toggleTheme()"
      >
        <i :class="themeIcon" />
      </button>
      <button class="ink-icon-btn" title="AI 设置" @click="router.push('/settings')">
        <i class="fa-solid fa-gear" />
      </button>
      <template v-if="needsControls">
        <span class="tb-sep" />
        <button class="tb-ctrl" title="最小化" @click="minimize">
          <i class="fa-solid fa-minus" />
        </button>
        <button class="tb-ctrl" title="最大化" @click="toggleMaximize">
          <i :class="maximized ? 'fa-regular fa-window-restore' : 'fa-regular fa-square'" />
        </button>
        <button class="tb-ctrl tb-ctrl--close" title="关闭" @click="close">
          <i class="fa-solid fa-xmark" />
        </button>
      </template>
    </div>
  </header>
</template>

<style scoped lang="scss">
.title-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: var(--titlebar-h);
  padding: 0 12px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--divider);
  background: var(--surface-1);
  backdrop-filter: blur(12px);

  &--mac {
    padding-left: 84px; // 红绿灯空间
  }
}

.tb-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.tb-back {
  width: 28px;
  height: 28px;
  flex-shrink: 0;
}

.tb-watch-warn {
  color: var(--ink-warning);

  &:hover {
    color: var(--ink-warning);
    background: var(--surface-2);
  }
}

.tb-update-hint {
  color: var(--ink-accent);

  &:hover {
    color: var(--ink-accent);
    background: var(--surface-2);
  }
}

.tb-logo {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  font-size: 15px;
  letter-spacing: 0;
}

.tb-title {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  max-width: 40vw;
  padding: 5px 12px;
  border-radius: 8px;
  color: var(--ink-main);
  font-family: inherit;
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;

  i {
    color: var(--ink-accent);
    font-size: 12px;
    flex-shrink: 0;
  }
}

.tb-right {
  display: flex;
  align-items: center;
  gap: 4px;
}

.tb-sep {
  width: 1px;
  height: 16px;
  margin: 0 4px;
  background: var(--divider);
}

.tb-ctrl {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 30px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--ink-sec);
  cursor: pointer;
  font-size: 12px;
  transition: all 0.15s;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &--close:hover {
    background: var(--state-danger);
    color: #fff;
  }
}
</style>
