<script setup lang="ts">
import { computed, ref } from 'vue'
import type { BookMetaDto } from '@/api/backend'
import { AUDIENCES, GENRES, POVS, SCALES, STYLES, WORD_OPTIONS } from '@/constants/bookMeta'

// 书籍信息表单字段区（无外壳）——手动新建对话框与工作台书籍信息对话框共用。
// 文本不做即时 trim（避免打断输入），保存前由调用方 normalizeBookMeta 归一。

const props = defineProps<{ modelValue: BookMetaDto }>()
const emit = defineEmits<{ 'update:modelValue': [meta: BookMetaDto] }>()

// 「自动」是建书向导专用（交给 AI 判断），表单里不出现
const genreChips = GENRES.filter((g) => g.name !== '自动')
const customGenre = ref('')

function set<K extends keyof BookMetaDto>(key: K, value: BookMetaDto[K]) {
  emit('update:modelValue', { ...props.modelValue, [key]: value })
}

function genreActive(name: string) {
  return !customGenre.value.trim() && props.modelValue.genre === name
}

function pickGenre(name: string) {
  customGenre.value = ''
  set('genre', props.modelValue.genre === name ? null : name)
}

function onCustomGenre(v: string) {
  customGenre.value = v
  set('genre', v.trim() || null)
}

const scaleValue = computed({
  get: () => (props.modelValue.targetWords == null ? '' : String(props.modelValue.targetWords)),
  set: (v: string) => set('targetWords', v === '' ? null : Number(v)),
})

const wordsValue = computed({
  get: () => (props.modelValue.chapterWords == null ? '' : String(props.modelValue.chapterWords)),
  set: (v: string) => set('chapterWords', v === '' ? null : Number(v)),
})

const scaleOptions = SCALES.map(([label, n]) => [String(n), label] as const)
const wordOptions = WORD_OPTIONS.map((n) => [String(n), n] as const)
</script>

<template>
  <div class="bm-form">
    <div class="bm-field">
      <label>题材</label>
      <div class="bm-chips">
        <button
          v-for="g in genreChips"
          :key="g.name"
          type="button"
          class="bm-chip"
          :class="{ 'is-active': genreActive(g.name) }"
          @click="pickGenre(g.name)"
        >
          <i class="fa-solid" :class="g.icon" />
          {{ g.name }}
        </button>
      </div>
      <input
        class="bm-custom"
        :value="customGenre"
        placeholder="或自定义题材（填入后优先生效）"
        @input="onCustomGenre(($event.target as HTMLInputElement).value)"
      />
    </div>

    <div class="bm-field">
      <label>一句话主线 <span class="bm-tip">续排大纲与逐章成书的走向基准</span></label>
      <textarea
        rows="2"
        :value="modelValue.premise ?? ''"
        placeholder="如：灵灯将熄，守灯少年查明百年前祖师熄灯之谜"
        @input="set('premise', ($event.target as HTMLTextAreaElement).value || null)"
      />
    </div>

    <div class="bm-field">
      <label>主题</label>
      <input
        :value="modelValue.theme ?? ''"
        placeholder="如：执念与传承（可选）"
        @input="set('theme', ($event.target as HTMLInputElement).value || null)"
      />
    </div>

    <div class="bm-cols">
      <div class="bm-field">
        <label>面向人群</label>
        <select
          :value="modelValue.audience ?? ''"
          @change="set('audience', ($event.target as HTMLSelectElement).value || null)"
        >
          <option value="">未设置</option>
          <option v-for="a in AUDIENCES" :key="a" :value="a">{{ a }}</option>
        </select>
      </div>
      <div class="bm-field">
        <label>叙事视角</label>
        <select
          :value="modelValue.pov ?? ''"
          @change="set('pov', ($event.target as HTMLSelectElement).value || null)"
        >
          <option value="">未设置</option>
          <option v-for="p in POVS" :key="p" :value="p">{{ p }}</option>
        </select>
      </div>
      <div class="bm-field">
        <label>文风基调</label>
        <select
          :value="modelValue.style ?? ''"
          @change="set('style', ($event.target as HTMLSelectElement).value || null)"
        >
          <option value="">未设置</option>
          <option v-for="s in STYLES" :key="s" :value="s">{{ s }}</option>
        </select>
      </div>
    </div>

    <div class="bm-cols bm-cols--2">
      <div class="bm-field">
        <label>体量（目标字数）</label>
        <select v-model="scaleValue">
          <option value="">未设置</option>
          <option v-for="[v, l] in scaleOptions" :key="v" :value="v">{{ l }}</option>
        </select>
      </div>
      <div class="bm-field">
        <label>单章字数</label>
        <select v-model="wordsValue">
          <option value="">未设置</option>
          <option v-for="[v, n] in wordOptions" :key="v" :value="v">{{ n }} 字</option>
        </select>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.bm-form {
  display: flex;
  flex-direction: column;
  gap: 13px;
}

.bm-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;

  label {
    font-size: 12px;
    font-weight: 600;
    color: var(--ink-main);
  }
}

.bm-tip {
  font-size: 10.5px;
  font-weight: 400;
  color: var(--ink-disabled);
}

.bm-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.bm-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 27px;
  padding: 0 11px;
  border: 1px solid var(--ui-border);
  border-radius: 14px;
  background: transparent;
  color: var(--ink-sec);
  font-family: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;

  i {
    font-size: 10.5px;
  }

  &:hover {
    border-color: var(--ink-accent);
    color: var(--ink-accent);
  }

  &.is-active {
    border-color: var(--ink-accent);
    background: var(--ink-accent-soft);
    color: var(--ink-accent);
  }
}

.bm-custom,
.bm-form input,
.bm-form textarea,
.bm-form select {
  width: 100%;
  padding: 7px 10px;
  border-radius: 7px;
  border: 1px solid var(--ui-border);
  background: var(--input-bg);
  color: var(--ink-main);
  font-family: inherit;
  font-size: 12.5px;
  outline: none;

  &:focus {
    border-color: var(--ink-accent);
  }
}

.bm-form textarea {
  resize: vertical;
  line-height: 1.6;
}

.bm-custom {
  margin-top: 2px;
}

.bm-cols {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;

  &--2 {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
