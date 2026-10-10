// UI 偏好 — 主题 / 最近项目 / 右栏面板（localStorage 持久化）
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'

export type ThemeName = 'theme-ink' | 'theme-dark'
export type DockPanel = 'miao' | 'stats' | 'book' | 'chapters' | 'entities' | 'anchoring' | 'worldview' | 'threads' | 'diagnostics' | 'prompts' | 'export' | 'git' | null

export interface RecentProject {
  root: string
  name: string
  openedAt: number
}

/** 每个项目的上次编辑位置（首页「继续写作」直达） */
export interface LastScene {
  sceneId: string
  chapterTitle: string
  sceneTitle: string
  ts: number
}

/** 主题直接落 localStorage，避免依赖插件的恢复时机 */
const THEME_KEY = 'ss-theme'

// 右栏宽度边界：拖拽与持久化恢复共用（越界值一律拉回范围内）
export const DOCK_WIDTH_MIN = 300
export const DOCK_WIDTH_MAX = 600
export const DOCK_WIDTH_DEFAULT = 360

function clampDockWidth(w: number): number {
  return Math.min(DOCK_WIDTH_MAX, Math.max(DOCK_WIDTH_MIN, w))
}

export const useUiStore = defineStore(
  'ui',
  () => {
    const theme = ref<ThemeName>(
      localStorage.getItem(THEME_KEY) === 'theme-dark' ? 'theme-dark' : 'theme-ink',
    )
    const recentProjects = ref<RecentProject[]>([])
    /** root → 上次编辑的场景（随最近项目一起清理与持久化） */
    const lastScene = ref<Record<string, LastScene>>({})
    const activePanel = ref<DockPanel>(null)
    const dockWidth = ref(DOCK_WIDTH_DEFAULT)
    /** 编辑器 AI 幽灵补全（Tab 采纳 / Esc 关闭） */
    const aiGhost = ref(true)
    /** 全文检索对话框（StructureTree 入口 + Cmd/Ctrl+F） */
    const searchOpen = ref(false)
    

    function applyTheme() {
      const el = document.documentElement
      el.classList.remove('theme-ink', 'theme-dark')
      el.classList.add(theme.value)
      localStorage.setItem(THEME_KEY, theme.value)
    }

    function toggleTheme() {
      theme.value = theme.value === 'theme-ink' ? 'theme-dark' : 'theme-ink'
    }

    function rememberProject(root: string, name: string) {
      const list = recentProjects.value.filter((p) => p.root !== root)
      list.unshift({ root, name, openedAt: Date.now() })
      recentProjects.value = list.slice(0, 10)
      // 位置记忆随最近列表一起收敛，避免无限增长
      const alive = new Set(recentProjects.value.map((p) => p.root))
      const kept: Record<string, LastScene> = {}
      for (const [k, v] of Object.entries(lastScene.value)) {
        if (alive.has(k)) kept[k] = v
      }
      lastScene.value = kept
    }

    function forgetProject(root: string) {
      recentProjects.value = recentProjects.value.filter((p) => p.root !== root)
      if (lastScene.value[root]) {
        const next = { ...lastScene.value }
        delete next[root]
        lastScene.value = next
      }
    }

    function rememberScene(root: string, scene: LastScene) {
      lastScene.value = { ...lastScene.value, [root]: scene }
    }

    function togglePanel(panel: Exclude<DockPanel, null>) {
      activePanel.value = activePanel.value === panel ? null : panel
    }

    watch(theme, applyTheme)
    applyTheme()

    // 持久化恢复或其他写入方给出越界宽度时拉回范围内
    watch(dockWidth, (w) => {
      const clamped = clampDockWidth(w)
      if (clamped !== w) dockWidth.value = clamped
    })

    return {
      theme,
      recentProjects,
      lastScene,
      activePanel,
      dockWidth,
      aiGhost,
      searchOpen,
      toggleTheme,
      rememberProject,
      forgetProject,
      rememberScene,
      togglePanel,
    }
  },
  {
    persist: {
      storage: localStorage,
      pick: ['recentProjects', 'lastScene', 'activePanel', 'dockWidth', 'aiGhost'],
    },
  },
)
