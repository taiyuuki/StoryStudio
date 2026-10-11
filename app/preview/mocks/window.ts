// 浏览器预览用：AppTitleBar 等组件的窗口 API 桩（/settings 路由不加载它，保险起见）
export function getCurrentWindow() {
  return {
    label: 'main',
    minimize: async () => {},
    toggleMaximize: async () => {},
    close: async () => {},
    isMaximized: async () => false,
    setTitle: async () => {},
    listen: async () => () => {},
  }
}
