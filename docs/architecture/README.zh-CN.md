# LicoUp CLI 架构

| 关联文档 | 语言 / 路径 | 权威职责 |
|:---|:---|:---|
| **规范版本** | [English (Normative)](README.md) | 本仓库架构索引的英文规范 |
| **本地化** | 简体中文 | 中文本地化投影 |
| **文档索引** | [docs/README.md](../README.md) | 文档目录索引 |

本仓库承载 LicoUp CLI 工作区：`licoup`
背后的 Rust crate 集合及其 schemas、SDK 与组件。Flutter 桌面客户端与产品级
指南位于 LicoUp monorepo；本页只索引本仓库保留的架构文档。

## 架构文档索引

| 文档 | 范围 |
|:---|:---|
| [CLIENT-NATIVE-INTERACTION.md](CLIENT-NATIVE-INTERACTION.md) | 宿主与原生核心之间的 `licoup.stdio.v1` 结构化方法帧契约 |
| [NATIVE-CLI.zh-CN.md](NATIVE-CLI.zh-CN.md) | 注册表派生命令、常驻原生方法与独立 MCP 进程生命周期 |
| [CONVERSATION-DOMAIN.zh-CN.md](CONVERSATION-DOMAIN.zh-CN.md) | 规范 Conversation 领域：Membership、调度门、轮次宿主、状态机与流程 |
| [CONVERSATION-VERTICAL-CONTRACT.md](CONVERSATION-VERTICAL-CONTRACT.md) | Conversation 垂直切片的 L1–L6 接口规范 |
| [AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md](AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md) | Agent 适配器分类与协议归一（ACP、app-server、RPC、CLI） |
| [RUST-INFRASTRUCTURE-LAYER.zh-CN.md](RUST-INFRASTRUCTURE-LAYER.zh-CN.md) | SQLite 存储、动态配置、密钥保管、传输、PTY/TTY |
| [SECURITY-AND-DATA-BOUNDARY.zh-CN.md](SECURITY-AND-DATA-BOUNDARY.zh-CN.md) | 虚拟机探测隔离、LicoArc 设备边界、平台密钥保管与数据零信任规则 |
| [MODEL-REGISTRY.zh-CN.md](MODEL-REGISTRY.zh-CN.md) | 标准模型身份、动态目录与用量控制 |
| [EXTENSION-PLATFORM.md](EXTENSION-PLATFORM.md) | 扩展契约、profiles、生命周期与内核裁决 |
| [DEPLOYMENT-PROFILES.md](DEPLOYMENT-PROFILES.md) | 安装 profile、安装闭环与各能力归属 |
| [CONTINUOUS-ASSISTANT.zh-CN.md](CONTINUOUS-ASSISTANT.zh-CN.md) | 持续协作目标语义 |
| [ASSISTANT-WORKFLOW-CONTROL.zh-CN.md](ASSISTANT-WORKFLOW-CONTROL.zh-CN.md) | 编译器分层、节点发布订阅与优雅干预 |
| [CLIENT-UPDATE-AND-STATE-MIGRATION.zh-CN.md](CLIENT-UPDATE-AND-STATE-MIGRATION.zh-CN.md) | 客户端更新与客户端状态迁移归属 |
| [../protocols/README.md](../protocols/README.md) | 线路协议与产物格式 |
| [../adrs/README.md](../adrs/README.md) | 架构决策记录 |
| [../modules/endpoint-collaboration.md](../modules/endpoint-collaboration.md) | 端点协作模块说明 |

## 仓库结构

| 路径 | 用途 |
|:---|:---|
| `crates/` | `licoup-*` 工作区 crate：`licoup` 宿主、领域 crate、Agent 适配器与运行时、网关、平台桥接、状态存储 |
| `components/` | 拥有独立工作区的打包组件（`appearance/`、`endpoint-collaboration/`） |
| `schemas/` | 客户端拥有的线路与契约 schema（`client_bridge`、`conversation_protocol`、`extensions`、`subagent_mcp`） |
| `sdk/` | 适配器 SDK 支撑材料 |
| `tests/` | 基于合成数据的契约、集成与冒烟测试 |
| `tools/` | 构建、回归与发布工具 |
