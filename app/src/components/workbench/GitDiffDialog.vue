<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'
import type { GitDiffDetailDto, GitDiffFileDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

// 版本档案的改动对比：文件级统计 + 点开单文件看增删行。
// target 由 GitPanel 设置：from → to（to 为 null 表示对比工作区未存档改动）。
export interface GitDiffTarget {
  fromId: string
  fromLabel: string
  toId: string | null
  toLabel: string
}

const props = defineProps<{ target: GitDiffTarget | null }>()
const emit = defineEmits<{ 'update:target': [GitDiffTarget | null] }>()

const store = useWorkspaceStore()

const visible = computed({
  get: () => !!props.target,
  set: (v: boolean) => {
    if (!v) emit('update:target', null)
  },
})

const files = ref<GitDiffFileDto[]>([])
const loading = ref(false)
/** 展开态 + 明细缓存（path → detail） */
const expanded = ref<Set<string>>(new Set())
const details = ref<Map<string, GitDiffDetailDto>>(new Map())
const detailLoading = ref<string | null>(null)

watch(
  () => props.target,
  (t) => {
    expanded.value = new Set()
    details.value = new Map()
    files.value = []
    if (t) void loadSummary()
  },
)

async function loadSummary() {
  if (!store.root || !props.target) return
  loading.value = true
  try {
    files.value = await api.gitDiffSummary(
      store.root,
      props.target.fromId,
      props.target.toId,
    )
    // 默认展开第一个片段类文件（最常看）
    const firstFrag = files.value.find((f) => f.path.startsWith('fragments/'))
    if (firstFrag) void toggle(firstFrag)
  } catch (err) {
    ElMessage.error(`读取改动失败：${(err as Error).message}`)
  } finally {
    loading.value = false
  }
}

async function toggle(file: GitDiffFileDto) {
  if (expanded.value.has(file.path)) {
    expanded.value.delete(file.path)
    expanded.value = new Set(expanded.value)
    return
  }
  expanded.value.add(file.path)
  expanded.value = new Set(expanded.value)
  if (details.value.has(file.path) || file.status === 'D') return // 删除文件没有正文可看
  if (!store.root || !props.target) return
  detailLoading.value = file.path
  try {
    const detail = await api.gitDiffFile(
      store.root,
      props.target.fromId,
      props.target.toId,
      file.path,
    )
    details.value.set(file.path, detail)
    details.value = new Map(details.value)
  } catch (err) {
    ElMessage.error(`读取文件改动失败：${(err as Error).message}`)
  } finally {
    detailLoading.value = null
  }
}

const STATUS_META: Record<string, { label: string; cls: string }> = {
  A: { label: '新增', cls: 'st-add' },
  M: { label: '修改', cls: 'st-mod' },
  D: { label: '删除', cls: 'st-del' },
}

function statusMeta(s: string) {
  return STATUS_META[s] ?? { label: s, cls: 'st-mod' }
}

/** fragments/f_0001.md → f_0001 · 正文；entities/…/c_01.toml → c_01 · 设定 */
function fileLabel(path: string): { name: string; kind: string } {
  const base = path.split('/').pop() ?? path
  const id = base.replace(/\.(md|toml)$/, '')
  if (path.startsWith('fragments/')) return { name: id, kind: '正文' }
  if (path.startsWith('entities/')) return { name: id, kind: '设定' }
  if (path === 'structure.toml') return { name: id, kind: '结构' }
  if (path === 'chapters.toml') return { name: id, kind: '脉络' }
  if (path === 'threads.toml') return { name: id, kind: '线索' }
  if (path === 'relations.toml') return { name: id, kind: '关系' }
  if (path.startsWith('worldview/')) return { name: id, kind: '世界观' }
  if (path.startsWith('knowledge/')) return { name: id, kind: '知识' }
  return { name: id, kind: '文件' }
}

const totalAdded = computed(() => files.value.reduce((s, f) => s + f.added, 0))
const totalDeleted = computed(() => files.value.reduce((s, f) => s + f.deleted, 0))
</script>

<template>
  <ElDialog v-model="visible" title="改动对比" width="720px" append-to-body>
    <div v-if="target" class="gd">
      <div class="gd-head">
        <div class="gd-range">
          <span class="gd-side">{{ target.fromLabel }}</span>
          <i class="fa-solid fa-arrow-right-long" />
          <span class="gd-side is-to">{{ target.toLabel }}</span>
        </div>
        <div v-if="files.length" class="gd-total num">
          <span class="gd-add">+{{ totalAdded }}</span>
          <span class="gd-del">−{{ totalDeleted }}</span>
          <span class="gd-count">{{ files.length }} 个文件</span>
        </div>
      </div>

      <div v-if="loading" class="gd-empty">读取改动中…</div>
      <div v-else-if="!files.length" class="gd-empty">这两个存档之间没有文件改动。</div>

      <div v-else class="gd-list">
        <div v-for="f in files" :key="f.path" class="gd-file">
          <button class="gd-file-row" @click="toggle(f)">
            <i
              class="fa-solid chev"
              :class="expanded.has(f.path) ? 'fa-angle-down' : 'fa-angle-right'"
            />
            <span class="gd-status" :class="statusMeta(f.status).cls">{{ statusMeta(f.status).label }}</span>
            <span class="gd-name num">{{ fileLabel(f.path).name }}</span>
            <span class="gd-kind">{{ fileLabel(f.path).kind }}</span>
            <span class="gd-flex" />
            <span class="num gd-add">+{{ f.added }}</span>
            <span class="num gd-del">−{{ f.deleted }}</span>
          </button>

          <div v-if="expanded.has(f.path)" class="gd-detail">
            <div v-if="detailLoading === f.path" class="gd-detail-hint">读取明细…</div>
            <div v-else-if="f.status === 'D'" class="gd-detail-hint">文件已删除。</div>
            <template v-else-if="details.get(f.path)">
              <template v-if="details.get(f.path)!.binary">
                <div class="gd-detail-hint">二进制文件，无法展示文本改动。</div>
              </template>
              <template v-else>
                <div
                  v-for="(h, hi) in details.get(f.path)!.hunks"
                  :key="hi"
                  class="gd-hunk"
                >
                  <div class="gd-hunk-head num">{{ h.header }}</div>
                  <div
                    v-for="(l, li) in h.lines"
                    :key="li"
                    class="gd-line"
                    :class="`is-${l.kind}`"
                  >{{ l.text || '　' }}</div>
                </div>
                <div v-if="details.get(f.path)!.truncated" class="gd-detail-hint">
                  改动行数过多，已截断——完整内容可在版本面板先恢复再对比。
                </div>
                <div
                  v-if="!details.get(f.path)!.hunks.length"
                  class="gd-detail-hint"
                >
                  没有文本改动。
                </div>
              </template>
            </template>
          </div>
        </div>
      </div>
    </div>
  </ElDialog>
</template>

<style scoped lang="scss">
.gd {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-height: 120px;
}

.gd-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  flex-wrap: wrap;
}

