// 后端 API — invoke 封装 + 全部 DTO 类型（与 src-tauri/src/dto.rs 一一对应）
import { invoke, Channel } from '@tauri-apps/api/core'

// ============================================================================
// 类型
// ============================================================================

export interface ProfileDto {
  race: string | null
  gender: string | null
  occupation: string | null
  birthYear: number | null
  appearance: string | null
  background: string | null
  personality: string | null
  habits: string | null
}

export interface GeographyDto {
  parent: string | null
  kind: string | null
  position: string | null
  features: string | null
  summary: string | null
}

export interface OrgDto {
  kind: string | null
  leader: string | null
  territory: string | null
  ideology: string | null
  summary: string | null
  location: string | null
}

export interface ItemDto {
  kind: string | null
  grade: string | null
  description: string | null
  abilities: string | null
  unique: boolean
}

export type EntityKind = 'character' | 'location' | 'item' | 'organization'

export interface EntityDto {
  id: string
  kind: EntityKind | string
  displayName: string
  symbol: string | null
  aliases: string[]
  profile: ProfileDto | null
  geography: GeographyDto | null
  item: ItemDto | null
  org: OrgDto | null
  attributes: Record<string, string>
}

export interface RelationDto {
  id: string
  from: string
  to: string
  relation: string
}

export interface ThreadDto {
  id: string
  title: string
  description: string | null
  goal: string | null
  priority: 'main' | 'minor' | string
  /** 计划埋设卷号（null = 全书长线） */
  dueVolume: number | null
  tags: string[]
  plantedAt: string | null
  revealedAt: string | null
  resolvedAt: string | null
  abandoned: boolean
}

/** 未锚定指代台账记录（anchoring.toml） */
export interface UnresolvedRefDto {
  fragmentId: string
  quote: string
  hint: string
  /** pending | ignored | resolved */
  status: string
  target: string | null
}

// ── 世界观（概念 / 体系 / 历法；地理与势力走实体） ──────────

export interface ConceptDto {
  name: string
  category: string | null
  summary: string | null
  details: string | null
}

export interface SystemDto {
  name: string
  kind: string | null
  levels: string | null
  rules: string | null
  summary: string | null
}

export interface WorldviewDto {
  concepts: ConceptDto[]
  systems: SystemDto[]
  eras: string[]
}

// ── 章节脉络（chapters.toml：卷分组 + 逐章脉络） ─────────────

export interface VolumeGroupDto {
  name: string
  order: number
  theme: string | null
  summary: string | null
}

export interface ChapterOutlineItemDto {
  /** 对齐 structure.toml 的章节 ID；设计阶段（虚拟章）为 null */
  chapterId: string | null
  title: string
  /** 所属卷序号（1-based）；0 = 未分卷 */
  volumeOrder: number
  summary: string | null
  goal: string | null
  beats: string[]
  /** 涉及角色 ID */
  characters: string[]
  /** 涉及线索 ID */
  threads: string[]
  /** 场景规划（流水线回填，前端只读透传） */
  scenes: string[]
  hook: string | null
  notes: string | null
  /** planned / writing / completed */
  status: string
}

export interface ChaptersOutlineDto {
  volumes: VolumeGroupDto[]
  chapters: ChapterOutlineItemDto[]
}

export interface SceneDto {
  id: string
  chapterId: string
  title: string | null
  pov: string | null
  mood: string | null
  summary: string | null
  fragmentCount: number
  wordCount: number
}

export interface ChapterDto {
  id: string
  title: string
  scenes: SceneDto[]
}

export interface WorkspaceDto {
  root: string
  name: string
  author: string | null
  language: string | null
  bookId: string
  bookTitle: string
  chapters: ChapterDto[]
  entities: EntityDto[]
  relations: RelationDto[]
  threads: ThreadDto[]
  unresolvedRefs: UnresolvedRefDto[]
  totalWords: number
}

export interface AnchorDto {
  entity: string
  role: string
}

export interface FragmentDto {
  id: string
  text: string
  anchors: AnchorDto[]
  wordCount: number
}

