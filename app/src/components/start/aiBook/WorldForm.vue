<script setup lang="ts">
import { computed } from 'vue'
import type {
  ConceptItemDto,
  LocationItemDto,
  OrganizationItemDto,
  SystemItemDto,
  WorldDraftDto,
} from '@/api/backend'

const draft = defineModel<WorldDraftDto>({ required: true })

const ROLES: [string, string][] = [
  ['protagonist', '主角'],
  ['love_interest', '感情线'],
  ['antagonist', '反派'],
  ['mentor', '导师'],
  ['support', '配角'],
]
const SYSTEM_KINDS = ['cultivation', 'magic', 'technology', 'power']
const LOCATION_KINDS: [string, string][] = [
  ['continent', '大陆'],
  ['ocean', '海洋'],
  ['plane', '位面'],
  ['planet', '星球'],
  ['nation', '国家'],
  ['kingdom', '王国'],
  ['empire', '帝国'],
  ['region', '区域'],
  ['island', '岛屿'],
  ['mountain', '山脉'],
  ['forest', '森林'],
  ['desert', '沙漠'],
  ['river', '河流'],
  ['lake', '湖泊'],
  ['landform', '其他地形'],
  ['city', '城市'],
  ['town', '城镇'],
  ['village', '村庄'],
  ['settlement', '聚落'],
  ['building', '建筑'],
  ['ruins', '遗迹'],
  ['realm', '秘境'],
]
const ORG_KINDS = ['sect', 'nation', 'clan', 'guild', 'organization']

const charNames = computed(() => draft.value.characters.map((c) => c.name).filter(Boolean))
const locationNames = computed(() => draft.value.locations.map((l) => l.name).filter(Boolean))

function addCharacter() {
  draft.value.characters.push({
    name: '',
    role: 'support',
    gender: '',
    occupation: '',
    personality: '',
    background: '',
    aliases: [],
  })
}
function addConcept() {
  draft.value.concepts.push({ name: '', category: '', summary: '' })
}
function addSystem() {
  draft.value.systems.push({ name: '', kind: 'magic', levels: '', rules: '', summary: '' })
}
function addLocation() {
  draft.value.locations.push({ name: '', kind: 'city', parent: '', features: '', summary: '' })
}
function addOrganization() {
  draft.value.organizations.push({ name: '', kind: 'sect', leader: '', territory: '', ideology: '', summary: '' })
}
function addRelation() {
  draft.value.relations.push({ from: '', to: '', relation: '' })
}
function addThread() {
  draft.value.threads.push({ title: '', description: '', priority: 'minor' })
}
</script>

