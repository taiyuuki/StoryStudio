<script setup lang="ts">
import { computed, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { useWorkspaceStore } from '@/stores/workspace'
import { api } from '@/api/backend'
import type { BookMetaDto, ChapterDto, SceneDto } from '@/api/backend'
import { normalizeBookMeta } from '@/constants/bookMeta'
import BookMetaForm from '@/components/BookMetaForm.vue'

const store = useWorkspaceStore()

const query = ref('')
const collapsed = ref<Set<string>>(new Set())
const renaming = ref<{ kind: 'chapter' | 'scene'; id: string; value: string } | null>(null)

// ── 书籍信息（outline.toml [story] 段的编辑入口） ───────────
// meta 用 ref：BookMetaForm 的 v-model 是整体替换赋值，reactive 常量接不住
const metaVisible = ref(false)
const metaLoading = ref(false)
const metaSaving = ref(false)
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

async function openMeta() {
  metaVisible.value = true
  if (!store.root) return
  metaLoading.value = true
  try {
    meta.value = await api.getBookMeta(store.root)
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    metaLoading.value = false
  }
}

async function saveMeta() {
  if (!store.root || metaSaving.value) return
  const next = normalizeBookMeta(meta.value)
  metaSaving.value = true
  try {
    await api.saveBookMeta(store.root, next)
    meta.value = next
    metaVisible.value = false
    ElMessage.success('书籍信息已保存')
    if (!next.premise) {
      ElMessage.info('续排大纲需要「一句话主线」，建议补上')
    }
  } catch (err) {
    ElMessage.error((err as Error).message)
  } finally {
    metaSaving.value = false
  }
}

interface TreeScene extends SceneDto {
  chapterTitle: string
}

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const chapters: (ChapterDto & { scenes: SceneDto[] })[] = []
  for (const ch of store.ws?.chapters ?? []) {
    if (!q || ch.title.toLowerCase().includes(q)) {
      chapters.push({ ...ch, scenes: [...ch.scenes] })
      continue
    }
    const scenes = ch.scenes.filter(
      (s) => (s.title ?? '').toLowerCase().includes(q) || s.id.toLowerCase().includes(q),
    )
    if (scenes.length > 0) chapters.push({ ...ch, scenes })
  }
  return chapters
})

const flatScenes = computed<TreeScene[]>(() => {
  const list: TreeScene[] = []
  for (const ch of store.ws?.chapters ?? []) {
    for (const sc of ch.scenes) list.push({ ...sc, chapterTitle: ch.title })
  }
  return list
})

const sceneIndex = computed(() => {
  const idx = flatScenes.value.findIndex((s) => s.id === store.scene?.id)
  return idx >= 0 ? idx : -1
})

function toggleChapter(id: string) {
  const set = new Set(collapsed.value)
  if (set.has(id)) set.delete(id)
  else set.add(id)
  collapsed.value = set
}

function selectScene(id: string) {
  if (renaming.value) return
  void store.selectScene(id)
}

function chapterWords(ch: ChapterDto): number {
  return ch.scenes.reduce((sum, s) => sum + s.wordCount, 0)
}

// ── 新建 ────────────────────────────────────────────────
const creating = ref<'chapter' | null>(null)
const newName = ref('')

async function startCreateChapter() {
  creating.value = 'chapter'
  newName.value = ''
}

async function confirmCreate() {
  const title = newName.value.trim()
  if (!title) {
    creating.value = null
    return
  }
  const ch = await store.createChapter(title)
  if (ch) collapsed.value.delete(ch.id)
  creating.value = null
}

async function createScene(chapterId: string) {
  const { value } = await ElMessageBox.prompt('场景标题', '新建场景', {
    confirmButtonText: '创建',
    cancelButtonText: '取消',
    inputPlaceholder: '例：夜访旧宅',
    inputValue: '',
  }).catch(() => ({ value: '' }))
  if (value.trim()) await store.createScene(chapterId, value.trim())
}

