import { createRouter, createWebHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      // 应用壳：导航栏 + 分区页面（首页 / 作品 / AI 建书 / 设置）
      path: '/',
      component: () => import('@/layouts/NavShell.vue'),
      children: [
        { path: '', name: 'home', component: () => import('@/views/HomeView.vue') },
        { path: 'works', name: 'works', component: () => import('@/views/WorksView.vue') },
        { path: 'create', name: 'create', component: () => import('@/views/CreateBookView.vue') },
        {
          path: 'settings',
          name: 'settings',
          component: () => import('@/views/SettingsView.vue'),
        },
      ],
    },
    {
      // 工作台：全屏三栏编辑，不进导航壳
      path: '/workbench',
      name: 'workbench',
      component: () => import('@/views/WorkbenchView.vue'),
    },
  ],
})
