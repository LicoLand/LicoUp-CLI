# 智能体适配器架构规范 (Agent Adapters Architecture)

| 关联文档 | 语言 / 路径 | 权威职责 |
|:---|:---|:---|
| **规范版本** | [English (Normative)](AGENT-ADAPTERS-ARCHITECTURE.md) | 智能体适配器架构英文规范 |
| **架构主文档** | [docs/architecture/README.zh-CN.md](README.zh-CN.md) | 四层顶层客户端架构与总览 |
| **兼容性矩阵** | [docs/COMPATIBILITY.zh-CN.md](../COMPATIBILITY.zh-CN.md) | 13 个智能体支持度与驱动清单事实 |
| **报文解析 ADR** | [docs/adrs/0008-native-agent-parser-and-conversation-integrity.md](../adrs/0008-native-agent-parser-and-conversation-integrity.md) | 隔离解析器与终态仲裁设计决策 |
| **命令身份 ADR** | [docs/adrs/0007-user-terminal-agent-command-identity.md](../adrs/0007-user-terminal-agent-command-identity.md) | 用户终端命令发现与启动绑定决策 |
| **Rust 基础设施层** | [RUST-INFRASTRUCTURE-LAYER.zh-CN.md](RUST-INFRASTRUCTURE-LAYER.zh-CN.md) | PTY/TTY、网络传输与动态配置 |

本文档定义 LicoUp 中 **13 个智能体（Agent）适配器的分类体系、协议适配方案、报文解析归一化与运行时调度架构**。

---

## 1. 智能体适配方案两大阵营

LicoUp 针对主流智能体的底层协议特征，将 13 个打包驱动划分为 **标准/开放行业协议方案** 与 **厂商私有/专属接口方案** 两大阵营：

```mermaid
flowchart TB
    subgraph STANDARDS["一、标准 / 开放行业协议方案 (Standard / Open Protocols)"]
        direction TB
        ACP["1. ACP 协议族 (Agent Client Protocol)<br/>• OpenClaw (openclaw-acp-stdio-jsonrpc)<br/>• Copilot (copilot-acp-v1-stdio-ndjson)<br/>• Hermes (hermes-acp-stdio-jsonrpc)<br/>• Kimi Code (kimi-code-acp-v1-stdio-ndjson)"]
        RPC["2. RPC 协议族 (Stdio JSON-RPC / JSONL)<br/>• Pi (pi-rpc-stdio-jsonl)<br/>• DeepSeek Harness (deepseek-harness-sdk-stdio-jsonrpc)<br/>• Lico Agent (lico-agent-rpc-stdio-jsonl)"]
        PTY_CLI["3. PTY / CLI 命令行方案 (CLI Stream & TTY)<br/>• Antigravity (antigravity-cli-argv-hook-v1)<br/>• Cursor (cursor-agent-cli-v1)<br/>• Claude Code (claude-code-cli-stream-json)"]
    end

    subgraph PROPRIETARY["二、厂商私有 / 专属接口方案 (Proprietary / Vendor Protocols)"]
        direction TB
        APP_SERVER["4. App Server 专属协议 (Codex)<br/>• OpenAI Codex 专用的 stdio 双向 JSON-RPC 控制协议<br/>(codex-app-server-stdio-jsonrpc)"]
        SERVE_HTTP["5. 本地独立 HTTP+SSE 服务协议 (OpenCode / Kilo Code)<br/>• OpenCode (opencode-serve-http-v1)<br/>• Kilo Code (kilo-code-serve-http-v1)<br/>• 通过回环 TCP 发起 REST 请求并监听专用 SSE 流"]
    end

    STANDARDS --> UNIFIED_BOUNDARY["统一报文解析与归一化边界 (native_agent_parser)"]
    PROPRIETARY --> UNIFIED_BOUNDARY
    UNIFIED_BOUNDARY --> TYPED_TRANSITIONS["封闭的规范状态机迁移 (Typed Transitions)<br/>Reasoning · ToolCall · ContentDelta · Usage · Terminal"]
```

---

## 2. 详细分类与适配协议矩阵

