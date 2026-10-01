import MarkdownIt from 'markdown-it'

// 妙笔会话 markdown 渲染：
// - html: false — 模型输出里的原始 HTML 一律转义成文本，防止注入 Tauri webview
// - breaks: true — 单个换行即换行（聊天语义，而非段落到空行才换）
// - linkify: true — 裸 URL 自动转为可点击链接
const md = new MarkdownIt({ html: false, breaks: true, linkify: true })

/** 把 markdown 文本渲染成 HTML 片段。容忍流式半截语法，未闭合的部分原样输出。 */
export function renderMarkdown(text: string): string {
  return md.render(text ?? '')
}
