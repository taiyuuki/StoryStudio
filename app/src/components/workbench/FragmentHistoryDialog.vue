<script setup lang="ts">
import { ref, watch } from 'vue'
import { ElMessageBox } from 'element-plus'
import type { FragmentDiffLineDto, FragmentVersionDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

const props = defineProps<{
  fragmentId: string
}>()

const model = defineModel<boolean>({ default: false })

const store = useWorkspaceStore()
const versions = ref<FragmentVersionDto[]>([])
const selected = ref<FragmentVersionDto | null>(null)
const preview = ref('')
const loading = ref(false)
const restoring = ref(false)

// —— 版本对比：基准 A → 对比 B（可为「当前内容」）——
const compareA = ref<FragmentVersionDto | null>(null)
const compareB = ref<FragmentVersionDto | null>(null)
/** B 端是否为当前磁盘内容（此时 compareB 为 null 但对比已成立） */
const compareBCurrent = ref(false)
const diffLines = ref<FragmentDiffLineDto[]>([])
const diffLoading = ref(false)

const diffActive = ref(false)

watch(model, async (open) => {
  if (!open || !props.fragmentId) return
  loading.value = true
  selected.value = null
  preview.value = ''
  clearCompare()
  try {
    versions.value = await store.loadVersions(props.fragmentId)
  } finally {
    loading.value = false
  }
})

async function pick(v: FragmentVersionDto) {
  selected.value = v
  diffActive.value = false
  preview.value = await store.readVersion(props.fragmentId, v.timestamp)
}

/** 点对比图标：第一次选基准 A，第二次选对比 B；再点 A 取消 */
async function toggleCompare(v: FragmentVersionDto) {
  if (compareA.value?.timestamp === v.timestamp && !compareB.value && !compareBCurrent.value) {
    clearCompare()
    return
  }
  if (!compareA.value || compareB.value || compareBCurrent.value) {
    // 开启新一轮对比
    clearCompare()
    compareA.value = v
    return
  }
  compareB.value = v
  await runDiff()
}

/** 基准版与当前磁盘内容对比 */
async function compareWithCurrent() {
  if (!compareA.value) return
  compareBCurrent.value = true
  await runDiff()
}

async function runDiff() {
  if (!compareA.value) return
  diffLoading.value = true
  diffActive.value = true
  try {
    const bTs = compareBCurrent.value ? null : compareB.value?.timestamp ?? null
    diffLines.value = await store.diffVersion(props.fragmentId, compareA.value.timestamp, bTs)
  } finally {
    diffLoading.value = false
  }
}

function clearCompare() {
  compareA.value = null
  compareB.value = null
  compareBCurrent.value = false
  diffLines.value = []
  diffActive.value = false
}

function compareLabel(): string {
  const a = compareA.value ? formatTime(compareA.value.timestamp) : ''
  const b = compareBCurrent.value
    ? '当前内容'
    : compareB.value
      ? formatTime(compareB.value.timestamp)
      : ''
  return `${a} → ${b}`
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleString('zh-CN', { hour12: false })
}

async function restore() {
  if (!selected.value) return
  try {
    await ElMessageBox.confirm(
      '恢复前会先把现在的内容存进历史，换错了随时能再换回来。要恢复到这一版吗？',
      '恢复版本',
      { type: 'warning', confirmButtonText: '恢复', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  restoring.value = true
  try {
    if (await store.restoreVersion(props.fragmentId, selected.value.timestamp)) {
      model.value = false
    }
  } finally {
    restoring.value = false
  }
}
</script>

<template>
  <ElDialog
    v-model="model"
    title="版本历史"
    width="760px"
    align-center
    append-to-body
    class="history-dialog"
  >
    <div v-loading="loading" class="history-body">
      <div v-if="versions.length === 0 && !loading" class="history-empty">
        <i class="fa-regular fa-clock" />
        <p>暂无历史版本</p>
        <p class="hint">写作时会自动留底：停笔一会儿，或一次改动比较大，就会存下一版</p>
      </div>

      <template v-else>
        <ul class="version-list">
          <li
            v-for="v in versions"
            :key="v.timestamp"
            :class="{ 'is-active': selected?.timestamp === v.timestamp }"
            @click="pick(v)"
          >
            <i class="fa-regular fa-clock" />
            <div class="v-meta">
              <span class="v-time">{{ formatTime(v.timestamp) }}</span>
              <span class="v-words num">{{ v.words.toLocaleString() }} 字</span>
            </div>
            <button
              class="v-diff-btn"
              :class="{ 'is-a': compareA?.timestamp === v.timestamp, 'is-b': compareB?.timestamp === v.timestamp }"
              :title="compareA?.timestamp === v.timestamp && !compareB && !compareBCurrent ? '已选为对比基准，再点取消' : '选为对比版本'"
              @click.stop="toggleCompare(v)"
            >
              {{ compareA?.timestamp === v.timestamp ? 'A' : compareB?.timestamp === v.timestamp ? 'B' : '⇄' }}
            </button>
          </li>
        </ul>
        <div class="version-preview">
          <template v-if="diffActive">
            <div v-loading="diffLoading" class="diff-wrap">
              <div class="diff-head">
                <span class="diff-title"><i class="fa-solid fa-code-compare" /> {{ compareLabel() }}</span>
                <span class="diff-stats">
                  <template v-if="diffLines.length">
                    <span class="add">+{{ diffLines.filter(l => l.kind === 'added').length }}</span>
                    <span class="del">−{{ diffLines.filter(l => l.kind === 'removed').length }}</span>
                  </template>
                </span>
                <button class="diff-clear" @click="clearCompare">退出对比</button>
              </div>
              <div class="diff-body">
                <template v-for="(line, i) in diffLines" :key="i">
                  <div v-if="line.kind === 'skipped'" class="d-line d-skip">
                    ⋯ 未改动 {{ line.count }} 行 ⋯
                  </div>
                  <div v-else :class="['d-line', `d-${line.kind}`]">
                    <span class="d-ln">{{ line.oldLn ?? '' }}</span>
                    <span class="d-ln">{{ line.newLn ?? '' }}</span>
                    <span class="d-sign">{{ line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : '' }}</span>
                    <span class="d-text">{{ line.text || ' ' }}</span>
                  </div>
                </template>
                <p v-if="!diffLoading && diffLines.every(l => l.kind === 'same' || l.kind === 'skipped')" class="diff-same">
                  两版内容一致
                </p>
              </div>
            </div>
          </template>
          <pre v-else-if="preview">{{ preview }}</pre>
          <p v-else class="hint">选择左侧版本查看内容；点 ⇄ 选两版对比差异</p>
        </div>
      </template>
    </div>

    <template #footer>
      <template v-if="compareA && !compareB && !compareBCurrent">
        <button class="ink-btn ink-btn--sm" @click="compareWithCurrent">
          <i class="fa-solid fa-file-lines" />
          与当前内容对比
        </button>
      </template>
      <button class="ink-btn ink-btn--sm" @click="model = false">关闭</button>
      <button
        class="ink-btn ink-btn--sm ink-btn--primary"
        :disabled="!selected || restoring"
        @click="restore"
      >
        <i :class="restoring ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-rotate-left'" />
        恢复此版本
      </button>
    </template>
  </ElDialog>
</template>

<style scoped lang="scss">
.history-body {
  display: flex;
  gap: 14px;
  min-height: 320px;
  max-height: 56vh;
}

.history-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--ink-disabled);

  i {
    font-size: 26px;
  }

  p {
    font-size: 13px;
  }

  .hint {
    font-size: 11.5px;
    max-width: 300px;
    text-align: center;
    line-height: 1.7;
  }
}

.version-list {
  width: 230px;
  flex-shrink: 0;
  margin: 0;
  padding: 0;
  list-style: none;
  overflow-y: auto;
  border-right: 1px solid var(--divider);

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 8px 9px 10px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--ink-muted);
    font-size: 12px;
    transition: background 0.15s;

    &:hover {
      background: var(--surface-2);

      .v-diff-btn {
        opacity: 1;
      }
    }

    &.is-active {
      background: var(--ink-accent-soft);
      color: var(--ink-accent);
    }
  }

  .v-meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .v-time {
    color: inherit;
  }

  .v-words {
    font-size: 11px;
    opacity: 0.7;
  }

  .v-diff-btn {
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    border: 1px solid var(--divider);
    border-radius: 6px;
    background: transparent;
    color: var(--ink-muted);
    font-size: 11px;
    line-height: 1;
    cursor: pointer;
    opacity: 0;
    transition: all 0.15s;

    &:hover {
      border-color: var(--ink-accent);
      color: var(--ink-accent);
    }

    &.is-a,
    &.is-b {
      opacity: 1;
      background: var(--ink-accent);
      border-color: var(--ink-accent);
      color: #fff;
    }
  }
}

