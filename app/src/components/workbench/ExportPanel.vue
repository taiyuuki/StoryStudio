<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { open, save } from '@tauri-apps/plugin-dialog'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'
import { revealItemInDir } from '@tauri-apps/plugin-opener'

const store = useWorkspaceStore()

type ExportFmt = 'markdown' | 'txt'

const formats: { value: ExportFmt; label: string; icon: string }[] = [
  { value: 'markdown', label: 'Markdown', icon: 'fa-brands fa-markdown' },
  { value: 'txt', label: '纯文本', icon: 'fa-solid fa-file-lines' },
]

const fmt = ref<ExportFmt>('markdown')
const isTxt = computed(() => fmt.value === 'txt')

const opts = reactive({
  includeAppendix: false,
  sceneTitles: false,
  chapterNumbering: false,
  paragraphIndent: true,
})

const busy = ref(false)
const lastExported = ref('')

async function exportWhole() {
  if (!store.root) return
  busy.value = true
  try {
    const output = isTxt.value
      ? await api.exportTxt(
          store.root,
          opts.includeAppendix,
          opts.sceneTitles,
          opts.chapterNumbering,
          opts.paragraphIndent,
        )
      : await api.exportMarkdown(
          store.root,
          opts.includeAppendix,
          opts.sceneTitles,
          opts.chapterNumbering,
        )
    const path = await save({
      title: isTxt.value ? '导出整本 TXT' : '导出整本 Markdown',
      defaultPath: output.filename,
      filters: isTxt.value
        ? [{ name: '纯文本', extensions: ['txt'] }]
        : [{ name: 'Markdown', extensions: ['md'] }],
    })
    if (!path) return
    await api.writeTextFile(path, output.content)
    lastExported.value = path
    ElMessage.success('整本导出完成')
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    busy.value = false
  }
}

async function exportByChapter() {
  if (!store.root) return
  busy.value = true
  try {
    const files = isTxt.value
      ? await api.exportChaptersTxt(
          store.root,
          opts.includeAppendix,
          opts.sceneTitles,
          opts.chapterNumbering,
          opts.paragraphIndent,
        )
      : await api.exportChapters(
          store.root,
          opts.includeAppendix,
          opts.sceneTitles,
          opts.chapterNumbering,
        )
    if (files.length === 0) {
      ElMessage.warning('还没有可导出的内容')
      return
    }
    const dir = await open({
      directory: true,
      title: `选择保存位置（将导出 ${files.length} 个章节文件）`,
    })
    if (typeof dir !== 'string' || !dir) return
    for (const f of files) {
      await api.writeTextFile(`${dir}/${f.filename}`, f.content)
    }
    lastExported.value = `${dir}/${files[files.length - 1].filename}`
    ElMessage.success(`已导出 ${files.length} 个章节文件`)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    busy.value = false
  }
}

async function revealLast() {
  if (!lastExported.value) return
  try {
    await revealItemInDir(lastExported.value)
  } catch {
    ElMessage.warning('无法打开文件位置')
  }
}
</script>

<template>
  <div class="export-panel">
    <div class="ep-desc">
      <i class="fa-solid fa-file-lines" />
      <p>把整本书导出为 Markdown 或纯文本文稿，纯文本模式会自动清除格式标记。</p>
    </div>

    <div class="ep-format">
      <button
        v-for="f in formats"
        :key="f.value"
        class="ef-item"
        :class="{ 'is-active': fmt === f.value }"
        @click="fmt = f.value"
      >
        <i :class="f.icon" />
        {{ f.label }}
      </button>
    </div>

    <div class="ep-option">
      <div>
        <div class="eo-label">角色一览</div>
        <div class="eo-sub">在文末附上所有角色的设定速览</div>
      </div>
      <ElSwitch v-model="opts.includeAppendix" />
    </div>
    <div class="ep-option">
      <div>
        <div class="eo-label">场景标题</div>
        <div class="eo-sub">开启时显示每场戏的标题，关闭时用分割线分隔</div>
      </div>
      <ElSwitch v-model="opts.sceneTitles" />
    </div>
    <div class="ep-option">
      <div>
        <div class="eo-label">章节编号</div>
        <div class="eo-sub">自动加上「第N章」前缀</div>
      </div>
      <ElSwitch v-model="opts.chapterNumbering" />
    </div>
    <div v-if="isTxt" class="ep-option">
      <div>
        <div class="eo-label">段首缩进</div>
        <div class="eo-sub">每段开头加两个全角空格，符合投稿排版习惯</div>
      </div>
      <ElSwitch v-model="opts.paragraphIndent" />
    </div>

    <div class="ep-actions">
      <button
        class="ink-btn ink-btn--primary"
        style="width: 100%"
        :disabled="busy"
        @click="exportWhole"
      >
        <i :class="busy ? 'fa-solid fa-circle-notch fa-spin' : 'fa-solid fa-book'" />
        导出整本
      </button>
      <button class="ink-btn" style="width: 100%" :disabled="busy" @click="exportByChapter">
        <i class="fa-solid fa-layer-group" />
        按章导出
      </button>
    </div>

    <div v-if="lastExported" class="ep-last" @click="revealLast">
      <i class="fa-solid fa-folder-open" />
      <span>{{ lastExported }}</span>
    </div>
  </div>
</template>

<style scoped lang="scss">
.export-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.ep-desc {
  display: flex;
  gap: 9px;
  padding: 12px;
  border-radius: 10px;
  background: var(--surface-2);
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    margin-top: 4px;
  }
}

.ep-format {
  display: flex;
  gap: 8px;
}

.ef-item {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  padding: 8px 0;
  border-radius: 10px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
  font-size: 12px;
  color: var(--ink-muted);
  cursor: pointer;
  transition:
    color 0.2s,
    border-color 0.2s,
    background 0.2s;

  &:hover {
    color: var(--ink-main);
  }

  &.is-active {
    border-color: var(--ink-accent);
    color: var(--ink-accent);
    background: var(--surface-2);
  }
}

.ep-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--divider);
  background: var(--surface-1);
}

.eo-label {
  font-size: 13px;
  color: var(--ink-main);
}

.eo-sub {
  margin-top: 2px;
  font-size: 11px;
  color: var(--ink-muted);
}

.ep-actions {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 4px;
}

.ep-last {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 8px 10px;
  border-radius: 8px;
  background: var(--surface-2);
  font-size: 11px;
  color: var(--ink-muted);
  cursor: pointer;
  transition: color 0.2s;

  span {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl;
    text-align: left;
  }

  &:hover {
    color: var(--ink-accent);
  }
}
</style>
