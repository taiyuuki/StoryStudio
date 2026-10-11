// 设置页布局预览专用：mock Tauri invoke，仅返回静态数据。不入 src，验证完删除。
const now = Date.now()

const profiles = [
  {
    id: 'p1',
    preset: 'deepseek',
    name: 'DeepSeek V3',
    provider: 'openai',
    model: 'deepseek-chat',
    apiKey: 'sk-9f3e·······························a2c1',
    apiUrl: 'https://api.deepseek.com/v1',
    apiUrlFull: false,
    thinking: 'off',
    timeoutSecs: 300,
    maxTokens: 16384,
    longContext: null,
    apiMode: 'chat',
    extraBody: null,
    extraHeaders: null,
    lastTest: { ok: true, at: now - 55 * 60_000, latencyMs: 412, detail: '模型列表获取成功（14 个模型）' },
  },
  {
    id: 'p2',
    preset: 'zhipu',
    name: 'GLM-4.7 妙笔',
    provider: 'openai',
    model: 'glm-4.7',
    apiKey: '',
    apiUrl: 'https://open.bigmodel.cn/api/paas/v4',
    apiUrlFull: false,
    thinking: 'high',
    timeoutSecs: 600,
    maxTokens: 65536,
    longContext: true,
    apiMode: 'chat',
    extraBody: { thinking: { type: 'enabled' } },
    extraHeaders: null,
    lastTest: { ok: false, at: now - 2 * 3600_000, latencyMs: 183, detail: '401 Unauthorized：API Key 无效或已过期' },
  },
  {
    id: 'p3',
    preset: 'ollama',
    name: '本地 Ollama',
    provider: 'openai',
    model: 'qwen3:14b',
    apiKey: '',
    apiUrl: 'http://127.0.0.1:11434/v1',
    apiUrlFull: true,
    thinking: 'off',
    timeoutSecs: null,
    maxTokens: null,
    longContext: null,
    apiMode: 'chat',
    extraBody: null,
    extraHeaders: { 'X-Title': 'StoryStudio' },
    lastTest: null,
  },
]

const presets = [
  { id: 'openai', name: 'OpenAI', provider: 'openai', apiUrl: 'https://api.openai.com/v1', apiUrlFull: false, defaultModel: 'gpt-5.2', note: '需要科学上网；Responses 模式支持 GPT-5 / o 系列原生接口' },
  { id: 'anthropic', name: 'Anthropic（Claude）', provider: 'anthropic', apiUrl: 'https://api.anthropic.com', apiUrlFull: false, defaultModel: 'claude-sonnet-4-6', note: null },
  { id: 'deepseek', name: 'DeepSeek（深度求索）', provider: 'openai', apiUrl: 'https://api.deepseek.com/v1', apiUrlFull: false, defaultModel: 'deepseek-chat', note: null },
  { id: 'zhipu', name: '智谱 BigModel', provider: 'openai', apiUrl: 'https://open.bigmodel.cn/api/paas/v4', apiUrlFull: false, defaultModel: 'glm-4.7', note: null },
  { id: 'zhipu-coding', name: '智谱·编码包', provider: 'openai', apiUrl: 'https://open.bigmodel.cn/api/coding/paas/v4', apiUrlFull: false, defaultModel: 'glm-4.7', note: '编码包订阅专用线路，需订阅后凭 Key 使用' },
  { id: 'moonshot', name: 'Kimi（月之暗面）', provider: 'openai', apiUrl: 'https://api.moonshot.cn/v1', apiUrlFull: false, defaultModel: 'kimi-k2', note: null },
  { id: 'qwen', name: '通义千问（阿里云百炼）', provider: 'openai', apiUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', apiUrlFull: false, defaultModel: 'qwen3-max', note: null },
  { id: 'openrouter', name: 'OpenRouter', provider: 'openai', apiUrl: 'https://openrouter.ai/api/v1', apiUrlFull: false, defaultModel: null, note: '一个 Key 聚合上百家模型，按量计费' },
  { id: 'siliconflow', name: 'SiliconFlow（硅基流动）', provider: 'openai', apiUrl: 'https://api.siliconflow.cn/v1', apiUrlFull: false, defaultModel: null, note: null },
  { id: 'gemini', name: 'Google Gemini', provider: 'gemini', apiUrl: 'https://generativelanguage.googleapis.com', apiUrlFull: false, defaultModel: 'gemini-3-pro', note: null },
  { id: 'ollama', name: 'Ollama（本地）', provider: 'openai', apiUrl: 'http://127.0.0.1:11434/v1', apiUrlFull: true, defaultModel: null, note: '本地运行免费；需要先安装 Ollama 并拉取模型' },
]

export function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  switch (cmd) {
    case 'get_ai_config':
      return Promise.resolve({
        profiles,
        activeProfile: 'p1',
        purposeProfiles: { write: 'p2', complete: 'p3', check: 'p2' },
        agentMaxToolTurns: 60,
      }) as Promise<T>
    case 'provider_presets':
      return Promise.resolve(presets) as Promise<T>
    case 'mcp_server_status': {
      const running = new URLSearchParams(location.search).has('mcp')
      return Promise.resolve(
        running
          ? { running: true, url: 'http://127.0.0.1:27520/mcp', port: 27520, token: 'preview-token-0f3a9c', readOnly: false, projectDir: '/Users/demo/Documents/我的小说', toolCount: 42 }
          : { running: false, url: '', port: 27520, token: '', readOnly: false, projectDir: '', toolCount: 0 },
      ) as Promise<T>
    }
    case 'set_ai_config':
      return Promise.resolve(undefined as T)
    default:
      console.warn('[preview mock] unhandled invoke:', cmd, args)
      return Promise.resolve([] as unknown as T)
  }
}

export class Channel<T = unknown> {
  onmessage: ((msg: T) => void) | null = null
}
