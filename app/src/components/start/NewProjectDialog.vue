<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { documentDir, homeDir } from '@tauri-apps/api/path'
import { ElMessage } from 'element-plus'
import { api } from '@/api/backend'
import type { BookMetaDto } from '@/api/backend'
import { isBookMetaEmpty, normalizeBookMeta } from '@/constants/bookMeta'
import BookMetaForm from '@/components/BookMetaForm.vue'

const props = defineProps<{ modelValue: boolean }>()

const emit = defineEmits<{
  'update:modelValue': [v: boolean]
  created: [root: string]
}>()

const bookName = ref('')
const parentDir = ref('')
const creating = ref(false)
/** 默认存放位置：文稿/StoryStudio（取不到 Documents 时退回用户目录下的 StoryStudio） */
const defaultDir = ref('')
/** 书籍信息（可选，折叠收纳：跳过稍后在结构树「书籍信息」里补）。
 *  必须用 ref：BookMetaForm 的 v-model 是整体替换赋值，reactive 常量接不住。 */
const metaExpanded = ref(false)
const meta = ref<BookMetaDto>({
  genre: null,
  premise: null,
  theme: null,
  audience: null,
  pov: null,
  style: null,
  targetWords: null,
  chapterWords: null,
})

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

// 每次打开清空上次的书名与书籍信息，目录选择保留（连续开书常见）
watch(
  () => props.modelValue,
  (v) => {
    if (v) {
      bookName.value = ''
      metaExpanded.value = false
      Object.assign(meta.value, {
        genre: null,
        premise: null,
        theme: null,
        audience: null,
        pov: null,
        style: null,
        targetWords: null,
        chapterWords: null,
      })
    }
  },
)

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
    // 书籍信息补写 outline.toml：项目已建，失败只提示不回滚
    if (!isBookMetaEmpty(meta.value)) {
      try {
        await api.saveBookMeta(root, normalizeBookMeta(meta.value))
      } catch (err) {
        ElMessage.warning(`书籍信息保存失败：${(err as Error).message}`)
      }
    }
    ElMessage.success('项目已创建')
    emit('update:modelValue', false)
    emit('created', root)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    creating.value = false
  }
}
</script>

<template>
  <ElDialog
    :model-value="modelValue"
    title="开卷 · 新建作品"
    width="520px"
    align-center
    append-to-body
    :close-on-click-modal="false"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="npd">
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

      <!-- 书籍信息（可选）——册页式折叠 -->
      <div class="meta-fold" :class="{ 'is-open': metaExpanded }">
        <button type="button" class="meta-toggle" @click="metaExpanded = !metaExpanded">
          <span class="meta-fold-line" />
          <i class="fa-solid fa-book-open meta-fold-icon" />
          <span class="meta-fold-label">书籍信息</span>
          <span class="meta-fold-tag">可选</span>
          <i class="fa-solid fa-chevron-right meta-fold-arrow" />
          <span class="meta-fold-line meta-fold-line--end" />
        </button>
        <p class="meta-fold-hint">
          {{ metaExpanded ? '题材 · 主线 · 体量……填了更懂你要写什么' : '题材 / 主线 / 体量……稍后也可在工作台补' }}
        </p>
        <Transition name="meta-unfold">
          <div v-if="metaExpanded" class="meta-body">
            <BookMetaForm v-model="meta" />
          </div>
        </Transition>
      </div>

      <div class="npd-foot">
        <button class="ink-btn" @click="emit('update:modelValue', false)">取消</button>
        <button
          class="ink-btn ink-btn--primary ink-btn--lg create-btn"
          :disabled="creating"
          @click="create"
        >
          <i class="fa-solid fa-wand-magic-sparkles" />
          {{ creating ? '正在落墨…' : '创建作品' }}
        </button>
      </div>
    </div>
  </ElDialog>
</template>

<style scoped lang="scss">
.npd {
  display: flex;
  flex-direction: column;
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
  margin: 2px 0 20px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-muted);

  i {
    margin-top: 4px;
    flex-shrink: 0;
  }
}

.npd-foot {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

// ── 册页式折叠开关：两侧墨线渐隐，展开体左缘一道朱色「书脊」 ──
.meta-fold {
  margin: 2px 0 18px;
}

.meta-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  border: none;
  background: transparent;
  padding: 0;
  color: var(--ink-sec);
  font-family: inherit;
  cursor: pointer;
  transition: color 0.2s;

  &:hover {
    color: var(--ink-accent);
  }
}

.meta-fold-line {
  flex: 1;
  height: 1px;
  background: linear-gradient(90deg, transparent, var(--divider));
  transition: background 0.2s;

  // 右侧镜像：由实渐隐
  &--end {
    background: linear-gradient(90deg, var(--divider), transparent);
  }

  .meta-toggle:hover & {
    background: linear-gradient(90deg, transparent, var(--ink-accent-soft));

    &--end {
      background: linear-gradient(90deg, var(--ink-accent-soft), transparent);
    }
  }
}

.meta-fold-icon {
  font-size: 11.5px;
  color: var(--ink-accent);
  opacity: 0.85;
}

.meta-fold-label {
  font-size: 12.5px;
  letter-spacing: 0.12em;
}

// 「可选」小印
.meta-fold-tag {
  padding: 0 6px;
  height: 17px;
  line-height: 17px;
  border: 1px solid var(--ink-accent-soft);
  border-radius: 4px;
  color: var(--ink-accent);
  font-size: 10px;
  letter-spacing: 0.08em;
}

.meta-fold-arrow {
  font-size: 9.5px;
  color: var(--ink-disabled);
  transition: transform 0.25s ease;
}

.meta-fold.is-open .meta-fold-arrow {
  transform: rotate(90deg);
}

.meta-fold-hint {
  margin: 5px 0 0;
  text-align: center;
  font-size: 11px;
  color: var(--ink-disabled);
  letter-spacing: 0.02em;
}

// 展开体：宣纸底 + 左缘朱色书脊线（上下渐隐）
.meta-body {
  position: relative;
  margin-top: 13px;
  padding: 16px 16px 15px 19px;
  border: 1px solid var(--divider);
  border-radius: 10px;
  background: var(--surface-1);

  &::before {
    content: '';
    position: absolute;
    left: 0;
    top: 10px;
    bottom: 10px;
    width: 2px;
    border-radius: 1px;
    background: linear-gradient(180deg, transparent, var(--ink-accent), transparent);
    opacity: 0.55;
  }
}

.meta-unfold-enter-active,
.meta-unfold-leave-active {
  transition: opacity 0.22s ease, transform 0.22s ease;
}

.meta-unfold-enter-from,
.meta-unfold-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

.create-btn {
  min-width: 128px;
}
</style>
