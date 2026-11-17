// 删除撤销 — 全局单槽撤销条（WorkbenchView 单挂 UndoToast 渲染）
//
// 各删除入口在删除成功后调 offerUndo(label, restore)：底部浮条倒计时
// 8 秒，期间点「撤销」执行 restore 重建数据，超时静默放弃。单槽设计：
// 连续删除只保留最新一条可撤销。restore 闭包必须捕获 DTO 纯数据副本与
// 删除时的 root 路径，不得引用已删的响应式行或 store 的当前 root——
// 面板切换销毁、甚至 8 秒内切换项目都不影响撤销语义。
import { ref } from 'vue'
import { ElMessage } from 'element-plus'

export interface UndoEntry {
  /** 递增序号，组件用它做 v-for/动画的 key（同类删除连发也要重新计时） */
  id: number
  label: string
  restore: () => Promise<void>
}

/** 撤销条存活时长（毫秒） */
export const UNDO_TTL_MS = 8000

const current = ref<UndoEntry | null>(null)
const restoring = ref(false)
let timer: ReturnType<typeof setTimeout> | null = null
let seq = 0

function clearTimer() {
  if (timer !== null) {
    clearTimeout(timer)
    timer = null
  }
}

/** 提供一次撤销机会；新条目覆盖旧条目并重置倒计时 */
function offerUndo(label: string, restore: () => Promise<void>) {
  clearTimer()
  current.value = { id: ++seq, label, restore }
  timer = setTimeout(() => {
    current.value = null
    timer = null
  }, UNDO_TTL_MS)
}

/** 关闭浮条（放弃本次撤销） */
function dismissUndo() {
  clearTimer()
  current.value = null
}

/** 执行撤销；执行期间禁点防连击，无论成败都收起浮条（失败如实报错） */
async function runUndo() {
  const entry = current.value
  if (!entry || restoring.value) return
  restoring.value = true
  try {
    await entry.restore()
    dismissUndo()
    ElMessage.success('已撤销删除')
  } catch (err) {
    dismissUndo()
    ElMessage.error(`撤销失败：${err instanceof Error ? err.message : String(err)}`)
  } finally {
    restoring.value = false
  }
}

export function useUndo() {
  return { current, restoring, offerUndo, dismissUndo, runUndo }
}
