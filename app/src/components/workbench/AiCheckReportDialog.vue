<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'
import type { AiCheckItemDto } from '@/api/backend'
import { useAiCheck } from '@/composables/useAiCheck'
import { requestFragmentReveal } from '@/composables/useFragmentReveal'
import { useMiaoStore } from '@/stores/miao'
import { useWorkspaceStore } from '@/stores/workspace'

// AI 审校报告 — 检测结果给人看的呈现；可勾选条目送妙笔逐条改进，
// 也可以只对照报告自己手改。挂载于 SceneEditor（单一实例，状态在 useAiCheck 单例里）。
const { report, reportOpen, closeReport, dismissReport, sendToMiao } = useAiCheck()
const miao = useMiaoStore()
const store = useWorkspaceStore()

const visible = computed({
  get: () => reportOpen.value && !!report.value,
  set: (v: boolean) => {
    if (!v) closeReport()
  },
})

/** 妙笔会话进行中（含等待回答）：此刻送入的文本会串进当前会话，置灰等待 */
const miaoBusy = computed(() => miao.running)

const SEV_META: Record<string, { label: string; cls: string }> = {
  Strong: { label: '强', cls: 'sev-strong' },
  Medium: { label: '中', cls: 'sev-medium' },
  Weak: { label: '弱', cls: 'sev-weak' },
}

function sevMeta(it: AiCheckItemDto) {
  return SEV_META[it.severity] ?? { label: it.severity, cls: 'sev-weak' }
}

/** 勾选态（报告切换时重置为全选） */
const checkedIdx = ref<Set<number>>(new Set())
watch(
  () => report.value,
  (r) => {
    checkedIdx.value = new Set((r?.result.items ?? []).map((_, i) => i))
  },
)

function toggle(i: number) {
  const next = new Set(checkedIdx.value)
  if (next.has(i)) next.delete(i)
  else next.add(i)
  checkedIdx.value = next
}

const checkedItems = computed(() =>
  (report.value?.result.items ?? []).filter((_, i) => checkedIdx.value.has(i)),
)

function send() {
  if (!report.value || !checkedItems.value.length) return
  sendToMiao(report.value, checkedItems.value)
}

/** 跳到命中位置：必要时切场景，编辑器滚动到引文处并高亮 */
async function jumpToFragment(it: AiCheckItemDto) {
  if (!it.fragmentId) return
  let sceneId: string
  try {
    sceneId = await api.sceneOfFragment(store.root, it.fragmentId)
  } catch (err) {
    ElMessage.error(`定位失败：${(err as Error).message}`)
    return
  }
  // 片段不在当前场景：先切过去，等编辑器挂载完再定位
  if (store.scene?.id !== sceneId) {
    await store.selectScene(sceneId)
    await nextTick()
  }
  closeReport()
  requestFragmentReveal(it.fragmentId, it.quote)
}
</script>

<template>
  <ElDialog v-model="visible" title="AI 审校报告" width="680px" append-to-body>
    <div v-if="report" class="acr">
      <div class="acr-head">
        <div class="acr-scope">
          <i class="fa-solid fa-magnifying-glass" />
          {{ report.label }}
          <span class="acr-count num">{{ report.result.items.length }} 处命中</span>
        </div>
        <p class="acr-summary">{{ report.result.summary }}</p>
      </div>

      <div class="acr-list">
        <div
          v-for="(it, i) in report.result.items"
          :key="i"
          class="acr-item"
          :class="{ 'is-off': !checkedIdx.has(i) }"
        >
          <button class="acr-check" :title="checkedIdx.has(i) ? '取消勾选' : '勾选送妙笔改进'" @click="toggle(i)">
            <i :class="checkedIdx.has(i) ? 'fa-regular fa-square-check' : 'fa-regular fa-square'" />
          </button>
          <span class="acr-sev" :class="sevMeta(it).cls">{{ sevMeta(it).label }}</span>
          <span class="acr-feature">{{ it.featureName }}</span>
          <button
            v-if="it.fragmentId"
            class="acr-frag num"
            :class="{ 'is-uncertain': it.fragmentUncertain }"
            :title="
              it.fragmentUncertain
                ? '引文未能与原文精确对上，片段归属为就近推测——跳转后请以引文实际所在片段为准'
                : '跳到命中位置（必要时切换场景）'
            "
            @click="jumpToFragment(it)"
          >
            <i class="fa-solid fa-location-arrow" />
            {{ it.fragmentId }}{{ it.fragmentUncertain ? '?' : '' }}
          </button>
          <p class="acr-explain">{{ it.explanation }}</p>
          <blockquote class="acr-quote">{{ it.quote }}</blockquote>
        </div>
      </div>
    </div>

    <template #footer>
      <button
        v-if="report"
        class="ink-btn ink-btn--ghost"
        title="关闭并彻底丢弃这份报告（关闭对话框本身不会丢）"
        @click="dismissReport"
      >
        丢弃报告
      </button>
      <span style="flex: 1" />
      <button class="ink-btn" title="只是收起，工具栏可重新打开" @click="closeReport">关闭</button>
      <button
        class="ink-btn ink-btn--primary"
        style="margin-left: 10px"
        :disabled="!checkedItems.length || miaoBusy"
        :title="
          miaoBusy
            ? '妙笔会话还在进行中，等本轮结束（或停止）后再送，避免串进当前会话'
            : '把勾选的命中项格式化为改进指令，填入妙笔输入框'
        "
        @click="send"
      >
        <i class="fa-solid fa-feather-pointed" />
        送妙笔改进（{{ checkedItems.length }}）
      </button>
    </template>
  </ElDialog>
