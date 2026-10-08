<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { ElMessage } from 'element-plus'
import { useWorkspaceStore } from '@/stores/workspace'
import { useUiStore } from '@/stores/ui'
import { api, type ProjectPeekDto } from '@/api/backend'
import ContinueCard from '@/components/start/ContinueCard.vue'
import NewProjectDialog from '@/components/start/NewProjectDialog.vue'
import { formatRelativeTime, formatWordCount } from '@/utils/format'

const router = useRouter()
const store = useWorkspaceStore()
const ui = useUiStore()

/** root → 轻量概览（null = 目录不可用；不在 map = 尚未加载） */
const peeks = ref<Record<string, ProjectPeekDto | null>>({})
const showNew = ref(false)
/** 拖着单个文件夹悬停时显示落点提示 */
const dragOver = ref(false)
let unlistenDrop: (() => void) | null = null

const projects = computed(() => ui.recentProjects)
const primary = computed(() => projects.value[0] ?? null)
const primaryPeek = computed(() =>
  primary.value ? peeks.value[primary.value.root] : undefined,
)
/** 首页只挂最近 4 条，全量管理在「作品」页 */
const recentList = computed(() => projects.value.slice(0, 4))

// 今日字数跨项目求和；每日目标/连续天数语义属于单项目，取主项目
const todaySum = computed(() => {
  let manual = 0
  let ai = 0
  for (const p of Object.values(peeks.value)) {
    if (!p) continue
    manual += p.today.manual
    ai += p.today.ai
  }
  return { manual, ai }
})
const todayGoal = computed(() => primaryPeek.value?.dailyGoal ?? 0)
const streakDays = computed(() => primaryPeek.value?.streakDays ?? 0)
const todayPct = computed(() => {
  const total = todaySum.value.manual + todaySum.value.ai
  return todayGoal.value ? Math.min(100, Math.round((total / todayGoal.value) * 100)) : 0
})

const greeting = computed(() => {
  const h = new Date().getHours()
  if (h < 5) return '夜深了'
  if (h < 11) return '早上好'
  if (h < 14) return '中午好'
  if (h < 18) return '下午好'
  return '晚上好'
})

const dateLine = computed(() => {
  const d = new Date()
  const wd = '日一二三四五六'[d.getDay()]
  return `${d.getMonth() + 1}月${d.getDate()}日 · 星期${wd}`
})

async function loadPeeks() {
  const roots = ui.recentProjects.map((p) => p.root)
  if (!roots.length) return
  try {
    const entries = await api.peekProjects(roots)
    const map: Record<string, ProjectPeekDto | null> = {}
    for (const en of entries) map[en.root] = en.peek
    peeks.value = map
  } catch {
    // 概览拉不到不弹错误打扰，卡片回退骨架/灰化态
  }
}

onMounted(async () => {
  loadPeeks()
  // 整窗拖放打开作品（Tauri 原生事件，仅文件/文件夹拖放触发）：
  // 换机器 / 重装后没有最近列表，把作品文件夹拖进来即可继续写
  unlistenDrop = await getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload
    if (p.type === 'enter') {
      dragOver.value = p.paths.length === 1
    } else if (p.type === 'over') {
      // over 载荷不带 paths；enter 必先于 over，保持 enter 时算好的状态
    } else if (p.type === 'leave') {
      dragOver.value = false
    } else if (p.type === 'drop') {
      dragOver.value = false
      if (store.loading) return
      if (p.paths.length === 1) void openProject(p.paths[0])
      else if (p.paths.length > 1) ElMessage.warning('一次只能打开一个作品文件夹')
    }
  })
})

onUnmounted(() => unlistenDrop?.())

async function openProject(root: string) {
  if (await store.openRoot(root)) router.push('/workbench')
}

/** 选一个已有作品文件夹打开（project.toml 所在目录） */
async function pickAndOpen() {
  const dir = await openDialog({
    directory: true,
    title: '选择 StoryStudio 作品文件夹（含 project.toml）',
  })
  if (typeof dir === 'string') openProject(dir)
}

async function openBookPanel(root: string) {
  if (await store.openRoot(root)) {
    ui.activePanel = 'book'
    router.push('/workbench')
  }
}
</script>