// ── 重命名 ──────────────────────────────────────────────
function startRename(kind: 'chapter' | 'scene', id: string, current: string) {
  renaming.value = { kind, id, value: current }
}

async function confirmRename() {
  const r = renaming.value
  renaming.value = null
  if (!r || !r.value.trim() || r.value === r.id) return
  const title = r.value.trim()
  if (r.kind === 'chapter') await store.renameChapter(r.id, title)
  else await store.updateScene(r.id, { title })
}

function sceneTitle(s: SceneDto): string {
  return s.title?.trim() || '未命名场景'
}
</script>

<template>
  <aside class="tree-panel">
    <div class="tree-head">
      <div class="search-row">
        <i class="fa-solid fa-magnifying-glass" />
        <input v-model="query" placeholder="搜索章节 / 场景" />
      </div>
      <button class="ink-icon-btn" title="书籍信息（题材 / 主线 / 体量……）" @click="openMeta">
        <i class="fa-solid fa-circle-info" />
      </button>
      <button class="ink-icon-btn" title="新建章节" @click="startCreateChapter">
        <i class="fa-solid fa-plus" />
      </button>
    </div>

    <!-- 书籍信息编辑 -->
    <ElDialog
      v-model="metaVisible"
      title="书籍信息"
      width="560px"
      align-center
      append-to-body
      :close-on-click-modal="false"
    >
      <div v-if="metaLoading" class="meta-loading">
        <i class="fa-solid fa-circle-notch fa-spin" />
      </div>
      <template v-else>
        <BookMetaForm v-model="meta" />
        <p class="meta-hint">
          题材、主线与体量会传给续排大纲与逐章成书，作为走向与篇幅基准；填写后随时可改。
        </p>
      </template>
      <template #footer>
        <button class="ink-btn" :disabled="metaSaving" @click="metaVisible = false">取消</button>
        <button
          class="ink-btn ink-btn--primary"
          :disabled="metaLoading || metaSaving"
          @click="saveMeta"
        >{{ metaSaving ? '保存中…' : '保存' }}</button>
      </template>
    </ElDialog>

    <div v-if="creating === 'chapter'" class="new-chapter-row">
      <i class="fa-solid fa-layer-group" />
      <input
        v-model="newName"
        placeholder="章节标题"
        autofocus
        @keyup.enter="confirmCreate"
        @keyup.escape="creating = null"
        @blur="confirmCreate"
      />
    </div>

    <div class="tree-scroll">
      <div v-if="filtered.length === 0" class="tree-empty">
        <i class="fa-regular fa-file-lines" />
        <p>尚无章节</p>
        <button class="ink-btn ink-btn--sm" @click="startCreateChapter">写第一章</button>
      </div>

      <div v-for="ch in filtered" :key="ch.id" class="chapter">
        <div class="chapter-row" @click="toggleChapter(ch.id)">
          <i
            class="fa-solid fa-angle-right chev"
            :class="{ 'chev--open': !collapsed.has(ch.id) }"
          />
          <template v-if="renaming?.kind === 'chapter' && renaming.id === ch.id">
            <input
              v-model="renaming.value"
              class="rename-input"
              autofocus
              @keyup.enter="confirmRename"
              @keyup.escape="renaming = null"
              @blur="confirmRename"
              @click.stop
            />
          </template>
          <template v-else>
            <span class="chapter-title">{{ ch.title }}</span>
            <span class="chapter-words num">{{ chapterWords(ch) }}</span>
            <ElDropdown trigger="click" @command="(cmd: string) => {
              if (cmd === 'scene') createScene(ch.id)
              else if (cmd === 'rename') startRename('chapter', ch.id, ch.title)
              else if (cmd === 'delete') store.deleteChapter(ch.id)
            }">
              <button class="row-menu" @click.stop>
                <i class="fa-solid fa-ellipsis" />
              </button>
              <template #dropdown>
                <ElDropdownMenu>
                  <ElDropdownItem command="scene">
                    <i class="fa-solid fa-plus" /> 新建场景
                  </ElDropdownItem>
                  <ElDropdownItem command="rename">
                    <i class="fa-solid fa-pen" /> 重命名
                  </ElDropdownItem>
                  <ElDropdownItem command="delete" divided>
                    <i class="fa-solid fa-trash" /> 删除章节
                  </ElDropdownItem>
                </ElDropdownMenu>
              </template>
            </ElDropdown>
          </template>
        </div>

        <div v-show="!collapsed.has(ch.id)" class="scene-list">
          <div
            v-for="sc in ch.scenes"
            :key="sc.id"
            class="scene-row"
            :class="{ 'is-active': sc.id === store.scene?.id }"
            :title="sc.summary ?? undefined"
            role="button"
            tabindex="0"
            @click="selectScene(sc.id)"
            @keyup.enter="selectScene(sc.id)"
          >
            <input
              v-if="renaming?.kind === 'scene' && renaming.id === sc.id"
              v-model="renaming.value"
              class="rename-input"
              autofocus
              @keyup.enter="confirmRename"
              @keyup.escape="renaming = null"
              @blur="confirmRename"
              @click.stop
              @keyup.stop
            />
            <template v-else>
              <span class="scene-dot" />
              <span class="scene-title">{{ sceneTitle(sc) }}</span>
              <span v-if="sc.pov" class="scene-pov" :title="`视角：${store.entityName(sc.pov)}`">
                {{ store.entityName(sc.pov) }}
              </span>
              <span class="scene-words num">{{ sc.wordCount }}</span>
              <ElDropdown
                trigger="click"
                @command="(cmd: string) => {
                  if (cmd === 'rename') startRename('scene', sc.id, sc.title ?? '')
                  else if (cmd === 'delete') store.deleteScene(sc.id)
                }"
              >
                <button class="row-menu" title="场景操作" @click.stop>
                  <i class="fa-solid fa-ellipsis" />
                </button>
                <template #dropdown>
                  <ElDropdownMenu>
                    <ElDropdownItem command="rename">
                      <i class="fa-solid fa-pen" /> 重命名
                    </ElDropdownItem>
                    <ElDropdownItem command="delete" divided>
                      <i class="fa-solid fa-trash" /> 删除场景
                    </ElDropdownItem>
                  </ElDropdownMenu>
                </template>
              </ElDropdown>
            </template>
          </div>
          <button class="scene-add" @click="createScene(ch.id)">
            <i class="fa-solid fa-plus" />
            新建场景
          </button>
        </div>
      </div>
    </div>

    <div class="tree-foot">
      <span class="num">{{ store.ws?.chapters.length ?? 0 }} 章</span>
      <span class="dot">·</span>
      <span class="num">{{ flatScenes.length }} 场</span>
      <span class="dot">·</span>
      <span class="num">{{ (store.ws?.totalWords ?? 0).toLocaleString() }} 字</span>
    </div>
  </aside>
