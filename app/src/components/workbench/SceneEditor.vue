<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { useAiCheck } from '@/composables/useAiCheck'
import FragmentBlock from './FragmentBlock.vue'
import SceneMetaPopover from './SceneMetaPopover.vue'
import AiWriteDialog from './AiWriteDialog.vue'
import AiCheckReportDialog from './AiCheckReportDialog.vue'

const store = useWorkspaceStore()
const ui = useUiStore()

const scene = computed(() => store.scene)
const aiDialogOpen = ref(false)
const metaOpen = ref(false)

// AI 审校：整场景检测，结果填入妙笔让 Agent 改进
const { checking, run: runAiCheck, report: checkReport, openReport } = useAiCheck()

function checkScene() {
  if (!scene.value) return
  const label = `${scene.value.chapterTitle} · ${scene.value.title || '未命名场景'}`
  void runAiCheck('scene', scene.value.id, label)
}

const povName = computed(() =>
  scene.value?.pov ? store.entityName(scene.value.pov) : '',
)

async function updateTitle(value: string) {
  if (!scene.value) return
  const next = value.trim()
  if (next === (scene.value.title ?? '')) return
  await store.updateScene(scene.value.id, { title: next })
}

// ── 场景摘要（纸面行内编辑，失焦保存） ──────────────────────
const summaryEditing = ref(false)
const summaryDraft = ref('')
const summaryEl = ref<HTMLTextAreaElement | null>(null)

function startSummaryEdit() {
  summaryDraft.value = scene.value?.summary ?? ''
  summaryEditing.value = true
  void nextTick(() => summaryEl.value?.focus())
}

async function saveSummary() {
  if (!summaryEditing.value || !scene.value) return
  summaryEditing.value = false
  const next = summaryDraft.value.trim()
  if (next === (scene.value.summary ?? '')) return
  await store.updateScene(scene.value.id, { summary: next || null })
}
</script>

