<script setup lang="ts">
import { useRouter } from 'vue-router'
import { useWorkspaceStore } from '@/stores/workspace'
import NewProjectCard from '@/components/start/NewProjectCard.vue'
import RecentProjectList from '@/components/start/RecentProjectList.vue'

const router = useRouter()
const store = useWorkspaceStore()

async function openProject(root: string) {
  if (await store.openRoot(root)) router.push('/workbench')
}
</script>

<template>
  <div class="ink-bg-container start-view">
    <div class="paper-texture" />
    <div class="start-wrap">
      <div class="start-hero">
        <div class="hero-seal seal-font">书</div>
        <h1 class="hero-title">StoryStudio</h1>
        <p class="hero-sub">设定为骨 · 伏笔为脉 · 一气呵成</p>
      </div>

      <div class="start-columns">
        <NewProjectCard @created="openProject">
          <div class="ai-book-entry">
            <div class="entry-divider"><span>或者，让 AI 帮你开书</span></div>
            <button class="ink-btn ai-book-btn" @click="router.push('/create')">
              <i class="fa-solid fa-wand-magic-sparkles" />
              一句话灵感，一步步开出一本书
            </button>
          </div>
        </NewProjectCard>
        <div class="fusion-card recent-card">
          <div class="card-head">
            <i class="fa-solid fa-clock-rotate-left" />
            <span>最近打开</span>
          </div>
          <RecentProjectList @open="openProject" />
        </div>
      </div>

      <p class="start-foot text-ink-muted">
        作品以纯文本形式保存在你选择的文件夹里，随时可备份、可迁移
      </p>
    </div>
  </div>
</template>

<style scoped lang="scss">
.start-view {
  overflow-y: auto;
}

.start-wrap {
  position: relative;
  z-index: 1;
  max-width: 860px;
  margin: 0 auto;
  padding: 7vh 32px 40px;
}

.start-hero {
  text-align: center;
  margin-bottom: 44px;
  animation: rise 0.6s cubic-bezier(0.22, 1, 0.36, 1) both;

  .hero-seal {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 76px;
    height: 76px;
    border-radius: 18px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-color);
    font-size: 38px;
    box-shadow: var(--ui-shadow-hover);
    margin-bottom: 18px;
  }

  .hero-title {
    font-size: 30px;
    font-weight: 600;
    letter-spacing: 0.12em;
    color: var(--ink-main);
  }

  .hero-sub {
    margin-top: 10px;
    font-size: 14px;
    letter-spacing: 0.28em;
    color: var(--ink-muted);
  }
}

.start-columns {
  display: grid;
  grid-template-columns: 1.1fr 1fr;
  gap: 20px;
  animation: rise 0.6s 0.1s cubic-bezier(0.22, 1, 0.36, 1) both;
}

.recent-card {
  padding: 22px 24px;
  display: flex;
  flex-direction: column;
  min-height: 300px;
}

// ── AI 开书入口 ─────────────────────────────────────────
.entry-divider {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 4px 0 12px;
  color: var(--ink-disabled);
  font-size: 11.5px;

  &::before,
  &::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--divider);
  }
}

.ai-book-btn {
  width: 100%;

  i {
    color: var(--ink-accent);
  }

  &:hover i {
    animation: wand-wave 0.7s ease;
  }
}

@keyframes wand-wave {
  0%,
  100% {
    transform: rotate(0deg);
  }
  25% {
    transform: rotate(-14deg);
  }
  60% {
    transform: rotate(10deg);
  }
}

.card-head {
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 14px;
  color: var(--ink-main);
  padding-bottom: 14px;
  border-bottom: 1px solid var(--divider);
  margin-bottom: 8px;

  i {
    color: var(--ink-accent);
  }
}

.start-foot {
  margin-top: 36px;
  text-align: center;
  font-size: 12px;
  letter-spacing: 0.1em;
  animation: rise 0.6s 0.2s cubic-bezier(0.22, 1, 0.36, 1) both;
}

@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(18px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
</style>
