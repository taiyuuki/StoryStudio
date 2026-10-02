<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'

const route = useRoute()

// 导航项：后续扩展（榜单 / 素材库……）在此追加即可
const navItems = [
  { to: '/', label: '首页', icon: 'fa-house' },
  { to: '/works', label: '作品', icon: 'fa-book-open' },
  { to: '/create', label: 'AI 建书', icon: 'fa-wand-magic-sparkles' },
]

function isActive(to: string): boolean {
  if (to === '/') return route.path === '/'
  return route.path === to || route.path.startsWith(`${to}/`)
}

const settingsActive = computed(() => isActive('/settings'))
</script>

<template>
  <aside class="nav-rail">
    <router-link class="nav-brand" to="/">
      <span class="brand-seal seal-font">书</span>
      <span class="brand-name">StoryStudio</span>
    </router-link>

    <nav class="nav-group">
      <router-link
        v-for="item in navItems"
        :key="item.to"
        class="nav-item"
        :class="{ 'is-active': isActive(item.to) }"
        :to="item.to"
      >
        <i class="fa-solid" :class="item.icon" />
        <span>{{ item.label }}</span>
      </router-link>
    </nav>

    <div class="nav-spacer" />

    <nav class="nav-group nav-group--bottom">
      <router-link class="nav-item" :class="{ 'is-active': settingsActive }" to="/settings">
        <i class="fa-solid fa-gear" />
        <span>设置</span>
      </router-link>
    </nav>
  </aside>
</template>

<style scoped lang="scss">
.nav-rail {
  width: 192px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  padding: 14px 12px;
  background: var(--surface-1);
  border-right: 1px solid var(--divider);
  backdrop-filter: blur(12px);
}

.nav-brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 10px 18px;
  text-decoration: none;
}

.brand-seal {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  font-size: 16px;
  box-shadow: var(--ui-shadow);
  letter-spacing: 0;
  flex-shrink: 0;
}

.brand-name {
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--ink-main);
}

.nav-group {
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.nav-spacer {
  flex: 1;
}

.nav-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 11px;
  height: 38px;
  padding: 0 12px;
  border-radius: 9px;
  border-left: 3px solid transparent;
  color: var(--ink-sec);
  font-size: 13.5px;
  text-decoration: none;
  transition: background 0.18s, color 0.18s;

  i {
    width: 16px;
    text-align: center;
    font-size: 13.5px;
    opacity: 0.85;
  }

  &:hover {
    background: var(--surface-2);
    color: var(--ink-main);
  }

  &.is-active {
    background: var(--nav-active-bg);
    border-left-color: var(--nav-active-bar-color);
    color: var(--ink-main);
    font-weight: 600;

    i {
      color: var(--ink-accent);
      opacity: 1;
    }
  }
}

.nav-group--bottom {
  padding-top: 10px;
  border-top: 1px solid var(--divider);
}
</style>
