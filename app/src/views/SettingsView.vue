<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  api,
  type AiConfigDto,
  type AiProfileDto,
  type McpServerStatusDto,
  type ProfileTestRecordDto,
  type ProviderPresetDto,
} from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const router = useRouter()
const workspace = useWorkspaceStore()

// ── 配置状态 ────────────────────────────────────────────
const config = ref<AiConfigDto>({ profiles: [], activeProfile: null, purposeProfiles: null, agentMaxToolTurns: null })
const loaded = ref(false)
const saving = ref(false)
const editingId = ref<string | null>(null)

const draft = reactive<AiProfileDto>(emptyProfile())

function emptyProfile(): AiProfileDto {
  return {
    id: '',
    preset: null,
    name: '',
    provider: 'anthropic',
    model: '',
    apiKey: '',
    apiUrl: '',
    apiUrlFull: false,
    thinking: 'low',
    timeoutSecs: null,
    maxTokens: null,
    longContext: null,
    apiMode: 'chat',
    extraBody: null,
    extraHeaders: null,
    lastTest: null,
  }
}

// ── 顶部分栏 ────────────────────────────────────────────
const TABS = [
  { key: 'providers', label: 'AI 服务商', icon: 'fa-solid fa-layer-group' },
  { key: 'purposes', label: '用途分配', icon: 'fa-solid fa-shuffle' },
  { key: 'mcp', label: 'MCP 服务', icon: 'fa-solid fa-plug' },
] as const
type TabKey = (typeof TABS)[number]['key']
const activeTab = ref<TabKey>('providers')

// ── 服务商预设 ──────────────────────────────────────────
const presets = ref<ProviderPresetDto[]>([])
const CUSTOM_OPTIONS = [
  { value: 'custom:openai', label: '自定义 · OpenAI 兼容' },
  { value: 'custom:anthropic', label: '自定义 · Anthropic 兼容' },
  { value: 'custom:gemini', label: '自定义 · Gemini 原生' },
]

/** 预设 → 图标：`/` 开头取 public 静态图，否则按 FontAwesome 类名渲染 */
const PRESET_ICONS: Record<string, string> = {
  openai: '/provider-icons/openai.svg',
  anthropic: '/provider-icons/claude.svg',
  deepseek: '/provider-icons/deepseek.ico',
  zhipu: '/provider-icons/bigmodel.png',
  'zhipu-coding': '/provider-icons/bigmodel.png',
  moonshot: '/provider-icons/kimi.png',
  qwen: '/provider-icons/tongyi.svg',
  openrouter: '/provider-icons/openrouter.ico',
  siliconflow: '/provider-icons/siliconflow.png',
  gemini: '/provider-icons/gemini.svg',
  ollama: '/provider-icons/local.svg',
}
const CUSTOM_ICON = 'fa-sliders'

/** 卡片上只显示主名，去掉括号补充说明（「Kimi（月之暗面）」→「Kimi」） */
function shortLabel(label: string): string {
  return label.replace(/（[^）]*）/g, '').trim()
}

/** 档案卡片用的服务商图标 */
function iconOf(p: AiProfileDto): string {
  return (p.preset && PRESET_ICONS[p.preset]) || CUSTOM_ICON
}

/** 档案卡片用的服务商一行字 */
function providerLabel(p: AiProfileDto): string {
  if (p.provider === 'gemini') return 'Gemini'
  if (p.provider === 'anthropic') return 'Anthropic'
  return p.apiMode === 'responses' ? 'OpenAI · Responses' : 'OpenAI · Chat'
}

const providerValue = computed(() => {
  if (draft.preset && presets.value.some((p) => p.id === draft.preset)) return draft.preset
  if (draft.provider === 'gemini') return 'custom:gemini'
  return draft.provider === 'anthropic' ? 'custom:anthropic' : 'custom:openai'
})

const currentPreset = computed(() => presets.value.find((p) => p.id === draft.preset) ?? null)

/** 卡片网格用的完整选项（预设 + 自定义），带图标 */
const providerOptions = computed(() => [
  ...presets.value.map((p) => ({ value: p.id, label: p.name, icon: PRESET_ICONS[p.id] ?? CUSTOM_ICON })),
  ...CUSTOM_OPTIONS.map((c) => ({ value: c.value, label: c.label, icon: CUSTOM_ICON })),
])

function applyProvider(value: string) {
  const preset = presets.value.find((p) => p.id === value)
  // 切换前的预设：用于判断名称/模型是不是它填的（是 → 跟着换；用户手改过 → 保留）
  const prev = presets.value.find((p) => p.id === draft.preset) ?? null
  // 仅在真正换卡时清 Key：重复点击当前卡片不清，避免误删刚输入的值
  const switching = value !== providerValue.value
  fetchedModels.value = []
  if (preset) {
    draft.preset = preset.id
    draft.provider = preset.provider
    draft.apiUrl = preset.apiUrl ?? ''
    draft.apiUrlFull = preset.apiUrlFull
    if (prev?.defaultModel && draft.model === prev.defaultModel) {
      // 模型来自上一个预设：换成新预设的建议模型（没有就清空，提示拉取）
      draft.model = preset.defaultModel ?? ''
    } else if (!draft.model && preset.defaultModel) {
      draft.model = preset.defaultModel
    }
    const nameFromPreset = prev ? draft.name === prev.name : !draft.name || draft.name.startsWith('档案 ')
    if (nameFromPreset) draft.name = preset.name
  } else {
    draft.preset = null
    draft.provider =
      value === 'custom:anthropic' ? 'anthropic' : value === 'custom:gemini' ? 'gemini' : 'openai'
    draft.apiUrl = ''
    draft.apiUrlFull = false
  }
  // 换服务商不继承旧 Key（不同服务的 Key 不通用，留着容易忘改导致鉴权失败）
  if (switching) draft.apiKey = ''
  // 非 OpenAI 档案不使用接口模式；切换时归位
  if (draft.provider !== 'openai') draft.apiMode = 'chat'
}

// ── 档案选择 / 增删 ────────────────────────────────────
/** 私有参数 / 请求头的编辑态（JSON 文本 ↔ 对象在保存与测试时转换） */
const extraBodyText = ref('')
const extraHeadersText = ref('')

function edit(p: AiProfileDto) {
  editingId.value = p.id
  Object.assign(draft, JSON.parse(JSON.stringify(p)))
  // 旧档案没有接口模式字段，统一回退 Chat
  if (!draft.apiMode) draft.apiMode = 'chat'
  // 切换档案：模型列表、私有参数文本都重置
  fetchedModels.value = []
  extraBodyText.value = p.extraBody ? JSON.stringify(p.extraBody, null, 2) : ''
  extraHeadersText.value = p.extraHeaders ? JSON.stringify(p.extraHeaders, null, 2) : ''
}

/** 从编辑卡片返回档案列表（未保存的修改会被丢弃） */
function closeEditor() {
  editingId.value = null
}

function addProfile() {
  const p = emptyProfile()
  p.id = crypto.randomUUID()
  p.name = `档案 ${config.value.profiles.length + 1}`
  config.value.profiles.push(p)
  edit(p)
}