</template>

<style scoped lang="scss">
.tree-panel {
  width: 272px;
  display: flex;
  flex-direction: column;
  border-right: 1px solid var(--divider);
  background: var(--surface-1);
  backdrop-filter: blur(12px);
  min-height: 0;
}

.meta-loading {
  display: flex;
  justify-content: center;
  padding: 30px 0;
  color: var(--ink-muted);
}

.meta-hint {
  margin: 14px 0 0;
  font-size: 11.5px;
  line-height: 1.7;
  color: var(--ink-muted);
}

.tree-head {
  display: flex;
  gap: 8px;
  padding: 14px 14px 10px;
}

.search-row {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding: 0 10px;
  border-radius: 8px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  transition: border-color 0.2s;

  &:focus-within {
    border-color: var(--ink-accent);
  }

  i {
    font-size: 12px;
    color: var(--ink-muted);
  }

  input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    color: var(--ink-main);
    font-family: inherit;
    font-size: 12.5px;

    &::placeholder {
      color: var(--ink-disabled);
    }
  }
}

.new-chapter-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 14px 8px;
  padding: 0 10px;
  height: 34px;
  border-radius: 8px;
  border: 1px dashed var(--ink-accent);
  background: var(--ink-accent-soft);

  i {
    color: var(--ink-accent);
    font-size: 12px;
  }

  input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    color: var(--ink-main);
    font-family: inherit;
    font-size: 13px;
  }
}

