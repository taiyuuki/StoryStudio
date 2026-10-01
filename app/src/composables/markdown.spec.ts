import { describe, expect, it } from 'vitest'

import { renderMarkdown } from './markdown'

describe('renderMarkdown', () => {
  it('渲染基础语法（加粗 / 行内代码 / 列表）', () => {
    const html = renderMarkdown('**重点**与 `code`\n- 甲\n- 乙')
    expect(html).toContain('<strong>重点</strong>')
    expect(html).toContain('<code>code</code>')
    expect(html).toContain('<li>甲</li>')
    expect(html).toContain('<li>乙</li>')
  })

  it('转义原始 HTML，防止注入 webview', () => {
    const html = renderMarkdown('<script>alert(1)</script>')
    expect(html).not.toContain('<script>')
    expect(html).toContain('&lt;script&gt;')
  })

  it('单个换行渲染为 <br>（聊天语义）', () => {
    const html = renderMarkdown('第一行\n第二行')
    expect(html).toContain('<br')
  })

  it('裸 URL 自动转为链接', () => {
    const html = renderMarkdown('参见 https://example.com/a?b=1')
    expect(html).toContain('href="https://example.com/a?b=1"')
  })

  it('围栏代码块完整渲染', () => {
    const html = renderMarkdown('```\nlet x = 1\n```')
    expect(html).toContain('<pre><code>')
    expect(html).toContain('let x = 1')
  })

  it('容忍流式半截语法，不抛异常', () => {
    expect(() => renderMarkdown('**加粗到一半')).not.toThrow()
    expect(() => renderMarkdown('```未闭合代码块')).not.toThrow()
    expect(() => renderMarkdown('| 表头 | 未')).not.toThrow()
    // 半截加粗按原样文本输出
    expect(renderMarkdown('**加粗到一半')).toContain('**加粗到一半')
  })

  it('空文本渲染为空串', () => {
    expect(renderMarkdown('')).toBe('')
  })
})