export interface StoryTimeDto {
  year: number
  month: number | null
  day: number | null
}

export interface SceneDetailDto {
  id: string
  chapterId: string
  chapterTitle: string
  title: string | null
  pov: string | null
  mood: string | null
  function: string | null
  summary: string | null
  storyTime: StoryTimeDto | null
  fragments: FragmentDto[]
}

/** 三态补丁：undefined=不改 / null=清空 / 值=设置 */
export interface ScenePatch {
  title?: string | null
  pov?: string | null
  mood?: string | null
  function?: string | null
  summary?: string | null
  storyTime?: StoryTimeDto | null
}

export interface DiagDto {
  severity: 'error' | 'warning' | 'hint' | 'info' | string
  code: string
  message: string
  rule: string | null
  entity: string | null
  fragment: string | null
}

export interface StatsDto {
  fragmentCount: number
  entityCount: number
  edgeCount: number
  assertionCount: number
  evidenceCount: number
}

export interface CompileDto {
  ok: boolean
  error: string | null
  diagnostics: DiagDto[]
  stats: StatsDto
}

export interface ProfileTestRecordDto {
  ok: boolean
  /** 测试时间（毫秒时间戳） */
  at: number
  latencyMs: number
  detail: string
}

export interface AiProfileDto {
  id: string
  /** 来源预设 id（deepseek 等；null = 手动配置） */
  preset: string | null
  name: string
  provider: string | null
  model: string | null
  apiKey: string | null
  apiUrl: string | null
  apiUrlFull: boolean | null
  /** 思考深度：auto | off | low | medium | high（auto = 不传参数跟随模型默认） */
  thinking: string | null
  /** 单次生成请求超时（秒）；null = 默认 300 */
  timeoutSecs: number | null
  /** 单次生成的输出 token 上限（含思考型模型的思考配额）；null = 各功能默认值 */
  maxTokens: number | null
  /** 模型是否支持 100 万 token 长上下文；null = 跟随默认预算（影响妙笔对话的记忆压缩时机） */
  longContext: boolean | null
  /** OpenAI 接口模式：chat = Chat Completions（兼容最广）| responses = OpenAI 原生（GPT-5/o 系列）；仅 openai 档案有效 */
  apiMode: 'chat' | 'responses' | null
  /** 私有参数（深合并进请求体，支持 provider 私有字段） */
  extraBody: Record<string, unknown> | null
  /** 自定义请求头（追加，不覆盖认证头） */
  extraHeaders: Record<string, string> | null
  /** 上次连接测试结果 */
  lastTest: ProfileTestRecordDto | null
}

export interface ProviderPresetDto {
  id: string
  name: string
  /** anthropic | openai */
  provider: string
  apiUrl: string | null
  apiUrlFull: boolean
  defaultModel: string | null
  note: string | null
}

export interface AiConfigDto {
  profiles: AiProfileDto[]
  activeProfile: string | null
  /** 用途 → 档案 id（write / agent / workflow…），未列出的用途回退激活档案 */
  purposeProfiles: Record<string, string> | null
}

export interface ExportOutputDto {
  content: string
  filename: string
}

export interface ChapterFileDto {
  filename: string
  content: string
}

export interface AiWriteResultDto {
  fragmentIds: string[]
  tokensUsed: number
}

export interface AiCompleteResultDto {
  text: string
  tokensUsed: number
}

// ── AI 味检测（手动触发） ─────────────────────────────────

export interface AiCheckItemDto {
  /** 命中处原文引用 */
  quote: string
  /** 特征编号（如 "A"、"14"） */
  featureId: string
  /** 特征名称 */
  featureName: string
  /** Strong | Medium | Weak */
  severity: 'Strong' | 'Medium' | 'Weak' | string
  /** 一两句话说明问题 */
  explanation: string
  /** 命中所在片段（单片段检测时为被检片段） */
  fragmentId?: string
}