<template>
  <section class="editor-col">
    <template v-if="scene">
      <div class="editor-toolbar">
        <div class="crumb">
          <span class="crumb-chapter">{{ scene.chapterTitle }}</span>
          <i class="fa-solid fa-angle-right" />
          <span class="crumb-scene">场景</span>
        </div>

        <div class="tool-right">
          <button
            class="ink-btn ink-btn--sm ai-ghost-toggle"
            :class="{ 'is-on': ui.aiGhost }"
            :title="
              ui.aiGhost
                ? 'AI 行内补全已开启：输入停顿时预测下文（灰色幽灵字），Tab 采纳 / Esc 关闭 / Alt+\\ 手动触发。点击关闭'
                : 'AI 行内补全已关闭。点击开启：输入停顿时预测下文，Tab 采纳'
            "
            @click="ui.aiGhost = !ui.aiGhost"
          >
            <i class="fa-solid fa-ghost" />
            {{ ui.aiGhost ? '补全开' : '补全关' }}
          </button>

          <button
            class="ink-btn ink-btn--sm"
            :disabled="checking"
            title="AI 味检测本场景，结果自动填入妙笔由它逐条改进"
            @click="checkScene"
          >
            <i :class="checking ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-magnifying-glass'" />
            {{ checking ? '检测中…' : 'AI 审校' }}
          </button>

          <button
            v-if="checkReport"
            class="ink-btn ink-btn--sm ai-report-btn"
            title="查看最近一次 AI 审校报告（关闭对话框不会丢）"
            @click="openReport"
          >
            <i class="fa-solid fa-clipboard-list" />
            审校报告
            <span class="ai-report-badge num">{{ checkReport.result.items.length }}</span>
          </button>

          <ElPopover v-model:visible="metaOpen" placement="bottom-end" :width="380" trigger="click">
            <template #reference>
              <button class="ink-btn ink-btn--sm" title="场景设定">
                <i class="fa-solid fa-sliders" />
                场景设定
              </button>
            </template>
            <SceneMetaPopover @close="metaOpen = false" />
          </ElPopover>

          <button
            class="ink-btn ink-btn--sm ink-btn--primary"
            :disabled="store.aiWriting"
            @click="aiDialogOpen = true"
          >
            <i :class="store.aiWriting ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-wand-magic-sparkles'" />
            {{ store.aiWriting ? '落墨中…' : '续写' }}
          </button>
        </div>
      </div>

      <div class="paper-scroll">
        <div class="paper">
          <header class="paper-head">
            <div class="paper-chapter seal-font">{{ scene.chapterTitle }}</div>
            <input
              class="scene-title-input"
              :value="scene.title ?? ''"
              placeholder="场景标题…"
              maxlength="80"
              @change="updateTitle(($event.target as HTMLInputElement).value)"
            />
            <div v-if="povName || scene.mood" class="paper-meta">
              <span v-if="povName">
                <i class="fa-solid fa-eye" /> {{ povName }}
              </span>
              <span v-if="scene.mood">
                <i class="fa-solid fa-mug-hot" /> {{ scene.mood }}
              </span>
            </div>

            <!-- 场景摘要：写完顺手补一句，AI 写续与全书回顾会用到 -->
            <div v-if="summaryEditing" class="paper-summary-edit">
              <textarea
                ref="summaryEl"
                v-model="summaryDraft"
                rows="3"
                placeholder="一句话概括这个场景发生了什么（Esc 取消）"
                @blur="saveSummary"
                @keydown.escape.prevent="summaryEditing = false"
              />
            </div>
            <button
              v-else
              class="paper-summary"
              :class="{ 'is-empty': !scene.summary }"
              title="场景摘要——点击编辑"
              @click="startSummaryEdit"
            >
              <i class="fa-solid fa-quote-left" />
              <span>{{ scene.summary || '写一句这场发生了什么…' }}</span>
            </button>
          </header>

          <div class="fragments">
            <FragmentBlock
              v-for="(frag, i) in scene.fragments"
              :key="frag.id"
              :fragment="frag"
              :index="i"
              :total="scene.fragments.length"
            />
            <div v-if="scene.fragments.length === 0 && !store.aiWriting" class="paper-empty">
              <i class="fa-solid fa-feather" />
              <p>场景还是一张白纸</p>
              <button class="ink-btn ink-btn--sm ink-btn--primary" @click="store.insertFragment(null)">
                落笔
              </button>
            </div>

            <!-- AI 流式落墨 -->
            <div v-if="store.aiWriting" class="ai-ghost">
              <div class="ghost-head">
                <i class="fa-solid fa-feather-pointed" />
                妙笔正在落墨…
                <button class="ghost-stop" @click="store.aiAbandonWait()">停止等待</button>
              </div>
              <p class="ghost-text">{{ store.aiStreamText || '正在酝酿文字…' }}</p>
            </div>

            <div v-else-if="scene.fragments.length > 0" class="frag-insert-row">
              <button class="frag-insert" @click="store.insertFragment(null)">
                <i class="fa-solid fa-plus" />
                新段落
              </button>
            </div>
          </div>
        </div>
      </div>

      <footer class="editor-statusbar">
        <span class="sb-item">
          <i class="fa-solid fa-layer-group" />
          {{ scene.fragments.length }} 段
        </span>
        <span class="sb-sep" />
        <span class="sb-item">
          <i class="fa-solid fa-pen-nib" />
          <span class="num">{{ store.sceneWordCount.toLocaleString() }}</span> 字
        </span>
        <span class="sb-sep" />
        <span class="sb-item">
          <i class="fa-solid fa-book" />
          全书 <span class="num">{{ (store.ws?.totalWords ?? 0).toLocaleString() }}</span> 字
        </span>
        <span class="sb-sep" />
        <span
          v-if="store.stats"
          class="sb-item"
          title="今天写了多少字（手写 + AI），点击看统计"
          @click="ui.activePanel = 'stats'"
        >
          <i class="fa-solid fa-feather" />
          今日 <span class="num">{{ ((store.stats.today.manual ?? 0) + (store.stats.today.ai ?? 0)).toLocaleString() }}</span> 字
        </span>
        <span class="sb-flex" />
        <span class="sb-item sb-status">
          <i
            class="fa-solid"
            :class="{
              'fa-circle-check text-success': Object.values(store.fragmentSave).every((s) => s === 'saved'),
              'fa-circle-notch fa-spin': Object.values(store.fragmentSave).some((s) => s === 'saving'),
              'fa-circle-exclamation text-danger': Object.values(store.fragmentSave).some((s) => s === 'error'),
            }"
          />
          {{
            Object.values(store.fragmentSave).some((s) => s === 'error')
              ? '保存失败'
              : Object.values(store.fragmentSave).some((s) => s === 'saving')
                ? '保存中'
                : '已保存'
          }}
        </span>
      </footer>

      <AiWriteDialog v-model="aiDialogOpen" />
      <!-- 审校报告（状态在 useAiCheck 单例，全应用单实例挂载） -->
      <AiCheckReportDialog />
    </template>

    <div v-else class="editor-blank">
      <div class="blank-seal seal-font">墨</div>
      <p class="text-ink-muted">从左侧选择一个场景开始写作</p>
    </div>
  </section>
