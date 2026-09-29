// UI 偏好 — 主题 / 最近项目 / 右栏面板（localStorage 持久化）
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'

export type ThemeName = 'theme-ink' | 'theme-dark'
export type DockPanel = 'miao' | 'stats' | 'book' | 'entities' | 'threads' | 'diagnostics' | 'export' | null

export interface RecentProject {
  root: string
  name: string
  openedAt: number
}

/** 主题直接落 localStorage，避免依赖插件的恢复时机 */
const THEME_KEY = 'ss-theme'

export const useUiStore = defineStore(
  'ui',
  () => {
    const theme = ref<ThemeName>(
      localStorage.getItem(THEME_KEY) === 'theme-dark' ? 'theme-dark' : 'theme-ink',
    )
    const recentProjects = ref<RecentProject[]>([])
    const activePanel = ref<DockPanel>(null)
    const dockWidth = ref(360)
    

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
    }

    function forgetProject(root: string) {
      recentProjects.value = recentProjects.value.filter((p) => p.root !== root)
    }

    function togglePanel(panel: Exclude<DockPanel, null>) {
      activePanel.value = activePanel.value === panel ? null : panel
    }

    watch(theme, applyTheme)
    applyTheme()

    return {
      theme,
      recentProjects,
      activePanel,
      dockWidth,
      toggleTheme,
      rememberProject,
      forgetProject,
      togglePanel,
    }
  },
  {
    persist: {
      storage: localStorage,
      pick: ['recentProjects', 'activePanel', 'dockWidth'],
    },
  },
)
