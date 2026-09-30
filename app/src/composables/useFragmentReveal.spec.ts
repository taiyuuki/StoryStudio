// 审校报告命中定位 — 引文匹配纯逻辑单测
import { describe, expect, it } from 'vitest'
import { quoteRange } from '@/composables/useFragmentReveal'

describe('quoteRange', () => {
  it('精确匹配返回原始偏移', () => {
    expect(quoteRange('雾没散透。他推开门。', '他推开门')).toEqual([5, 9])
  })

  it('找不到返回 null', () => {
    expect(quoteRange('完全不同的内容。', '他推开门')).toBeNull()
  })

  it('空白容错：换行/空格差异下仍能定位并映射回原偏移', () => {
    // 正文里引文带换行，报告里是压平的
    const text = '前半。\n他推开门，\n屋里很暗。'
    const range = quoteRange(text, '他推开门，屋里很暗。')
    expect(range).not.toBeNull()
    expect(text.slice(range![0], range![1])).toBe('他推开门，\n屋里很暗。')
  })

  it('空引文返回 null', () => {
    expect(quoteRange('正文', '')).toBeNull()
    expect(quoteRange('正文', '   ')).toBeNull()
  })
})
