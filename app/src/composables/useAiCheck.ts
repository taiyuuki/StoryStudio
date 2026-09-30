// AI 味检测 — 手动触发，报告预览，可送妙笔逐条改进
//
// 流程：冲刷未落盘编辑 → 调 ai_check → 打开报告对话框（给人看：严重度 /
// 特征 / 片段定位 / 原文引用，可勾选）。用户自己手改，或点「送妙笔改进」
// 把勾选项格式化为改进指令填入妙笔输入框。零命中只提示，不打扰。
import { ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { api, type AiCheckItemDto, type AiCheckResultDto } from '@/api/backend'
import { useMiaoStore } from '@/stores/miao'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'

/** 模块级单例：全应用同一时刻只跑一次检测，报告状态全局共享（单一挂载点渲染） */
const checking = ref(false)

/** 最近一次报告（null = 无报告；关闭对话框不清数据，可重开，新检测覆盖） */
export interface AiCheckReportView {
  label: string
  result: AiCheckResultDto
}
const report = ref<AiCheckReportView | null>(null)
const reportOpen = ref(false)

/** 换项目清掉旧报告（root watcher 只装一次，多组件调用不重复注册） */
let rootWatcherInstalled = false

const SEVERITY_LABEL: Record<string, string> = {
  Strong: '强',
  Medium: '中',
  Weak: '弱',
}

/** 勾选的命中项 → 妙笔输入框的改进指令文本 */
function buildImprovePrompt(label: string, summary: string, items: AiCheckItemDto[]): string {
  const lines = items.map((it, i) => {
    const where = it.fragmentId ? `（片段 ${it.fragmentId}）` : ''
    const sev = SEVERITY_LABEL[it.severity] ?? it.severity
    return `${i + 1}. [${sev}·${it.featureName}]${where}：${it.explanation}\n    原文：「${it.quote}」`
  })
  return [
    `我对「${label}」跑了一次 AI 味检测，共 ${items.length} 处命中。总体：${summary}`,
    '',
    '请逐条改进：直接用工具修改对应片段正文，保持情节、事实与人称不变，只消除 AI 味；个别条目若你判断是误伤，说明理由后跳过。',
    '',
    ...lines,
  ].join('\n')
}

/**
 * 等待当前场景的防抖保存落盘（800ms 防抖 + 在途请求），检测读的是磁盘，
 * 不等会把刚写的字漏掉。最多等 3.6s，超时照常检测。
 */
async function waitForSceneSaves(): Promise<void> {
  const store = useWorkspaceStore()
  for (let i = 0; i < 12; i++) {
    const states = Object.values(store.fragmentSave)
    if (!states.some((s) => s === 'dirty' || s === 'saving')) return
    await new Promise((r) => setTimeout(r, 300))
  }
}

export function useAiCheck() {
  const workspace = useWorkspaceStore()
  const miao = useMiaoStore()
  const ui = useUiStore()

  if (!rootWatcherInstalled) {
    rootWatcherInstalled = true
    watch(
      () => workspace.root,
      () => {
        report.value = null
        reportOpen.value = false
      },
    )
  }

  async function run(scope: 'fragment' | 'scene' | 'chapter', id: string, label: string) {
    if (checking.value || !workspace.root) return
    checking.value = true
    try {
      await waitForSceneSaves()
      const result = await api.aiCheck(workspace.root, scope, id)
      if (!result.items.length) {
        ElMessage.success(`「${label}」未检出 AI 味`)
        return
      }
      report.value = { label, result }
      reportOpen.value = true
    } catch (err) {
      ElMessage.error(`AI 检测失败：${(err as Error).message}`)
    } finally {
      checking.value = false
    }
  }

  /** 关闭只收起对话框，报告数据保留（工具栏可重开） */
  function closeReport() {
    reportOpen.value = false
  }

  function openReport() {
    if (report.value) reportOpen.value = true
  }

  /** 彻底丢弃报告 */
  function dismissReport() {
    report.value = null
    reportOpen.value = false
  }

  /** 报告里勾选的条目 → 填入妙笔输入框并切到妙笔面板（数据保留可回看） */
  function sendToMiao(view: AiCheckReportView, items: AiCheckItemDto[]) {
    if (!items.length) return
    miao.input = buildImprovePrompt(view.label, view.result.summary, items)
    ui.activePanel = 'miao'
    reportOpen.value = false
  }

  return { checking, run, report, reportOpen, closeReport, openReport, dismissReport, sendToMiao }
}
