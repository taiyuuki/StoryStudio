<script setup lang="ts">
import { computed } from 'vue'
import type { OutlineBatchDraftDto } from '@/api/backend'

const draft = defineModel<OutlineBatchDraftDto>({ required: true })

/** 可选的出场角色（由调用方传入，续排时来自已保存实体） */
const props = defineProps<{
  characterNames?: string[]
  /** 已有章节数（编号从这之后开始显示） */
  existingCount?: number
}>()

const volumeOptions = computed(() => {
  const list = draft.value.volumes.map((v) => v.order)
  return list.length > 0 ? list : [1]
})

function volumeLabel(order: number): string {
  const v = draft.value.volumes.find((x) => x.order === order)
  return v?.name ? `第${order}卷 · ${v.name}` : `第${order}卷`
}

function addVolume() {
  const next = (draft.value.volumes.map((v) => v.order).reduce((a, b) => Math.max(a, b), 0) || 0) + 1
  draft.value.volumes.push({ name: '', order: next, theme: '', summary: '' })
}

function addChapter() {
  const last = draft.value.chapters[draft.value.chapters.length - 1]
  draft.value.chapters.push({
    title: '',
    volumeOrder: last?.volumeOrder ?? volumeOptions.value[0],
    summary: '',
    goal: '',
    beats: [],
    characters: [],
    hook: '',
  })
}
</script>

<template>
  <div class="outline-form">
    <!-- 卷 -->
    <section v-if="draft.volumes.length > 0">
      <h4 class="wz-sec">
        <i class="fa-solid fa-layer-group" />
        卷分组
      </h4>
      <div class="wz-cards">
        <div v-for="(v, i) in draft.volumes" :key="i" class="wz-card wz-card--flat">
          <button class="wz-card-del" @click="draft.volumes.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="wz-row">
            <div class="wz-field" style="max-width: 80px">
              <label>序号</label>
              <input v-model.number="v.order" type="number" min="1" />
            </div>
            <div class="wz-field"><label>卷名</label><input v-model="v.name" placeholder="如：第一卷 灯下" /></div>
            <div class="wz-field"><label>主题</label><input v-model="v.theme" /></div>
          </div>
          <div class="wz-field"><label>本卷梗概</label><textarea v-model="v.summary" rows="2" /></div>
        </div>
      </div>
      <button class="wz-add" @click="addVolume"><i class="fa-solid fa-plus" /> 加一卷</button>
    </section>

    <!-- 章节 -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-list-ol" />
        章节脉络
        <span class="num">共 {{ draft.chapters.length }} 章</span>
      </h4>
      <div class="wz-cards">
        <div v-for="(c, i) in draft.chapters" :key="i" class="wz-card">
          <button class="wz-card-del" title="删除这一章" @click="draft.chapters.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="ch-head">
            <span class="ch-idx num">第 {{ (existingCount ?? 0) + i + 1 }} 章</span>
            <div class="wz-field ch-title">
              <input v-model="c.title" placeholder="章节标题" />
            </div>
            <div class="wz-field ch-vol">
              <select v-model.number="c.volumeOrder">
                <option v-for="o in volumeOptions" :key="o" :value="o">{{ volumeLabel(o) }}</option>
              </select>
            </div>
          </div>
          <div class="wz-row">
            <div class="wz-field"><label>梗概</label><textarea v-model="c.summary" rows="2" /></div>
            <div class="wz-field"><label>本章目标</label><textarea v-model="c.goal" rows="2" /></div>
          </div>
          <div class="wz-field">
            <label>节拍（一行一个）</label>
            <textarea
              :value="c.beats.join('\n')"
              rows="3"
              placeholder="开场&#10;推进&#10;转折&#10;收束"
              @change="c.beats = ($event.target as HTMLTextAreaElement).value.split('\n').map((s) => s.trim()).filter(Boolean)"
            />
          </div>
          <div class="wz-row">
            <div class="wz-field">
              <label>出场角色（逗号分隔）</label>
              <input
                v-if="!characterNames?.length"
                :value="c.characters.join('，')"
                @change="c.characters = ($event.target as HTMLInputElement).value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)"
              />
              <input
                v-else
                :value="c.characters.join('，')"
                :list="`wz-chars-${i}`"
                @change="c.characters = ($event.target as HTMLInputElement).value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)"
              />
              <datalist v-if="characterNames?.length" :id="`wz-chars-${i}`">
                <option v-for="n in characterNames" :key="n" :value="n" />
              </datalist>
            </div>
            <div class="wz-field"><label>章末钩子</label><textarea v-model="c.hook" rows="2" placeholder="让读者翻页的理由" /></div>
          </div>
        </div>
      </div>
      <button class="wz-add" @click="addChapter"><i class="fa-solid fa-plus" /> 加一章</button>
    </section>
  </div>
</template>

<style scoped lang="scss">
.outline-form {
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.wz-sec {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0 0 9px;
  font-size: 12.5px;
  font-weight: 600;
  color: var(--ink-main);

  i {
    color: var(--ink-accent);
    font-size: 12px;
  }

  .num {
    font-weight: 400;
    color: var(--ink-disabled);
  }
}

.wz-cards {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 8px;
}

.wz-card--flat {
  padding: 8px 12px;
}

.ch-head {
  display: flex;
  align-items: center;
  gap: 8px;

  .ch-idx {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--ink-accent);
  }

  .ch-title {
    flex: 1;
  }

  .ch-vol {
    width: 170px;
    flex-shrink: 0;
  }
}
</style>
