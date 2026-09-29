// 主题对比度自检 — 解析 themes.scss，对「文字色 × 背景」做 WCAG 对比度计算
//
// 用法：node scripts/check-contrast.mjs
// 阈值：正文/次要文字 4.5:1（WCAG AA）；disabled/占位与强调图标 3:1

import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const stylesPath = join(dirname(fileURLToPath(import.meta.url)), '../src/styles/themes.scss')
const css = readFileSync(stylesPath, 'utf8')

// ── 解析主题块 ──────────────────────────────────────────
function parseBlock(selector) {
  const re = new RegExp(`${selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\{`)
  const m = css.match(re)
  if (!m) throw new Error(`找不到主题块：${selector}`)
  const start = css.indexOf('{', m.index)
  let depth = 1
  let i = start + 1
  while (depth > 0 && i < css.length) {
    if (css[i] === '{') depth++
    if (css[i] === '}') depth--
    i++
  }
  const body = css.slice(start + 1, i - 1)
  const vars = {}
  for (const line of body.split('\n')) {
    const kv = line.match(/^\s*(--[\w-]+)\s*:\s*([^;]+);/)
    if (kv) vars[kv[1]] = kv[2].trim()
  }
  return vars
}

const themes = {
  '素宣墨白(ink)': parseBlock('html.theme-ink'),
  '暗夜水墨(dark)': parseBlock('html.theme-dark'),
}

// ── 颜色计算 ────────────────────────────────────────────
function parseColor(value, vars, baseRgb) {
  // 变量引用（一层）
  if (value.startsWith('var(')) {
    const ref = value.match(/var\((.+)\)/)[1].trim()
    value = vars[ref] ?? ''
  }
  const hex = value.match(/^#([0-9a-f]{3,8})$/i)
  if (hex) {
    let h = hex[1]
    if (h.length === 3) h = [...h].map((c) => c + c).join('')
    const r = parseInt(h.slice(0, 2), 16)
    const g = parseInt(h.slice(2, 4), 16)
    const b = parseInt(h.slice(4, 6), 16)
    return [r, g, b, h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1]
  }
  const rgba = value.match(/^rgba?\(([^)]+)\)/)
  if (rgba) {
    const [r, g, b, a = 1] = rgba[1].split(',').map((x) => parseFloat(x.trim()))
    return [r, g, b, a]
  }
  throw new Error(`无法解析颜色：${value}`)
}

/** rgba 半透明背景合成到 base 上 */
function composite([r, g, b, a], base) {
  return [
    Math.round(r * a + base[0] * (1 - a)),
    Math.round(g * a + base[1] * (1 - a)),
    Math.round(b * a + base[2] * (1 - a)),
  ]
}

function luminance([r, g, b]) {
  const f = (c) => {
    c /= 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

function contrast(fg, bg) {
  const l1 = luminance(fg)
  const l2 = luminance(bg)
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05)
}

// ── 检查对 ──────────────────────────────────────────────
// fg 变量 → 阈值（正文 4.5，disabled/图标 3）
const FG_PAIRS = [
  ['--ink-main', 4.5],
  ['--ink-sec', 4.5],
  ['--ink-muted', 4.5],
  ['--ink-disabled', 3.0],
  ['--ink-accent', 3.0],
  ['--state-danger', 3.0],
  ['--state-success', 3.0],
  ['--state-warning', 3.0],
]
// 背景变量（半透明的合成到 bg-main 上）
const BG_PAIRS = [
  '--bg-main',
  '--surface-solid',
  '--surface-1',
  '--surface-2',
  '--popover-bg',
  '--input-bg',
  '--editor-paper-bg',
  '--statusbar-bg',
]

let failures = 0
const rows = []
for (const [themeName, vars] of Object.entries(themes)) {
  const baseRgb = parseColor(vars['--bg-main'], vars)
  for (const bgVar of BG_PAIRS) {
    const bg = composite(parseColor(vars[bgVar], vars), baseRgb)
    for (const [fgVar, threshold] of FG_PAIRS) {
      const fg = parseColor(vars[fgVar], vars)
      const ratio = contrast(fg, bg)
      const ok = ratio >= threshold
      if (!ok) failures++
      rows.push({ themeName, fgVar, bgVar, ratio, threshold, ok })
    }
  }
  // 主按钮文字
  const btnFg = parseColor(vars['--btn-primary-color'], vars)
  const btnBg = parseColor(vars['--btn-primary-bg'], vars)
  const ratio = contrast(btnFg, btnBg)
  if (ratio < 4.5) failures++
  rows.push({ themeName, fgVar: '--btn-primary-color', bgVar: '--btn-primary-bg', ratio, threshold: 4.5, ok: ratio >= 4.5 })
}

// ── 输出 ────────────────────────────────────────────────
const r2 = (n) => n.toFixed(2)
const bad = rows.filter((x) => !x.ok)
for (const x of bad) {
  console.log(
    `✗ [${x.themeName}] ${x.fgVar} × ${x.bgVar}: ${r2(x.ratio)}:1（需 ≥ ${x.threshold}:1）`,
  )
}
const summary = `${rows.length} 组合，${bad.length} 不达标`
if (failures > 0) {
  console.error(`\n对比度检查失败：${summary}`)
  process.exit(1)
}
console.log(`✓ 对比度检查通过：${summary}`)
