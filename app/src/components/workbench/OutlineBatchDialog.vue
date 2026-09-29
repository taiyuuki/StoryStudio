<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { api, type OutlineBatchDraftDto } from '@/api/backend'
import { useWorkspaceStore } from '@/stores/workspace'
import OutlineForm from '../start/aiBook/OutlineForm.vue'

const props = defineProps<{
  /** 已排章节数（编号显示用） */
  existingCount: number
}>()

const model = defineModel<boolean>({ default: false })
const emit = defineEmits<{ saved: [count: number] }>()

const store = useWorkspaceStore()
const count = ref(12)
const draft = ref<OutlineBatchDraftDto | null>(null)
const generating = ref(false)
const saving = ref(false)
const error = ref('')

const characterNames = computed(() =>
  (store.ws?.entities ?? []).filter((e) => e.kind === 'character').map((e) => e.displayName),
)

watch(model, (open) => {
  if (open) {
    draft.value = null
    error.value = ''
    count.value = 12
  }
})

async function generate() {
  if (!store.root) return
  generating.value = true
  error.value = ''
  try {
    draft.value = await api.aiBookOutlineBatch(store.root, count.value)
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    generating.value = false
  }
}

async function save() {
  if (!store.root || !draft.value) return
  saving.value = true
  error.value = ''
  try {
    const saved = await api.saveBookOutlineBatch(store.root, draft.value)
    ElMessage.success(`已追加 ${saved} 章脉络`)
    model.value = false
    emit('saved', saved)
  } catch (err) {
    error.value = (err as Error).message
  } finally {
    saving.value = false
  }
}

function cancelGenerating() {
  void api.aiBookCancel()
}
</script>

<template>
  <ElDialog
    v-model="model"
    title="续排章节大纲"
    width="680px"
    top="6vh"
    align-center
    append-to-body
    :close-on-click-modal="false"
  >
    <div class="obd">
      <!-- 生成参数 -->
      <div v-if="!draft && !generating" class="obd-form">
        <p class="obd-tip">
          接着第 {{ existingCount }} 章往后排。AI 会看主线、最近的章节脉络和已写完章节的梗概来续，
          所以写到哪儿排到哪儿正合适。已有 {{ existingCount }} 章。
        </p>
        <div class="wz-row">
          <div class="wz-field">
            <label>这一批排几章</label>
            <input v-model.number="count" type="number" min="1" max="40" />
          </div>
        </div>
        <button class="ink-btn ink-btn--primary" :disabled="generating" @click="generate">
          <i class="fa-solid fa-wand-magic-sparkles" />
          开始排
        </button>
        <div v-if="error" class="obd-error">{{ error }}</div>
      </div>

      <!-- 生成中 -->
      <div v-else-if="generating" class="obd-progress">
        <i class="fa-solid fa-circle-notch fa-spin" />
        <p>正在读前文、排后续章节…</p>
        <button class="ink-btn ink-btn--sm" @click="cancelGenerating">
          <i class="fa-solid fa-stop" />
          停止
        </button>
      </div>

      <!-- 编辑 -->
      <template v-else-if="draft">
        <p class="obd-tip">增删改随意，保存后追加到章节脉络末尾（不影响已写的章节）。</p>
        <div class="obd-editor">
          <OutlineForm v-model="draft" :character-names="characterNames" :existing-count="existingCount" />
        </div>
        <div class="obd-actions">
          <button class="ink-btn ink-btn--sm" @click="draft = null"><i class="fa-solid fa-arrow-left" /> 返回重排</button>
          <button class="ink-btn ink-btn--sm" @click="generate"><i class="fa-solid fa-rotate" /> 重新排</button>
          <button class="ink-btn ink-btn--primary ink-btn--sm" :disabled="saving || draft.chapters.length === 0" @click="save">
            <i class="fa-solid fa-check" />
            追加 {{ draft.chapters.length }} 章
          </button>
        </div>
        <div v-if="error" class="obd-error">{{ error }}</div>
      </template>
    </div>
  </ElDialog>
</template>

<style scoped lang="scss">
.obd {
  min-height: 160px;
  display: flex;
  flex-direction: column;
}

.obd-form {
  display: flex;
  flex-direction: column;
  gap: 13px;

  .wz-row {
    max-width: 240px;
  }
}

.obd-tip {
  margin: 0 0 4px;
  font-size: 12px;
  line-height: 1.8;
  color: var(--ink-muted);
}

.obd-progress {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
  padding: 36px 0 16px;

  > i {
    font-size: 24px;
    color: var(--ink-accent);
  }

  p {
    font-size: 13px;
    color: var(--ink-muted);
  }
}

.obd-editor {
  max-height: 52vh;
  overflow-y: auto;
  padding-right: 4px;
}

.obd-actions {
  position: sticky;
  bottom: 0;
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 10px;
  background: linear-gradient(transparent, var(--surface-1) 40%);
}

.obd-error {
  margin-top: 10px;
  padding: 8px 12px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--state-danger) 8%, transparent);
  color: var(--state-danger);
  font-size: 12px;
  line-height: 1.7;
  word-break: break-word;
}
</style>