.gd-range {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--ink-muted);
  min-width: 0;

  i {
    color: var(--ink-disabled);
  }

  .gd-side {
    max-width: 240px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;

    &.is-to {
      color: var(--ink-main);
      font-weight: 600;
    }
  }
}

.gd-total {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  font-size: 12px;
}

.gd-add {
  color: var(--state-success);
}

.gd-del {
  color: var(--state-danger);
}

.gd-count {
  color: var(--ink-disabled);
}

.gd-list {
  max-height: 56vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.gd-empty {
  padding: 26px 0;
  text-align: center;
  font-size: 12.5px;
  color: var(--ink-disabled);
}

.gd-file {
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);
  overflow: hidden;
}

.gd-file-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 12px;
  border: none;
  background: transparent;
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  text-align: left;

  .chev {
    font-size: 10px;
    color: var(--ink-disabled);
  }

  &:hover {
    background: var(--surface-2);
  }
}

.gd-status {
  padding: 1px 7px;
  border-radius: 4px;
  font-size: 11px;
  flex-shrink: 0;

  &.st-add {
    background: color-mix(in srgb, var(--state-success) 12%, transparent);
    color: var(--state-success);
  }

  &.st-mod {
    background: color-mix(in srgb, var(--state-warning) 14%, transparent);
    color: var(--state-warning);
  }

  &.st-del {
    background: color-mix(in srgb, var(--state-danger) 10%, transparent);
    color: var(--state-danger);
  }
}

.gd-name {
  color: var(--ink-main);
}

.gd-kind {
  font-size: 11px;
  color: var(--ink-disabled);
  flex-shrink: 0;
}

.gd-flex {
  flex: 1;
}

.gd-detail {
  border-top: 1px solid var(--divider);
}

.gd-detail-hint {
  padding: 10px 14px;
  font-size: 11.5px;
  color: var(--ink-disabled);
}

.gd-hunk {
  &:not(:first-child) {
    border-top: 1px dashed var(--divider);
  }
}

.gd-hunk-head {
  padding: 5px 14px;
  font-size: 10.5px;
  color: var(--ink-disabled);
  background: var(--surface-2);
}

.gd-line {
  padding: 2px 14px;
  font-size: 12.5px;
  line-height: 1.8;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--ink-sec);

  &.is-add {
    background: color-mix(in srgb, var(--state-success) 9%, transparent);
    color: var(--ink-main);
  }

  &.is-del {
    background: color-mix(in srgb, var(--state-danger) 8%, transparent);
    color: var(--ink-muted);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--state-danger) 45%, transparent);
  }
}
</style>
