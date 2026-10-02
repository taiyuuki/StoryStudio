<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { onBeforeRouteLeave, useRouter } from 'vue-router'
import { open } from '@tauri-apps/plugin-dialog'
import { documentDir, homeDir } from '@tauri-apps/api/path'
import { ElMessage, ElMessageBox } from 'element-plus'
import {
  api,
  type BookConceptDto,
  type BookCreationOptionsDto,
  type WorldDraftDto,
  type OutlineBatchDraftDto,
  type WorldSaveReportDto,
} from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'
import { AUDIENCES, GENRES, POVS, SCALES, STYLES, WORD_OPTIONS } from '@/constants/bookMeta'
import ConceptForm from '@/components/start/aiBook/ConceptForm.vue'
import WorldForm from '@/components/start/aiBook/WorldForm.vue'
import OutlineForm from '@/components/start/aiBook/OutlineForm.vue'

const router = useRouter()
const store = useWorkspaceStore()

const idea = ref('')
const genre = ref('自动')
const customGenre = ref('')
const audience = ref('')
const pov = ref('')
const style = ref('')
const scaleIdx = ref(1)
const chapterCount = ref(12)
const chapterWords = ref(3000)

const parentDir = ref('')
const defaultDir = ref('')
const dirHint = computed(() => parentDir.value || defaultDir.value || '默认：文稿/StoryStudio')

onMounted(async () => {
  try {
    const base = (await documentDir()) || (await homeDir())
    if (base) defaultDir.value = `${base.replace(/\/$/, '')}/StoryStudio`
  } catch {
    try {
      const base = await homeDir()
      if (base) defaultDir.value = `${base.replace(/\/$/, '')}/StoryStudio`
    } catch {
      // 静默退回：显示占位提示，运行时校验
    }
  }
})

async function pickDir() {
  const dir = await open({ directory: true, title: '选择新书存放目录' })
  if (typeof dir === 'string') parentDir.value = dir
}

function buildOptions(): BookCreationOptionsDto {
  // 自定义题材优先生效；否则用预设芯片；「自动」= 交给 AI 判断
  const custom = customGenre.value.trim()
  return {
    idea: idea.value.trim(),
    genre: custom || (genre.value === '自动' ? null : genre.value),
    chapterCount: chapterCount.value,
    targetWords: SCALES[scaleIdx.value]?.[1] ?? null,
    chapterWords: chapterWords.value || null,
    audience: audience.value || null,
    pov: pov.value || null,
    style: style.value || null,
  }
}

// ── 向导状态 ────────────────────────────────────────────
type Step = 0 | 1 | 2 | 3 | 4
const step = ref<Step>(0)
const STEP_META: { key: number; label: string }[] = [
  { key: 0, label: '灵感' },
  { key: 1, label: '构思' },
  { key: 2, label: '设定' },
  { key: 3, label: '大纲' },
]

const concepts = ref<BookConceptDto[]>([])
const concept = ref<BookConceptDto>(emptyConcept())
const worldDraft = ref<WorldDraftDto | null>(null)
const worldReport = ref<WorldSaveReportDto | null>(null)
const outlineDraft = ref<OutlineBatchDraftDto | null>(null)
const outlineSaved = ref(0)
const root = ref('')
const generating = ref(false)
const error = ref('')

/** 生成中提示按实际阶段显示（步骤号在请求期间不变，不能作依据） */
type Phase = 'concepts' | 'create' | 'world' | 'outline' | 'save'
const busyPhase = ref<Phase>('concepts')
const PHASE_TEXT: Record<Phase, string> = {
  concepts: '正在想三个候选方向…',
  create: '正在建档立卷…',
  world: '正在搭世界观与人物…',
  outline: '正在排章节脉络…',
  save: '正在保存章节…',
}

function emptyConcept(): BookConceptDto {
  return { title: '', tagline: '', blurb: '', premise: '', theme: '', setting: '' }
}

const busyText = computed(() => PHASE_TEXT[busyPhase.value])

/** 大纲步骤的角色候选（来自第 2 步的设定，输入时可联想） */
const outlineCharNames = computed(() =>
  worldDraft.value ? worldDraft.value.characters.map((c) => c.name).filter(Boolean) : [],
)