export interface AiCheckResultDto {
  /** "fragment" | "scene" | "chapter" */
  scope: string
  scopeId: string
  summary: string
  tokensUsed: number
  items: AiCheckItemDto[]
}

// ── 片段版本历史 / 码字统计 / AI 账本 ─────────────────────

export interface FragmentVersionDto {
  /** 快照时间（毫秒时间戳） */
  timestamp: number
  words: number
  bytes: number
}

export interface DayCountDto {
  manual: number
  ai: number
}

export interface RecentDayDto {
  date: string
  count: DayCountDto
}

export interface WordStatsSummaryDto {
  dailyGoal: number
  today: DayCountDto
  streakDays: number
  /** 近 7 天（旧 → 新，空日补零） */
  recentDays: RecentDayDto[]
  total: DayCountDto
}

export interface AiCallRecordDto {
  ts: number
  scene: string
  provider: string
  tokens: number
  durationMs: number
  ok: boolean
  error: string | null
}

export interface AiLedgerSummaryDto {
  totalCalls: number
  totalTokens: number
  totalDurationMs: number
  /** 最近若干条（新 → 旧） */
  recent: AiCallRecordDto[]
}

// ── Agent / 流式 ──────────────────────────────────────────

export interface WriteStreamMsg {
  kind: 'delta' | 'done' | string
  text: string
}

export type AgentEventKind =
  | 'user'
  | 'ai'
  | 'toolCall'
  | 'toolResult'
  | 'error'
  | 'plan'
  | 'planProgress'
  | 'reasoning'
  | 'compacted'
  | 'ask'
  | 'retrying'

export interface AgentEventDto {
  kind: AgentEventKind | string
  text: string
  tool: string | null
  detail: string | null
  options: string[]
  promptId: string | null
  extra: Record<string, unknown> | null
}

export interface AgentTraceDto {
  totalTokens: number
  toolCallCount: number
}

// ── Git 版本档案 ───────────────────────────────────────────

export interface GitStatusDto {
  /** 系统是否装有 git */
  available: boolean
  /** 项目目录本身是否已是 git 仓库 */
  initialized: boolean
  /** 是否嵌在别的 git 仓库里（不接管，仅提示） */
  nested: boolean
  /** 有无未存档变更 */
  dirty: boolean
}

export interface GitCommitDto {
  /** 短哈希 */
  id: string
  /** Unix 时间戳（秒） */
  timestamp: number
  message: string
}

export interface GitCommitResultDto {
  /** 是否真的产生了新提交（无变更时 false） */
  committed: boolean
  id: string | null
  detail: string
}

// ── 妙笔会话历史 ──────────────────────────────────────────

export interface ChatMetaDto {
  id: string
  title: string
  updatedAt: number
}

export interface ChatFileDto {
  id: string
  title: string
  updatedAt: number
  /** 时间线条目（前端自有结构，原样存取） */
  items: Record<string, unknown>[]
}

// ── AI 建书（分步向导）──────────────────────────────────────

export interface BookCreationOptionsDto {
  idea: string
  genre: string | null
  chapterCount: number
  targetWords: number | null
  chapterWords: number | null
  audience: string | null
  pov: string | null
  style: string | null
  characterCount: number | null
}

export interface BookConceptDto {
  title: string
  tagline: string
  blurb: string
  premise: string
  theme: string
  setting: string
}

export interface ConceptItemDto {
  name: string
  category: string
  summary: string
}

export interface SystemItemDto {
  name: string
  kind: string
  levels: string
  rules: string
  summary: string
}

export interface CharacterItemDto {
  name: string
  role: string
  gender: string
  occupation: string
  personality: string
  background: string
  aliases: string[]
}

export interface LocationItemDto {
  name: string
  kind: string
  /** 上级地点名（保存时解析为 Location 实体 ID） */
  parent: string
  features: string
  summary: string
}

export interface OrganizationItemDto {
  name: string
  kind: string
  leader: string
  territory: string
  ideology: string
  summary: string
}

export interface RelationItemDto {
  from: string
  to: string
  relation: string
}

