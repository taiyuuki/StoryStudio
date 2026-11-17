// 删除撤销 — 单槽撤销条状态机单测（offerUndo / runUndo / dismissUndo / 倒计时）
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

const mockSuccess = vi.hoisted(() => vi.fn())
const mockError = vi.hoisted(() => vi.fn())

vi.mock('element-plus', () => ({
  ElMessage: { success: mockSuccess, error: mockError },
}))

let useUndo: (typeof import('@/composables/useUndo'))['useUndo']
let UNDO_TTL_MS: number

beforeAll(async () => {
  ;({ useUndo, UNDO_TTL_MS } = await import('@/composables/useUndo'))
})

beforeEach(() => {
  vi.useFakeTimers()
  mockSuccess.mockClear()
  mockError.mockClear()
})

afterEach(() => {
  // 先清条目（同一套假时钟下清残留计时器），再还原真实时钟
  useUndo().dismissUndo()
  vi.useRealTimers()
})

describe('offerUndo 单槽登记', () => {
  it('登记条目并暴露 label', () => {
    const { current, offerUndo } = useUndo()
    offerUndo('线索「X」', async () => {})
    expect(current.value?.label).toBe('线索「X」')
  })

  it('新条目覆盖旧条目并重置倒计时', () => {
    const { current, offerUndo } = useUndo()
    offerUndo('A', async () => {})
    vi.advanceTimersByTime(UNDO_TTL_MS - 100)
    offerUndo('B', async () => {})
    // 距 A 已近超时，但 B 的窗口才刚开始
    vi.advanceTimersByTime(UNDO_TTL_MS - 100)
    expect(current.value?.label).toBe('B')
    vi.advanceTimersByTime(100)
    expect(current.value).toBeNull()
  })

  it('到期自动清除', () => {
    const { current, offerUndo } = useUndo()
    offerUndo('A', async () => {})
    vi.advanceTimersByTime(UNDO_TTL_MS)
    expect(current.value).toBeNull()
  })
})

describe('runUndo 执行撤销', () => {
  it('执行 restore、清槽并提示成功', async () => {
    const restore = vi.fn(async () => {})
    const { current, offerUndo, runUndo } = useUndo()
    offerUndo('A', restore)
    await runUndo()
    expect(restore).toHaveBeenCalledTimes(1)
    expect(current.value).toBeNull()
    expect(mockSuccess).toHaveBeenCalledWith('已撤销删除')
  })

  it('restore 失败：清槽并如实报错', async () => {
    const restore = vi.fn(async () => {
      throw new Error('项目已切换')
    })
    const { current, offerUndo, runUndo } = useUndo()
    offerUndo('A', restore)
    await runUndo()
    expect(current.value).toBeNull()
    expect(mockError).toHaveBeenCalledWith('撤销失败：项目已切换')
  })

  it('restore 执行期间防连点', async () => {
    let release!: () => void
    const gate = new Promise<void>((resolve) => (release = resolve))
    const restore = vi.fn(() => gate)
    const { offerUndo, runUndo } = useUndo()
    offerUndo('A', restore)
    const first = runUndo()
    const second = runUndo() // restoring 期间应直接返回
    release()
    await Promise.all([first, second])
    expect(restore).toHaveBeenCalledTimes(1)
  })

  it('到期后无条目可撤销（restore 不执行）', async () => {
    const restore = vi.fn(async () => {})
    const { offerUndo, runUndo } = useUndo()
    offerUndo('A', restore)
    vi.advanceTimersByTime(UNDO_TTL_MS)
    await runUndo()
    expect(restore).not.toHaveBeenCalled()
  })
})

describe('dismissUndo 主动放弃', () => {
  it('立即清除，restore 不再可执行', async () => {
    const restore = vi.fn(async () => {})
    const { current, offerUndo, dismissUndo, runUndo } = useUndo()
    offerUndo('A', restore)
    dismissUndo()
    expect(current.value).toBeNull()
    await runUndo()
    expect(restore).not.toHaveBeenCalled()
  })
})
