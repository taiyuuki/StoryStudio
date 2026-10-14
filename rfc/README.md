# RFC 索引

Story Studio 的设计文档集。RFC-0001 ~ RFC-0018 写于 2026-07-10 ~ 07-12，
先于代码仓库（2026-09-30 初始化）近三个月，是 crates/ 下 48 个源文件注释中
`RFC-xxxx §x.x` 引用的出处。

状态于 2026-10-12 对照代码库逐篇核对。RFC-0019 起为代码期设计文档
（为已落地功能补设计，或为新功能先立设计再实现）。

## 状态一览

| RFC | 标题 | 状态 | 备注 |
|-----|------|------|------|
| [0001](RFC-0001.md) | 愿景与总体架构 | ✅ Implemented | 四层架构与双索引均已落地 |
| [0002](RFC-0002.md) | Story IR | ✅ Implemented | crates/ir，双图 + 三重 ID |
| [0003](RFC-0003.md) | Project Format | ✅ Implemented | crates/project-format，被引用最多 |
| [0004](RFC-0004.md) | Compiler | ✅ Implemented | Rule / RuleRegistry / DryRun |
| [0005](RFC-0005.md) | Diagnostics | ⚠️ Partially | §6 抑制机制（内联抑制 / 规则禁用）未做 |
| [0006](RFC-0006.md) | Parser | ✅ Implemented | 解析管线 + 确定性实体链接 |
| [0007](RFC-0007.md) | Scene Model | ✅ Implemented | POV / Mood / 故事时间 |
| [0008](RFC-0008.md) | Character Model | ✅ Implemented | 自定义属性随插件系统未做 |
| [0009](RFC-0009.md) | Timeline | ✅ Implemented | StoryTime + 纪元历法 |
| [0010](RFC-0010.md) | AI Runtime | ✅ Implemented | 配置形态已演进为全局档案制（见文内说明） |
| [0011](RFC-0011.md) | Plugin System | ❌ Not Implemented | RFC 自身声明 v0.1 不做，trait 已预留 |
| [0012](RFC-0012.md) | Graph | ⚠️ Partially | 仅关系图落地；时间线 / 热度图 / 叙事结构视图未做 |
| [0013](RFC-0013.md) | Export | ⚠️ Partially | 仅 Markdown；EPUB / PDF / HTML 与 Keep 模式未做 |
| [0014](RFC-0014.md) | World Model | ✅ Implemented | Location / Item / Organization |
| [0015](RFC-0015.md) | Knowledge Extraction & Evidence Mode | ✅ Implemented | 断言 / 证据 / 抽取提交链路 |
| [0016](RFC-0016.md) | Story Outline | ✅ Implemented | outline.toml + 续排大纲 |
| [0017](RFC-0017.md) | Narrative Threads | ✅ Implemented | threads.toml + 状态推导 |
| [0018](RFC-0018.md) | Story Mechanics | ❌ Not Implemented | 现有豁免均为规则内硬编码特例 |
| [0019](RFC-0019.md) | File Watch & Live Sync | ✅ Implemented | Phase 1 落地；Phase 2 可选项（L3 记账 / 聚焦兜底 / 冲突 diff）未做 |

0001 ~ 0018 合计：Implemented 12 / Partially 3 / Not Implemented 2；
0019 起为 Draft / Implemented 状态随实现更新。

## 状态含义

- **Implemented** —— 设计已按本文档落地（细节偏差见文内 Status 注记）。
- **Partially Implemented** —— 核心落地，未实现部分在文内 Status 注记中标明。
- **Not Implemented** —— 仅有设计，代码中没有对应实现。

## 未走 RFC 的功能

RFC-0001 ~ 0018 覆盖 2026-07 的基础设计期。此后（尤其 2026-09 仓库初始化后）
大量功能直接开发、未补 RFC，主要包括：

- **MCP 服务**（crates/mcp）—— 向外部 Agent 暴露书籍的读写工具集，
  含会话持久化与跨进程读写闸门
- **AI 工作流** —— AI 建书向导、逐章成书、续排大纲、妙笔对话、续写、
  AI 检测报告；按用途分配模型
- **编辑器可靠性** —— IME 组词保护、保存单飞互斥、片段版本历史、
  取证日志、写入回读校验
- **桌面端体验** —— 码字统计双账本、整章阅读视图、全文检索、
  提示词覆盖、AI 档案设置页、水墨主题

这些功能的权威描述在 `app/README.md` 与代码本身。RFC-0019 即为其中
**MCP 服务的前端可感知性**补的第一份设计文档；其余若需补档，从 RFC-0020 起编号。
