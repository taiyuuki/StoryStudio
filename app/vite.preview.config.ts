// 设置页布局预览专用 vite 配置：真插件 + mock Tauri API。验证完连同 preview/ 一起删除。
import { fileURLToPath, URL } from 'node:url'

import vue from '@vitejs/plugin-vue'
import AutoImport from 'unplugin-auto-import/vite'
import Components from 'unplugin-vue-components/vite'
import { ElementPlusResolver } from 'unplugin-vue-components/resolvers'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [
    vue(),
    AutoImport({ imports: ['vue', 'vue-router', 'pinia'], resolvers: [ElementPlusResolver()], dts: false }),
    Components({ resolvers: [ElementPlusResolver()], dts: false }),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@tauri-apps/api/core': fileURLToPath(new URL('./preview/mocks/core.ts', import.meta.url)),
      '@tauri-apps/api/window': fileURLToPath(new URL('./preview/mocks/window.ts', import.meta.url)),
    },
  },
  server: { port: 5299, strictPort: true },
})
