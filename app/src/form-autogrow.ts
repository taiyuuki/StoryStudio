/**
 * 原生 textarea 自动随内容增高 —— 解决「文本域小、总要手动拉伸」。
 *
 * 实现为全局事件委托 + MutationObserver，各面板懒渲染 / v-if 弹出的表单零成本接入：
 * - 用户输入（input）与聚焦（focus）时即时贴合高度；
 * - 新挂载的 textarea（v-if 编辑表单回填长文本）在出现时 fit 一次；
 * - 不想自动增高的编辑器（如提示词大编辑区）加 `data-no-grow` 豁免，
 *   回到 rows 固定高度 + 手动 resize 的行为。
 */

const fit = (el: HTMLTextAreaElement) => {
  // 隐藏中的元素（v-show 的 tab、未激活的面板）量不出内容高度，等它可见后再 fit
  if (el.offsetParent === null && getComputedStyle(el).position !== 'fixed') return
  el.style.height = 'auto'
  // box-sizing: border-box 下 scrollHeight 不含边框，补回边框厚度避免末行被裁
  const border = el.offsetHeight - el.clientHeight
  el.style.height = `${el.scrollHeight + border}px`
}

const eligible = (el: EventTarget | null): el is HTMLTextAreaElement =>
  el instanceof HTMLTextAreaElement && !el.hasAttribute('data-no-grow')

const tryFitNode = (node: Node) => {
  if (node instanceof HTMLTextAreaElement && !node.hasAttribute('data-no-grow')) fit(node)
}

export function initFormAutoGrow() {
  document.addEventListener('input', (e) => {
    if (eligible(e.target)) fit(e.target)
  })
  document.addEventListener('focusin', (e) => {
    if (eligible(e.target)) fit(e.target)
  })

  const observer = new MutationObserver((records) => {
    for (const record of records) {
      record.addedNodes.forEach(tryFitNode)
    }
  })
  observer.observe(document.body, { childList: true, subtree: true })
}