| 协议阵营 | 通道族 (Channel Family) | 智能体 ID | 运行协议标识 (Wire Protocol) | 传输媒介与通信机制 | 厂商/标准特征 |
|:---|:---|:---|:---|:---|:---|
| **标准/开放协议** | **`acp`** | `openclaw` | `openclaw-acp-stdio-jsonrpc` | Stdio JSON-RPC | 标准 ACP 协商、会话列出与加载、事件流式派发 |
| | | `copilot` | `copilot-acp-v1-stdio-ndjson` | Stdio NDJSON | GitHub Copilot CLI 的 ACP 协议子集 |
| | | `hermes` | `hermes-acp-stdio-jsonrpc` | Stdio JSON-RPC | 本地直连 ACP；支持通过 SSH 远程连接 TUI Gateway |
| | | `kimi-code` | `kimi-code-acp-v1-stdio-ndjson` | Stdio NDJSON | Kimi CLI 的标准 ACP 协议流 |
| **标准/开放协议** | **`rpc`** | `pi` | `pi-rpc-stdio-jsonl` | Stdio JSONL | 轻量化行式 JSONL RPC 交互 |
| | | `deepseek-harness` | `deepseek-harness-sdk-stdio-jsonrpc` | Stdio JSON-RPC | DeepSeek Harness 标准 SDK 交互管道 |
| | | `lico-agent` | `lico-agent-rpc-stdio-jsonl` | Stdio JSONL | Lico 内部原生标准智能体通道 |
| **标准/开放协议** | **`cli` / `stream-json`** | `claude-code` | `claude-code-cli-stream-json` | Stdio Stream JSON | Anthropic Claude Code 的流式 JSON 输出 |
| | | `antigravity` | `antigravity-cli-argv-hook-v1` | PTY 伪终端 / 进程参数 | 终端环境模拟、ANSI 捕获与命令 Hook |
| | | `cursor` | `cursor-agent-cli-v1` | PTY 伪终端 / 进程参数 | Cursor CLI 交互式命令行包装 |
| **厂商私有协议** | **`app-server`** | **`codex`** | `codex-app-server-stdio-jsonrpc` | Stdio JSON-RPC | **OpenAI Codex 私有 App Server 协议**，拥有专属的请求握手、上下文补全与工具审批序列 |
| **厂商私有协议** | **`serve-http`** | **`opencode`** | `opencode-serve-http-v1` | 本地回环 HTTP + SSE | **OpenCode 私有守护进程协议**，启动本地 HTTP 监听端口，通过 REST 触发并监听专属 SSE 事件 |
| | | **`kilo-code`** | `kilo-code-serve-http-v1` | 本地回环 HTTP + SSE | **Kilo Code 专属服务协议**，通过本地 Server 交互与流式回传 |

---

## 3. 核心架构机制与归一化设计

无论底层是标准 ACP、命令行 PTY，还是 Codex / OpenCode 的私有协议，LicoUp 均通过以下四层机制实现架构解耦与归一：

### ① 报文边界单点解析（ADR 0008）
- **每个智能体都拥有独立解析器**：内核仍持有的智能体把解析器放在 `crates/licoup-native/src/platform/native_agent_parser/adapters/<agent>/`；协议已迁入适配器包的智能体把解析器放在该包内（`crates/licoup-agent-<agent>/src/parser.rs`），组合处只引用该包，不保留第二份拷贝；
- 原始返回帧（无论 JSON-RPC、NDJSON、SSE 还是 ANSI 字符串）**只进入一次**该解析器；
- 解析器输出且仅输出封闭的 `Typed Transition`：
  - `ThinkingDelta` / `Reasoning`（思考推理步骤）；
  - `ToolCallRequested`（工具调用请求与参数）；
  - `ContentDelta`（增量文字渲染）；
  - `UsageReport`（Token 消耗统计）；
  - `Terminal(Completed | Failed | Cancelled)`（终态裁决）。

### ② 统一进程监督与看门狗（L3 Process Supervisor）
- 底层传输只负责启动进程、管道读写与 HTTP/SSE 握手；
- 提供跨平台的 **退出监督阶梯**（Grace Period 优雅取消 $\to$ `SIGTERM` $\to$ `SIGKILL`）；
- 消除任何隐式硬编码超时（废除 120s 猜测机制），全部由协议显式 EOF 或错误帧触发终结。

### ③ 会话连续性与强校验（L4 Session Guard）
- **Exact Resume 校验**：当续接旧会话时，必须严格校验目标会话是否存在，找不到会话立即报错（Fail-Closed），严禁静默降级为新建会话；
- **In-Flight 状态声明**：进程异常退出时，诚实向持久层声明未决状态，不篡改历史。

### ④ 虚拟机发现与协议路由（Virtual Machine Routes）
- 针对运行在 OrbStack 或远程服务器上的 Agent（如 `machine@orb` 或 SSH 目标），统一通过系统原生 `ssh -o BatchMode=yes` 建立通道；
- 远程目标同样挂接对应的协议解析器（如 OpenClaw ACP 或 Hermes TUI Gateway），上层业务代码无需感知本地与远程差异。

---

## 4. 架构铁律

