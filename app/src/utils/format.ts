// 展示格式化 — 相对时间 / 字数（首页书案与卡片共用）

/** 相对时间：刚刚 / N 分钟前 / N 小时前 / 昨天 / N 天前 / M月D日（跨年带年份） */
export function formatRelativeTime(ts: number): string {
  const diff = Date.now() - ts
  if (diff < 60_000) return '刚刚'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`
  const days = Math.floor(diff / 86_400_000)
  if (days < 1) return `${Math.floor(diff / 3_600_000)} 小时前`
  if (days === 1) return '昨天'
  if (days < 7) return `${days} 天前`
  const d = new Date(ts)
  const sameYear = d.getFullYear() === new Date().getFullYear()
  return sameYear
    ? `${d.getMonth() + 1}月${d.getDate()}日`
    : `${d.getFullYear()}/${String(d.getMonth() + 1).padStart(2, '0')}/${String(d.getDate()).padStart(2, '0')}`
}

/** 字数：1 万以下千分位原样，以上折「万」保留一位小数（34,200 → 3.4 万） */
export function formatWordCount(n: number): string {
  if (n < 10_000) return n.toLocaleString('zh-Hans-CN')
  const w = n / 10_000
  return w >= 100 ? `${Math.round(w)} 万` : `${w.toFixed(1).replace(/\.0$/, '')} 万`
}
