<script setup lang="ts">
import type { ProjectPeekDto } from '@/api/backend'
import type { RecentProject } from '@/stores/ui'
import { formatRelativeTime, formatWordCount } from '@/utils/format'

defineProps<{
  recent: RecentProject
  /** undefined = 尚未加载；null = 目录不可用 */
  peek: ProjectPeekDto | null | undefined
}>()

const emit = defineEmits<{
  open: [root: string]
  remove: [root: string]
}>()
</script>

<template>
  <div
    class="fusion-card shelf-card"
    :class="{ 'is-missing': peek === null }"
    :title="peek ? recent.name : recent.root"
    @click="!!peek && emit('open', recent.root)"
  >
    <button
      v-if="peek"
      class="sc-remove"
      title="从列表移除"
      @click.stop="emit('remove', recent.root)"
    >
      <i class="fa-solid fa-xmark" />
    </button>

    <!-- 目录已不存在（peek 显式为 null）：明确不可点，常显移除 -->
    <template v-if="peek === null">
      <div class="sc-icon"><i class="fa-regular fa-folder-open" /></div>
      <div class="sc-name">{{ recent.name }}</div>
      <div class="sc-miss">目录已不存在或已被移动</div>
      <button class="ink-btn ink-btn--sm sc-remove-btn" @click.stop="emit('remove', recent.root)">
        <i class="fa-solid fa-xmark" />
        从列表移除
      </button>
      <div class="sc-time">{{ formatRelativeTime(recent.openedAt) }}</div>
    </template>

    <!-- 加载中 / 尚未返回：骨架 -->
    <template v-else-if="!peek">
      <div class="sk sk-name" />
      <div class="sk sk-meta" />
      <div class="sk sk-meta sk-short" />
    </template>

    <template v-else>
      <div class="sc-icon"><i class="fa-solid fa-book" /></div>
      <div class="sc-name">{{ peek.name }}</div>
      <div class="sc-meta num">
        {{ formatWordCount(peek.wordCount) }}字 · {{ peek.chapterCount }} 章
      </div>
      <div class="sc-time">{{ formatRelativeTime(recent.openedAt) }}</div>
    </template>
  </div>
</template>

<style scoped lang="scss">
.shelf-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 16px 18px;
  min-height: 112px;
  cursor: pointer;
  transition: transform 0.22s cubic-bezier(0.4, 0, 0.2, 1), box-shadow 0.3s, border-color 0.3s;

  &:hover {
    transform: translateY(-2px);
    box-shadow: var(--ui-shadow-hover);

    .sc-remove {
      opacity: 1;
    }
  }
}

.is-missing {
  cursor: default;
  opacity: 0.8;
  border-style: dashed;
  background: transparent;
  backdrop-filter: none;
}

.sc-remove {
  position: absolute;
  top: 8px;
  right: 8px;
  opacity: 0;
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--ink-muted);
  cursor: pointer;
  transition: all 0.15s;

  &:hover {
    background: var(--ink-accent-soft);
    color: var(--state-danger);
  }
}

.sc-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: 8px;
  background: var(--ink-accent-soft);
  color: var(--ink-accent);
  font-size: 14px;
  margin-bottom: 2px;
}

.is-missing .sc-icon {
  background: var(--surface-2);
  color: var(--ink-disabled);
}

.sc-name {
  font-size: 14.5px;
  font-weight: 600;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sc-meta {
  font-size: 12px;
  color: var(--ink-sec);
}

.sc-miss {
  font-size: 12px;
  color: var(--state-warning);
}

.sc-remove-btn {
  margin-top: 4px;
  align-self: flex-start;
  color: var(--ink-sec);
}

.sc-time {
  margin-top: auto;
  font-size: 11px;
  color: var(--ink-disabled);
}

// ── 加载骨架 ─────────────────────────────────────────────
.sk {
  border-radius: 5px;
  background: linear-gradient(
    90deg,
    var(--surface-2) 25%,
    color-mix(in srgb, var(--surface-2) 40%, var(--bg-main)) 50%,
    var(--surface-2) 75%
  );
  background-size: 200% 100%;
  animation: shimmer 1.4s ease infinite;
}

.sk-name {
  width: 65%;
  height: 16px;
}

.sk-meta {
  width: 80%;
  height: 11px;
}

.sk-short {
  width: 45%;
}

@keyframes shimmer {
  from {
    background-position: 200% 0;
  }
  to {
    background-position: -200% 0;
  }
}
</style>