<template>
  <div class="ink-bg-container home-view">
    <div class="paper-texture" />

    <!-- 空态：第一次进来 -->
    <div v-if="!projects.length" class="home-content home-welcome">
      <div class="welcome-hero">
        <div class="hero-seal seal-font">书</div>
        <h1 class="hero-title">StoryStudio</h1>
        <p class="hero-sub">设定为骨 · 伏笔为脉 · 一气呵成</p>
      </div>
      <div class="welcome-actions">
        <button class="ink-btn ink-btn--primary ink-btn--lg" @click="showNew = true">
          <i class="fa-solid fa-feather-pointed" />
          新建作品
        </button>
        <button class="ink-btn ink-btn--lg" @click="pickAndOpen">
          <i class="fa-regular fa-folder-open" />
          打开已有作品
        </button>
        <button class="ink-btn ink-btn--lg" @click="router.push('/create')">
          <i class="fa-solid fa-wand-magic-sparkles" />
          AI 开书
        </button>
      </div>
      <p class="welcome-hint">把作品文件夹直接拖进窗口，也能打开</p>
      <p class="home-foot text-ink-muted">作品以纯文本形式保存在你选择的文件夹里，随时可备份、可迁移</p>
    </div>

    <!-- 仪表盘 -->
    <div v-else class="home-content">
      <div class="home-head">
        <div>
          <h2 class="home-greeting">{{ greeting }}</h2>
          <span class="home-date num">{{ dateLine }}</span>
        </div>
        <div class="head-actions">
          <button class="ink-btn" @click="showNew = true">
            <i class="fa-solid fa-plus" />
            新建作品
          </button>
          <button class="ink-btn" @click="router.push('/create')">
            <i class="fa-solid fa-wand-magic-sparkles" />
            AI 建书
          </button>
        </div>
      </div>

      <ContinueCard
        v-if="primary"
        :recent="primary"
        :peek="primaryPeek"
        :last-scene="ui.lastScene[primary.root] ?? null"
        @continue="openProject(primary.root)"
        @book="openBookPanel(primary.root)"
        @remove="ui.forgetProject(primary.root)"
      />

      <!-- 今日统计：一行大数字 -->
      <div class="today-row">
        <div class="today-stat">
          <span class="ts-value num">{{ formatWordCount(todaySum.manual + todaySum.ai) }}</span>
          <span class="ts-label">今日字数</span>
          <span v-if="todaySum.ai > 0" class="ts-detail num">
            手写 {{ formatWordCount(todaySum.manual) }} · AI {{ formatWordCount(todaySum.ai) }}
          </span>
        </div>
        <div v-if="todayGoal > 0" class="today-stat">
          <span class="ts-value num" :class="{ 'ts-done': todayPct >= 100 }">{{ todayPct }}%</span>
          <span class="ts-label">每日目标</span>
          <span class="ts-detail num">{{ formatWordCount(todayGoal) }} 字</span>
        </div>
        <div class="today-stat">
          <span class="ts-value num" :class="{ 'ts-hot': streakDays > 0 }">{{ streakDays }}</span>
          <span class="ts-label">连续天数</span>
        </div>
      </div>

      <!-- 最近作品：行式列表，全量在「作品」页 -->
      <div class="recent-head">
        <span class="recent-title">最近作品</span>
        <button class="recent-more" @click="router.push('/works')">
          全部作品
          <i class="fa-solid fa-angle-right" />
        </button>
      </div>
      <div class="recent-list">
        <div
          v-for="p in recentList"
          :key="p.root"
          class="recent-row"
          :class="{ 'is-missing': peeks[p.root] === null }"
          :title="p.root"
          @click="peeks[p.root] !== null && openProject(p.root)"
        >
          <i class="fa-solid fa-book" />
          <span class="rr-name">{{ p.name }}</span>
          <span v-if="peeks[p.root]" class="rr-words num">
            {{ formatWordCount(peeks[p.root]!.wordCount) }} 字 · {{ peeks[p.root]!.chapterCount }} 章
          </span>
          <span v-else-if="peeks[p.root] === null" class="rr-miss">目录已不存在</span>
          <span v-else class="rr-words sk-inline" />
          <span class="rr-time">{{ formatRelativeTime(p.openedAt) }}</span>
        </div>
      </div>

      <p class="home-foot text-ink-muted">
        作品以纯文本形式保存在你选择的文件夹里，随时可备份、可迁移
      </p>
    </div>

    <NewProjectDialog v-model="showNew" @created="openProject" />

    <!-- 拖放落点提示：盖住全窗口，不拦截事件（落点仍由 webview 原生分发） -->
    <Transition name="drop-fade">
      <div v-if="dragOver" class="drop-overlay">
        <div class="drop-card">
          <i class="fa-regular fa-folder-open" />
          <span>松开 · 打开这个作品</span>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped lang="scss">
.home-view {
  flex: 1;
  overflow-y: auto;
  position: relative;
}

.home-content {
  position: relative;
  z-index: 1;
  max-width: 760px;
  margin: 0 auto;
  padding: 40px 36px 28px;
  animation: rise 0.5s cubic-bezier(0.22, 1, 0.36, 1) both;
}

// ── 问候 ─────────────────────────────────────────────────
.home-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 22px;
}

.head-actions {
  display: flex;
  gap: 10px;
  flex-shrink: 0;
  padding-bottom: 2px;
}

.home-greeting {
  font-size: 20px;
  font-weight: 600;
  letter-spacing: 0.08em;
  color: var(--ink-main);
}

.home-date {
  display: block;
  margin-top: 4px;
  font-size: 12.5px;
  color: var(--ink-muted);
  letter-spacing: 0.06em;
}