.version-preview {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  background: var(--surface-2);
  border-radius: 8px;
  padding: 12px 14px;

  pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: 'Noto Serif SC', 'Songti SC', serif;
    font-size: 13.5px;
    line-height: 2;
    color: var(--ink-main);
  }

  .hint {
    color: var(--ink-disabled);
    font-size: 12px;
    text-align: center;
    padding-top: 120px;
  }
}

// —— 对比视图 ——
.diff-wrap {
  display: flex;
  flex-direction: column;
  min-height: 280px;
}

.diff-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-bottom: 8px;
  margin-bottom: 8px;
  border-bottom: 1px solid var(--divider);
  font-size: 12px;

  .diff-title {
    color: var(--ink-main);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;

    i {
      color: var(--ink-accent);
      margin-right: 4px;
    }
  }

  .diff-stats {
    display: flex;
    gap: 8px;
    font-size: 11.5px;

    .add { color: var(--success, #4a9d6b); }
    .del { color: var(--danger, #c4554d); }
  }

  .diff-clear {
    border: none;
    background: transparent;
    color: var(--ink-muted);
    font-size: 12px;
    cursor: pointer;

    &:hover { color: var(--ink-accent); }
  }
}

.diff-body {
  flex: 1;
  overflow-y: auto;
  font-family: 'Noto Serif SC', 'Songti SC', serif;
  font-size: 13px;
  line-height: 1.9;
}

.d-line {
  display: flex;
  gap: 6px;
  padding: 0 6px;
  border-radius: 3px;
  white-space: pre-wrap;
  word-break: break-word;

  .d-ln {
    flex-shrink: 0;
    width: 26px;
    text-align: right;
    color: var(--ink-disabled);
    font-family: var(--font-mono, monospace);
    font-size: 10.5px;
    line-height: 2.2;
    user-select: none;
  }

  .d-sign {
    flex-shrink: 0;
    width: 12px;
    font-family: var(--font-mono, monospace);
    user-select: none;
  }

  .d-text {
    flex: 1;
    min-width: 0;
  }
}

.d-same { color: var(--ink-muted); }
.d-added {
  background: rgba(74, 157, 107, 0.12);
  color: var(--success, #4a9d6b);

  .d-text { color: var(--ink-main); }
}
.d-removed {
  background: rgba(196, 85, 77, 0.12);
  color: var(--danger, #c4554d);
  text-decoration: line-through;
  text-decoration-color: rgba(196, 85, 77, 0.4);

  .d-text { color: var(--ink-muted); }
}
.d-skip {
  color: var(--ink-disabled);
  font-size: 11.5px;
  text-align: center;
  padding: 2px 0;
  user-select: none;
}

.diff-same {
  text-align: center;
  color: var(--ink-disabled);
  padding-top: 60px;
  font-size: 12.5px;
}
</style>
