// 审校报告/锚定待审命中定位 — 引文匹配纯逻辑单测
import { describe, expect, it } from 'vitest'
import { quoteRange, quoteRanges } from '@/composables/useFragmentReveal'

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

describe('quoteRanges', () => {
  it('收集全部精确命中（非重叠，按出现顺序）', () => {
    const text = '摆的人走了。摆的人又回来了。'
    expect(quoteRanges(text, '摆的人')).toEqual([
      [0, 3],
      [6, 9],
    ])
  })

  it('单处命中返回一项', () => {
    expect(quoteRanges('雾没散透。他推开门。', '他推开门')).toEqual([[5, 9]])
  })

  it('无命中返回空数组', () => {
    expect(quoteRanges('完全不同的内容。', '他推开门')).toEqual([])
  })

  it('空白容错：多处命中各自映射回原始偏移', () => {
    // 两处命中都跨换行（无精确命中，全走容错路径）
    const text = '他推开门，\n屋里很暗。他推开门，\n屋里很暗。'
    const ranges = quoteRanges(text, '他推开门，屋里很暗')
    expect(ranges).toEqual([
      [0, 10],
      [11, 21],
    ])
    expect(text.slice(ranges[0][0], ranges[0][1])).toBe('他推开门，\n屋里很暗')
    expect(text.slice(ranges[1][0], ranges[1][1])).toBe('他推开门，\n屋里很暗')
  })

  it('精确命中优先于空白容错：有精确匹配时不走容错路径', () => {
    const text = '他开门。他说。他开门。'
    const ranges = quoteRanges(text, '他开门')
    expect(ranges).toEqual([
      [0, 3],
      [7, 10],
    ])
  })

  it('空引文返回空数组', () => {
    expect(quoteRanges('正文', '')).toEqual([])
    expect(quoteRanges('正文', ' \n ')).toEqual([])
  })
})