1. **禁止在 Flutter 端解析厂商原始协议**：Flutter 永远只消费 Rust 持久化后的 `ClientConversationEvent` 与 `EventPart`，不感知任何 ACP、SSE 或 App Server 细节。
2. **禁止驱动擅自判定完成**：严禁 13 个驱动各自猜完成（如依赖 100ms 静默），完成判定必须由 L1 解析器根据协议显式信号或传输 EOF 独占裁决。
3. **私有协议隔离演进**：Codex、Cursor、Antigravity 与 DeepSeek Harness 的私有协议变化只影响其适配器包（`crates/licoup-agent-<agent>/src/`）；内核仍持有的智能体的协议变化只影响其 `adapters/<agent>/` 目录。两者都不扩散至统一 Conversation 领域层。
4. **用户终端环境等价**：LicoUp 调起的 CLI 子智能体，其可见环境必须与用户从自身 ZSH 登录终端启动同一 CLI 时完全一致——代理变量、PATH、登录态、工作目录设置均不缺失、不增加、不筛选。禁止通过全局 shell export 或 `launchctl setenv` 实现该等价；等价本身是铁律，而非注入某个子集。
5. **禁止强制回复格式**：LicoUp 只按 Agent 原本产出的样子显示它的回复。不下发任何要求 Agent 自身文本形状的 schema、响应格式或输出契约；纯自然语言的回复，不会因为没有格式就被判成无效、空回复或弃权。宿主需要什么，都从 Agent 说了什么里得出。源码门禁强制这条：原生 Agent 回合上出现回复格式键，`client:verify:agent-native-output` 就会失败；上面各处边界声明也必须一直存在。

---

## 5. 供应商无关 Subagent Mesh

Codex、Cursor、Antigravity 与 Claude Code 还会以 Membership 作用域 target 身份参与
独立的 [Subagent MCP](../protocols/subagent-mcp.zh-CN.md)，mesh 也为这四个供应商各携带
一个 caller 集成。

客户端为 Cursor、Antigravity 与 Claude Code 把 caller 注册写进这些 Agent 各自的用户
MCP 配置，每次写入都需要 digest 绑定的一次性批准。Codex 不安装任何注册：承载 Codex
caller 注册的 plugin 属于 Codex 自身的 marketplace，由 Codex 适配器包负责；因此
ensure 面对 Codex 直接报告 `unsupported`，caller manager 的每个成员都拒绝，而不是替
用户克隆插件仓库。

```mermaid
flowchart LR
  P["供应商 MCP client"] --> C["轻量 stdio connector"]
  C --> H["已认证回环 HTTP"]
  H --> CLI["公开原生 CLI"]
  CLI --> A["SubagentApplication"]
  A --> R["唯一 caller + runtime adapter registry"]
  A --> S["Canonical Membership 与谱系 store"]
  R --> T["Target PersistentTurn"]
  T --> E["Canonical Event / Part"]
```

独立的 `licoup-public-gateway` crate 负责 MCP 协议，并通过公开原生 CLI 转发已准入的 subagent
操作。`SubagentApplication` 负责原生委派。`McpCallerIntegration` 与
`SubagentRuntimeAdapter` 是唯一供应商 port。caller
身份与服务器自有父谱系来自已认证请求 context，绝不作为工具参数。任何 adapter
效果前都先提交持久 active-edge claim。

Codex 保持准确 App Server thread 身份与原生 developer instructions。Cursor 保持
准确 create-chat/resume 身份、prompt acknowledgement 与 PTY 传输。Antigravity
保持准确 Hook receipt 身份、OAuth/权限预检与 PTY 传输。Cursor、Antigravity 与
Claude Code 的生成指令只使用一段普通、无标记、临时 wire prefix，不成为 Canonical
Event 内容。

验证分为两条相互独立的路径。upstream 先证明服务健康，再并发检查每个被覆盖供应商
只读的
标准 MCP 启动表面，不依赖自定义插件，不创建 Conversation、不发送 turn，也不修改
配置。该启动识别覆盖 CLI 暴露该表面的三个 Agent：Codex 通过只在当前进程生效的
`-c mcp_servers=…` 声明配合 `mcp list --json` 读回；Cursor 与 Antigravity 使用各自
支持的文本 `mcp list` 表面，缺少归属明确的 entry 时报告
`installer_configuration_required` 而不是失败。Claude Code 在这条路径上没有启动识别
探针。downstream 默认零效果；只有显式现场执行才直接发送一次已认证 delegate，并以
Canonical 入站记录、claim、选中的 Membership 与 PersistentTurn dispatch 事实作为
oracle。预检通过既有 target/Agent Hub 表面解析 Agent 版本、运行时就绪状态与报告的模型清单，复用共享
验证模型权威，并在 Conversation 或付费工作前按实际 caller 身份验证服务健康。最近版本的证据按 target 建 key，并在任何付费效果前去重。自动回归只使用
hermetic fixture，不包含现场路径。
