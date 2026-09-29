# StoryStudio 桌面端

水墨风小说创作工作台 —— 基于 Tauri 2 + Vue 3，直接编排 `story-studio-lib`，
所有数据仍是磁盘上的纯文本项目（TOML + Markdown），与 CLI 完全互操作。

## 启动

```bash
cd app
pnpm install
pnpm tauri dev      # 开发（vite :5190 + Rust 自动重编译）
pnpm tauri build    # 打包 .app / .dmg
```

## 结构

```
app/
├── src/                    # Vue 3 前端
│   ├── api/backend.ts      # invoke 封装 + DTO 类型（与 src-tauri/src/dto.rs 一一对应）
│   ├── stores/workspace.ts # 项目数据编排（单一数据源）
│   ├── stores/ui.ts        # 主题 / 最近项目 / 面板状态（localStorage 持久化）
│   ├── styles/             # 水墨主题 token（素宣墨白 / 暗夜水墨）+ Element Plus 运行时映射
│   ├── views/              # StartView 项目选择 / WorkbenchView 三栏工作台
│   └── components/         # 标题栏、结构树、纸面编辑器(CodeMirror 6)、右侧四面板
└── src-tauri/              # Rust 侧（workspace 成员 story-studio-app）
    └── src/commands.rs     # Tauri 命令，全部转发 studio-lib
```

## 功能

- **项目**：新建（拼音 slug 目录）/ 打开 / 最近列表；与 `story-studio` CLI 项目互通
- **结构**：章节 / 场景 CRUD（级联删除有确认），结构树搜索
- **写作**：纸面编辑器（衬线排版、片段分片、装订线序号）、800ms 防抖自动保存、
  片段上移/下移/插入/删除、场景设定（POV / 情绪 / 摘要 / 故事时间）
- **数据安全**：写入回读校验 + 取证日志（`.novelstudio/logs/`）、片段版本历史
  （保存时节流快照：距上版 ≥5 分钟或字数变化 ≥200；删除/恢复前强制留底）
- **编辑器可靠性**：IME 组词保护（组词中间态不落盘）、同片段保存单飞互斥
  （在途请求期间的新版本自动补跑）、保存失败可点击重试
- **码字统计**：手写 / AI 双账本（今日、近 7 天、连续天数、每日目标），
  状态栏「今日」可点入统计面板；按本地日历切日
- **AI 账本**：续写 / 断言抽取 / 妙笔对话 / 逐章成书的 token、耗时、成败记账
- **实体**：角色 / 地点 / 道具 / 组织四类，档案表单编辑、别名、关系网（预设关系）
- **线索**：主线 / 支线，状态（构想→埋设→揭示→回收 / 弃用）自动推导
- **诊断**：25 条内置 lint（死亡角色复活、年龄矛盾、孤儿引用……），变更后自动重查
- **导出**：整本 / 按章 Markdown，可带角色表附录与章节编号
- **AI 续写**：走全局配置（设置弹窗，存 `~/.story-studio/config.json`），
  以当前激活档案续写场景，生成结果追加为新片段
- **AI 设置（独立页面 /settings）**：左侧档案列表（含上次测试状态点与用途
  徽标）+ 右侧编辑表单，单档案独立保存不再整窗退出；内置 OpenAI / Anthropic /
  DeepSeek / 智谱 / Kimi / 通义 / OpenRouter / 硅基流动 / Gemini / Ollama(本地)
  预设；模型名一键拉取列表；私有参数与自定义请求头（JSON）支持各服务私有字段；
  每档案「测试连接」（先试零消耗的模型列表，失败退最小生成调用）并记录上次结果
- **按用途分配 AI**：续写正文 / 妙笔对话 / 逐章成书 / AI 建书可各用不同档案
  （例：续写配便宜快速的模型、对话配强模型）；未分配的用途跟随默认档案，
  悬空分配（档案已删）自动回退
- **AI 建书（分步向导）**：一句话灵感 → 3 个候选方向挑一个改定 → 世界观与
  人物设定（结构化表单增删改）→ 首批章节大纲（编辑后保存）→ 打开项目。
  支持体量 / 单章字数 / 面向人群 / 叙事视角 / 文风 / 角色数等参数，
  参数随项目持久化（outline.toml），逐章成书与续排都会遵守
- **续排大纲**：长篇按批排纲——「逐章成书」面板随时续排，AI 会参考
  主线、最近脉络与已写完章节的梗概接着排；待写章节用完时提示

## 图标

`app/scripts/gen_icon.swift`（Swift + CoreText 渲染「书」字 + 朱印）→
`tauri icon` 生成全套。重新生成：

```bash
swift scripts/gen_icon.swift src-tauri/icons/app-icon.png
pnpm exec tauri icon src-tauri/icons/app-icon.png
```
