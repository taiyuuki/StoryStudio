<script setup lang="ts">
import { UNDO_TTL_MS, useUndo } from '@/composables/useUndo'

const { current, restoring, runUndo, dismissUndo } = useUndo()
</script>

<template>
  <Transition name="undo-pop">
    <div
      v-if="current"
      :key="current.id"
      class="undo-toast"
      role="status"
    >
      <i class="fa-solid fa-trash-can undo-icon" />
      <span class="undo-label">已删除「{{ current.label }}」</span>
      <button
        class="ink-btn ink-btn--sm undo-btn"
        :disabled="restoring"
        @click="runUndo"
      >
        <i :class="restoring ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-rotate-left'" />
        撤销
      </button>
      <button
        class="undo-close"
        title="关闭"
        @click="dismissUndo"
      >
        <i class="fa-solid fa-xmark" />
      </button>
      <!-- 存活倒计时：随 TTL 线性收缩，条目 key 变化时自动重放 -->
      <span
        class="undo-bar"
        :style="{ animationDuration: `${UNDO_TTL_MS}ms` }"
      />
    </div>
  </Transition>
</template>

<style scoped lang="scss">
.undo-toast {
  position: fixed;
  left: 50%;
  bottom: 30px;
  z-index: 2100;
  display: flex;
  align-items: center;
  gap: 10px;
  max-width: min(72vw, 560px);
  padding: 10px 12px 12px;
  border-radius: 12px;
  overflow: hidden;
  background: var(--ui-glass-bg);
  border: 1px solid var(--ui-border);
  box-shadow: var(--ui-shadow-modal);
  backdrop-filter: blur(12px);
  transform: translateX(-50%);
}

.undo-icon {
  color: var(--state-danger);
  font-size: 14px;
}

.undo-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--ink-main);
  font-size: 13px;
}

.undo-btn {
  flex-shrink: 0;
}

.undo-close {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--ink-muted);
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }
}

.undo-bar {
  position: absolute;
  left: 0;
  bottom: 0;
  width: 100%;
  height: 2px;
  background: var(--ink-accent);
  transform-origin: left;
  animation: undo-shrink linear forwards;
}

@keyframes undo-shrink {
  from {
    transform: scaleX(1);
  }
  to {
    transform: scaleX(0);
  }
}

.undo-pop-enter-active,
.undo-pop-leave-active {
  transition:
    opacity 0.25s cubic-bezier(0.4, 0, 0.2, 1),
    transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
}

.undo-pop-enter-from,
.undo-pop-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(12px);
}
</style>