</template>

<style scoped lang="scss">
// 底部三按钮排布：丢弃在左，关闭/送妙笔在右
:deep(.el-dialog__footer) {
  display: flex;
  align-items: center;
}

.acr {
  display: flex;
  flex-direction: column;
  gap: 14px;
  min-height: 120px;
}

.acr-head {
  .acr-scope {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
    font-weight: 600;
    color: var(--ink-main);

    i {
      color: var(--ink-accent);
      font-size: 12px;
    }

    .acr-count {
      margin-left: auto;
      font-size: 11.5px;
      font-weight: 400;
      color: var(--ink-muted);
    }
  }

  .acr-summary {
    margin-top: 6px;
    font-size: 12.5px;
    line-height: 1.8;
    color: var(--ink-sec);
  }
}

.acr-list {
  max-height: 46vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-right: 2px;
}

.acr-item {
  display: grid;
  grid-template-columns: 24px auto auto 1fr;
  grid-template-areas:
    'check sev feature frag'
    'check explain explain explain'
    'check quote quote quote';
  column-gap: 8px;
  row-gap: 4px;
  padding: 10px 12px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);
  transition: opacity 0.2s;

  &.is-off {
    opacity: 0.45;
  }
}

.acr-check {
  grid-area: check;
  align-self: start;
  margin-top: 2px;
  border: none;
  background: transparent;
  color: var(--ink-accent);
  font-size: 15px;
  cursor: pointer;
  padding: 0;

  .fa-square {
    color: var(--ink-disabled);
  }

  &:hover .fa-square {
    color: var(--ink-muted);
  }
}

.acr-sev {
  grid-area: sev;
  align-self: center;
  padding: 1px 8px;
  border-radius: 4px;
  font-size: 11px;
  line-height: 1.6;

  &.sev-strong {
    background: color-mix(in srgb, var(--state-danger) 10%, transparent);
    color: var(--state-danger);
  }

  &.sev-medium {
    background: color-mix(in srgb, var(--state-warning) 12%, transparent);
    color: var(--state-warning);
  }

  &.sev-weak {
    background: var(--surface-2);
    color: var(--ink-muted);
  }
}

.acr-feature {
  grid-area: feature;
  align-self: center;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--ink-main);
}

.acr-frag {
  grid-area: frag;
  justify-self: end;
  align-self: center;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: transparent;
  color: var(--ink-muted);
  font-family: inherit;
  font-size: 11px;
  cursor: pointer;
  transition: all 0.15s;

  i {
    font-size: 9px;
  }

  &:hover {
    color: var(--ink-accent);
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
  }

  // 归属推测：引文与原文对不上时的就近兜底，样式弱化提示核对
  &.is-uncertain {
    border-style: dashed;
    color: var(--state-warning);
  }
}

.acr-explain {
  grid-area: explain;
  font-size: 12.5px;
  line-height: 1.8;
  color: var(--ink-sec);
}

.acr-quote {
  grid-area: quote;
  margin: 0;
  padding: 7px 11px;
  border-left: 3px solid var(--ink-accent);
  border-radius: 0 8px 8px 0;
  background: var(--surface-2);
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-muted);
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