// ── 今日统计 ─────────────────────────────────────────────
.today-row {
  display: flex;
  gap: 40px;
  margin-top: 26px;
  padding: 4px 6px;
  animation: rise 0.5s 0.08s cubic-bezier(0.22, 1, 0.36, 1) both;
}

.today-stat {
  display: flex;
  align-items: baseline;
  gap: 9px;
}

.ts-value {
  font-size: 26px;
  font-weight: 700;
  color: var(--ink-main);
  font-variant-numeric: tabular-nums;

  &.ts-done {
    color: var(--ink-positive);
  }

  &.ts-hot {
    color: var(--ink-accent);
  }
}

.ts-label {
  font-size: 12px;
  color: var(--ink-muted);
  letter-spacing: 0.08em;
}

.ts-detail {
  font-size: 11.5px;
  color: var(--ink-disabled);
}

// ── 最近作品 ─────────────────────────────────────────────
.recent-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin: 34px 0 6px;
}

.recent-title {
  font-size: 12px;
  color: var(--ink-muted);
  letter-spacing: 0.16em;
}

.recent-more {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  border: none;
  background: transparent;
  color: var(--ink-sec);
  font-size: 12.5px;
  font-family: inherit;
  cursor: pointer;
  padding: 4px 6px;
  border-radius: 6px;
  transition: all 0.18s;

  i {
    font-size: 11px;
  }

  &:hover {
    color: var(--ink-accent);
    background: var(--surface-2);
  }
}

.recent-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.recent-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 11px 12px;
  border-radius: 10px;
  cursor: pointer;
  transition: background 0.18s;
  animation: rise 0.5s 0.12s cubic-bezier(0.22, 1, 0.36, 1) both;

  > i {
    color: var(--ink-accent);
    font-size: 14px;
    flex-shrink: 0;
  }

  &:hover {
    background: var(--surface-2);
  }

  &:nth-child(2) {
    animation-delay: 0.16s;
  }

  &:nth-child(3) {
    animation-delay: 0.2s;
  }

  &:nth-child(4) {
    animation-delay: 0.24s;
  }
}

.rr-name {
  flex-shrink: 0;
  max-width: 40%;
  font-size: 13.5px;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rr-words {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.rr-miss {
  flex: 1;
  font-size: 12px;
  color: var(--state-warning);
}

.rr-time {
  flex-shrink: 0;
  font-size: 11.5px;
  color: var(--ink-disabled);
}

.recent-row.is-missing {
  cursor: default;

  .rr-name {
    color: var(--ink-muted);
  }
}

.sk-inline {
  display: inline-block;
  height: 11px;
  border-radius: 4px;
  background: linear-gradient(
    90deg,
    var(--surface-2) 25%,
    color-mix(in srgb, var(--surface-2) 40%, var(--bg-main)) 50%,
    var(--surface-2) 75%
  );
  background-size: 200% 100%;
  animation: shimmer 1.4s ease infinite;
}

// ── 空态欢迎 ─────────────────────────────────────────────
.home-welcome {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 72vh;
}

.welcome-hero {
  text-align: center;
  margin-bottom: 36px;

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

.welcome-actions {
  display: flex;
  gap: 14px;
}

.welcome-hint {
  margin-top: 20px;
  font-size: 11.5px;
  letter-spacing: 0.1em;
  color: var(--ink-disabled);
}

.home-foot {
  margin-top: 40px;
  text-align: center;
  font-size: 12px;
  letter-spacing: 0.1em;
}

// ── 拖放打开作品 ─────────────────────────────────────────
.drop-overlay {
  position: fixed;
  inset: 0;
  z-index: 3000; // 盖过 ElDialog，拖放期间模态框也在下面
  display: flex;
  align-items: center;
  justify-content: center;
  background: color-mix(in srgb, var(--bg-main) 72%, transparent);
  backdrop-filter: blur(3px);
  pointer-events: none;
}

.drop-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 36px 52px;
  border: 1.5px dashed var(--ink-accent);
  border-radius: 16px;
  background: var(--surface-1);
  box-shadow: var(--ui-shadow-hover);
  animation: drop-pop 0.2s cubic-bezier(0.22, 1, 0.36, 1) both;

  i {
    font-size: 28px;
    color: var(--ink-accent);
  }

  span {
    font-size: 14px;
    letter-spacing: 0.18em;
    color: var(--ink-main);
  }
}

.drop-fade-enter-active,
.drop-fade-leave-active {
  transition: opacity 0.16s ease;
}

.drop-fade-enter-from,
.drop-fade-leave-to {
  opacity: 0;
}

@keyframes drop-pop {
  from {
    transform: scale(0.96);
    opacity: 0;
  }
  to {
    transform: none;
    opacity: 1;
  }
}

@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(16px);
  }
  to {
    opacity: 1;
    transform: none;
  }
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
