<script setup lang="ts">
import { ref, watch } from 'vue'
import { useWorkspaceStore } from '@/stores/workspace'

const model = defineModel<boolean>({ default: false })
const store = useWorkspaceStore()

const instruction = ref('')

watch(model, (open) => {
  if (open) instruction.value = ''
})

async function run() {
  model.value = false
  await store.aiWriteScene(instruction.value.trim() || null)
}
</script>

<template>
  <ElDialog
    v-model="model"
    title="续写本场景"
    width="520px"
    :close-on-click-modal="!store.aiWriting"
  >
    <div class="ai-dialog">
      <p class="ai-tip">
        <i class="fa-solid fa-circle-info" />
        妙笔会结合本场景已写内容和视角角色的设定进行续写，写好的内容会自动接在场景末尾。
      </p>
      <textarea
        v-model="instruction"
        rows="4"
        placeholder="有什么想交代的吗？（可不填）&#10;例：写一段雨夜中的追逐，结尾埋下玉佩的伏笔"
        :disabled="store.aiWriting"
      />
    </div>
    <template #footer>
      <button class="ink-btn" :disabled="store.aiWriting" @click="model = false">取消</button>
      <button
        class="ink-btn ink-btn--primary"
        style="margin-left: 10px"
        :disabled="store.aiWriting"
        @click="run"
      >
        <i class="fa-solid fa-feather-pointed" />
        开始续写
      </button>
    </template>
  </ElDialog>
</template>

<style scoped lang="scss">
.ai-dialog {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.ai-tip {
  display: flex;
  gap: 8px;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--ink-muted);

  i {
    color: var(--ink-accent);
    margin-top: 4px;
  }
}

textarea {
  width: 100%;
  padding: 10px 12px;
  border-radius: 8px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 13px;
  line-height: 1.8;
  resize: vertical;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }

  &::placeholder {
    color: var(--ink-disabled);
  }
}
</style>
