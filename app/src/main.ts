import { createApp } from 'vue'
import { createPinia } from 'pinia'
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate'

import '@fortawesome/fontawesome-free/css/all.min.css'
// ElMessage / ElMessageBox 为代码直调（不经模板解析器），样式需手动引入
import 'element-plus/es/components/message/style/css'
import 'element-plus/es/components/message-box/style/css'
import '@/styles/common.scss'

import App from './App.vue'
import { router } from './router'
import { initFormAutoGrow } from './form-autogrow'

const app = createApp(App)
const pinia = createPinia()
pinia.use(piniaPluginPersistedstate)

app.use(pinia)
app.use(router)
app.mount('#app')

initFormAutoGrow()
