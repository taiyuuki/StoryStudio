// 审校送妙笔的改进指令拼装 — 纯逻辑单测
// ui store 在模块加载期访问 localStorage（主题持久化），node 环境先补内存版再动态导入
import { beforeAll, describe, expect, it, vi } from 'vitest'
import type { AiCheckItemDto } from '@/api/backend'

let buildImprovePrompt: (typeof import('@/composables/useAiCheck'))['buildImprovePrompt']

beforeAll(async () => {
  const mem = new Map<string, string>()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
  })
  ;({ buildImprovePrompt } = await import('@/composables/useAiCheck'))
})

const items: AiCheckItemDto[] = [
  {
    quote: '夜色如墨，万籁俱寂。',
    featureId: 'A',
    featureName: '形容词堆砌',
    severity: 'Strong',
    explanation: '环境描写堆叠四字词。',
    fragmentId: 'f_0004',
    fragmentUncertain: false,
  },
  {
    quote: '他知道，这一切终将过去。',
    featureId: '14',
    featureName: '格言式收束',
    severity: 'Medium',
    explanation: '段落结尾拔高说理。',
    fragmentUncertain: false,
  },
]

describe('buildImprovePrompt', () => {
  it('包含范围、命中数与总体概括', () => {
    const out = buildImprovePrompt('第1章 · 渡口', '整体 AI 味偏淡。', items)
    expect(out).toContain('「第1章 · 渡口」')
    expect(out).toContain('共 2 处命中')
    expect(out).toContain('整体 AI 味偏淡。')
  })

  it('逐条列出严重度/特征/片段归属与原文引用', () => {
    const out = buildImprovePrompt('场景', '概括。', items)
    expect(out).toContain('[强·形容词堆砌]（片段 f_0004）')
    expect(out).toContain('[中·格言式收束]')
    expect(out).toContain('「夜色如墨，万籁俱寂。」')
    // 无片段归属的条目不带片段标注
    expect(out).not.toContain('（片段 f_0005）')
  })

  it('归属推测的条目片段带 ? 并附核对提示', () => {
    const uncertain: AiCheckItemDto = { ...items[0], fragmentId: 'f_000D', fragmentUncertain: true }
    const out = buildImprovePrompt('场景', '概括。', [uncertain, items[1]])
    expect(out).toContain('（片段 f_000D?）')
    expect(out).toContain('就近推测')
    // 全部确定归属时不出现该提示
    const clean = buildImprovePrompt('场景', '概括。', items)
    expect(clean).not.toContain('就近推测')
  })

  it('给出改进约束（保持情节、误伤可跳过）', () => {
    const out = buildImprovePrompt('场景', '概括。', items)
    expect(out).toContain('保持情节、事实与人称不变')
    expect(out).toContain('误伤')
  })
})
