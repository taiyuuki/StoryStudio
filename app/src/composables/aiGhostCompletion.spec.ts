// 幽灵补全文本清洗 — 纯逻辑单测
import { describe, expect, it } from 'vitest'
import { sanitizeGhostText } from '@/composables/aiGhostCompletion'

describe('sanitizeGhostText', () => {
  it('普通续写原样通过', () => {
    expect(sanitizeGhostText('抬起头，望向窗外。', '他停下了笔，', 120)).toBe(
      '抬起头，望向窗外。',
    )
  })

  it('去掉前导空白', () => {
    expect(sanitizeGhostText(' 　续写', '前文', 120)).toBe('续写')
  })

  it('遇到换行即止（片段内不换行）', () => {
    expect(sanitizeGhostText('第一句。\n第二句。', '前文', 120)).toBe('第一句。')
  })

  it('剥掉与前文结尾重合的 echo（模型重抄了结尾几个字）', () => {
    // 整段重抄前文结尾 → 剥掉后只留新内容
    expect(sanitizeGhostText('他推开门屋里很暗。', '他推开门', 120)).toBe('屋里很暗。')
    // echo 带上了标点：剥掉重合字后残留的句号保留（正文可能正需要它衔接）
    expect(sanitizeGhostText('推开门。屋里很暗。', '他伸手推开门', 120)).toBe('。屋里很暗。')
  })

  it('单个字符的重合不算 echo（避免误伤正常衔接）', () => {
    // 续写恰以「门」开头（前文末字也是「门」）——1 字不剥
    expect(sanitizeGhostText('门开了。', '他敲了敲门', 120)).toBe('门开了。')
  })

  it('超长在句读处收尾', () => {
    const raw = '一'.repeat(50) + '。' + '二'.repeat(100)
    const out = sanitizeGhostText(raw, '前文', 60)
    expect(out.length).toBe(51)
    expect(out.endsWith('。')).toBe(true)
  })

  it('超长且无句读时硬截', () => {
    const out = sanitizeGhostText('风'.repeat(200), '前文', 50)
    expect(out.length).toBe(50)
  })

  it('纯 echo 剥完为空', () => {
    expect(sanitizeGhostText('推开门', '他推开门', 120)).toBe('')
  })
})