</template>

<style scoped lang="scss">
.editor-col {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: transparent;
}

// ── 工具条 ──────────────────────────────────────────────
.editor-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 46px;
  padding: 0 16px;
  flex-shrink: 0;
}

.crumb {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-muted);

  i {
    font-size: 9px;
  }
}

.tool-right {
  display: flex;
  gap: 8px;
}

// AI 行内补全开关：开启时以强调色点亮
.ai-ghost-toggle.is-on {
  color: var(--ink-accent);
  border-color: var(--ink-accent);
  background: var(--ink-accent-soft);
}

// 审校报告重开入口：有报告时出现，徽标为命中数
.ai-report-btn {
  position: relative;

  .ai-report-badge {
    padding: 0 5px;
    border-radius: 8px;
    background: var(--ink-accent);
    color: var(--btn-primary-color);
    font-size: 10px;
    line-height: 14px;
  }
}

// ── 纸面 ────────────────────────────────────────────────
.paper-scroll {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
}

.paper {
  max-width: 768px;
  margin: 0 auto;
  padding: 30px 48px 60px;
  background: var(--editor-paper-bg);
  border: 1px solid var(--divider);
  border-radius: 14px;
  box-shadow: var(--ui-shadow);
  min-height: calc(100% - 30px);
}

.paper-head {
  text-align: center;
  margin-bottom: 26px;
  padding-bottom: 18px;
  border-bottom: 1px solid var(--divider);
  position: relative;

  &::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: -1px;
    transform: translateX(-50%);
    width: 56px;
    height: 2px;
    background: var(--editor-rule-color);
  }
}

.paper-chapter {
  font-size: 12px;
  letter-spacing: 0.4em;
  color: var(--ink-accent);
  margin-bottom: 8px;
}

.scene-title-input {
  width: 100%;
  border: none;
  outline: none;
  background: transparent;
  text-align: center;
  font-family: inherit;
  font-size: 22px;
  font-weight: 600;
  color: var(--ink-main);
  padding: 4px 0;

  &::placeholder {
    color: var(--ink-disabled);
  }
}

.paper-meta {
  display: flex;
  justify-content: center;
  gap: 14px;
  margin-top: 8px;
  font-size: 12px;
  color: var(--ink-muted);

  span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
}

// 场景摘要：引文式小字，空态显示引导语，点击进入行内编辑
.paper-summary {
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: 7px;
  max-width: 540px;
  margin: 10px auto 0;
  padding: 2px 10px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 12px;
  line-height: 1.8;
  cursor: pointer;
  transition: background 0.2s, color 0.2s;

  i {
    font-size: 9px;
    color: var(--ink-disabled);
    flex-shrink: 0;
    transform: translateY(-1px);
  }

  span {
    min-width: 0;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    text-align: left;
  }

  &.is-empty {
    color: var(--ink-disabled);
  }

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);

    &.is-empty {
      color: var(--ink-accent);
    }
  }
}

