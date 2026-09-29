<script setup lang="ts">
import type { BookConceptDto } from '@/api/backend'

const concept = defineModel<BookConceptDto>({ required: true })
</script>

<template>
  <div class="concept-form">
    <div class="wz-row">
      <div class="wz-field">
        <label>书名</label>
        <input v-model="concept.title" maxlength="20" placeholder="2-8 字，有记忆点" />
      </div>
      <div class="wz-field">
        <label>主题</label>
        <input v-model="concept.theme" placeholder="如：身份认同" />
      </div>
    </div>
    <div class="wz-field">
      <label>一句话卖点</label>
      <input v-model="concept.tagline" maxlength="40" placeholder="读者为什么点开这本书" />
    </div>
    <div class="wz-field">
      <label>简介（给读者看）</label>
      <textarea v-model="concept.blurb" rows="3" placeholder="80-150 字" />
    </div>
    <div class="wz-field">
      <label>主线（核心设定与主线脉络，后续章节生成会以此为纲）</label>
      <textarea v-model="concept.premise" rows="5" placeholder="世界基调、主角处境、核心冲突与长线悬念" />
    </div>
    <div class="wz-field">
      <label>故事舞台</label>
      <input v-model="concept.setting" placeholder="故事发生在哪里" />
    </div>
  </div>
</template>

<style scoped lang="scss">
.concept-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
</style>