async function reset() {
  step.value = 0
  concepts.value = []
  concept.value = emptyConcept()
  worldDraft.value = null
  worldReport.value = null
  outlineDraft.value = null
  outlineSaved.value = 0
  root.value = ''
  error.value = ''
  idea.value = ''
}

// 项目已创建但流程未走完时，离开页面要先确认
onBeforeRouteLeave(async () => {
  if (root.value && step.value < 4) {
    try {
      await ElMessageBox.confirm(
        `项目「${concept.value.title}」已经创建${worldReport.value ? '，设定已保存' : ''}。现在离开的话，没保存的${step.value >= 3 ? '章节' : '设定和章节'}下次要用妙笔补齐。确定离开？`,
        '退出开书',
        { type: 'warning', confirmButtonText: '离开', cancelButtonText: '继续' },
      )
    } catch {
      return false
    }
  }
  if (generating.value) void api.aiBookCancel()
  return true
})

// ── 第 0 → 1 步：生成构思候选 ────────────────────────────
async function generateConcepts() {
  if (!idea.value.trim()) {
    ElMessage.warning('先写下一句话灵感，哪怕很粗糙也行')
    return
  }
  const dir = parentDir.value.trim() || defaultDir.value
  if (!dir) {
    ElMessage.warning('没能找到默认存放位置，请手动选择')
    return
  }
  generating.value = true
  busyPhase.value = 'concepts'
  error.value = ''
  try {
    concepts.value = await api.aiBookConcepts(buildOptions())
    concept.value = concepts.value[0]
    step.value = 1
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

function pickConcept(c: BookConceptDto) {
  concept.value = JSON.parse(JSON.stringify(c))
}

// ── 第 1 → 2 步：确认概念 → 建项目 → 生成设定 ────────────

/** 概念必填项校验，返回错误文案（null = 通过） */
function conceptProblem(): string | null {
  if (!concept.value.title.trim()) return '书名不能为空'
  if (!concept.value.premise.trim()) return '主线不能为空——后续设定和大纲都以此为纲'
  return null
}

async function confirmConcept(reroll: boolean) {
  if (reroll) {
    // 重新想：只重生成候选并留在本步，不建项目、不生成设定
    generating.value = true
    busyPhase.value = 'concepts'
    error.value = ''
    try {
      concepts.value = await api.aiBookConcepts(buildOptions())
      concept.value = concepts.value[0] ?? emptyConcept()
    } catch (err) {
      error.value = (err as Error).message
    } finally {
      generating.value = false
    }
    return
  }

  const problem = conceptProblem()
  if (problem) {
    ElMessage.warning(problem)
    return
  }
  const dir = parentDir.value.trim() || defaultDir.value
  generating.value = true
  busyPhase.value = 'world'
  error.value = ''
  try {
    // 项目一次向导只建一次；设定生成失败重试时不重复建目录
    if (!root.value) {
      root.value = await api.createBookProject(dir, concept.value, buildOptions())
    }
    worldDraft.value = await api.aiBookWorld(concept.value, buildOptions())
    worldReport.value = null
    step.value = 2
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

/** 走到这里就够了：存下构思直接打开书稿，设定和大纲以后在项目里补 */
async function openAfterConcept() {
  const problem = conceptProblem()
  if (problem) {
    ElMessage.warning(problem)
    return
  }
  const dir = parentDir.value.trim() || defaultDir.value
  generating.value = true
  busyPhase.value = 'create'
  error.value = ''
  try {
    root.value = await api.createBookProject(dir, concept.value, buildOptions())
    await openBook()
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

// ── 第 2 → 3 步：保存设定 → 生成首批大纲 ─────────────────
async function confirmWorld(reroll: boolean) {
  if (!worldDraft.value || !root.value) {
    // 正常流程不会走到（项目在确认构思时已创建）；提示而非静默无反应
    ElMessage.warning('项目还没创建——请回到「构思」一步重新确认后再继续')
    return
  }
  generating.value = true
  busyPhase.value = 'outline'
  error.value = ''
  try {
    // 设定只保存一次（重复保存会追加重复实体）；返回修改请在项目里用设定面板
    if (!reroll && !worldReport.value) {
      worldReport.value = await api.saveBookWorld(root.value, worldDraft.value)
    }
    outlineDraft.value = await api.aiBookOutlineBatch(root.value, chapterCount.value)
    step.value = 3
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

// ── 第 3 → 4 步：保存章节 → 完成 ─────────────────────────
async function confirmOutline() {
  if (!outlineDraft.value || !root.value) {
    ElMessage.warning('项目还没创建——请回到「构思」一步重新确认后再继续')
    return
  }
  if (outlineDraft.value.chapters.length === 0) {
    ElMessage.warning('至少要有一章')
    return
  }
  generating.value = true
  busyPhase.value = 'save'
  error.value = ''
  try {
    outlineSaved.value = await api.saveBookOutlineBatch(root.value, outlineDraft.value)
    step.value = 4
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

/** 设定存完就收工：不排大纲，直接打开书稿（大纲以后在项目里续排） */
async function openAfterWorld() {
  if (!worldDraft.value || !root.value) {
    ElMessage.warning('项目还没创建——请回到「构思」一步重新确认后再继续')
    return
  }
  generating.value = true
  busyPhase.value = 'save'
  error.value = ''
  try {
    // 设定只保存一次（重复保存会追加重复实体）；已存过就直接打开
    if (!worldReport.value) {
      worldReport.value = await api.saveBookWorld(root.value, worldDraft.value)
    }
    await openBook()
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

function backStep() {
  error.value = ''
  if (step.value === 2) step.value = 1
  else if (step.value === 3) step.value = 2
}

function cancelGenerating() {
  void api.aiBookCancel()
}

async function openBook() {
  if (await store.openRoot(root.value)) {
    // 中途存完直接打开也算走完全程：置为完成态，避免离开确认拦截
    step.value = 4
    router.push('/workbench')
  }
}

function goBack() {
  router.back()
}
</script>

<template>
  <div class="ink-bg-container create-view">
    <div class="paper-texture" />
    <div class="cb-wrap">
      <!-- 页头 -->
      <header class="cb-head">
        <button class="ink-icon-btn" title="返回" @click="goBack">
          <i class="fa-solid fa-arrow-left" />
        </button>
        <div class="cb-titles">
          <h1>AI 开书</h1>
          <p class="text-ink-muted">一句话灵感，一步步开出一本书；每一步都可以改和重来</p>
        </div>
      </header>

      <div class="fusion-card cb-card">
        <!-- 步骤指示 -->
        <div class="cb-steps">
          <template v-for="(s, i) in STEP_META" :key="s.key">
            <span class="cb-step" :class="{ 'is-done': step > s.key || step === 4, 'is-current': step === s.key && step !== 4 }">
              <span class="cb-step-dot">
                <i v-if="step > s.key || step === 4" class="fa-solid fa-check" />
                <span v-else class="num">{{ i + 1 }}</span>
              </span>
              {{ s.label }}
            </span>
            <span
              v-if="i < STEP_META.length - 1"
              class="cb-step-line"
              :class="{ 'is-done': step > s.key + 1 || step === 4 }"
            />
          </template>
        </div>

        <div class="cb-body">
          <!-- 第 0 步：灵感与参数 -->
          <div v-if="step === 0 && !generating" class="ab-form">
            <!-- 灵感 -->
            <section class="ab-sec">
              <h4 class="ab-sec-title">
                <i class="fa-solid fa-lightbulb" />
                你的灵感
              </h4>
              <div class="idea-box">
                <textarea
                  v-model="idea"
                  rows="4"
                  maxlength="500"
                  placeholder="例：守灯人只剩最后一个徒弟，灯一盏盏熄灭，他要在灯灭前找到真相"
                />
                <span class="idea-count num">{{ idea.length }} / 500</span>
              </div>
            </section>

            <!-- 题材 -->
            <section class="ab-sec">
              <h4 class="ab-sec-title">
                <i class="fa-solid fa-tags" />
                题材
              </h4>
              <div class="genre-chips">
                <button
                  v-for="g in GENRES"
                  :key="g.name"
                  class="genre-chip"
                  :class="{ 'is-picked': !customGenre.trim() && genre === g.name, 'is-auto': g.name === '自动' }"
                  @click="genre = g.name; customGenre = ''"
                >
                  <i class="fa-solid" :class="g.icon" />
                  {{ g.name }}
                </button>
              </div>
              <div class="wz-field">
                <label>自定义题材</label>
                <input
                  v-model="customGenre"
                  maxlength="20"
                  placeholder="预设不合意就写一个，如：无限流、克苏鲁、赛博朋克"
                />
              </div>
              <p class="ab-sec-hint">填了自定义题材就优先用它；否则选预设；都不想定就选「自动」，AI 会顺着灵感自己判断</p>
            </section>

            <!-- 篇幅与节奏 -->
            <section class="ab-sec">
              <h4 class="ab-sec-title">
                <i class="fa-solid fa-sliders" />
                篇幅与节奏
              </h4>
              <div class="wz-row">
                <div class="wz-field">
                  <label>首批章节数</label>
                  <input v-model.number="chapterCount" type="number" min="3" max="40" />
                </div>
                <div class="wz-field">
                  <label>体量</label>
                  <select v-model.number="scaleIdx">
                    <option v-for="(s, i) in SCALES" :key="i" :value="i">{{ s[0] }}</option>
                  </select>
                </div>
              </div>
              <div class="wz-field">
                <label>单章字数</label>
                <div class="word-chips">
                  <button
                    v-for="w in WORD_OPTIONS"
                    :key="w"
                    class="word-chip num"
                    :class="{ 'is-picked': chapterWords === w }"
                    @click="chapterWords = w"
                  >
                    {{ w }} 字
                  </button>
                </div>
              </div>
            </section>

            <!-- 基调与视角 -->
            <section class="ab-sec">
              <h4 class="ab-sec-title">
                <i class="fa-solid fa-compass" />
                基调与视角
              </h4>
              <div class="wz-row">
                <div class="wz-field">
                  <label>面向人群</label>
                  <select v-model="audience">
                    <option value="">不限</option>
                    <option v-for="a in AUDIENCES" :key="a" :value="a">{{ a }}</option>
                  </select>
                </div>
                <div class="wz-field">
                  <label>叙事视角</label>
                  <select v-model="pov">
                    <option value="">不限</option>
                    <option v-for="p in POVS" :key="p" :value="p">{{ p }}</option>
                  </select>
                </div>
              </div>
              <div class="wz-field">
                <label>文风基调</label>
                <select v-model="style">
                  <option value="">不限</option>
                  <option v-for="s in STYLES" :key="s" :value="s">{{ s }}</option>
                </select>
              </div>
            </section>

            <!-- 存放目录 -->
            <section class="ab-sec">
              <h4 class="ab-sec-title">
                <i class="fa-regular fa-folder-open" />
                存放目录（可选）
              </h4>
              <div class="ab-dir">
                <input :value="dirHint" readonly class="ab-dir-input" @click="pickDir" />
                <button class="ink-btn" @click="pickDir">
                  <i class="fa-regular fa-folder-open" />
                  浏览
                </button>
              </div>
            </section>

            <p class="ab-hint">
              <i class="fa-solid fa-circle-info" />
              分四步走：AI 想候选 → 你挑一个改定 → AI 搭设定 → 你增删改后保存 → 排首批章节大纲。不必走完全程——每一步存完都能直接打开开写，剩下的以后在项目里补；长篇也不必一次排完大纲，写一段排一段。
            </p>

            <button class="ink-btn ink-btn--primary ink-btn--lg ab-run" @click="generateConcepts">
              <i class="fa-solid fa-feather-pointed" />
              开始开书
            </button>
            <div v-if="error" class="ab-error-inline">{{ error }}</div>
          </div>

          <!-- 生成中 -->
          <div v-else-if="generating" class="ab-progress">
            <div class="prog-orb"><i class="fa-solid fa-feather-pointed" /></div>
            <p class="prog-text">{{ busyText }}</p>
            <p class="prog-sub">大模型落笔需要一两分钟，可以先去泡杯茶</p>
            <button class="ink-btn ink-btn--sm" @click="cancelGenerating">
              <i class="fa-solid fa-stop" />
              停止
            </button>
          </div>

          <!-- 第 1 步：候选与概念编辑 -->
          <div v-else-if="step === 1" class="step-body">
            <div class="cand-list">
              <button
                v-for="(c, i) in concepts"
                :key="i"
                class="cand"
                :class="{ 'is-picked': concept.title === c.title }"
                @click="pickConcept(c)"
              >
                <span class="cand-title">《{{ c.title }}》</span>
                <span class="cand-tag">{{ c.tagline }}</span>
                <span class="cand-blurb">{{ c.blurb }}</span>
              </button>
            </div>
            <p class="step-tip">点候选换方向，下面直接改；都不满意就重想。</p>
            <ConceptForm v-model="concept" />
            <div class="step-actions">
              <button class="ink-btn ink-btn--sm" @click="step = 0"><i class="fa-solid fa-arrow-left" /> 上一步</button>
              <button class="ink-btn ink-btn--sm" @click="confirmConcept(true)"><i class="fa-solid fa-rotate" /> 重新想</button>
              <button class="ink-btn ink-btn--sm" @click="openAfterConcept"><i class="fa-solid fa-book-open" /> 存下构思，直接打开</button>
              <button class="ink-btn ink-btn--primary" @click="confirmConcept(false)">
                就用这个，开书
                <i class="fa-solid fa-arrow-right" />
              </button>
            </div>
            <div v-if="error" class="ab-error-inline">{{ error }}</div>
          </div>

          <!-- 第 2 步：设定编辑 -->
          <div v-else-if="step === 2 && worldDraft" class="step-body">
            <p class="step-tip">
              增删改随意——名字、关系、伏笔都会原样落盘；保存后随时也能在「设定」「叙事线索」面板里继续改。
            </p>
            <p v-if="worldReport" class="step-tip text-ink-disabled">
              设定此前已保存过，这一步的改动不会重复保存；打开项目后可以在设定面板里修改。
            </p>
            <WorldForm v-model="worldDraft" />
            <div class="step-actions">
              <button class="ink-btn ink-btn--sm" @click="backStep"><i class="fa-solid fa-arrow-left" /> 上一步</button>
              <button class="ink-btn ink-btn--sm" @click="confirmWorld(true)"><i class="fa-solid fa-rotate" /> 重新生成</button>
              <button class="ink-btn ink-btn--sm" @click="openAfterWorld"><i class="fa-solid fa-book-open" /> 保存设定，直接打开</button>
              <button class="ink-btn ink-btn--primary" @click="confirmWorld(false)">
                保存设定，去排大纲
                <i class="fa-solid fa-arrow-right" />
              </button>
            </div>
            <div v-if="error" class="ab-error-inline">{{ error }}</div>
          </div>

          <!-- 第 3 步：大纲编辑 -->
          <div v-else-if="step === 3 && outlineDraft" class="step-body">
            <p class="step-tip">
              先排这 {{ outlineDraft.chapters.length }} 章（长篇以后写一段排一段，在「逐章成书」面板里续排）。
            </p>
            <OutlineForm v-model="outlineDraft" :character-names="outlineCharNames" />
            <div class="step-actions">
              <button class="ink-btn ink-btn--sm" @click="backStep"><i class="fa-solid fa-arrow-left" /> 上一步</button>
              <button class="ink-btn ink-btn--sm" @click="confirmWorld(true)"><i class="fa-solid fa-rotate" /> 重新排</button>
              <button class="ink-btn ink-btn--primary" @click="confirmOutline">
                <i class="fa-solid fa-check" />
                保存这批章节
              </button>
            </div>
            <div v-if="error" class="ab-error-inline">{{ error }}</div>
          </div>

          <!-- 第 4 步：完成 -->
          <div v-else-if="step === 4" class="ab-report">
            <div class="rep-head">
              <span class="rep-seal seal-font">书</span>
              <div class="rep-titles">
                <span class="rep-title">《{{ concept.title }}》</span>
                <span v-if="concept.tagline" class="rep-tagline">{{ concept.tagline }}</span>
              </div>
            </div>
            <p v-if="concept.blurb" class="rep-blurb">{{ concept.blurb }}</p>
            <div class="rep-stats">
              <span v-if="worldReport"><i class="fa-solid fa-users" /> {{ worldReport.characters }} 个角色</span>
              <span v-if="worldReport"><i class="fa-solid fa-route" /> {{ worldReport.threads }} 条线索</span>
              <span><i class="fa-solid fa-list-ol" /> 已排 {{ outlineSaved }} 章</span>
            </div>
            <p class="rep-hint">
              骨架齐了。打开后可以先翻翻设定和章节，然后到右栏「逐章成书」开始写正文；写到后面随时续排大纲。
            </p>
            <div class="rep-actions">
              <button class="ink-btn" @click="reset">再开一本</button>
              <button class="ink-btn ink-btn--primary ink-btn--lg" @click="openBook">
                <i class="fa-solid fa-book-open" />
                打开这本书
              </button>
            </div>
          </div>

          <!-- 其余错误态（非生成中的报错）-->
          <div v-else-if="error" class="ab-error">
            <i class="fa-solid fa-circle-exclamation" />
            <p>{{ error }}</p>
            <button class="ink-btn ink-btn--sm" @click="error = ''">返回重试</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.create-view {
  overflow-y: auto;
}

.cb-wrap {
  position: relative;
  z-index: 1;
  max-width: 860px;
  margin: 0 auto;
  padding: 4vh 32px 48px;
}

.cb-head {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 24px;
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

.cb-card {
  animation: rise 0.5s 0.08s cubic-bezier(0.22, 1, 0.36, 1) both;
}

// ── 步骤条 ──────────────────────────────────────────────
.cb-steps {
  display: flex;
  align-items: center;
  padding: 20px 28px 16px;
  border-bottom: 1px solid var(--divider);
}

.cb-step {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-disabled);
  white-space: nowrap;
  transition: color 0.25s;

  .cb-step-dot {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 1px solid var(--divider);
    font-size: 11px;
    transition: all 0.25s;
  }

  &.is-current {
    color: var(--ink-accent);

    .cb-step-dot {
      border-color: var(--ink-accent);
      color: var(--ink-accent);
      background: var(--ink-accent-soft);
    }
  }

  &.is-done {
    color: var(--ink-muted);

    .cb-step-dot {
      border-color: var(--state-success);
      color: var(--state-success);
    }
  }
}

.cb-step-line {
  flex: 1;
  height: 1px;
  margin: 0 14px;
  background: var(--divider);
  transition: background 0.25s;

  &.is-done {
    background: var(--state-success);
    opacity: 0.55;
  }
}

.cb-body {
  padding: 24px 28px 28px;
}

// ── 第 0 步表单 ─────────────────────────────────────────
.ab-form {
  display: flex;
  flex-direction: column;
  gap: 22px;
}

.ab-sec {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.ab-sec-title {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--ink-main);

  i {
    color: var(--ink-accent);
    font-size: 12.5px;
    width: 16px;
    text-align: center;
  }
}

.ab-sec-hint {
  margin: 0;
  font-size: 11.5px;
  color: var(--ink-disabled);
}

// 灵感输入：大输入框 + 字数角标
.idea-box {
  position: relative;

  textarea {
    width: 100%;
    padding: 13px 15px 26px;
    border-radius: 10px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 14px;
    line-height: 1.9;
    outline: none;
    resize: vertical;
    min-height: 108px;
    transition: border-color 0.2s, box-shadow 0.2s;

    &::placeholder {
      color: var(--ink-disabled);
    }

    &:focus {
      border-color: var(--ink-accent);
      box-shadow: 0 0 0 3px var(--ink-accent-soft);
    }
  }

  .idea-count {
    position: absolute;
    right: 12px;
    bottom: 9px;
    font-size: 10.5px;
    color: var(--ink-disabled);
    pointer-events: none;
  }
}

// 题材芯片
.genre-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.genre-chip {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 15px;
  border-radius: 999px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: all 0.18s;

  i {
    font-size: 12px;
    color: var(--ink-muted);
    transition: color 0.18s;
  }

  &:hover {
    border-color: var(--ui-border-hover);
    color: var(--ink-main);

    i {
      color: var(--ink-main);
    }
  }

  &.is-auto {
    border-style: dashed;
  }

  &.is-picked {
    border-color: var(--ink-accent);
    border-style: solid;
    background: var(--ink-accent-soft);
    color: var(--ink-main);

    i {
      color: var(--ink-accent);
    }
  }
}

// 单章字数芯片
.word-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.word-chip {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 34px;
  min-width: 74px;
  padding: 0 14px;
  border-radius: 999px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-sec);
  font-size: 12.5px;
  cursor: pointer;
  transition: all 0.18s;

  &:hover {
    border-color: var(--ui-border-hover);
    color: var(--ink-main);
  }

  &.is-picked {
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
    color: var(--ink-main);
    font-weight: 600;
  }
}

.ab-dir {
  display: flex;
  gap: 8px;

  .ab-dir-input {
    flex: 1;
    height: 36px;
    padding: 0 11px;
    border-radius: 7px;
    border: 1px dashed var(--ui-border);
    background: transparent;
    color: var(--ink-muted);
    font-size: 13px;
    cursor: pointer;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;

    &:hover {
      border-color: var(--ui-border-hover);
    }
  }
}

.ab-hint {
  display: flex;
  gap: 7px;
  font-size: 11.5px;
  line-height: 1.8;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    margin-top: 4px;
    flex-shrink: 0;
  }
}

.ab-run {
  align-self: center;
  min-width: 220px;
}

// ── 生成中 ──────────────────────────────────────────────
.ab-progress {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 46px 0 26px;
}

.prog-orb {
  position: relative;
  width: 72px;
  height: 72px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--ink-accent-soft);
  color: var(--ink-accent);
  font-size: 26px;
  margin-bottom: 8px;

  &::before {
    content: '';
    position: absolute;
    inset: -8px;
    border: 1.5px dashed var(--ink-accent);
    border-radius: 50%;
    opacity: 0.45;
    animation: orb-spin 16s linear infinite;
  }
}

@keyframes orb-spin {
  to {
    transform: rotate(360deg);
  }
}

.prog-text {
  font-size: 14px;
  color: var(--ink-main);
}

.prog-sub {
  margin: 0 0 8px;
  font-size: 11.5px;
  color: var(--ink-disabled);
}

// ── 步骤内容 ────────────────────────────────────────────
.step-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.step-tip {
  margin: 0;
  font-size: 11.5px;
  color: var(--ink-muted);
  line-height: 1.7;
}

.step-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 0 2px;
  border-top: 1px solid var(--divider);
}

// ── 第 1 步候选 ─────────────────────────────────────────
.cand-list {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;
}

.cand {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 14px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);
  cursor: pointer;
  text-align: left;
  font-family: inherit;
  transition: all 0.2s;

  &:hover {
    border-color: var(--ink-accent);
  }

  &.is-picked {
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
    box-shadow: 0 0 0 1px var(--ink-accent) inset;
  }
}

.cand-title {
  font-size: 14.5px;
  font-weight: 600;
  color: var(--ink-main);
}

.cand-tag {
  font-size: 11px;
  color: var(--ink-accent);
}

.cand-blurb {
  font-size: 11px;
  line-height: 1.7;
  color: var(--ink-muted);
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

// ── 错误 ────────────────────────────────────────────────
.ab-error-inline {
  padding: 8px 12px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--state-danger) 8%, transparent);
  color: var(--state-danger);
  font-size: 12px;
  line-height: 1.7;
  word-break: break-word;
}

.ab-error {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 24px 0 8px;
  text-align: center;

  > i {
    font-size: 26px;
    color: var(--state-danger);
  }

  p {
    font-size: 12.5px;
    line-height: 1.8;
    color: var(--ink-muted);
    max-width: 400px;
    word-break: break-word;
  }
}

// ── 完成报告 ────────────────────────────────────────────
.ab-report {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 12px 0 4px;
}

.rep-head {
  display: flex;
  align-items: center;
  gap: 16px;
}

.rep-seal {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 58px;
  height: 58px;
  border-radius: 14px;
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  font-size: 29px;
  flex-shrink: 0;
  box-shadow: var(--ui-shadow-hover);
}

.rep-titles {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}

.rep-title {
  font-size: 21px;
  font-weight: 600;
  color: var(--ink-main);
}

.rep-tagline {
  font-size: 12.5px;
  color: var(--ink-muted);
}

.rep-blurb {
  margin: 0;
  padding: 12px 16px;
  border-left: 3px solid var(--ink-accent);
  border-radius: 4px;
  background: var(--surface-2);
  font-size: 13px;
  line-height: 1.9;
  color: var(--ink-sec);
}

.rep-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 16px;
  font-size: 12.5px;
  color: var(--ink-muted);

  span {
    display: inline-flex;
    align-items: center;
    gap: 6px;

    i {
      color: var(--ink-accent);
      font-size: 11px;
    }
  }
}

.rep-hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-muted);
}

.rep-actions {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 10px;
  padding-top: 12px;
  border-top: 1px solid var(--divider);
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
</style>
