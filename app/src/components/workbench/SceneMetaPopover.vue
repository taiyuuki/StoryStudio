<script setup lang="ts">
import { reactive, watch } from 'vue'
import { useWorkspaceStore } from '@/stores/workspace'
import type { StoryTimeDto } from '@/api/backend'

const emit = defineEmits<{ close: [] }>()
const store = useWorkspaceStore()

const form = reactive({
  title: '',
  pov: '' as string,
  mood: '',
  year: null as number | null,
  month: null as number | null,
  day: null as number | null,
})

watch(
  () => store.scene?.id,
  () => {
    const s = store.scene
    if (!s) return
    form.title = s.title ?? ''
    form.pov = s.pov ?? ''
    form.mood = s.mood ?? ''
    form.year = s.storyTime?.year ?? null
    form.month = s.storyTime?.month ?? null
    form.day = s.storyTime?.day ?? null
  },
  { immediate: true },
)

async function save() {
  if (!store.scene) return
  const storyTime: StoryTimeDto | null =
    form.year != null
      ? { year: form.year, month: form.month, day: form.day }
      : null
  await store.updateScene(store.scene.id, {
    title: form.title,
    pov: form.pov || null,
    mood: form.mood || null,
    storyTime,
  })
  emit('close')
}
</script>

<template>
  <div class="meta-form">
    <div class="mf-row">
      <label>场景标题</label>
      <input v-model="form.title" placeholder="场景标题" />
    </div>

    <div class="mf-row">
      <label>视角角色</label>
      <select v-model="form.pov">
        <option value="">（未指定）</option>
        <option v-for="c in store.characters" :key="c.id" :value="c.id">
          {{ c.displayName }}
        </option>
      </select>
    </div>

    <div class="mf-row">
      <label>情绪基调</label>
      <input v-model="form.mood" placeholder="例：压抑、惊惧" />
    </div>

    <div class="mf-row">
      <label>故事时间</label>
      <div class="time-row">
        <input v-model.number="form.year" type="number" placeholder="年" class="num" />
        <input v-model.number="form.month" type="number" placeholder="月" min="1" max="12" class="num" />
        <input v-model.number="form.day" type="number" placeholder="日" min="1" max="31" class="num" />
      </div>
    </div>

    <div class="mf-actions">
      <button class="ink-btn ink-btn--sm ink-btn--primary" @click="save">保存</button>
    </div>
  </div>
</template>

<style scoped lang="scss">
.meta-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 4px 2px;
}

.mf-row {
  display: flex;
  flex-direction: column;
  gap: 6px;

  label {
    font-size: 12px;
    color: var(--ink-sec);
    letter-spacing: 0.06em;
  }

  input,
  select,
  textarea {
    padding: 8px 11px;
    border-radius: 7px;
    border: 1px solid var(--ui-border);
    background: var(--input-bg);
    color: var(--ink-main);
    font-family: inherit;
    font-size: 14px;
    outline: none;
    transition: border-color 0.2s;

    &:focus {
      border-color: var(--ink-accent);
    }

    &::placeholder {
      color: var(--ink-disabled);
    }
  }

  textarea {
    resize: vertical;
    line-height: 1.7;
  }
}

.time-row {
  display: flex;
  gap: 8px;

  input {
    width: 0;
    flex: 1;
  }
}

.mf-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
