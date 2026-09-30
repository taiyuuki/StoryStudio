<script setup lang="ts">
import { onActivated, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api } from '@/api/backend'
import type { GitCommitDto, GitStatusDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'

// 版本档案 — 项目目录的 git 快照 / 历史 / 一键恢复。
// AI 批量改稿（妙笔批改、逐章成书）前后自动存档，任何存档可整体回滚，
// 恢复前先自动备份当前状态，历史全程保留。
defineOptions({ name: 'GitPanel' })

const store = useWorkspaceStore()

const status = ref<GitStatusDto | null>(null)
const commits = ref<GitCommitDto[]>([])
const loading = ref(false)
const message = ref('')
const committing = ref(false)
const restoringId = ref<string | null>(null)

async function refresh() {
  if (!store.root) return
  loading.value = true
  try {
    status.value = await api.gitStatus(store.root)
    commits.value = status.value.initialized ? await api.gitLog(store.root, 50) : []
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    loading.value = false
  }
}

async function initRepo() {
  if (!store.root) return
  try {
    status.value = await api.gitInit(store.root)
    ElMessage.success('版本档案已启用')
    await refresh()
  } catch (err) {
    ElMessage.error((err as Error).message)
  }
}

async function commitNow() {
  if (!store.root || committing.value) return
  committing.value = true
  try {
    const r = await api.gitCommit(store.root, message.value.trim() || undefined)
    if (r.committed) {
      ElMessage.success(r.detail)
      message.value = ''
    } else {
      ElMessage.info(r.detail)
    }
    await refresh()
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    committing.value = false
  }
}

async function restore(c: GitCommitDto) {
  if (!store.root || restoringId.value) return
  try {
    await ElMessageBox.confirm(
      `把整本书恢复到「${c.message}」这个存档？当前未存档的改动会先自动备份，历史不会丢失，恢复错了还能再恢复回来。`,
      '恢复存档',
      { type: 'warning', confirmButtonText: '恢复', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  restoringId.value = c.id
  try {
    const r = await api.gitRestore(store.root, c.id)
    ElMessage.success(r.committed ? r.detail : r.detail)
    // 磁盘已变：整个工作台按当前状态重载（结构/设定/当前场景）
    await store.openRoot(store.root)
    await refresh()
  } catch (err) {
    ElMessage.error(`恢复失败：${(err as Error).message}`)
  } finally {
    restoringId.value = null
  }
}

function fmtTime(ts: number): string {
  const d = new Date(ts * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getMonth() + 1}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

onMounted(() => void refresh())
onActivated(() => void refresh())
</script>

<template>
  <div class="git-panel">
    <!-- git 未安装 -->
    <div v-if="status && !status.available" class="git-hint">
      <i class="fa-solid fa-circle-info" />
      <p>系统未安装 git，版本档案不可用。安装 git 后重启应用即可启用。</p>
    </div>

    <!-- 嵌套仓库 -->
    <div v-else-if="status && status.nested" class="git-hint is-warn">
      <i class="fa-solid fa-code-branch" />
      <p>这个项目位于另一个 git 仓库里。为避免影响原仓库，StoryStudio 不在此启用版本档案。</p>
    </div>

    <!-- 未初始化 -->
    <div v-else-if="status && !status.initialized" class="git-init">
      <div class="gi-icon"><i class="fa-solid fa-code-branch" /></div>
      <h3>版本档案</h3>
      <p class="text-ink-muted">
        为这本书启用 git 快照：AI 批量改稿前后自动存档，任何时点可整体恢复——
        正文、设定、章节脉络一起回滚，改坏了也不怕。
      </p>
      <button class="ink-btn ink-btn--primary" @click="initRepo">
        <i class="fa-solid fa-code-branch" />
        启用版本档案
      </button>
    </div>

    <!-- 已就绪 -->
    <template v-else-if="status">
      <div class="git-toolbar">
        <span class="git-state" :class="{ 'is-dirty': status.dirty }">
          <i :class="status.dirty ? 'fa-solid fa-circle-dot' : 'fa-solid fa-circle-check'" />
          {{ status.dirty ? '有未存档变更' : '已全部存档' }}
        </span>
        <span class="git-flex" />
        <button class="ink-icon-btn" title="刷新" @click="refresh">
          <i :class="loading ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-rotate'" />
        </button>
      </div>

      <div class="git-commit-row">
        <input
          v-model="message"
          class="git-msg-input"
          placeholder="存档说明（可留空）"
          maxlength="60"
          @keydown.enter.prevent="commitNow"
        />
        <button class="ink-btn ink-btn--sm ink-btn--primary" :disabled="committing" @click="commitNow">
          <i :class="committing ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-floppy-disk'" />
          存档
        </button>
      </div>

      <div class="git-list">
        <div v-if="!commits.length && !loading" class="git-empty">还没有存档，点上面「存档」建立第一个。</div>
        <div
          v-for="(c, i) in commits"
          :key="c.id"
          class="git-item"
          :class="{ 'is-head': i === 0 }"
        >
          <div class="gi-main">
            <div class="gi-msg">{{ c.message }}</div>
            <div class="gi-meta">
              <span class="num">{{ c.id }}</span>
              <span class="num">{{ fmtTime(c.timestamp) }}</span>
              <span v-if="i === 0" class="gi-now">当前</span>
            </div>
          </div>
          <button
            v-if="i > 0"
            class="ink-btn ink-btn--sm gi-restore"
            :disabled="restoringId !== null"
            title="把整本书恢复到这个存档（恢复前会自动备份当前状态）"
            @click="restore(c)"
          >
            <i :class="restoringId === c.id ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-clock-rotate-left'" />
            恢复
          </button>
        </div>
      </div>

      <p class="git-foot">
        妙笔批改与逐章成书会自动存档；恢复永远先备份当前状态，历史不丢。
      </p>
    </template>
  </div>
</template>

<style scoped lang="scss">
.git-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

// 提示态（未装 git / 嵌套仓库）
.git-hint {
  display: flex;
  gap: 9px;
  padding: 12px 14px;
  border-radius: 12px;
  background: var(--surface-2);
  color: var(--ink-muted);
  font-size: 12.5px;
  line-height: 1.8;

  i {
    margin-top: 4px;
    color: var(--ink-accent);
  }

  &.is-warn i {
    color: var(--state-warning);
  }
}

// 初始化引导
.git-init {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 30px 16px;
  text-align: center;

  .gi-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    border-radius: 13px;
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
    font-size: 20px;
  }

  h3 {
    font-size: 16px;
    color: var(--ink-main);
  }

  p {
    font-size: 12.5px;
    line-height: 1.9;
  }
}

// 工具行
.git-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
}

.git-state {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--state-success);

  &.is-dirty {
    color: var(--state-warning);
  }
}

.git-flex {
  flex: 1;
}

// 存档输入
.git-commit-row {
  display: flex;
  gap: 8px;

  .git-msg-input {
    flex: 1;
    min-width: 0;
    padding: 7px 11px;
    border: 1px solid var(--ui-border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12.5px;
    outline: none;

    &::placeholder {
      color: var(--ink-disabled);
    }

    &:focus {
      border-color: var(--ink-accent);
    }
  }
}

// 历史列表
.git-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 52vh;
  overflow-y: auto;
  padding-right: 2px;
}

.git-empty {
  padding: 26px 0;
  text-align: center;
  font-size: 12.5px;
  color: var(--ink-disabled);
}

.git-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 11px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);

  &.is-head {
    border-color: color-mix(in srgb, var(--state-success) 45%, var(--divider));
    background: color-mix(in srgb, var(--state-success) 5%, var(--surface-1));
  }
}

.gi-main {
  flex: 1;
  min-width: 0;
}

.gi-msg {
  font-size: 12.5px;
  color: var(--ink-main);
  line-height: 1.6;
  word-break: break-all;
}

.gi-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 3px;
  font-size: 10.5px;
  color: var(--ink-disabled);
}

.gi-now {
  padding: 0 6px;
  border-radius: 7px;
  background: var(--surface-2);
  color: var(--state-success);
  font-size: 10px;
  line-height: 16px;
}

.gi-restore {
  flex-shrink: 0;
}

.git-foot {
  font-size: 11px;
  color: var(--ink-disabled);
  line-height: 1.7;
}
</style>