.tree-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 2px 8px 12px;
}

.tree-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  margin-top: 48px;
  color: var(--ink-disabled);
  font-size: 12.5px;

  i {
    font-size: 26px;
  }
}

.chapter {
  margin-bottom: 2px;
}

.chapter-row {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  padding: 0 8px 0 6px;
  border-radius: 0 8px 8px 0;
  cursor: pointer;
  border-left: 3px solid transparent;
  transition: all 0.2s;

  &:hover {
    background: var(--surface-2);

    .chapter-words,
    .row-menu {
      opacity: 1;
    }
  }
}

.chev {
  font-size: 10px;
  color: var(--ink-muted);
  transition: transform 0.2s;
  width: 12px;

  &--open {
    transform: rotate(90deg);
  }
}

.chapter-title {
  flex: 1;
  font-size: 13.5px;
  color: var(--ink-main);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.chapter-words {
  opacity: 0;
  font-size: 11px;
  color: var(--ink-muted);
  transition: opacity 0.2s;
}

.row-menu {
  opacity: 0;
  border: none;
  background: transparent;
  color: var(--ink-muted);
  width: 22px;
  height: 22px;
  border-radius: 5px;
  cursor: pointer;
  transition: all 0.15s;

  &:hover {
    background: var(--divider);
    color: var(--ink-main);
  }
}

.rename-input {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 6px;
  border-radius: 5px;
  border: 1px solid var(--ink-accent);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 13px;
  outline: none;
}

.scene-list {
  padding-left: 14px;
}

.scene-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: 32px;
  padding: 0 8px 0 10px;
  border: none;
  border-left: 3px solid transparent;
  border-radius: 0 8px 8px 0;
  background: transparent;
  cursor: pointer;
  transition: all 0.2s;

  .row-menu {
    opacity: 0;
    flex-shrink: 0;
  }

  &:hover {
    background: var(--surface-2);

    .scene-words,
    .row-menu {
      opacity: 1;
    }
  }

  &.is-active {
    background: var(--nav-active-bg);
    border-left-color: var(--nav-active-bar-color);

    .scene-dot {
      background: var(--ink-accent);
    }

    .scene-title {
      color: var(--ink-main);
      font-weight: 600;
    }
  }
}

.scene-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--ink-disabled);
  flex-shrink: 0;
}

.scene-title {
  flex: 1;
  text-align: left;
  font-size: 12.5px;
  color: var(--ink-sec);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.scene-pov {
  flex-shrink: 0;
  max-width: 64px;
  font-size: 10.5px;
  color: var(--ink-accent);
  background: var(--ink-accent-soft);
  padding: 1px 6px;
  border-radius: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.scene-words {
  opacity: 0;
  font-size: 10.5px;
  color: var(--ink-muted);
  transition: opacity 0.2s;
}

.scene-add {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 30px;
  padding: 0 10px;
  margin: 2px 0 4px;
  border: none;
  border-radius: 0 8px 8px 0;
  background: transparent;
  color: var(--ink-disabled);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: color 0.2s;

  &:hover {
    color: var(--ink-accent);
  }
}

.tree-foot {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 10px;
  border-top: 1px solid var(--divider);
  font-size: 11.5px;
  color: var(--ink-muted);
  letter-spacing: 0.06em;

  .dot {
    opacity: 0.5;
  }
}
</style>
