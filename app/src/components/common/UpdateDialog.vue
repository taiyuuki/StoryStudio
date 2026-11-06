<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { useUpdateCheck } from '@/composables/useUpdateCheck'
import MarkdownView from '@/components/common/MarkdownView.vue'

// 更新弹窗 — App.vue 单一挂载，由 useUpdateCheck 全局状态驱动
// （启动自动检查 / 设置页「关于」手动检查 / 标题栏提醒入口共用）
const { update, dialogOpen, downloading, progress, runUpdate, skipVersion } = useUpdateCheck()

const currentVersion = ref('')

onMounted(async () => {
  try {
    currentVersion.value = await getVersion()
  } catch {
    // 拿不到就不展示当前版本，不影响更新流程
  }
})

/** 进度百分比（total 未知时为 null，退文本字节数） */
const percent = computed(() => {
  const p = progress.value
  if (!p || !p.total) return null
  return Math.min(100, Math.round((p.received / p.total) * 100))
})

function fmtMB(n: number): string {
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function close() {
  if (!downloading.value) dialogOpen.value = false
}
</script>

<template>
  <ElDialog
    :model-value="dialogOpen"
    title="检测到新版本"
    width="480px"
    align-center
    append-to-body
    :close-on-click-modal="false"
    :show-close="!downloading"
    @update:model-value="dialogOpen = $event"
  >
    <div v-if="update" class="upd">
      <div class="upd-versions">
        <span class="upd-ver">v{{ currentVersion || '…' }}</span>
        <i class="fa-solid fa-arrow-right-long" />
        <span class="upd-ver upd-ver--new">v{{ update.version }}</span>
      </div>

      <div v-if="update.body" class="upd-notes">
        <MarkdownView :text="update.body" />
      </div>

      <div v-if="downloading" class="upd-progress">
        <ElProgress
          v-if="percent !== null"
          :percentage="percent"
          :stroke-width="8"
          striped
          striped-flow
        />
        <p v-else class="upd-progress-text">
          正在下载… 已下载 {{ fmtMB(progress?.received ?? 0) }}
        </p>
      </div>

      <div class="upd-foot">
        <button
          class="ink-btn upd-skip"
          :disabled="downloading"
          title="此版本不再自动提醒（设置页仍可手动检查）"
          @click="skipVersion"
        >
          跳过此版本
        </button>
        <div class="upd-actions">
          <button class="ink-btn" :disabled="downloading" @click="close">以后再说</button>
          <button
            class="ink-btn ink-btn--primary ink-btn--lg"
            :disabled="downloading"
            @click="runUpdate"
          >
            <i :class="downloading ? 'fa-solid fa-spinner fa-spin' : 'fa-solid fa-circle-arrow-down'" />
            {{ downloading ? '下载中…' : '更新并重启' }}
          </button>
        </div>
      </div>
    </div>
  </ElDialog>
</template>

<style scoped lang="scss">
.upd {
  display: flex;
  flex-direction: column;
}

.upd-versions {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  margin-bottom: 14px;
}

.upd-ver {
  font-size: 14px;
  color: var(--ink-sec);
  letter-spacing: 0.04em;

  &--new {
    font-size: 17px;
    font-weight: 600;
    color: var(--ink-accent);
  }

  + i {
    font-size: 12px;
    color: var(--ink-disabled);
  }
}

.upd-notes {
  max-height: 240px;
  overflow-y: auto;
  margin-bottom: 14px;
  padding: 12px 14px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);
  font-size: 13px;
}

.upd-progress {
  margin-bottom: 14px;
}

.upd-progress-text {
  margin: 0;
  text-align: center;
  font-size: 12.5px;
  color: var(--ink-muted);
}

.upd-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.upd-skip {
  color: var(--ink-muted);
}

.upd-actions {
  display: flex;
  gap: 10px;
}
</style>