.paper-summary-edit {
  display: flex;
  justify-content: center;
  margin-top: 10px;

  textarea {
    width: 100%;
    max-width: 540px;
    min-height: 118px;
    padding: 11px 14px;
    border-radius: 8px;
    border: 1px solid var(--ink-accent);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12.5px;
    line-height: 1.8;
    resize: vertical;
    outline: none;

    &::placeholder {
      color: var(--ink-disabled);
    }
  }
}

.fragments {
  display: flex;
  flex-direction: column;
}

.paper-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 60px 0 30px;
  color: var(--ink-disabled);

  i {
    font-size: 28px;
  }

  p {
    font-size: 13px;
    letter-spacing: 0.15em;
  }
}

.frag-insert-row {
  display: flex;
  justify-content: center;
  padding: 6px 0 2px;
}

.frag-insert {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 6px 16px;
  border: 1px dashed var(--ui-border);
  border-radius: 20px;
  background: transparent;
  color: var(--ink-disabled);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    color: var(--ink-accent);
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
  }
}

// ── AI 流式落墨 ──────────────────────────────────────────
.ai-ghost {
  margin: 10px 0 4px;
  padding: 16px 18px 18px;
  border: 1px dashed var(--ink-accent);
  border-radius: 12px;
  background: var(--ink-accent-soft);
  animation: ghost-in 0.3s cubic-bezier(0.22, 1, 0.36, 1);

  .ghost-head {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-bottom: 10px;
    font-size: 12px;
    color: var(--ink-accent);
    letter-spacing: 0.1em;

    i {
      animation: float 1.6s ease-in-out infinite;
    }

    .ghost-stop {
      margin-left: auto;
      border: none;
      background: transparent;
      color: var(--ink-muted);
      font-family: inherit;
      font-size: 11.5px;
      letter-spacing: 0.05em;
      cursor: pointer;
      padding: 2px 6px;
      border-radius: 6px;
      transition: all 0.2s;

      &:hover {
        color: var(--state-danger);
        background: color-mix(in srgb, var(--state-danger) 10%, transparent);
      }
    }
  }

  .ghost-text {
    font-size: 17px;
    line-height: 2;
    color: var(--ink-main);
    white-space: pre-wrap;
    word-break: break-word;
    min-height: 34px;
  }
}

@keyframes ghost-in {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

@keyframes float {
  0%,
  100% {
    transform: translateY(0);
  }
  50% {
    transform: translateY(-3px);
  }
}

// ── 状态栏 ──────────────────────────────────────────────
.editor-statusbar {
  display: flex;
  align-items: center;
  gap: 12px;
  height: 34px;
  padding: 0 18px;
  flex-shrink: 0;
  border-top: 1px solid var(--divider);
  background: var(--statusbar-bg);
  font-size: 11.5px;
  color: var(--ink-muted);
}

.sb-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

// 今日码字：点击跳转统计面板
.sb-item[title='今天写了多少字（手写 + AI），点击看统计'] {
  cursor: pointer;
  border-radius: 6px;
  padding: 2px 6px;
  transition: all 0.15s;

  &:hover {
    background: var(--surface-2);
    color: var(--ink-accent);
  }
}

.sb-sep {
  width: 1px;
  height: 12px;
  background: var(--divider);
}

.sb-flex {
  flex: 1;
}

// ── 空白态 ──────────────────────────────────────────────
.editor-blank {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 18px;

  .blank-seal {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 64px;
    height: 64px;
    border-radius: 14px;
    border: 2px solid var(--divider);
    color: var(--ink-disabled);
    font-size: 30px;
  }

  p {
    font-size: 13px;
    letter-spacing: 0.15em;
  }
}
</style>