async function removeProfile(id: string) {
  const p = config.value.profiles.find((x) => x.id === id)
  try {
    await ElMessageBox.confirm(
      `删除「${p?.name ?? '该档案'}」后，用它做默认或分配了用途的场景会回退到其它档案。确定删除？`,
      '删除档案',
      { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  config.value.profiles = config.value.profiles.filter((x) => x.id !== id)
  if (config.value.activeProfile === id) {
    config.value.activeProfile = config.value.profiles[0]?.id ?? null
  }
  if (config.value.purposeProfiles) {
    for (const key of Object.keys(config.value.purposeProfiles)) {
      if (config.value.purposeProfiles[key] === id) delete config.value.purposeProfiles[key]
    }
    if (Object.keys(config.value.purposeProfiles).length === 0) config.value.purposeProfiles = null
  }
  await persist('已删除')
  if (editingId.value === id) editingId.value = null
}

/** 复刻当前编辑的档案：以编辑中的内容为准（所见即所得），改名换 id 后立即可微调 */
async function duplicateProfile() {
  if (!editingId.value) return
  let extraBody: Record<string, unknown> | null
  let extraHeaders: Record<string, string> | null
  try {
    extraBody = parseJsonField(extraBodyText.value, '私有参数')
    extraHeaders = parseJsonField(extraHeadersText.value, '自定义请求头') as Record<string, string> | null
  } catch (err) {
    ElMessage.error((err as Error).message)
    return
  }
  const clone: AiProfileDto = {
    ...JSON.parse(JSON.stringify(draft)),
    extraBody,
    extraHeaders,
    timeoutSecs: normTimeout(),
    maxTokens: normMaxTokens(),
    lastTest: null,
  }
  clone.id = crypto.randomUUID()
  // 连续复刻不叠「副本 副本」：先剥掉源名尾部的副本标记，再找不重名的编号
  const base = draft.name.replace(/\s*副本(\s*\d+)?$/, '').trim() || '未命名'
  const names = new Set(config.value.profiles.map((p) => p.name))
  clone.name = `${base} 副本`
  for (let i = 2; names.has(clone.name); i++) clone.name = `${base} 副本 ${i}`
  config.value.profiles.push(clone)
  edit(clone)
  await persist(`已复刻为「${clone.name}」，可微调后保存`)
}

function pickActive(id: string) {
  config.value.activeProfile = id
  void persist('已设为默认')
}

// ── 保存（单档案粒度）──────────────────────────────────
/** JSON 文本 → 对象；文本为空返回 null，非法抛带定位信息的错误 */
function parseJsonField(text: string, label: string): Record<string, unknown> | null {
  const err = validateJsonField(text, label === '自定义请求头')
  if (err) throw new Error(err)
  const t = text.trim()
  if (!t) return null
  return JSON.parse(t) as Record<string, unknown>
}

/** 即时校验：空 = 合法（可选项）；返回错误消息，空串表示通过 */
function validateJsonField(text: string, requireStringValues: boolean): string {
  const t = text.trim()
  if (!t) return ''
  let v: unknown
  try {
    v = JSON.parse(t)
  } catch (err) {
    return `JSON 语法错误：${(err as Error).message}`
  }
  if (typeof v !== 'object' || v === null || Array.isArray(v)) {
    return '必须是 JSON 对象（{} 包裹的键值对）'
  }
  if (requireStringValues) {
    for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
      if (typeof val !== 'string') {
        return `请求头的值必须是字符串：「${k}」的值是 ${JSON.stringify(val)}（记得加引号）`
      }
    }
  }
  return ''
}

const extraBodyError = computed(() => validateJsonField(extraBodyText.value, false))
const extraHeadersError = computed(() => validateJsonField(extraHeadersText.value, true))
const extraJsonInvalid = computed(() => !!extraBodyError.value || !!extraHeadersError.value)

/** 超时归一化：空/非法 → null（后端默认 300），并夹到 10–7200 秒 */
function normTimeout(): number | null {
  const n = Number(draft.timeoutSecs)
  if (!Number.isFinite(n) || n <= 0) return null
  return Math.min(Math.max(Math.round(n), 10), 7200)
}

/** 输出上限归一化：空/非法 → null（各功能默认值），并夹到 256–262144 token */
function normMaxTokens(): number | null {
  const n = Number(draft.maxTokens)
  if (!Number.isFinite(n) || n <= 0) return null
  return Math.min(Math.max(Math.round(n), 256), 262144)
}

/** 输出上限常用档位（token）；65536 覆盖思考模型的 reasoning 配额 */
const MAX_TOKEN_PRESETS = [8192, 16384, 32768, 65536, 131072, 262144]

/** 持久化整个配置；返回是否成功（失败时编辑卡片不应退出） */
async function persist(successMsg: string): Promise<boolean> {
  saving.value = true
  try {
    await api.setAiConfig(config.value)
    if (successMsg) ElMessage.success(successMsg)
    return true
  } catch (err) {
    ElMessage.error((err as Error).message)
    return false
  } finally {
    saving.value = false
  }
}

async function saveProfile() {
  if (!draft.id) {
    ElMessage.warning('先从列表选择或新建一个档案')
    return
  }
  if (!draft.name.trim()) {
    ElMessage.warning('档案名称不能为空')
    return
  }
  try {
    draft.extraBody = parseJsonField(extraBodyText.value, '私有参数')
    draft.extraHeaders = parseJsonField(extraHeadersText.value, '自定义请求头') as
      | Record<string, string>
      | null
  } catch (err) {
    ElMessage.error((err as Error).message)
    return
  }
  draft.timeoutSecs = normTimeout()
  draft.maxTokens = normMaxTokens()
  const idx = config.value.profiles.findIndex((p) => p.id === draft.id)
  if (idx >= 0) config.value.profiles[idx] = JSON.parse(JSON.stringify(draft))
  if (!config.value.activeProfile) config.value.activeProfile = draft.id
  // 保存成功才返回列表，失败留在编辑卡片里
  if (await persist('已保存')) editingId.value = null
}

// ── 模型拉取 ────────────────────────────────────────────
const fetchedModels = ref<string[]>([])
const fetchingModels = ref(false)

async function fetchModels() {
  fetchingModels.value = true
  let list: string[] = []
  let failed = false
  try {
    list = await api.aiListModels(
      draft.provider || 'openai',
      draft.apiUrl || null,
      draft.apiUrlFull ?? false,
      draft.apiKey || null,
    )
  } catch (err) {
    failed = true
    ElMessage.error((err as Error).message)
  } finally {
    fetchingModels.value = false
  }
  if (failed) return
  if (list.length > 0) {
    fetchedModels.value = list
  } else {
    ElMessage.info('这个服务没有返回模型列表，直接填模型名即可')
  }
}

// ── 连接测试 ────────────────────────────────────────────
const testing = ref(false)

async function testProfile() {
  if (!draft.id) return
  testing.value = true
  try {
    // 用编辑中的内容测试；私有参数参与（不合法会在后端报错）
    let extraBody: Record<string, unknown> | null = null
    let extraHeaders: Record<string, string> | null = null
    try {
      extraBody = parseJsonField(extraBodyText.value, '私有参数')
      extraHeaders = parseJsonField(extraHeadersText.value, '自定义请求头') as
        | Record<string, string>
        | null
    } catch (err) {
      ElMessage.error((err as Error).message)
      return
    }
    const payload: AiProfileDto = {
      ...JSON.parse(JSON.stringify(draft)),
      extraBody,
      extraHeaders,
      timeoutSecs: normTimeout(),
      maxTokens: normMaxTokens(),
    }
    const record: ProfileTestRecordDto = await api.aiTestProfile(payload)
    draft.lastTest = record
    const idx = config.value.profiles.findIndex((p) => p.id === draft.id)
    if (idx >= 0) config.value.profiles[idx].lastTest = record
    // 测试结果随保存持久化
    await persist(record.ok ? '' : '')
    if (record.ok) ElMessage.success(`测试通过（${record.latencyMs}ms）`)
    else ElMessage.error(`测试失败：${record.detail}`)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    testing.value = false
  }
}

function testDotClass(p: AiProfileDto): string {
  if (!p.lastTest) return 'is-unknown'
  return p.lastTest.ok ? 'is-ok' : 'is-fail'
}

function testDotTitle(p: AiProfileDto): string {
  if (!p.lastTest) return '尚未测试'
  return `${p.lastTest.ok ? '测试通过' : '测试失败'} · ${timeAgo(p.lastTest.at)}`
}

function timeAgo(ts: number): string {
  const diff = Date.now() - ts
  const m = Math.floor(diff / 60_000)
  if (m < 1) return '刚刚'
  if (m < 60) return `${m} 分钟前`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时前`
  return `${Math.floor(h / 24)} 天前`
}

// ── 用途分配 ────────────────────────────────────────────
const PURPOSES = [
  { key: 'write', label: '续写正文', hint: '生成接下来的情节文字' },
  { key: 'check', label: 'AI 检测', hint: '编辑器「AI 审校」按钮的 AI 味检测；输出结构化报告，宜配便宜快的模型' },
  { key: 'complete', label: '行内补全', hint: '编辑时幽灵预测下文，Tab 采纳（宜配快而便宜的模型）' },
  { key: 'agent', label: '妙笔对话', hint: '右栏助手：改稿、查设定、管大纲' },
  { key: 'workflow', label: '逐章成书', hint: '按大纲自动写整章' },
  { key: 'design', label: 'AI 建书', hint: '从一句话灵感生成新书' },
] as const

const activeProfileName = computed(
  () => config.value.profiles.find((p) => p.id === config.value.activeProfile)?.name ?? '',
)

function purposeValue(key: string): string {
  return config.value.purposeProfiles?.[key] ?? ''
}

async function setPurpose(key: string, profileId: string) {
  if (!config.value.purposeProfiles) config.value.purposeProfiles = {}
  if (profileId) {
    config.value.purposeProfiles[key] = profileId
  } else {
    delete config.value.purposeProfiles[key]
    if (Object.keys(config.value.purposeProfiles).length === 0) {
      config.value.purposeProfiles = null
    }
  }
  await persist('用途分配已保存')
}

function purposesOf(id: string): string {
  return PURPOSES.filter((p) => purposeValue(p.key) === id)
    .map((p) => p.label)
    .join(' · ')
}

// ── 妙笔对话：每轮工具调用上限 ──────────────────────────
const AGENT_TURNS_DEFAULT = 40
const AGENT_TURNS_MAX = 200

const agentTurns = computed(() => config.value.agentMaxToolTurns ?? AGENT_TURNS_DEFAULT)

async function setAgentTurns(raw: string, target: HTMLInputElement) {
  const n = Math.round(Number(raw))
  if (!Number.isFinite(n) || n < 1 || n > AGENT_TURNS_MAX) {
    ElMessage.warning(`每轮工具调用上限需在 1–${AGENT_TURNS_MAX} 之间`)
    target.value = String(agentTurns.value) // 非法输入回退当前生效值
    return
  }
  if (n === agentTurns.value) return
  config.value.agentMaxToolTurns = n
  await persist(`妙笔每轮上限已设为 ${n}`)
}

// ── MCP 服务（外部 Agent 接入）──────────────────────────
const MCP_PORT_DEFAULT = 27520
// localStorage 偏好键（workspace.ts 的 mcpFollowProject 读取同名键）
const MCP_PREF = { enabled: 'mcp.autoStart', port: 'mcp.port', readOnly: 'mcp.readOnly' } as const

const mcpStatus = ref<McpServerStatusDto | null>(null)
const mcpPortInput = ref(MCP_PORT_DEFAULT)
const mcpReadOnly = ref(false)
const mcpBusy = ref(false)
const mcpEnabled = computed(() => mcpStatus.value?.running ?? false)

async function mcpRefresh() {
  try {
    mcpStatus.value = await api.mcpServerStatus()
    if (mcpStatus.value.running) {
      mcpPortInput.value = mcpStatus.value.port
      mcpReadOnly.value = mcpStatus.value.readOnly
    }
  } catch {
    // 状态查询失败不弹窗，卡片显示为未运行
  }
}

function mcpSavePref() {
  localStorage.setItem(MCP_PREF.enabled, mcpEnabled.value ? 'on' : 'off')
  localStorage.setItem(MCP_PREF.port, String(mcpPortInput.value))
  localStorage.setItem(MCP_PREF.readOnly, mcpReadOnly.value ? '1' : '0')
}

async function mcpToggle(on: boolean) {
  if (!on) {
    mcpBusy.value = true
    try {
      await api.mcpServerStop()
      mcpSavePref()
    } catch (err) {
      ElMessage.error((err as Error).message)
    } finally {
      mcpBusy.value = false
    }
    await mcpRefresh()
    return
  }
  if (!workspace.root) {
    ElMessage.warning('请先打开一个项目，再启动 MCP 服务')
    await mcpRefresh()
    return
  }
  mcpBusy.value = true
  try {
    mcpStatus.value = await api.mcpServerStart(workspace.root, mcpPortInput.value, mcpReadOnly.value)
    mcpSavePref()
    ElMessage.success(`MCP 服务已启动：${mcpStatus.value.url}`)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    mcpBusy.value = false
  }
}

/** 端口 / 只读变化时热重启（未运行则只记住偏好） */
async function mcpApplySettings() {
  mcpSavePref()
  if (!mcpEnabled.value) return
  const cur = mcpStatus.value
  if (cur && cur.port === mcpPortInput.value && cur.readOnly === mcpReadOnly.value) return
  mcpBusy.value = true
  try {
    mcpStatus.value = await api.mcpServerStart(workspace.root, mcpPortInput.value, mcpReadOnly.value)
    ElMessage.success('MCP 服务已按新设置重启')
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    mcpBusy.value = false
  }
}

/**
 * 接入信息的可复制形态：
 * - json：`mcpServers` 通用格式，Claude Code / Cursor / Cline / Cherry Studio 等大多数客户端直接可用
 * - zcode：ZCode settings.json 的 `mcp.servers` 格式
 * - claude-cli：`claude mcp add` 一行命令
 * - url / token：单项裸值，供只有表单输入框的客户端手动填写
 */
type McpCopyFormat = 'json' | 'zcode' | 'claude-cli' | 'url' | 'token'

const MCP_COPY_DONE: Record<McpCopyFormat, string> = {
  json: '已复制通用 JSON 配置（含 URL 与 Token）',
  zcode: '已复制 ZCode 配置（含 URL 与 Token）',
  'claude-cli': '已复制 Claude Code 添加命令',
  url: '已复制 URL',
  token: '已复制 Token',
}

function mcpCopyText(st: McpServerStatusDto, fmt: McpCopyFormat): string {
  const auth = `Bearer ${st.token}`
  switch (fmt) {
    case 'url':
      return st.url
    case 'token':
      return st.token
    case 'claude-cli':
      return `claude mcp add --transport http story-studio ${st.url} --header "Authorization: ${auth}"`
    case 'zcode':
      return JSON.stringify(
        {
          mcp: {
            servers: {
              'story-studio': { type: 'http', url: st.url, headers: { Authorization: auth }, timeoutMs: 300000 },
            },
          },
        },
        null,
        2,
      )
    case 'json':
      return JSON.stringify(
        { mcpServers: { 'story-studio': { type: 'http', url: st.url, headers: { Authorization: auth } } } },
        null,
        2,
      )
  }
}

async function mcpCopy(fmt: McpCopyFormat) {
  const st = mcpStatus.value
  if (!st?.running) return
  try {
    await navigator.clipboard.writeText(mcpCopyText(st, fmt))
    ElMessage.success(MCP_COPY_DONE[fmt])
  } catch (err) {
    ElMessage.error(`复制失败：${(err as Error).message}`)
  }
}

const mcpTokenShown = ref(false)
/** 遮罩显示的 Token：保留首尾各 4 位便于核对，其余打点 */
const mcpTokenMasked = computed(() => {
  const t = mcpStatus.value?.token ?? ''
  if (t.length <= 12) return '••••••••••••'
  return `${t.slice(0, 4)}••••••••••••${t.slice(-4)}`
})

// ── 页面加载 / 返回 ────────────────────────────────────
onMounted(async () => {
  void mcpRefresh()
  try {
    const [cfg, presetList] = await Promise.all([api.getAiConfig(), api.providerPresets()])
    presets.value = presetList
    config.value = cfg
    loaded.value = true
    if (config.value.profiles.length === 0) {
      const p = emptyProfile()
      p.id = crypto.randomUUID()
      p.name = '默认档案'
      config.value.profiles.push(p)
      config.value.activeProfile = p.id
    }
    if (!config.value.activeProfile) config.value.activeProfile = config.value.profiles[0].id
  } catch (err) {
    ElMessage.error((err as Error).message)
  }
})

function goBack() {
  router.back()
}
</script>

<template>
  <div class="ink-bg-container settings-view">
    <div class="paper-texture" />
    <div class="sv-wrap">
      <!-- 页头 -->
      <header class="sv-head">
        <button class="ink-icon-btn" title="返回" @click="goBack">
          <i class="fa-solid fa-arrow-left" />
        </button>
        <div class="sv-titles">
          <h1>AI 设置</h1>
          <p class="text-ink-muted">档案保存在你自己的电脑上（~/.story-studio/config.json），所有作品共用</p>
        </div>
      </header>

      <div v-if="!loaded" class="sv-loading">
        <i class="fa-solid fa-circle-notch fa-spin" />
        <span>正在读取配置…</span>
      </div>

      <template v-else>
        <!-- 分栏（编辑档案时让位给编辑卡片） -->
        <nav v-if="!editingId" class="sv-tabs">
          <button
            v-for="t in TABS"
            :key="t.key"
            type="button"
            :class="{ 'is-active': activeTab === t.key }"
            @click="activeTab = t.key"
          >
            <i :class="t.icon" />
            {{ t.label }}
          </button>
        </nav>

        <!-- ── Tab：AI 服务商 ─────────────────────────────── -->
        <section v-if="activeTab === 'providers'" class="sv-panel">
          <!-- 编辑 / 新建档案：独立卡片视图，保存或返回后回到列表 -->
          <div v-if="editingId" class="fusion-card sv-editor">
            <div class="card-head">
              <button class="ink-icon-btn" title="返回档案列表" @click="closeEditor">
                <i class="fa-solid fa-arrow-left" />
              </button>
              <i class="fa-solid fa-pen-nib" />
              <span>编辑档案</span>
              <button
                v-if="editingId !== config.activeProfile"
                class="ink-btn ink-btn--sm sv-head-btn"
                @click="pickActive(editingId)"
              >
                设为默认
              </button>
              <button
                class="ink-btn ink-btn--sm sv-head-btn"
                title="以当前档案为底本复制一份新档案，可微调参数后另存"
                @click="duplicateProfile"
              >
                <i class="fa-regular fa-clone" />
                复刻
              </button>
            </div>

            <div class="sv-form">
              <div class="wz-field">
                <label>档案名称</label>
                <input v-model="draft.name" placeholder="例：我的 DeepSeek" />
              </div>

              <!-- 服务商：图标 + 名称卡片（选中态高亮，点选即应用预设） -->
              <div class="wz-field">
                <label>服务商（选预设自动填地址与模型）</label>
                <div class="pv-grid">
                  <button
                    v-for="opt in providerOptions"
                    :key="opt.value"
                    type="button"
                    class="pv-card"
                    :class="{ 'is-picked': providerValue === opt.value }"
                    :title="opt.label"
                    @click="applyProvider(opt.value)"
                  >
                    <span class="pv-icon" :class="{ 'is-img': opt.icon.startsWith('/') }">
                      <img v-if="opt.icon.startsWith('/')" :src="opt.icon" alt="" />
                      <i v-else class="fa-solid" :class="opt.icon" />
                    </span>
                    <span class="pv-name">{{ shortLabel(opt.label) }}</span>
                  </button>
                </div>
              </div>

              <p v-if="currentPreset?.note" class="sv-note">
                <i class="fa-solid fa-circle-info" />
                {{ currentPreset.note }}
              </p>

              <!-- ── 连接 ───────────────────────────────── -->
              <div class="sv-group-title">连接</div>

              <div class="wz-field">
                <label>API Key</label>
                <input v-model="draft.apiKey" type="password" placeholder="留空则读取环境变量" />
              </div>

              <div class="wz-row">
                <div class="wz-field">
                  <label>API URL</label>
                  <input v-model="draft.apiUrl" placeholder="默认官方 endpoint" class="num" />
                </div>
                <div class="wz-field sv-switch-field">
                  <label>完整 URL 模式</label>
                  <ElSwitch
                    :model-value="draft.apiUrlFull ?? false"
                    @update:model-value="draft.apiUrlFull = $event as boolean"
                  />
                </div>
              </div>

              <!-- 接口模式：仅 OpenAI 档案（Anthropic 只有一种协议） -->
              <div v-if="draft.provider === 'openai'" class="wz-field sv-mode-field">
                <label>接口模式</label>
                <div class="sv-mode-seg">
                  <button
                    type="button"
                    class="sv-mode-btn"
                    :class="{ 'is-on': draft.apiMode !== 'responses' }"
                    @click="draft.apiMode = 'chat'"
                  >
                    Chat Completions
                  </button>
                  <button
                    type="button"
                    class="sv-mode-btn"
                    :class="{ 'is-on': draft.apiMode === 'responses' }"
                    @click="draft.apiMode = 'responses'"
                  >
                    Responses
                  </button>
                </div>
                <label class="sv-mini">绝大多数兼容服务只支持 Chat；Responses 是 OpenAI 原生接口（GPT-5 / o 系列），请求地址与格式都不同</label>
              </div>

              <!-- ── 模型与生成 ──────────────────────────── -->
              <div class="sv-group-title">模型与生成</div>

              <div class="wz-row">
                <div class="wz-field">
                  <label>模型</label>
                  <div class="sv-model-row">
                    <input
                      v-if="fetchedModels.length === 0"
                      v-model="draft.model"
                      placeholder="可点右侧「拉取」从服务获取列表"
                      class="num"
                    />
                    <select v-else v-model="draft.model">
                      <option v-if="draft.model && !fetchedModels.includes(draft.model)" :value="draft.model">{{ draft.model }}</option>
                      <option v-for="m in fetchedModels" :key="m" :value="m">{{ m }}</option>
                    </select>
                    <button
                      class="ink-btn ink-btn--sm"
                      :disabled="fetchingModels"
                      title="从服务商拉取可用模型列表"
                      @click="fetchModels"
                    >
                      <i :class="fetchingModels ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-cloud-arrow-down'" />
                      {{ fetchingModels ? '拉取中' : '拉取' }}
                    </button>
                  </div>
                  <label v-if="fetchedModels.length > 0" class="sv-mini num">拉到 {{ fetchedModels.length }} 个模型</label>
                </div>
                <div class="wz-field sv-thinking-field">
                  <label>思考模式</label>
                  <select v-model="draft.thinking">
                    <option value="auto">自动</option>
                    <option value="off">不思考</option>
                    <option value="low">浅思考</option>
                    <option value="medium">适中</option>
                    <option value="high">深度</option>
                  </select>
                </div>
              </div>

              <div class="wz-row">
                <div class="wz-field sv-gen-field">
                  <label>输出上限</label>
                  <select
                    v-model="draft.maxTokens"
                    title="单次生成的输出 token 上限（思考型模型的思考也算在内）；留空用各功能默认值，思考模型输出被截断或为空时调大（如 65536）"
                  >
                    <option :value="null">默认（各功能自定）</option>
                    <option
                      v-if="draft.maxTokens != null && !MAX_TOKEN_PRESETS.includes(draft.maxTokens)"
                      :value="draft.maxTokens"
                    >
                      {{ draft.maxTokens }}（自定义）
                    </option>
                    <option v-for="v in MAX_TOKEN_PRESETS" :key="v" :value="v">
                      {{ v === 65536 ? '65536（思考推荐）' : v }}
                    </option>
                  </select>
                </div>
                <div class="wz-field sv-gen-field">
                  <label>长上下文</label>
                  <select
                    v-model="draft.longContext"
                    title="模型是否支持 100 万 token 上下文窗口；影响妙笔对话的记忆压缩时机：支持 → 预算 90 万（用满大窗口），不支持 → 12 万（128K 窗口保守值），默认 → 50 万"
                  >
                    <option :value="null">跟随默认</option>
                    <option :value="true">支持（100 万）</option>
                    <option :value="false">不支持（保守）</option>
                  </select>
                </div>
                <div class="wz-field sv-gen-field">
                  <label>超时（秒）</label>
                  <input
                    v-model.number="draft.timeoutSecs"
                    type="number"
                    min="30"
                    max="3600"
                    placeholder="300"
                    title="单次生成请求的超时时间；深度思考或长文生成较慢时可调大"
                    class="num"
                  />
                </div>
              </div>

              <!-- 私有参数 -->
              <ElCollapse class="sv-collapse">
                <ElCollapseItem title="私有参数与自定义请求头（JSON，可选）" name="extra">
                  <div class="wz-field">
                    <label>私有参数（深合并进请求体，支持各服务的私有字段，如 top_p、enable_thinking）</label>
                    <textarea v-model="extraBodyText" rows="4" spellcheck="false" placeholder='{ "top_p": 0.9 }' class="num" :class="{ 'is-invalid': extraBodyError }" />
                    <span v-if="extraBodyError" class="json-check is-invalid">
                      <i class="fa-solid fa-circle-exclamation" />
                      {{ extraBodyError }}
                    </span>
                    <span v-else-if="extraBodyText.trim()" class="json-check is-valid">
                      <i class="fa-solid fa-circle-check" />
                      JSON 格式正确
                    </span>
                  </div>
                  <div class="wz-field">
                    <label>自定义请求头（追加，不覆盖认证头）</label>
                    <textarea v-model="extraHeadersText" rows="3" spellcheck="false" placeholder='{ "X-Title": "StoryStudio" }' class="num" :class="{ 'is-invalid': extraHeadersError }" />
                    <span v-if="extraHeadersError" class="json-check is-invalid">
                      <i class="fa-solid fa-circle-exclamation" />
                      {{ extraHeadersError }}
                    </span>
                    <span v-else-if="extraHeadersText.trim()" class="json-check is-valid">
                      <i class="fa-solid fa-circle-check" />
                      JSON 格式正确
                    </span>
                  </div>
                </ElCollapseItem>
              </ElCollapse>

              <!-- 连接测试 -->
              <div class="sv-test">
                <div class="sv-test-main">
                  <button class="ink-btn ink-btn--sm" :disabled="testing || extraJsonInvalid" @click="testProfile">
                    <i :class="testing ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-plug-circle-check'" />
                    {{ testing ? '测试中…' : '测试连接' }}
                  </button>
                  <div v-if="draft.lastTest" class="sv-test-result" :class="draft.lastTest.ok ? 'is-ok' : 'is-fail'">
                    <i class="fa-solid" :class="draft.lastTest.ok ? 'fa-circle-check' : 'fa-circle-exclamation'" />
                    <div class="sv-test-text">
                      <span class="sv-test-line1">
                        {{ draft.lastTest.ok ? '上次测试通过' : '上次测试失败' }}
                        · {{ draft.lastTest.latencyMs }}ms · {{ timeAgo(draft.lastTest.at) }}
                      </span>
                      <span class="sv-test-detail">{{ draft.lastTest.detail }}</span>
                    </div>
                  </div>
                  <p v-else class="sv-test-empty">还没测试过这个档案</p>
                </div>
              </div>
            </div>

            <!-- 保存（保存成功后返回列表）-->
            <div class="sv-actions">
              <button
                v-if="config.profiles.length > 1"
                class="ink-btn ink-btn--sm sv-remove"
                @click="removeProfile(editingId!)"
              >
                <i class="fa-regular fa-trash-can" />
                删除此档案
              </button>
              <span class="sv-actions-flex" />
              <button class="ink-btn" @click="closeEditor">取消</button>
              <button class="ink-btn ink-btn--primary" :disabled="saving || extraJsonInvalid" @click="saveProfile">
                <i class="fa-solid fa-check" />
                {{ saving ? '保存中…' : '保存档案' }}
              </button>
            </div>
          </div>

          <!-- 档案卡片列表（cc-switch 式：点卡片切换默认，卡片上编辑/删除） -->
          <div v-else class="sv-grid">
            <div
              v-for="p in config.profiles"
              :key="p.id"
              class="fusion-card sv-card"
              :class="{ 'is-default': p.id === config.activeProfile }"
              :title="p.id === config.activeProfile ? '默认档案' : '点击设为默认档案'"
              @click="pickActive(p.id)"
            >
              <div class="sv-card-top">
                <span class="sv-card-icon" :class="{ 'is-img': iconOf(p).startsWith('/') }">
                  <img v-if="iconOf(p).startsWith('/')" :src="iconOf(p)" alt="" />
                  <i v-else class="fa-solid" :class="iconOf(p)" />
                </span>
                <div class="sv-card-name-wrap">
                  <span class="sv-card-name">
                    {{ p.name }}
                    <span class="sv-dot" :class="testDotClass(p)" :title="testDotTitle(p)" />
                  </span>
                  <span class="sv-card-sub num">{{ providerLabel(p) }}<template v-if="p.model"> · {{ p.model }}</template></span>
                </div>
              </div>
              <div class="sv-card-url num" :title="p.apiUrl || '官方默认地址'">
                {{ p.apiUrl || '官方默认地址' }}
              </div>
              <div class="sv-card-foot">
                <span v-if="purposesOf(p.id)" class="sv-card-purposes" :title="purposesOf(p.id)">{{ purposesOf(p.id) }}</span>
                <span v-else class="sv-card-purposes" />
                <span class="sv-card-acts">
                  <button class="ink-icon-btn" title="编辑此档案" @click.stop="edit(p)">
                    <i class="fa-regular fa-pen-to-square" />
                  </button>
                  <button
                    v-if="config.profiles.length > 1"
                    class="ink-icon-btn sv-card-del"
                    title="删除此档案"
                    @click.stop="removeProfile(p.id)"
                  >
                    <i class="fa-regular fa-trash-can" />
                  </button>
                </span>
              </div>
              <span v-if="p.id === config.activeProfile" class="sv-card-badge">默认</span>
            </div>

            <!-- 添加：虚线占位卡 -->
            <button type="button" class="sv-card sv-card--add" @click="addProfile">
              <i class="fa-solid fa-plus" />
              <span>添加档案</span>
            </button>
          </div>
        </section>

        <!-- ── Tab：用途分配 ─────────────────────────────── -->
        <section v-else-if="activeTab === 'purposes'" class="sv-panel">
          <div class="fusion-card sv-purpose-card">
            <div class="card-head">
              <i class="fa-solid fa-shuffle" />
              <span>用途分配</span>
            </div>
            <p class="sv-purpose-sub">
              没单独指定的用途，都用默认档案<template v-if="activeProfileName">「{{ activeProfileName }}」</template>（在「AI 服务商」里点卡片切换）
            </p>
            <div class="sv-purpose-list">
              <div v-for="pu in PURPOSES" :key="pu.key" class="sv-purpose-item">
                <span class="sv-purpose-label">{{ pu.label }}</span>
                <span class="sv-purpose-hint" :title="pu.hint">{{ pu.hint }}</span>
                <select :value="purposeValue(pu.key)" @change="setPurpose(pu.key, ($event.target as HTMLSelectElement).value)">
                  <option value="">跟随默认档案</option>
                  <option v-for="p in config.profiles" :key="p.id" :value="p.id">{{ p.name }}</option>
                </select>
              </div>
            </div>
          </div>

          <!-- 妙笔对话：每轮工具调用上限 -->
          <div class="fusion-card sv-agent-card">
            <div class="card-head">
              <i class="fa-solid fa-wand-magic-sparkles" />
              <span>妙笔对话</span>
            </div>
            <div class="sv-opt-row">
              <div class="sv-opt">
                <span class="sv-opt-label">每轮工具调用上限</span>
                <input
                  class="num"
                  type="number"
                  min="1"
                  :max="AGENT_TURNS_MAX"
                  step="1"
                  :value="agentTurns"
                  @change="setAgentTurns(($event.target as HTMLInputElement).value, $event.target as HTMLInputElement)"
                />
              </div>
              <span class="sv-card-note sv-opt-note">
                一轮对话里最多连续请求工具的次数。默认 40；通读全书、批量改稿等长任务可调高（上限 200）。
                越高越费 token，中途纠偏也越难；改完对妙笔的下一轮对话生效。
              </span>
            </div>
          </div>
        </section>

        <!-- ── Tab：MCP 服务 ─────────────────────────────── -->
        <section v-else class="sv-panel">
          <div class="fusion-card sv-mcp-card">
            <div class="card-head">
              <i class="fa-solid fa-plug" />
              <span>MCP 服务</span>
              <ElSwitch
                class="sv-mcp-switch"
                :model-value="mcpEnabled"
                :loading="mcpBusy"
                :disabled="!workspace.root && !mcpEnabled"
                @update:model-value="mcpToggle($event as boolean)"
              />
            </div>
            <div class="sv-mcp-body">
              <div class="sv-mcp-text">
                <p class="sv-purpose-sub">
                  把当前项目的创作工具集暴露给外部 Agent（ZCode / Claude 等）。
                  仅监听本机；服务跟随当前打开的项目自动切换，Token 持久化在
                  ~/.story-studio/mcp-token，换书、重启都不变——外部客户端配置一次长期有效。
                </p>
                <p class="sv-card-note">端口默认 {{ MCP_PORT_DEFAULT }}；只读模式仅暴露查询类工具，不会改动项目内容。</p>
              </div>
              <div class="sv-mcp-opts">
                <div class="sv-opt">
                  <span class="sv-opt-label">端口</span>
                  <input
                    class="num"
                    type="number"
                    min="1"
                    max="65535"
                    step="1"
                    v-model.number="mcpPortInput"
                    @change="mcpApplySettings"
                  />
                </div>
                <div class="sv-opt">
                  <span class="sv-opt-label">只读模式</span>
                  <ElSwitch
                    :model-value="mcpReadOnly"
                    @update:model-value="((mcpReadOnly = $event as boolean), mcpApplySettings())"
                  />
                </div>
              </div>
            </div>
            <template v-if="mcpEnabled && mcpStatus">
              <div class="sv-mcp-status is-ok">
                <i class="fa-solid fa-circle-check" />
                <span>运行中 · {{ mcpStatus.toolCount }} 个工具</span>
                <ElDropdown trigger="click" class="sv-mcp-copy" @command="mcpCopy">
                  <button class="ink-btn ink-btn--sm">
                    <i class="fa-regular fa-copy" />
                    复制接入配置
                    <i class="fa-solid fa-chevron-down sv-mcp-caret" />
                  </button>
                  <template #dropdown>
                    <ElDropdownMenu>
                      <ElDropdownItem command="json">通用 JSON（mcpServers）</ElDropdownItem>
                      <ElDropdownItem command="zcode">ZCode（mcp.servers）</ElDropdownItem>
                      <ElDropdownItem command="claude-cli">Claude Code 命令行（claude mcp add）</ElDropdownItem>
                    </ElDropdownMenu>
                  </template>
                </ElDropdown>
              </div>
              <div class="sv-mcp-fields">
                <div class="sv-mcp-field">
                  <span class="sv-mcp-field-label">URL</span>
                  <button class="sv-mcp-field-val num" title="点击复制 URL" @click="mcpCopy('url')">
                    <span>{{ mcpStatus.url }}</span>
                    <i class="fa-regular fa-copy" />
                  </button>
                  <span class="sv-mcp-field-spacer" />
                </div>
                <div class="sv-mcp-field">
                  <span class="sv-mcp-field-label">Token</span>
                  <button class="sv-mcp-field-val num" title="点击复制 Token" @click="mcpCopy('token')">
                    <span>{{ mcpTokenShown ? mcpStatus.token : mcpTokenMasked }}</span>
                    <i class="fa-regular fa-copy" />
                  </button>
                  <button
                    class="ink-icon-btn"
                    :title="mcpTokenShown ? '隐藏 Token' : '显示 Token'"
                    @click="mcpTokenShown = !mcpTokenShown"
                  >
                    <i class="fa-regular" :class="mcpTokenShown ? 'fa-eye-slash' : 'fa-eye'" />
                  </button>
                </div>
              </div>
              <p class="sv-card-note sv-mcp-hint">
                通用 JSON 适用于 Claude Code / Cursor / Cline / Cherry Studio 等大多数客户端；
                手动填表单时，请求头填 <code class="num">Authorization: Bearer &lt;Token&gt;</code>。
              </p>
            </template>
            <p v-else-if="!workspace.root" class="sv-mcp-status is-off">
              <i class="fa-regular fa-circle" />
              <span>先打开一个项目，才能启动 MCP 服务</span>
            </p>
          </div>
        </section>
      </template>
    </div>
  </div>
</template>

<style scoped lang="scss">
.settings-view {
  overflow-y: auto;
}

.sv-wrap {
  position: relative;
  z-index: 1;
  max-width: 1080px;
  margin: 0 auto;
  padding: 4vh 32px 48px;
}

.sv-head {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 20px;
  animation: rise 0.5s cubic-bezier(0.22, 1, 0.36, 1) both;

  h1 {
    font-size: 22px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--ink-main);
  }

  p {
    margin-top: 4px;
    font-size: 12px;
  }
}

.sv-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 80px 0;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
  }
}

// ── 顶部分栏 ────────────────────────────────────────────
.sv-tabs {
  display: flex;
  gap: 6px;
  border-bottom: 1px solid var(--divider);
  margin-bottom: 22px;
  animation: rise 0.5s 0.05s cubic-bezier(0.22, 1, 0.36, 1) both;

  button {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 10px 16px 12px;
    margin-bottom: -1px;
    border: none;
    border-bottom: 2px solid transparent;
    background: transparent;
    font-family: inherit;
    font-size: 13.5px;
    color: var(--ink-sec);
    cursor: pointer;
    transition: color 0.2s, border-color 0.2s;

    i {
      font-size: 12px;
      opacity: 0.85;
    }

    &:hover {
      color: var(--ink-main);
    }

    &.is-active {
      color: var(--ink-accent);
      border-bottom-color: var(--ink-accent);
      font-weight: 600;

      i {
        opacity: 1;
      }
    }
  }
}

.sv-panel {
  animation: rise 0.5s 0.1s cubic-bezier(0.22, 1, 0.36, 1) both;
}

// ── 档案卡片列表（cc-switch 式）──────────────────────────
.sv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(292px, 1fr));
  gap: 14px;
}

.sv-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 16px 18px 12px;
  cursor: pointer;
  transition: box-shadow 0.3s cubic-bezier(0.4, 0, 0.2, 1), border-color 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    background 0.2s;

  &:hover {
    border-color: var(--ui-border-hover);
  }

  &.is-default {
    border-color: var(--ink-accent);
    background: color-mix(in srgb, var(--ink-accent-soft) 55%, var(--ui-glass-bg));
  }
}

.sv-card-top {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

// 品牌图标垫白底（暗色主题下深色 logo 也能看清）；图标字体用面层底
.sv-card-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: 8px;
  flex-shrink: 0;

  &.is-img {
    background: #fff;
    border: 1px solid var(--divider);

    img {
      width: 19px;
      height: 19px;
      object-fit: contain;
    }
  }

  &:not(.is-img) {
    background: var(--surface-2);
    color: var(--ink-sec);
    font-size: 13px;
  }
}

.sv-card-name-wrap {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.sv-card-name {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 14px;
  font-weight: 600;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sv-card-sub {
  font-size: 10.5px;
  color: var(--ink-disabled);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sv-card-url {
  font-size: 11px;
  color: var(--ink-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  padding: 5px 9px;
  border-radius: 6px;
  background: var(--surface-2);
}

.sv-card-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: auto;
}

.sv-card-purposes {
  flex: 1;
  min-width: 0;
  font-size: 10.5px;
  color: var(--ink-accent);
  opacity: 0.8;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sv-card-acts {
  display: inline-flex;
  gap: 2px;
  opacity: 0;
  transition: opacity 0.18s;

  // 触屏 / 无悬停设备常显
  @media (hover: none) {
    opacity: 1;
  }
}

.sv-card:hover .sv-card-acts,
.sv-card:focus-within .sv-card-acts {
  opacity: 1;
}

.sv-card-del:hover {
  color: var(--state-danger);
  background: color-mix(in srgb, var(--state-danger) 10%, transparent);
}

// 默认档案角标
.sv-card-badge {
  position: absolute;
  top: 10px;
  right: 12px;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--ink-accent);
  color: #fff;
  font-size: 10px;
  letter-spacing: 0.08em;
  pointer-events: none;
}

// 添加占位卡
.sv-card--add {
  border: 1.5px dashed var(--ui-border);
  background: transparent;
  min-height: 118px;
  align-items: center;
  justify-content: center;
  gap: 8px;
  font-family: inherit;
  font-size: 13px;
  color: var(--ink-muted);
  box-shadow: none;

  i {
    font-size: 18px;
  }

  &:hover {
    color: var(--ink-accent);
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
  }
}

// ── 测试状态点 ──────────────────────────────────────────
.sv-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--ink-disabled);

  &.is-ok {
    background: var(--state-success);
  }

  &.is-fail {
    background: var(--state-danger);
  }
}

// ── 卡片头（各卡片通用）────────────────────────────────
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13.5px;
  color: var(--ink-main);
  padding-bottom: 12px;
  border-bottom: 1px solid var(--divider);
  margin-bottom: 12px;

  > i {
    color: var(--ink-accent);
  }

  > span {
    flex: 1;
  }
}

.sv-purpose-card,
.sv-agent-card,
.sv-mcp-card,
.sv-editor {
  padding: 18px 20px;
}

.sv-head-btn {
  padding: 3px 10px;
  font-size: 11.5px;
}

// ── 用途分配（整行式：名称 / 说明 / 下拉）───────────────
.sv-purpose-card {
  margin-bottom: 18px;
}

.sv-purpose-sub {
  margin: 0 0 12px;
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-muted);
}

.sv-purpose-list {
  display: flex;
  flex-direction: column;
}

.sv-purpose-item {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 2px;
  flex-wrap: wrap;

  & + & {
    border-top: 1px solid var(--divider);
  }

  select {
    width: 230px;
    flex-shrink: 0;
    height: 34px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12.5px;
    outline: none;
    transition: border-color 0.2s;

    &:focus {
      border-color: var(--ink-accent);
    }
  }
}

.sv-purpose-label {
  flex: 0 0 108px;
  font-size: 13px;
  color: var(--ink-main);
}

.sv-purpose-hint {
  flex: 1;
  min-width: 160px;
  font-size: 11.5px;
  color: var(--ink-disabled);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

// ── 设置项小面板（妙笔上限 / MCP 端口等通用行）──────────
.sv-opt-row {
  display: flex;
  align-items: center;
  gap: 16px;
}

.sv-opt {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 10px;
  height: 46px;
  padding: 0 12px;
  border: 1px solid var(--divider);
  border-radius: 9px;
  background: var(--surface-1);
}

.sv-opt-label {
  font-size: 12.5px;
  color: var(--ink-main);
  white-space: nowrap;
}

// 数字输入沿用 .num 的无衬线字体，这里只管外形
.sv-opt input {
  width: 92px;
  flex-shrink: 0;
  height: 32px;
  padding: 0 8px;
  border-radius: 7px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  text-align: center;
  outline: none;
  transition: border-color 0.2s;

  &:focus {
    border-color: var(--ink-accent);
  }
}

// 设置项旁 / 下方的补充说明
.sv-card-note {
  margin: 0;
  font-size: 11px;
  line-height: 1.7;
  color: var(--ink-muted);
}

.sv-opt-note {
  flex: 1;
  min-width: 200px;
}

// ── 编辑区 ──────────────────────────────────────────────
.sv-form {
  display: flex;
  flex-direction: column;
  gap: 13px;
  max-width: 820px;
}

.sv-note {
  display: flex;
  gap: 7px;
  margin: -4px 0 2px;
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    margin-top: 4px;
    flex-shrink: 0;
  }
}

.sv-model-row {
  display: flex;
  gap: 8px;

  > input,
  > select {
    flex: 1;
  }

  button {
    flex-shrink: 0;
    align-self: center;
  }
}

// ── 分组标题：小字 + 右侧延展分隔线 ─────────────────────
.sv-group-title {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 6px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.14em;
  color: var(--ink-disabled);
  user-select: none;

  &::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--divider);
  }
}

.sv-mini {
  margin-top: 5px;
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.sv-switch-field {
  max-width: 130px;
  justify-content: flex-end;

  > label {
    flex: 1;
  }
}

// 生成参数三联字段：均分整行
.sv-gen-field {
  input,
  select {
    text-align: center;
  }
}

// 思考模式占三分之一，与下方参数三联的列线对齐（模型占其余三分之二）
.sv-thinking-field {
  flex: 0 0 calc((100% - 20px) / 3);
}

// ── 服务商卡片网格（编辑表单内）─────────────────────────
.pv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(94px, 1fr));
  gap: 8px;
}

.pv-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 7px;
  padding: 11px 6px 9px;
  border: 1px solid var(--ui-border);
  border-radius: 10px;
  background: var(--input-bg);
  cursor: pointer;
  font-family: inherit;
  transition: all 0.18s;

  &:hover {
    border-color: var(--ui-border-hover);

    .pv-name {
      color: var(--ink-main);
    }
  }

  &.is-picked {
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);

    .pv-name {
      color: var(--ink-main);
    }
  }
}

// 品牌图标垫白底（暗色主题下深色 logo 也能看清）；图标字体用面层底
.pv-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 8px;
  flex-shrink: 0;

  &.is-img {
    background: #fff;
    border: 1px solid var(--divider);

    img {
      width: 18px;
      height: 18px;
      object-fit: contain;
    }
  }

  &:not(.is-img) {
    background: var(--surface-2);
    color: var(--ink-sec);
    font-size: 13px;
  }
}

.pv-name {
  font-size: 11px;
  color: var(--ink-sec);
  text-align: center;
  line-height: 1.4;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  transition: color 0.18s;
}

// ── 接口模式分段选择 ─────────────────────────────────────
.sv-mode-field {
  .sv-mode-seg {
    display: inline-flex;
    width: fit-content;
    border: 1px solid var(--ui-border);
    border-radius: 8px;
    overflow: hidden;
  }

  .sv-mode-btn {
    border: none;
    background: transparent;
    color: var(--ink-muted);
    font-family: inherit;
    font-size: 12px;
    height: 32px;
    padding: 0 18px;
    cursor: pointer;
    transition: all 0.18s;

    & + & {
      border-left: 1px solid var(--ui-border);
    }

    &:hover {
      color: var(--ink-main);
    }

    &.is-on {
      background: var(--ink-accent-soft);
      color: var(--ink-main);
      font-weight: 600;
      box-shadow: inset 0 -2px 0 var(--ink-accent);
    }
  }
}

.sv-collapse {
  border: none;

  :deep(.el-collapse-item__header) {
    font-size: 12px;
    color: var(--ink-muted);
    background: transparent;
    border-bottom-color: var(--divider);
  }

  :deep(.el-collapse-item__wrap) {
    background: transparent;
    border-bottom-color: var(--divider);
  }

  :deep(.el-collapse-item__content) {
    padding: 10px 0 2px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  textarea.num {
    font-size: 12px;
    line-height: 1.7;

    &.is-invalid {
      border-color: var(--state-danger);

      &:focus {
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--state-danger) 15%, transparent);
      }
    }
  }
}

.json-check {
  display: inline-flex;
  align-items: flex-start;
  gap: 5px;
  margin-top: 2px;
  font-size: 11px;
  line-height: 1.6;
  word-break: break-all;

  i {
    margin-top: 3px;
    flex-shrink: 0;
  }

  &.is-invalid {
    color: var(--state-danger);
  }

  &.is-valid {
    color: var(--state-success);
    opacity: 0.8;
  }
}

// ── 测试 ────────────────────────────────────────────────
.sv-test {
  padding: 12px 14px;
  border: 1px dashed var(--divider);
  border-radius: 10px;
}

.sv-test-main {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
}

.sv-test-result {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  flex: 1;
  min-width: 240px;

  > i {
    margin-top: 3px;
    font-size: 13px;
  }

  &.is-ok > i {
    color: var(--state-success);
  }

  &.is-fail > i {
    color: var(--state-danger);
  }
}

.sv-test-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.sv-test-line1 {
  font-size: 12px;
  color: var(--ink-main);
}

.sv-test-detail {
  font-size: 11px;
  line-height: 1.6;
  color: var(--ink-muted);
  word-break: break-word;
}

.sv-test-empty {
  margin: 0;
  font-size: 11.5px;
  color: var(--ink-disabled);
}

// ── 底部动作 ────────────────────────────────────────────
.sv-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px solid var(--divider);
}

.sv-actions-flex {
  flex: 1;
}

.sv-remove {
  color: var(--state-danger);
}

// ── MCP 服务 ────────────────────────────────────────────
.sv-mcp-card {
  .sv-mcp-switch {
    margin-left: auto;
  }

  .sv-mcp-body {
    display: flex;
    gap: 20px;
    align-items: flex-start;
  }

  .sv-mcp-text {
    flex: 1;
    min-width: 260px;

    .sv-card-note {
      margin-top: 4px;
    }
  }

  .sv-mcp-opts {
    flex: 0 0 280px;
    display: flex;
    flex-direction: column;
    gap: 10px;

    .sv-opt {
      flex: 1;
    }
  }

  .sv-mcp-status {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 16px;
    font-size: 12.5px;

    > i {
      font-size: 13px;
    }

    &.is-ok > i {
      color: var(--state-success);
    }

    &.is-off {
      color: var(--ink-muted);
    }
  }

  .sv-mcp-copy {
    margin-left: auto;
  }

  .sv-mcp-caret {
    font-size: 9px;
    margin-left: 2px;
    opacity: 0.7;
  }

  // 接入信息两行：标签 / 可点击复制的值 / 右侧操作位（URL 行用占位保持两行右边对齐）
  .sv-mcp-fields {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }

  .sv-mcp-field {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .sv-mcp-field-label {
    flex: 0 0 44px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--ink-disabled);
  }

  .sv-mcp-field-val {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border: 1px solid var(--ui-border);
    border-radius: 6px;
    background: var(--input-bg);
    color: var(--ink-main);
    font-size: 12px;
    text-align: left;
    cursor: copy;
    transition: border-color 0.2s;

    > span {
      flex: 1;
      min-width: 0;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }

    > i {
      flex-shrink: 0;
      font-size: 12px;
      color: var(--ink-muted);
      transition: color 0.2s;
    }

    &:hover {
      border-color: var(--ink-accent);

      > i {
        color: var(--ink-accent);
      }
    }
  }

  .sv-mcp-field-spacer {
    flex: 0 0 30px;
  }

  .sv-mcp-hint {
    margin-top: 10px;

    code {
      padding: 1px 5px;
      border-radius: 4px;
      background: var(--surface-2);
      font-size: 10.5px;
    }
  }
}

@keyframes rise {
  from {
    opacity: 0;
    transform: translateY(14px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

// 窄窗口：卡片单列、MCP 两栏堆叠
@media (max-width: 760px) {
  .sv-grid {
    grid-template-columns: 1fr;
  }

  .sv-mcp-card .sv-mcp-body {
    flex-direction: column;
  }

  .sv-mcp-card .sv-mcp-opts {
    flex-basis: auto;
  }
}
</style>
