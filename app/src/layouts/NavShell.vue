<script setup lang="ts">
import NavRail from '@/components/NavRail.vue'
</script>

<template>
  <div class="nav-shell">
    <NavRail />
    <div class="nav-body">
      <router-view />
    </div>
  </div>
</template>

<style scoped lang="scss">
.nav-shell {
  display: flex;
  height: 100%;
  min-height: 0;
}

.nav-body {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
</style>
