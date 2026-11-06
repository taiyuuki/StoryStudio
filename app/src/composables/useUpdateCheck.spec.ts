// 检测更新 — 弹窗时机与跳过逻辑单测
// ui store 在模块加载期访问 localStorage（主题持久化），node 环境先补内存版再动态导入
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const mockCheck = vi.hoisted(() => vi.fn())
const mockRelaunch = vi.hoisted(() => vi.fn())
const mockConfirm = vi.hoisted(() => vi.fn())
const mockSuccess = vi.hoisted(() => vi.fn())
const mockError = vi.hoisted(() => vi.fn())

vi.mock('@tauri-apps/plugin-updater', () => ({ check: mockCheck }))
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: mockRelaunch }))
vi.mock('element-plus', () => ({
  ElMessage: { success: mockSuccess, error: mockError },
  ElMessageBox: { confirm: mockConfirm },
}))

let useUpdateCheck: (typeof import('@/composables/useUpdateCheck'))['useUpdateCheck']
let useUiStore: (typeof import('@/stores/ui'))['useUiStore']

beforeAll(async () => {
  const mem = new Map<string, string>()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => void mem.clear(),
  })
  // ui store 创建即 applyTheme()，node 环境补最小 document 桩
  vi.stubGlobal('document', {
    documentElement: { classList: { remove: () => {}, add: () => {} } },
  })
  setActivePinia(createPinia())
  ;({ useUpdateCheck } = await import('@/composables/useUpdateCheck'))
  ;({ useUiStore } = await import('@/stores/ui'))
})

/** 造一个假 Update：version 可指定，downloadAndInstall 依回调模拟进度事件 */
function fakeUpdate(version: string) {
  return {
    version,
    body: '更新说明',
    date: new Date(),
    downloadAndInstall: vi.fn(async (onEvent: (e: unknown) => void) => {
      onEvent({ event: 'Started', data: { contentLength: 100 } })
      onEvent({ event: 'Progress', data: { chunkLength: 40 } })
    }),
  }
}

beforeEach(() => {
  mockCheck.mockReset()
  mockRelaunch.mockReset()
  mockConfirm.mockReset()
  mockSuccess.mockReset()
  mockError.mockReset()
  const uc = useUpdateCheck()
  uc.dialogOpen.value = false
  uc.downloading.value = false
  uc.progress.value = null
  useUiStore().setSkippedVersion(null)
})

describe('checkForUpdate', () => {
  it('自动检查无更新：不弹窗不提示', async () => {
    mockCheck.mockResolvedValue(null)
    await useUpdateCheck().checkForUpdate(false)
    expect(useUpdateCheck().dialogOpen.value).toBe(false)
    expect(mockSuccess).not.toHaveBeenCalled()
  })

  it('自动检查有更新且未跳过：弹窗', async () => {
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await useUpdateCheck().checkForUpdate(false)
    expect(useUpdateCheck().dialogOpen.value).toBe(true)
  })

  it('自动检查有更新但已跳过：不弹窗', async () => {
    useUiStore().setSkippedVersion('0.2.0')
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await useUpdateCheck().checkForUpdate(false)
    expect(useUpdateCheck().dialogOpen.value).toBe(false)
  })

  it('手动检查无更新：提示已是最新', async () => {
    mockCheck.mockResolvedValue(null)
    await useUpdateCheck().checkForUpdate(true)
    expect(mockSuccess).toHaveBeenCalledWith('已是最新版本')
    expect(useUpdateCheck().dialogOpen.value).toBe(false)
  })

  it('手动检查有更新：即便已跳过也弹窗', async () => {
    useUiStore().setSkippedVersion('0.2.0')
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await useUpdateCheck().checkForUpdate(true)
    expect(useUpdateCheck().dialogOpen.value).toBe(true)
  })

  it('检查失败：自动静默，手动报错', async () => {
    mockCheck.mockRejectedValue(new Error('网络超时'))
    await useUpdateCheck().checkForUpdate(false)
    expect(mockError).not.toHaveBeenCalled()
    await useUpdateCheck().checkForUpdate(true)
    expect(mockError).toHaveBeenCalledWith('检查更新失败：网络超时')
  })
})

describe('runUpdate', () => {
  it('累计下载进度，确认后重启', async () => {
    const uc = useUpdateCheck()
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await uc.checkForUpdate(true)
    mockConfirm.mockResolvedValue(undefined)

    await uc.runUpdate()

    // Progress 事件累计：Started total=100 + Progress chunk=40
    expect(uc.progress.value).toBeNull() // 完成后清空
    expect(mockRelaunch).toHaveBeenCalledTimes(1)
    expect(uc.downloading.value).toBe(false)
  })

  it('用户选择稍后重启：不调 relaunch', async () => {
    const uc = useUpdateCheck()
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await uc.checkForUpdate(true)
    mockConfirm.mockRejectedValue(new Error('cancel'))

    await uc.runUpdate()

    expect(mockRelaunch).not.toHaveBeenCalled()
  })
})

describe('skipVersion', () => {
  it('记入跳过版本并关弹窗，自动检查不再提醒', async () => {
    const uc = useUpdateCheck()
    mockCheck.mockResolvedValue(fakeUpdate('0.2.0'))
    await uc.checkForUpdate(false)
    uc.skipVersion()
    expect(uc.dialogOpen.value).toBe(false)
    expect(useUiStore().skippedVersion).toBe('0.2.0')

    uc.dialogOpen.value = false
    await uc.checkForUpdate(false)
    expect(uc.dialogOpen.value).toBe(false)
  })
})