export interface ThreadItemDto {
  title: string
  description: string
  priority: string
}

export interface WorldDraftDto {
  concepts: ConceptItemDto[]
  systems: SystemItemDto[]
  eras: string[]
  characters: CharacterItemDto[]
  locations: LocationItemDto[]
  organizations: OrganizationItemDto[]
  relations: RelationItemDto[]
  threads: ThreadItemDto[]
}

export interface WorldSaveReportDto {
  characters: number
  locations: number
  organizations: number
  relations: number
  threads: number
}

export interface VolumeItemDto {
  name: string
  order: number
  theme: string
  summary: string
}

export interface ChapterItemDto {
  title: string
  volumeOrder: number
  summary: string
  goal: string
  beats: string[]
  characters: string[]
  hook: string
}

export interface OutlineBatchDraftDto {
  volumes: VolumeItemDto[]
  chapters: ChapterItemDto[]
}

// ── 逐章成书流水线 ─────────────────────────────────────────

export interface WorkflowIssueDto {
  /** word_count | truncated | dup_paragraph | compile_error | compile_warning */
  code: string
  /** 硬伤（需要停下来等作者决定） */
  hard: boolean
  message: string
}

export interface WorkflowEventDto {
  /** runStarted | chapterStarted | beats | materialized | delta
   *  | committed | qualityCheck | chapterCompleted | paused | cancelled | finished */
  kind: string
  index: number | null
  title: string | null
  text: string | null
  beats: string[]
  words: number | null
  passed: boolean | null
  issues: WorkflowIssueDto[]
  pending: number | null
  resume: boolean | null
  completedThisRun: number | null
  remaining: number | null
}

export interface WorkflowReportDto {
  /** done | paused | cancelled | batch_limit */
  outcome: string
  completedThisRun: number
  remaining: number
  pausedIndex: number | null
  pausedTitle: string | null
}

export interface WorkflowStateDto {
  /** idle | paused | done */
  phase: string
  pausedIndex: number | null
  pausedTitle: string | null
  pausedIssues: WorkflowIssueDto[]
  targetWords: number
}

export interface ChapterProgressDto {
  index: number
  title: string
  /** planned | writing | completed */
  status: string
}

// ============================================================================
// invoke 封装 — 统一错误为 Error
// ============================================================================

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args)
  } catch (err) {
    throw new Error(typeof err === 'string' ? err : String(err))
  }
}

