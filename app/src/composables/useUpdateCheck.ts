// 检测更新 — 启动时静默检查（App.vue 挂 PROD 门），发现新版本弹窗；
// 设置页「关于」分栏可手动检查。版本比较由 updater 插件完成
// （latest.json 的 version 对比当前版本），无需自研 semver。
//
// 自动检查失败静默（网络问题不打扰，loadPeeks 同风格）；手动检查提示结果。
// 下载安装完成后询问重启（Windows 上安装器会先退出应用，询问可能不出现）。
import { ref, shallowRef } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { useUiStore } from '@/stores/ui'

/** 模块级单例：全应用共享同一次检查结果，弹窗单一挂载点渲染（App.vue） */
const update = shallowRef<Update | null>(null)
const checking = ref(false)
const dialogOpen = ref(false)
const downloading = ref(false)
/** 下载进度（null = 未在下载；total 未知时为 0） */
const progress = ref<{ received: number; total: number } | null>(null)

/**
 * 检查更新。manual = 设置页手动触发：无更新报「已最新」，失败报错，
 * 有更新即便已跳过也展示；自动检查（manual = false）只对未跳过的新版本弹窗，
 * 其余情况一律静默。
 */
async function checkForUpdate(manual: boolean) {
  if (checking.value) return
  checking.value = true
  try {
    const found = await check({ timeout: 15_000 })
    update.value = found ?? null
    if (found) {
      if (manual || found.version !== useUiStore().skippedVersion) {
        dialogOpen.value = true
      }
    } else if (manual) {
      ElMessage.success('已是最新版本')
    }
  } catch (err) {
    if (manual) {
      ElMessage.error(`检查更新失败：${err instanceof Error ? err.message : String(err)}`)
    }
  } finally {
    checking.value = false
  }
}

/** 下载并安装当前检测到的更新，完成后询问是否立即重启 */
async function runUpdate() {
  const u = update.value
  if (!u || downloading.value) return
  downloading.value = true
  progress.value = { received: 0, total: 0 }
  try {
    await u.downloadAndInstall((e) => {
      if (e.event === 'Started' && e.data.contentLength) {
        progress.value = { received: 0, total: e.data.contentLength }
      } else if (e.event === 'Progress') {
        progress.value = {
          received: (progress.value?.received ?? 0) + e.data.chunkLength,
          total: progress.value?.total ?? 0,
        }
      }
    })
    downloading.value = false
    progress.value = null
    try {
      await ElMessageBox.confirm('更新已下载完成，重启应用以完成安装。', '更新就绪', {
        type: 'success',
        confirmButtonText: '立即重启',
        cancelButtonText: '稍后重启',
      })
      await relaunch()
    } catch {
      // 稍后重启：安装包已就位，下次启动即为新版本
    }
  } catch (err) {
    downloading.value = false
    progress.value = null
    ElMessage.error(`下载更新失败：${err instanceof Error ? err.message : String(err)}`)
  }
}

/** 跳过当前新版本（不再自动弹窗；设置页手动检查仍可见） */
function skipVersion() {
  const u = update.value
  if (!u) return
  useUiStore().setSkippedVersion(u.version)
  dialogOpen.value = false
}

/** 打开更新弹窗（标题栏提醒入口用） */
function openDialog() {
  if (update.value) dialogOpen.value = true
}

export function useUpdateCheck() {
  return {
    update,
    checking,
    dialogOpen,
    downloading,
    progress,
    checkForUpdate,
    runUpdate,
    skipVersion,
    openDialog,
  }
}
