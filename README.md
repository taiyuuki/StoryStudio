# StoryStudio

面向长篇小说创作的「写作 IDE」。所有数据都是磁盘上的纯文本项目（TOML + Markdown）：
Rust 内核负责解析、编译检查与知识沉淀，桌面端 / CLI / MCP 三种入口操作同一份项目，完全互操作。

[![CI](https://github.com/taiyuuki/StoryStudio/actions/workflows/ci.yml/badge.svg)](https://github.com/taiyuuki/StoryStudio/actions/workflows/ci.yml)
[![Release](https://github.com/taiyuuki/StoryStudio/actions/workflows/release.yml/badge.svg)](https://github.com/taiyuuki/StoryStudio/actions/workflows/release.yml)

## 功能一览

- **写作**：纸面编辑器（CodeMirror 6，衬线排版、片段分片）、IME 组词保护、
  片段版本历史、码字统计双账本（手写 / AI）
- **结构**：卷章场景树、场景设定（POV / 情绪 / 故事时间）、大纲与叙事线索状态推导
- **数据**：角色 / 地点 / 道具 / 组织四类实体与关系网、知识断言 + 证据链
- **诊断**：25 条内置 lint（死亡角色复活、年龄矛盾、孤儿引用……），变更后自动重查
- **AI**：续写、逐章成书流水线（节拍 → 整章 → 质检 → 硬伤暂停等人决策）、
  断言抽取、妙笔对话；多档案多 Provider，按用途分配模型
- **MCP**：把项目工具集暴露给外部 Agent，支持会话持久化与跨进程读写闸门

完整功能表见 [app/README.md](app/README.md)。

## 仓库结构

| 目录 | 内容 |
|------|------|
| `crates/ir` | Story IR — 类型化故事图模型（RFC-0002） |
| `crates/project-format` | 项目格式 — 文件读写（RFC-0003） |
| `crates/parser` | 解析器 — 文本到 IR，确定性实体链接（RFC-0006） |
| `crates/compiler` | 编译器 — 规则引擎 + 诊断（RFC-0004/0005） |
| `crates/knowledge` | 知识层 — 断言 + 证据（RFC-0015） |
| `crates/exporter` | 导出 — IR 到发布格式（RFC-0013） |
| `crates/ai-runtime` | AI Runtime — Provider 抽象 + 任务模型（RFC-0010） |
| `crates/studio-lib` | 库门面 — 高层编排，桌面端与 CLI 共用 |
| `crates/mcp` | MCP 服务器 — 项目工具集暴露给外部 Agent |
| `crates/cli` | `story-studio` 命令行 |
| `app/` | 桌面端 — Tauri 2 + Vue 3 水墨风工作台 |
| `rfc/` | 设计文档（RFC-0001 起，含实现状态索引） |

## 快速开始

前置要求：Rust stable；桌面端另需 Node ≥ 22 与 pnpm 12。

### 桌面端

```bash
cd app
pnpm install
pnpm tauri dev      # 开发（vite :5190 + Rust 自动重编译）
pnpm tauri build    # 打包 .app / .dmg（macOS）
```

### CLI

```bash
cargo install --path crates/cli

story-studio init 我的小说                    # 新建项目（目录名自动拼音 slug 化）
story-studio compile -d <项目目录>            # 解析 + 全量检查
story-studio export -d <项目目录> -f markdown -o out --appendix
story-studio write -d <项目目录> -s <场景ID>  # AI 续写
story-studio workflow run -d <项目目录>       # 逐章成书流水线
```

### MCP

```bash
story-studio mcp -d <项目目录>                     # stdio，由客户端拉起
story-studio mcp -d <项目目录> --transport http    # HTTP 常驻，默认 127.0.0.1:27520
story-studio mcp --workspace <目录>                # 工作区模式：Agent 可建书 / 切书
```

加 `--read-only` 只暴露查询类工具，不修改项目。

## 测试与检查

```bash
cargo test --workspace --exclude story-studio-app   # Rust 库 + CLI
cargo fmt --all --check && cargo clippy --workspace --exclude story-studio-app --all-targets -- -D warnings
cd app && pnpm test && pnpm build                   # 前端测试 + 构建检查
```

## 发布

推送 `v*` tag 触发 [release.yml](.github/workflows/release.yml) 自动发布：

1. 同步三处版本号：`app/src-tauri/tauri.conf.json`、根 `Cargo.toml`（workspace）、`app/package.json`
2. `git tag vX.Y.Z && git push origin vX.Y.Z`

CI 会构建 macOS（Apple Silicon / Intel `.dmg`）、Windows（`.msi` / `.exe`）、
Linux（`.deb` / `.rpm` / `.AppImage`）安装包，以及三平台 `story-studio` CLI
二进制，一并发布到 GitHub Release。tag 与 `tauri.conf.json` 版本不一致时流水线会直接报错。

发布同时产出签名校验的应用内更新产物（`latest.json`）——已安装旧版的桌面端
在启动时静默检测（或于「设置 → 关于」手动检查），可直接在应用内下载更新并重启。
更新签名密钥：私钥存 GitHub secret `TAURI_SIGNING_PRIVATE_KEY`（本地备份于
`~/.tauri/storystudio.key`，**务必另外备份**，丢失则已安装用户收不到后续更新），
公钥固化在 `app/src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`。

## 文档

- [rfc/README.md](rfc/README.md) — 设计文档索引（RFC-0001 ~ 0019，逐篇标注实现状态）
- [app/README.md](app/README.md) — 桌面端功能全表与结构说明

## License

MIT
