<script setup lang="ts">
import type { ProjectPeekDto } from '@/api/backend'
import type { LastScene, RecentProject } from '@/stores/ui'
import { formatRelativeTime, formatWordCount } from '@/utils/format'

defineProps<{
  recent: RecentProject
  /** undefined = 尚未加载；null = 目录不可用 */
  peek: ProjectPeekDto | null | undefined
  lastScene: LastScene | null
}>()

const emit = defineEmits<{
  continue: []
  book: []
  remove: []
}>()

function pct(peek: ProjectPeekDto): number {
  if (!peek.targetWords) return 0
  return Math.min(100, Math.round((peek.wordCount / peek.targetWords) * 100))
}
</script>

<template>
  <!-- 目录已不可用（peek 显式为 null）：灰化 + 只提供移除 -->
  <div v-if="peek === null" class="fusion-card continue-card is-missing">
    <div class="cc-spine"><i class="fa-regular fa-folder-open" /></div>
    <div class="cc-main">
      <div class="cc-title">{{ recent.name }}</div>
      <div class="cc-miss-note">目录已不存在或已损坏（{{ recent.root }}）</div>
    </div>
    <button class="ink-btn ink-btn--danger" @click="emit('remove')">从列表移除</button>
  </div>

  <!-- 加载中 / 尚未返回：骨架 -->
  <div v-else-if="!peek" class="fusion-card continue-card">
    <div class="cc-spine sk sk-spine" />
    <div class="cc-main">
      <div class="sk sk-title" />
      <div class="sk sk-line" />
      <div class="sk sk-bar" />
    </div>
  </div>

  <!-- 正常：书脊式大卡 -->
  <div v-else class="fusion-card continue-card" :class="{ 'is-fresh': !lastScene }">
    <div class="cc-spine">
      <i class="fa-solid fa-book" />
      <span class="cc-spine-mark seal-font">卷</span>
    </div>

    <div class="cc-main">
      <div class="cc-head">
        <div class="cc-title-row">
          <h3 class="cc-title">{{ peek.name }}</h3>
          <span v-if="lastScene" class="cc-due text-ink-muted">
            上次 {{ formatRelativeTime(lastScene.ts) }}
          </span>
        </div>
        <div class="cc-actions">
          <button class="ink-btn ink-btn--lg" title="打开逐章成书面板" @click="emit('book')">
            <i class="fa-solid fa-layer-group" />
            逐章成书
          </button>
          <button class="ink-btn ink-btn--primary ink-btn--lg" @click="emit('continue')">
            <i class="fa-solid fa-pen-nib" />
            继续写作
          </button>
        </div>
      </div>

      <!-- 进度：有目标时进度条，无目标时只展示字数 -->
      <div class="cc-progress">
        <div class="cc-progress-line">
          <span class="cc-words num">{{ formatWordCount(peek.wordCount) }} 字</span>
          <span v-if="peek.targetWords" class="cc-target num">
            / {{ formatWordCount(peek.targetWords) }} 字 · {{ pct(peek) }}%
          </span>
          <span class="cc-dot">·</span>
          <span class="cc-counts num">{{ peek.chapterCount }} 章 · {{ peek.sceneCount }} 场</span>
        </div>
        <div v-if="peek.targetWords" class="cc-bar">
          <div class="cc-bar-fill" :style="{ width: `${pct(peek)}%` }" />
        </div>
      </div>

      <div class="cc-last">
        <i class="fa-solid fa-location-dot" />
        <template v-if="lastScene">
          上次写到：<span class="cc-last-pos">{{ lastScene.chapterTitle }} · {{ lastScene.sceneTitle }}</span>
        </template>
        <template v-else>尚未落笔，从第一章开始</template>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.continue-card {
  display: flex;
  overflow: hidden;
  animation: rise 0.5s cubic-bezier(0.22, 1, 0.36, 1) both;
}

.is-missing {
  opacity: 0.8;

  .cc-spine {
    background: var(--surface-2);
    color: var(--ink-disabled);
  }

  .cc-main {
    justify-content: center;
  }
}

// ── 书脊 ─────────────────────────────────────────────────
.cc-spine {
  position: relative;
  width: 56px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  background: var(--ink-accent-soft);
  border-right: 1px solid var(--divider);
  color: var(--ink-accent);
  font-size: 20px;
}

.cc-spine-mark {
  font-size: 12px;
  letter-spacing: 0;
  opacity: 0.55;
}

// ── 主体 ─────────────────────────────────────────────────
.cc-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 22px 26px;
}

.cc-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
}

.cc-title-row {
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 12px;
}

.cc-title {
  font-size: 23px;
  font-weight: 600;
  letter-spacing: 0.05em;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cc-due {
  font-size: 12px;
  flex-shrink: 0;
}

.cc-actions {
  display: flex;
  gap: 10px;
  flex-shrink: 0;
}

.cc-progress {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.cc-progress-line {
  display: flex;
  align-items: baseline;
  gap: 7px;
  font-size: 12.5px;
  flex-wrap: wrap;
}

.cc-words {
  font-size: 15px;
  font-weight: 600;
  color: var(--ink-main);
}

.cc-target {
  color: var(--ink-muted);
}

.cc-dot {
  color: var(--ink-disabled);
  margin: 0 2px;
}

.cc-counts {
  color: var(--ink-sec);
}

.cc-bar {
  height: 5px;
  border-radius: 3px;
  background: var(--surface-2);
  overflow: hidden;
}

.cc-bar-fill {
  height: 100%;
  border-radius: 3px;
  background: linear-gradient(
    90deg,
    color-mix(in srgb, var(--ink-accent) 60%, transparent),
    var(--ink-accent)
  );
  transition: width 0.6s cubic-bezier(0.22, 1, 0.36, 1);
}

.cc-last {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    font-size: 11.5px;
    flex-shrink: 0;
    opacity: 0.8;
  }
}

.cc-last-pos {
  color: var(--ink-sec);
}

.cc-miss-note {
  margin-top: 4px;
  font-size: 12px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

// ── 加载骨架 ─────────────────────────────────────────────
.sk {
  border-radius: 6px;
  background: linear-gradient(
    90deg,
    var(--surface-2) 25%,
    color-mix(in srgb, var(--surface-2) 40%, var(--bg-main)) 50%,
    var(--surface-2) 75%
  );
  background-size: 200% 100%;
  animation: shimmer 1.4s ease infinite;
}

.sk-spine {
  border-radius: 0;
  width: 56px;
}

.sk-title {
  width: 260px;
  height: 24px;
}

.sk-line {
  width: 200px;
  height: 12px;
}

.sk-bar {
  width: 70%;
  height: 5px;
}

@keyframes shimmer {
  from {
    background-position: 200% 0;
  }
  to {
    background-position: -200% 0;
  }
}

@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(14px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
</style>
