<script setup lang="ts">
import { nextTick, onUnmounted, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import type { ChapterReadDto } from '@/api/backend'
import { api } from '@/api/backend'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import MarkdownView from '@/components/common/MarkdownView.vue'

const ui = useUiStore()
const store = useWorkspaceStore()

const chapter = ref<ChapterReadDto | null>(null)
const loading = ref(false)

watch(
  () => ui.chapterRead,
  async (chapterId) => {
    chapter.value = null
    if (!chapterId || !store.root) return
    loading.value = true
    try {
      chapter.value = await api.readChapter(store.root, chapterId)
      // 从正在编辑的场景接着读：定位到该场景（阅读是快照，正文改动以编辑视图为准）
      await nextTick()
      if (store.scene) {
        document.getElementById(`read-scene-${store.scene.id}`)?.scrollIntoView()
      }
    } catch (err) {
      ElMessage.error((err as Error).message)
      ui.chapterRead = null
    } finally {
      loading.value = false
    }
  },
  { immediate: true },
)

function close() {
  ui.chapterRead = null
}

/** 跳到某场景编辑（点场景标题） */
async function editScene(sceneId: string) {
  ui.chapterRead = null
  if (store.scene?.id !== sceneId) await store.selectScene(sceneId)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && ui.chapterRead) close()
}
window.addEventListener('keydown', onKeydown)
onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <div v-if="ui.chapterRead" class="ink-bg-container read-overlay">
    <div class="paper-texture" />

    <header class="read-topbar">
      <button class="ink-btn ink-btn--sm" title="关闭阅读（Esc）" @click="close">
        <i class="fa-solid fa-arrow-left" />
        回到编辑
      </button>
      <div v-if="chapter" class="read-topmeta">
        <span class="seal-font">{{ chapter.title }}</span>
        <span class="num">{{ chapter.scenes.length }} 场景 · {{ chapter.wordCount.toLocaleString() }} 字</span>
      </div>
      <!-- 占位平衡，让顶栏信息视觉居中 -->
      <span class="read-top-spacer" />
    </header>

    <div class="read-scroll">
      <div v-if="loading" class="read-loading">
        <i class="fa-solid fa-circle-notch fa-spin" />
      </div>

      <div v-else-if="chapter" class="read-paper">
        <h1 class="read-chapter-title">{{ chapter.title }}</h1>

        <section
          v-for="(s, i) in chapter.scenes"
          :id="`read-scene-${s.sceneId}`"
          :key="s.sceneId"
          class="read-scene"
          :class="{ 'is-current': s.sceneId === store.scene?.id }"
        >
          <button
            class="read-scene-title"
            title="点击回到该场景编辑"
            @click="editScene(s.sceneId)"
          >
            <span class="read-scene-no seal-font">第{{ ['一', '二', '三', '四', '五', '六', '七', '八', '九', '十'][i] ?? i + 1 }}场</span>
            <span class="read-scene-name">{{ s.title?.trim() || '未命名场景' }}</span>
            <i v-if="s.sceneId === store.scene?.id" class="fa-solid fa-location-dot" title="正在编辑" />
          </button>
          <MarkdownView v-if="s.text" :text="s.text" class="read-body" />
          <p v-else class="read-scene-blank">（这一场还是白纸）</p>
        </section>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.read-overlay {
  position: fixed;
  // 让出应用顶栏（AppTitleBar，含 mac 红绿灯 / win 窗口控件），
  // 顶栏保持可见可交互，「回到编辑」不与窗口按钮重叠
  top: var(--titlebar-h);
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 120;
  display: flex;
  flex-direction: column;
}

.read-topbar {
  position: relative;
  z-index: 1;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 18px;
  border-bottom: 1px solid var(--divider);
  backdrop-filter: blur(10px);
  background: color-mix(in srgb, var(--bg-main) 72%, transparent);
}

.read-topmeta {
  display: flex;
  align-items: baseline;
  gap: 12px;
  color: var(--ink-muted);
  font-size: 12px;
  min-width: 0;

  > span:first-child {
    color: var(--ink-accent);
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
}

.read-top-spacer {
  // 与左侧按钮同宽，顶栏信息居中
  min-width: 96px;
}

.read-scroll {
  position: relative;
  z-index: 1;
  flex: 1;
  overflow-y: auto;
  min-height: 0;
}

.read-loading {
  display: flex;
  justify-content: center;
  padding: 80px 0;
  color: var(--ink-muted);
  font-size: 20px;
}

.read-paper {
  max-width: 768px;
  margin: 0 auto;
  padding: 44px 48px 90px;
}

.read-chapter-title {
  margin: 0 0 40px;
  text-align: center;
  font-family: 'Songti SC', 'STSong', 'SimSun', serif;
  font-size: 26px;
  font-weight: 600;
  letter-spacing: 0.25em;
  color: var(--ink-main);
}

.read-scene {
  margin-bottom: 44px;

  // scrollIntoView 定位时给顶栏留余量
  scroll-margin-top: 24px;

  &.is-current .read-scene-name {
    color: var(--ink-accent);
  }
}

.read-scene-title {
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: 10px;
  width: 100%;
  margin-bottom: 18px;
  padding: 4px 8px;
  border: none;
  border-radius: 7px;
  background: transparent;
  font-family: inherit;
  cursor: pointer;
  transition: background 0.2s;

  &:hover {
    background: var(--ink-accent-soft);

    .read-scene-name {
      color: var(--ink-accent);
    }
  }
}

.read-scene-no {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--ink-disabled);
}

.read-scene-name {
  font-size: 15px;
  font-weight: 600;
  color: var(--ink-sec);
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: color 0.2s;

  i {
    font-size: 10px;
    color: var(--ink-accent);
  }
}

.read-scene-blank {
  text-align: center;
  color: var(--ink-disabled);
  font-size: 12.5px;
}

// 阅读排版：覆盖 MarkdownView 的聊天向密度，换成书页向
.read-body :deep(.md-body) {
  p {
    margin: 0 0 1.05em;
    font-size: 16.5px;
    line-height: 2.15;
    color: var(--ink-main);
    text-align: justify;
  }

  blockquote p {
    font-size: 15px;
    color: var(--ink-sec);
  }
}
</style>
