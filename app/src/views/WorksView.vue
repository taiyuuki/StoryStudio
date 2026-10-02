<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { useWorkspaceStore } from '@/stores/workspace'
import { useUiStore } from '@/stores/ui'
import { api, type ProjectPeekDto } from '@/api/backend'
import ShelfCard from '@/components/start/ShelfCard.vue'
import NewProjectDialog from '@/components/start/NewProjectDialog.vue'

const router = useRouter()
const store = useWorkspaceStore()
const ui = useUiStore()

const peeks = ref<Record<string, ProjectPeekDto | null>>({})
const showNew = ref(false)

const projects = computed(() => ui.recentProjects)

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

onMounted(loadPeeks)

async function openProject(root: string) {
  if (await store.openRoot(root)) router.push('/workbench')
}

async function pickAndOpen() {
  const dir = await openDialog({
    directory: true,
    title: '选择 StoryStudio 项目目录（含 project.toml）',
  })
  if (typeof dir === 'string') openProject(dir)
}
</script>

<template>
  <div class="ink-bg-container works-view">
    <div class="paper-texture" />

    <div class="works-content">
      <!-- 空态 -->
      <div v-if="!projects.length" class="works-empty">
        <div class="we-icon"><i class="fa-regular fa-folder-open" /></div>
        <h2 class="we-title">还没有作品</h2>
        <p class="we-sub text-ink-muted">从一本新书开始，或者让 AI 从一句话灵感帮你开书</p>
        <div class="we-actions">
          <button class="ink-btn ink-btn--primary ink-btn--lg" @click="showNew = true">
            <i class="fa-solid fa-feather-pointed" />
            新建作品
          </button>
          <button class="ink-btn ink-btn--lg" @click="router.push('/create')">
            <i class="fa-solid fa-wand-magic-sparkles" />
            AI 开书
          </button>
        </div>
      </div>

      <!-- 项目库 -->
      <template v-else>
        <div class="works-head">
          <div>
            <h2 class="works-title">作品</h2>
            <span class="works-sub text-ink-muted">共 {{ projects.length }} 部 · 点击卡片继续写作</span>
          </div>
          <div class="works-actions">
            <button class="ink-btn" @click="pickAndOpen">
              <i class="fa-regular fa-folder-open" />
              打开项目目录
            </button>
            <button class="ink-btn ink-btn--primary" @click="showNew = true">
              <i class="fa-solid fa-plus" />
              新建作品
            </button>
          </div>
        </div>

        <div class="works-grid">
          <ShelfCard
            v-for="p in projects"
            :key="p.root"
            :recent="p"
            :peek="peeks[p.root]"
            @open="openProject"
            @remove="ui.forgetProject"
          />
        </div>

        <p class="works-foot text-ink-muted">
          作品以纯文本形式保存在你选择的文件夹里，随时可备份、可迁移
        </p>
      </template>
    </div>

    <NewProjectDialog v-model="showNew" @created="openProject" />
  </div>
</template>

<style scoped lang="scss">
.works-view {
  flex: 1;
  overflow-y: auto;
  position: relative;
}

.works-content {
  position: relative;
  z-index: 1;
  max-width: 980px;
  margin: 0 auto;
  padding: 40px 36px 28px;
  animation: rise 0.5s cubic-bezier(0.22, 1, 0.36, 1) both;
}

// ── 页头 ─────────────────────────────────────────────────
.works-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 18px;
  margin-bottom: 24px;
  flex-wrap: wrap;
}

.works-title {
  font-size: 20px;
  font-weight: 600;
  letter-spacing: 0.1em;
  color: var(--ink-main);
}

.works-sub {
  display: block;
  margin-top: 4px;
  font-size: 12.5px;
  letter-spacing: 0.04em;
}

.works-actions {
  display: flex;
  gap: 10px;
}

// ── 网格 ─────────────────────────────────────────────────
.works-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(212px, 1fr));
  gap: 16px;
}

.works-foot {
  margin-top: 36px;
  text-align: center;
  font-size: 12px;
  letter-spacing: 0.1em;
}

// ── 空态 ─────────────────────────────────────────────────
.works-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 68vh;
  text-align: center;
}

.we-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 72px;
  height: 72px;
  border-radius: 18px;
  background: var(--surface-2);
  color: var(--ink-muted);
  font-size: 28px;
  margin-bottom: 20px;
}

.we-title {
  font-size: 18px;
  font-weight: 600;
  letter-spacing: 0.1em;
  color: var(--ink-main);
}

.we-sub {
  margin: 8px 0 26px;
  font-size: 13px;
}

.we-actions {
  display: flex;
  gap: 14px;
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
</style>