<template>
  <div class="world-form">
    <!-- 主要角色 -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-users" />
        主要角色
        <span class="num">{{ draft.characters.length }}</span>
      </h4>
      <div class="wz-cards">
        <div v-for="(c, i) in draft.characters" :key="i" class="wz-card">
          <button class="wz-card-del" title="删除这个角色" @click="draft.characters.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="wz-row">
            <div class="wz-field">
              <label>姓名</label>
              <input v-model="c.name" placeholder="角色名" />
            </div>
            <div class="wz-field">
              <label>定位</label>
              <select v-model="c.role">
                <option v-for="[v, label] in ROLES" :key="v" :value="v">{{ label }}</option>
              </select>
            </div>
            <div class="wz-field">
              <label>性别</label>
              <input v-model="c.gender" placeholder="可留空" />
            </div>
            <div class="wz-field">
              <label>职业</label>
              <input v-model="c.occupation" placeholder="如：游方医" />
            </div>
          </div>
          <div class="wz-row">
            <div class="wz-field">
              <label>性格</label>
              <textarea v-model="c.personality" rows="2" />
            </div>
            <div class="wz-field">
              <label>别名（逗号分隔）</label>
              <input
                :value="c.aliases.join('，')"
                placeholder="小名、绰号、化名"
                @change="c.aliases = ($event.target as HTMLInputElement).value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)"
              />
            </div>
          </div>
          <div class="wz-field">
            <label>背景</label>
            <textarea v-model="c.background" rows="2" />
          </div>
        </div>
      </div>
      <button class="wz-add" @click="addCharacter"><i class="fa-solid fa-plus" /> 加一个角色</button>
    </section>

    <!-- 世界观 -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-globe" />
        世界观
      </h4>
      <ElCollapse class="wz-collapse">
        <ElCollapseItem :title="`基础概念（${draft.concepts.length}）`" name="concepts">
          <div class="wz-cards">
            <div v-for="(c, i) in draft.concepts" :key="i" class="wz-card">
              <button class="wz-card-del" @click="draft.concepts.splice(i, 1)">
                <i class="fa-regular fa-trash-can" />
              </button>
              <div class="wz-row">
                <div class="wz-field"><label>名称</label><input v-model="c.name" /></div>
                <div class="wz-field"><label>分类</label><input v-model="c.category" placeholder="世界规则 / 纪元 / 种族…" /></div>
              </div>
              <div class="wz-field"><label>说明</label><textarea v-model="c.summary" rows="2" /></div>
            </div>
          </div>
          <button class="wz-add" @click="addConcept"><i class="fa-solid fa-plus" /> 加概念</button>
        </ElCollapseItem>

        <ElCollapseItem :title="`力量体系（${draft.systems.length}）`" name="systems">
          <div class="wz-cards">
            <div v-for="(s, i) in draft.systems" :key="i" class="wz-card">
              <button class="wz-card-del" @click="draft.systems.splice(i, 1)">
                <i class="fa-regular fa-trash-can" />
              </button>
              <div class="wz-row">
                <div class="wz-field"><label>名称</label><input v-model="s.name" /></div>
                <div class="wz-field">
                  <label>类型</label>
                  <select v-model="s.kind">
                    <option v-for="k in SYSTEM_KINDS" :key="k" :value="k">{{ k }}</option>
                  </select>
                </div>
              </div>
              <div class="wz-field"><label>等级链</label><textarea v-model="s.levels" rows="2" placeholder="炼气→筑基→金丹" /></div>
              <div class="wz-field"><label>规则与代价</label><textarea v-model="s.rules" rows="2" /></div>
            </div>
          </div>
          <button class="wz-add" @click="addSystem"><i class="fa-solid fa-plus" /> 加体系</button>
        </ElCollapseItem>

        <ElCollapseItem :title="`组织与势力（${draft.organizations.length}）`" name="organizations">
          <div class="wz-cards">
            <div v-for="(f, i) in draft.organizations" :key="i" class="wz-card">
              <button class="wz-card-del" @click="draft.organizations.splice(i, 1)">
                <i class="fa-regular fa-trash-can" />
              </button>
              <div class="wz-row">
                <div class="wz-field"><label>名称</label><input v-model="f.name" /></div>
                <div class="wz-field">
                  <label>类型</label>
                  <select v-model="f.kind">
                    <option v-for="k in ORG_KINDS" :key="k" :value="k">{{ k }}</option>
                  </select>
                </div>
                <div class="wz-field"><label>首领</label><input v-model="f.leader" /></div>
              </div>
              <div class="wz-row">
                <div class="wz-field"><label>势力范围</label><input v-model="f.territory" /></div>
                <div class="wz-field"><label>理念</label><textarea v-model="f.ideology" rows="2" /></div>
              </div>
              <div class="wz-field"><label>简介</label><textarea v-model="f.summary" rows="2" /></div>
            </div>
          </div>
          <button class="wz-add" @click="addOrganization"><i class="fa-solid fa-plus" /> 加组织</button>
        </ElCollapseItem>

        <ElCollapseItem title="纪元（故事内历法）" name="eras">
          <div class="wz-field">
            <label>纪元名（逗号分隔，按时间顺序）</label>
            <input
              :value="draft.eras.join('，')"
              placeholder="如：创世，灵历，开元"
              @change="draft.eras = ($event.target as HTMLInputElement).value.split(/[,，]/).map((s) => s.trim()).filter(Boolean)"
            />
          </div>
        </ElCollapseItem>
      </ElCollapse>
    </section>

    <!-- 地点（世界地理 + 剧情发生地，统一为 Location 实体） -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-map-location-dot" />
        地点（世界地理与剧情发生地）
      </h4>
      <div class="wz-cards">
        <div v-for="(l, i) in draft.locations" :key="i" class="wz-card">
          <button class="wz-card-del" @click="draft.locations.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="wz-row">
            <div class="wz-field"><label>名称</label><input v-model="l.name" /></div>
            <div class="wz-field">
              <label>类型</label>
              <select v-model="l.kind">
                <option v-if="!LOCATION_KINDS.some(([v]) => v === l.kind)" :value="l.kind">{{ l.kind }}</option>
                <option v-for="[v, label] in LOCATION_KINDS" :key="v" :value="v">{{ label }}</option>
              </select>
            </div>
            <div class="wz-field">
              <label>上级</label>
              <select v-model="l.parent">
                <option value="">无</option>
                <option v-for="n in locationNames.filter((x) => x !== l.name)" :key="n" :value="n">{{ n }}</option>
              </select>
            </div>
          </div>
          <div class="wz-field"><label>特征</label><textarea v-model="l.features" rows="2" placeholder="风貌、遗迹、气候等" /></div>
          <div class="wz-field"><label>说明</label><input v-model="l.summary" /></div>
        </div>
      </div>
      <button class="wz-add" @click="addLocation"><i class="fa-solid fa-plus" /> 加地点</button>
    </section>

    <!-- 关系 -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-arrows-left-right" />
        角色关系
      </h4>
      <div class="wz-cards">
        <div v-for="(r, i) in draft.relations" :key="i" class="wz-card wz-card--flat">
          <button class="wz-card-del" @click="draft.relations.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="wz-row">
            <div class="wz-field">
              <label>甲方</label>
              <select v-model="r.from">
                <option value="">选择角色</option>
                <option v-for="n in charNames" :key="n" :value="n">{{ n }}</option>
              </select>
            </div>
            <div class="wz-field">
              <label>乙方</label>
              <select v-model="r.to">
                <option value="">选择角色</option>
                <option v-for="n in charNames" :key="n" :value="n">{{ n }}</option>
              </select>
            </div>
            <div class="wz-field">
              <label>关系</label>
              <input v-model="r.relation" placeholder="师徒 / 宿敌 / 兄妹" />
            </div>
          </div>
        </div>
      </div>
      <button class="wz-add" @click="addRelation"><i class="fa-solid fa-plus" /> 加关系</button>
    </section>

    <!-- 叙事线索 -->
    <section>
      <h4 class="wz-sec">
        <i class="fa-solid fa-route" />
        叙事线索（伏笔与悬念）
      </h4>
      <div class="wz-cards">
        <div v-for="(t, i) in draft.threads" :key="i" class="wz-card">
          <button class="wz-card-del" @click="draft.threads.splice(i, 1)">
            <i class="fa-regular fa-trash-can" />
          </button>
          <div class="wz-row">
            <div class="wz-field"><label>标题</label><input v-model="t.title" /></div>
            <div class="wz-field" style="max-width: 110px">
              <label>级别</label>
              <select v-model="t.priority">
                <option value="main">主线</option>
                <option value="minor">支线</option>
              </select>
            </div>
          </div>
          <div class="wz-field"><label>说明</label><textarea v-model="t.description" rows="2" /></div>
        </div>
      </div>
      <button class="wz-add" @click="addThread"><i class="fa-solid fa-plus" /> 加线索</button>
    </section>
  </div>
</template>

<style scoped lang="scss">
.world-form {
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

.wz-collapse {
  border: none;

  :deep(.el-collapse-item__header) {
    font-size: 12px;
    color: var(--ink-muted);
    background: transparent;
    border-bottom-color: var(--divider);
  }

  :deep(.el-collapse-item__wrap) {
    background: transparent;
    border-bottom-color: var(--divider);
  }

  :deep(.el-collapse-item__content) {
    padding: 10px 0 4px;
  }
}
</style>
