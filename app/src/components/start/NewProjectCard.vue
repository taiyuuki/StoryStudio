<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { documentDir, homeDir } from '@tauri-apps/api/path'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'

const emit = defineEmits<{ created: [root: string] }>()

const bookName = ref('')
const parentDir = ref('')
const creating = ref(false)
/** 默认存放位置：文稿/StoryStudio（取不到 Documents 时退回用户目录下的 StoryStudio） */
const defaultDir = ref('')

onMounted(async () => {
  try {
    const base = (await documentDir()) || (await homeDir())
    if (base) defaultDir.value = `${base.replace(/\/$/, '')}/StoryStudio`
  } catch {
    try {
      const base = await homeDir()
      if (base) defaultDir.value = `${base.replace(/\/$/, '')}/StoryStudio`
    } catch {
      // 都拿不到就退回必须手选
    }
  }
})

async function pickDir() {
  const dir = await open({ directory: true, title: '选择项目存放目录' })
  if (typeof dir === 'string') parentDir.value = dir
}

async function create() {
  if (!bookName.value.trim()) {
    ElMessage.warning('先给书起个名字')
    return
  }
  // 目录可选：不选就放到默认位置（文稿/StoryStudio），后端会自动创建
  const dir = parentDir.value.trim() || defaultDir.value
  if (!dir) {
    ElMessage.warning('没能找到默认存放位置，请手动选择')
    return
  }
  creating.value = true
  try {
    const root = await api.createProject(dir, bookName.value.trim())
    ElMessage.success('项目已创建')
    emit('created', root)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    creating.value = false
  }
}
</script>

<template>
  <div class="fusion-card new-card">
    <div class="card-head">
      <i class="fa-solid fa-feather-pointed" />
      <span>开卷 · 新建作品</span>
    </div>

    <label class="field">
      <span class="field-label">书名</span>
      <input
        v-model="bookName"
        class="ink-input"
        placeholder="例：山海拾遗录"
        maxlength="60"
        @keyup.enter="create"
      />
    </label>

    <label class="field">
      <span class="field-label">存放目录（可选）</span>
      <div class="dir-row">
        <input
          v-model="parentDir"
          class="ink-input"
          :placeholder="defaultDir ? `默认：${defaultDir}` : '默认：文稿/StoryStudio'"
          readonly
          @click="pickDir"
        />
        <button class="ink-btn" @click="pickDir">
          <i class="fa-regular fa-folder-open" />
          浏览
        </button>
      </div>
    </label>

    <div class="hint">
      <i class="fa-solid fa-circle-info" />
      不选也行，会根据书名在「文稿/StoryStudio」里自动创建一个文件夹，所有内容都保存在里面
    </div>

    <button class="ink-btn ink-btn--primary ink-btn--lg create-btn" :disabled="creating" @click="create">
      <i class="fa-solid fa-wand-magic-sparkles" />
      {{ creating ? '正在落墨…' : '创建作品' }}
    </button>

    <slot />
  </div>
</template>

<style scoped lang="scss">
.new-card {
  padding: 22px 24px;
  display: flex;
  flex-direction: column;
}

.card-head {
  display: flex;
  align-items: center;
  gap: 9px;
  font-size: 14px;
  color: var(--ink-main);
  padding-bottom: 14px;
  border-bottom: 1px solid var(--divider);
  margin-bottom: 18px;

  i {
    color: var(--ink-accent);
  }
}

.field {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 16px;
}

.field-label {
  font-size: 12.5px;
  color: var(--ink-sec);
  letter-spacing: 0.08em;
}

.ink-input {
  height: 38px;
  padding: 0 12px;
  border-radius: 8px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 13.5px;
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s;

  &::placeholder {
    color: var(--ink-disabled);
  }

  &:focus {
    border-color: var(--ink-accent);
    box-shadow: 0 0 0 3px var(--ink-accent-soft);
  }
}

.dir-row {
  display: flex;
  gap: 8px;

  .ink-input {
    flex: 1;
    cursor: pointer;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
}

.hint {
  display: flex;
  gap: 7px;
  align-items: flex-start;
  margin: 2px 0 18px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-muted);

  i {
    margin-top: 4px;
    flex-shrink: 0;
  }

  code {
    font-family: 'SF Mono', Menlo, monospace;
    font-size: 11px;
    background: var(--surface-2);
    padding: 1px 5px;
    border-radius: 4px;
  }
}

.create-btn {
  margin-top: auto;
}
</style>