// ── 项目 ────────────────────────────────────────────────────
export const api = {
  createProject(parentDir: string, bookName: string) {
    return call<string>('create_project', { parentDir, bookName })
  },
  openProject(root: string) {
    return call<WorkspaceDto>('open_project', { root })
  },
  refreshWorkspace(root: string) {
    return call<WorkspaceDto>('refresh_workspace', { root })
  },

  // ── 结构 ──────────────────────────────────────────────
  createChapter(root: string, title: string) {
    return call<ChapterDto>('create_chapter', { root, title })
  },
  renameChapter(root: string, chapterId: string, title: string) {
    return call<void>('rename_chapter', { root, chapterId, title })
  },
  deleteChapter(root: string, chapterId: string) {
    return call<void>('delete_chapter', { root, chapterId })
  },
  createScene(root: string, chapterId: string, title: string, pov?: string | null) {
    return call<SceneDetailDto>('create_scene', { root, chapterId, title, pov: pov ?? null })
  },
  renameScene(root: string, sceneId: string, title: string) {
    return call<void>('rename_scene', { root, sceneId, title })
  },
  deleteScene(root: string, sceneId: string) {
    return call<void>('delete_scene', { root, sceneId })
  },
  updateScene(root: string, sceneId: string, patch: ScenePatch) {
    return call<string[]>('update_scene', { root, sceneId, patch })
  },
  getScene(root: string, sceneId: string) {
    return call<SceneDetailDto>('get_scene', { root, sceneId })
  },
  insertFragment(root: string, sceneId: string, afterId: string | null, text: string) {
    return call<FragmentDto>('insert_fragment', { root, sceneId, afterId, text })
  },
  writeFragment(root: string, fragmentId: string, text: string) {
    return call<void>('write_fragment', { root, fragmentId, text })
  },
  deleteFragment(root: string, sceneId: string, fragmentId: string) {
    return call<void>('delete_fragment', { root, sceneId, fragmentId })
  },
  moveFragment(root: string, sceneId: string, fragmentId: string, newIndex: number) {
    return call<void>('move_fragment', { root, sceneId, fragmentId, newIndex })
  },
  fragmentVersions(root: string, fragmentId: string) {
    return call<FragmentVersionDto[]>('fragment_versions', { root, fragmentId })
  },
  fragmentVersionRead(root: string, fragmentId: string, timestamp: number) {
    return call<string>('fragment_version_read', { root, fragmentId, timestamp })
  },
  fragmentRestoreVersion(root: string, fragmentId: string, timestamp: number) {
    return call<FragmentDto>('fragment_restore_version', { root, fragmentId, timestamp })
  },
  statsSummary(root: string) {
    return call<WordStatsSummaryDto>('stats_summary', { root })
  },
  statsSetGoal(root: string, goal: number) {
    return call<void>('stats_set_goal', { root, goal })
  },
  statsSyncClock(root: string, tzOffsetMin: number) {
    return call<void>('stats_sync_clock', { root, tzOffsetMin })
  },
  aiLedgerSummary(root: string) {
    return call<AiLedgerSummaryDto>('ai_ledger_summary', { root })
  },

  // ── 实体 / 关系 ───────────────────────────────────────
  createEntity(root: string, kind: EntityKind, name: string) {
    return call<EntityDto>('create_entity', { root, kind, name })
  },
  deleteEntity(root: string, entityId: string) {
    return call<void>('delete_entity', { root, entityId })
  },
  saveEntity(root: string, entity: EntityDto) {
    return call<EntityDto>('save_entity', { root, entity })
  },
  mergeEntities(root: string, fromEntityId: string, intoEntityId: string) {
    return call<string>('merge_entities', { root, fromEntityId, intoEntityId })
  },
  resolveUnresolved(
    root: string,
    fragmentId: string,
    quote: string,
    action: 'create' | 'merge' | 'ignore',
    opts?: { targetEntityId?: string; name?: string; kind?: EntityKind },
  ) {
    return call<string>('resolve_unresolved', {
      root,
      fragmentId,
      quote,
      action,
      targetEntityId: opts?.targetEntityId,
      name: opts?.name,
      kind: opts?.kind,
    })
  },
  addRelation(root: string, from: string, to: string, relation: string) {
    return call<RelationDto>('add_relation', { root, from, to, relation })
  },
  removeRelation(root: string, relationId: string) {
    return call<void>('remove_relation', { root, relationId })
  },

  // ── 叙事线索 ──────────────────────────────────────────
  createThread(
    root: string,
    title: string,
    description: string,
    priority: string,
    dueVolume?: number | null,
  ) {
    return call<ThreadDto>('create_thread', {
      root,
      title,
      description,
      priority,
      dueVolume: dueVolume ?? null,
    })
  },
  updateThread(
    root: string,
    threadId: string,
    patch: {
      title?: string
      description?: string
      goal?: string
      priority?: string
      /** 传 0 表示清除（恢复全书长线） */
      dueVolume?: number
    },
  ) {
    return call<void>('update_thread', { root, threadId, ...patch })
  },
  deleteThread(root: string, threadId: string) {
    return call<void>('delete_thread', { root, threadId })
  },

  // ── 世界观 ────────────────────────────────────────────
  getWorldview(root: string) {
    return call<WorldviewDto>('get_worldview', { root })
  },
  saveWorldview(root: string, worldview: WorldviewDto) {
    return call<void>('save_worldview', { root, worldview })
  },

  // ── 章节脉络 ──────────────────────────────────────────
  getChapterOutline(root: string) {
    return call<ChaptersOutlineDto>('get_chapter_outline', { root })
  },
  saveChapterOutline(root: string, outline: ChaptersOutlineDto) {
    return call<void>('save_chapter_outline', { root, outline })
  },

  // ── 编译 / 导出 ───────────────────────────────────────
  compileProject(root: string) {
    return call<CompileDto>('compile_project', { root })
  },
  exportMarkdown(
    root: string,
    includeAppendix: boolean,
    sceneTitles: boolean,
    chapterNumbering: boolean,
  ) {
    return call<ExportOutputDto>('export_markdown', {
      root,
      includeAppendix,
      sceneTitles,
      chapterNumbering,
    })
  },
  exportChapters(
    root: string,
    includeAppendix: boolean,
    sceneTitles: boolean,
    chapterNumbering: boolean,
  ) {
    return call<ChapterFileDto[]>('export_chapters', {
      root,
      includeAppendix,
      sceneTitles,
      chapterNumbering,
    })
  },
  writeTextFile(path: string, content: string) {
    return call<void>('write_text_file', { path, content })
  },

  // ── AI ────────────────────────────────────────────────
  getAiConfig() {
    return call<AiConfigDto>('get_ai_config')
  },
  setAiConfig(config: AiConfigDto) {
    return call<void>('set_ai_config', { config })
  },
  /** 内置服务商预设 */
  providerPresets() {
    return call<ProviderPresetDto[]>('provider_presets')
  },
  /** 拉取模型列表（用编辑中的档案参数，不必先保存） */
  /** 测试档案连通性（先试零消耗的模型列表，失败退最小生成调用） */
  aiTestProfile(profile: AiProfileDto) {
    return call<ProfileTestRecordDto>('ai_test_profile', { profile })
  },
  aiListModels(
    provider: string,
    apiUrl: string | null,
    apiUrlFull: boolean,
    apiKey: string | null,
  ) {
    return call<string[]>('ai_list_models', { provider, apiUrl, apiUrlFull, apiKey })
  },
  /** 流式续写：增量与完成信号均经 Channel 推送（done.text 为新段落 id 逗号分隔），
   *  返回 Promise 作兜底；两条通路谁先到谁生效 */
  aiWriteScene(
    root: string,
    sceneId: string,
    instruction: string | null,
    onEvent: (msg: WriteStreamMsg) => void,
  ) {
    const channel = new Channel<WriteStreamMsg>()
    channel.onmessage = onEvent
    return call<AiWriteResultDto>('ai_write_scene', {
      root,
      sceneId,
      instruction,
      onEvent: channel,
    })
  },
  /** 行内补全结果（幽灵补全，不落盘） */
  aiCompleteFragment(
    root: string,
    fragmentId: string,
    before: string,
    after: string | null,
    onDelta: (delta: string) => void,
  ) {
    const channel = new Channel<WriteStreamMsg>()
    channel.onmessage = (msg) => {
      if (msg.kind === 'delta') onDelta(msg.text)
    }
    return call<AiCompleteResultDto>('ai_complete_fragment', {
      root,
      fragmentId,
      before,
      after,
      onEvent: channel,
    })
  },
  /** 取消进行中的行内补全（继续打字 / 关闭幽灵时） */
  aiCompleteCancel() {
    return call<boolean>('ai_complete_cancel')
  },
  /** AI 味检测（单片段 / 整场景 / 整章），结果结构化回传 */
  aiCheck(root: string, scope: 'fragment' | 'scene' | 'chapter', id: string) {
    return call<AiCheckResultDto>('ai_check', { root, scope, id })
  },
  /** 定位片段所属场景（审校报告精确跳转用） */
  sceneOfFragment(root: string, fragmentId: string) {
    return call<string>('scene_of_fragment', { root, fragmentId })
  },
  /** Agent 对话：步骤经回调实时推送 */
  agentChat(
    root: string,
    sessionId: string,
    message: string,
    onEvent: (ev: AgentEventDto) => void,
  ) {
    const channel = new Channel<AgentEventDto>()
    channel.onmessage = onEvent
    return call<AgentTraceDto>('agent_chat', { root, sessionId, message, onEvent: channel })
  },
  agentRespond(sessionId: string, answer: string) {
    return call<boolean>('agent_respond', { sessionId, answer })
  },
  agentCancel(sessionId: string) {
    return call<boolean>('agent_cancel', { sessionId })
  },
  agentDropSession(sessionId: string) {
    return call<void>('agent_drop_session', { sessionId })
  },

  // ── 妙笔会话历史 ─────────────────────────────────────
  chatList(root: string) {
    return call<ChatMetaDto[]>('chat_list', { root })
  },
  chatLoad(root: string, id: string) {
    return call<ChatFileDto>('chat_load', { root, id })
  },
  chatSave(root: string, chat: ChatFileDto) {
    return call<void>('chat_save', { root, chat })
  },
  chatDelete(root: string, id: string) {
    return call<void>('chat_delete', { root, id })
  },

  // ── Git 版本档案 ────────────────────────────────────────
  gitStatus(root: string) {
    return call<GitStatusDto>('git_status', { root })
  },
  gitInit(root: string) {
    return call<GitStatusDto>('git_init', { root })
  },
  gitCommit(root: string, message?: string) {
    return call<GitCommitResultDto>('git_commit', { root, message: message ?? null })
  },
  gitLog(root: string, limit?: number) {
    return call<GitCommitDto[]>('git_log', { root, limit: limit ?? 50 })
  },
  gitRestore(root: string, commitId: string) {
    return call<GitCommitResultDto>('git_restore', { root, commitId })
  },

  // ── AI 建书（分步向导）──────────────────────────────────
  /** 第 1 步：构思候选（3 个方向） */
  aiBookConcepts(options: BookCreationOptionsDto) {
    return call<BookConceptDto[]>('ai_book_concepts', { options })
  },
  /** 第 2 步：确认概念后建项目，返回根目录 */
  createBookProject(parentDir: string, concept: BookConceptDto, options: BookCreationOptionsDto) {
    return call<string>('create_book_project', { parentDir, concept, options })
  },
  /** 第 3 步：生成设定草稿（可编辑，保存前不落盘） */
  aiBookWorld(concept: BookConceptDto, options: BookCreationOptionsDto) {
    return call<WorldDraftDto>('ai_book_world', { concept, options })
  },
  /** 第 3 步确认：保存设定 */
  saveBookWorld(root: string, draft: WorldDraftDto) {
    return call<WorldSaveReportDto>('save_book_world', { root, draft })
  },
  /** 第 4 步 / 续排：生成下一批章节脉络草稿 */
  aiBookOutlineBatch(root: string, count: number) {
    return call<OutlineBatchDraftDto>('ai_book_outline_batch', { root, count })
  },
  /** 保存章节批次（追加 chapters.toml） */
  saveBookOutlineBatch(root: string, draft: OutlineBatchDraftDto) {
    return call<number>('save_book_outline_batch', { root, draft })
  },
  aiBookCancel() {
    return call<boolean>('ai_book_cancel')
  },

  // ── 逐章成书流水线 ─────────────────────────────────────
  /** 运行流水线：事件经 Channel 推送（delta 为正文增量），返回值作兜底 */
  workflowRun(
    root: string,
    action: 'run' | 'approve' | 'rewrite' | 'skip',
    targetWords: number | null,
    maxChapters: number | null,
    onEvent: (ev: WorkflowEventDto) => void,
  ) {
    const channel = new Channel<WorkflowEventDto>()
    channel.onmessage = onEvent
    return call<WorkflowReportDto>('workflow_run', {
      root,
      action,
      targetWords,
      maxChapters,
      onEvent: channel,
    })
  },
  workflowCancel() {
    return call<boolean>('workflow_cancel')
  },
  workflowStatus(root: string) {
    return call<WorkflowStateDto>('workflow_status', { root })
  },
  chapterProgress(root: string) {
    return call<ChapterProgressDto[]>('chapter_progress', { root })
  },
}
