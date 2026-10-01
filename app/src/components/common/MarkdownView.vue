<script setup lang="ts">
import { computed } from 'vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { renderMarkdown } from '@/composables/markdown'

// 妙笔 agent 回复的 markdown 渲染容器。
// 流式期间每帧全文重渲染（markdown-it 处理几千字在毫秒级，无需节流）。
const props = defineProps<{
  text: string
  /** 流式中的闪烁光标，追加在文本末尾 */
  caret?: boolean
}>()

const CARET_HTML = '<span class="md-caret"></span>'

// markdown 输出是块级结构，光标直接拼在末尾会另起一行；
// 借 DOMParser 下钻到最后一个元素的叶子处再插入，使其紧跟末行文字
function withCaret(html: string): string {
  try {
    const doc = new DOMParser().parseFromString(`<div>${html}</div>`, 'text/html')
    const root = doc.body.firstElementChild
    let el = root
    while (el?.lastElementChild) el = el.lastElementChild
    if (!root || !el || el === root) return html + CARET_HTML
    el.insertAdjacentHTML('beforeend', CARET_HTML)
    return root.innerHTML
  } catch {
    return html + CARET_HTML
  }
}

const html = computed(() => {
  const body = renderMarkdown(props.text)
  return props.caret ? withCaret(body) : body
})

// Tauri webview 里放任 <a> 默认跳转会顶掉整个应用，统一改走系统浏览器
async function onClick(e: MouseEvent) {
  const anchor = (e.target as HTMLElement).closest('a')
  if (!(anchor instanceof HTMLAnchorElement)) return
  const href = anchor.getAttribute('href') ?? ''
  if (!/^https?:\/\//i.test(href)) return
  e.preventDefault()
  try {
    await openUrl(href)
  } catch {
    window.open(href, '_blank')
  }
}
</script>

<template>
  <div class="md-body" v-html="html" @click="onClick" />
</template>

<style scoped lang="scss">
.md-body {
  word-break: break-word;

  // v-html 内容不带 scope id，统一用 :deep() 下钻；颜色全部走主题变量，跟随明暗主题
  :deep() {
    > *:first-child {
      margin-top: 0;
    }

    > *:last-child {
      margin-bottom: 0;
    }

    p {
      margin: 0 0 0.6em;
    }

    h1,
    h2,
    h3,
    h4,
    h5,
    h6 {
      margin: 0.8em 0 0.4em;
      font-weight: 600;
      line-height: 1.5;
    }

    h1 {
      font-size: 1.25em;
    }

    h2 {
      font-size: 1.18em;
    }

    h3 {
      font-size: 1.1em;
    }

    h4,
    h5,
    h6 {
      font-size: 1em;
    }

    ul,
    ol {
      margin: 0.4em 0;
      padding-left: 1.4em;
    }

    li {
      margin: 0.15em 0;
    }

    li::marker {
      color: var(--ink-muted);
    }

    code {
      font-family: Consolas, 'Courier New', monospace;
      font-size: 0.92em;
      background: var(--surface-2);
      border-radius: 4px;
      padding: 0.1em 0.35em;
    }

    pre {
      margin: 0.5em 0;
      padding: 8px 10px;
      border-radius: 8px;
      background: var(--surface-2);
      overflow-x: auto;
      line-height: 1.6;

      code {
        background: none;
        padding: 0;
      }
    }

    blockquote {
      margin: 0.5em 0;
      padding: 0.1em 0 0.1em 0.8em;
      border-left: 2px solid var(--ink-accent-soft);
      color: var(--ink-sec);

      > *:last-child {
        margin-bottom: 0;
      }
    }

    a {
      color: var(--ink-accent);
    }

    table {
      border-collapse: collapse;
      margin: 0.5em 0;
    }

    th,
    td {
      border: 1px solid var(--divider);
      padding: 4px 8px;
    }

    th {
      background: var(--surface-2);
    }

    hr {
      border: none;
      border-top: 1px solid var(--divider);
      margin: 0.8em 0;
    }

    img {
      max-width: 100%;
      border-radius: 6px;
    }

    .md-caret {
      display: inline-block;
      width: 2px;
      height: 1em;
      margin-left: 2px;
      vertical-align: -0.15em;
      background: var(--caret-color);
      animation: md-blink 0.9s steps(1) infinite;
    }
  }
}
</style>

<style lang="scss">
// keyframes 供 v-html 注入的 .md-caret 使用，必须全局可见（scoped 作用域打不到它）
@keyframes md-blink {
  50% {
    opacity: 0;
  }
}
</style>
