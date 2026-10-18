# 统一 Conversation 垂直领域架构规范

| 关联文档 | 语言 / 路径 | 权威职责 |
|:---|:---|:---|
| **规范版本** | [English (Normative)](CONVERSATION-DOMAIN.md) | 统一 Conversation 垂直架构英文规范 |
| **架构主文档** | [docs/architecture/README.zh-CN.md](README.zh-CN.md) | 四层顶层客户端架构与总览 |
| **Rust 基础设施层** | [RUST-INFRASTRUCTURE-LAYER.zh-CN.md](RUST-INFRASTRUCTURE-LAYER.zh-CN.md) | 数据库、动态配置、密钥管理、网络与 PTY 规范 |
| **智能体适配器** | [AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md](AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md) | 13 驱动分类、标准与私有协议归一化 |
| **桥接协议规范** | [CLIENT-NATIVE-INTERACTION.md](CLIENT-NATIVE-INTERACTION.md) | 前后端 RPC / FFI 协议格式与帧规范 |
| **产品定义** | [PRODUCT.zh-CN.md](../../PRODUCT.zh-CN.md) | 长期「一个 Conversation」产品目标与理念 |

在 LicoUp 中，**Conversation（智能体对话）被正式确立为一个端到端的垂直业务架构（End-to-End Vertical Architecture Slice）**，而非仅仅局限于 Rust 内部的一个孤立模块。它自上而下纵向贯穿四个水平架构层级，连接用户交互、通信协议、领域状态机、基础设施与底层操作系统。

本文档完整定义智能体对话的**垂直四层分解、前后端双向绑定模型、单聊基石与群聊协同封装、Profile 抽象、严格状态机驱动、进度同步反射及端到端生命周期时序**。

---

## 1. 垂直切片全景架构

```mermaid
flowchart TB
    %% 样式与视觉定义
    classDef t1 fill:#e8f4fd,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef t2 fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef t3_d fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef t4 fill:#f1f3f5,stroke:#495057,stroke-width:2px,color:#212529;
    classDef box fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;
    classDef coreBox fill:#ffffff,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;

    subgraph TIER1["【第 1 层】Flutter 用户外观与应用层 (Presentation Layer)"]
        direction TB
        subgraph T1_VIEWS["用户交互与呈现组件 (UI Views)"]
            direction LR
            V_COMPOSER["Composer 输入框<br/>(发送拦截 · 草稿)"]:::box
            V_PROFILE["Profile 编辑回显<br/>(角色设定 · 头像)"]:::box
            V_APPROVAL["审批确认弹窗<br/>(脱敏参数 · 卡片)"]:::box
            V_STREAM["气泡流式工作区<br/>(Markdown · 历史)"]:::box
            V_BLACKBOARD["过程黑板 & 进度条<br/>(推理步骤 · 状态镜像)"]:::box
        end
        UI_CTRL["ClientConversationController 交互控制器 (状态防抖 · 进度反射)"]:::coreBox
        V_COMPOSER --> UI_CTRL
        V_PROFILE --> UI_CTRL
        V_APPROVAL --> UI_CTRL
        UI_CTRL --> V_STREAM
        UI_CTRL --> V_BLACKBOARD
    end

    subgraph TIER2["【第 2 层】Bridging Contract 桥接协议层 (Contract Layer)"]
        direction LR
        RPC_DESK["桌面端 JSON-RPC<br/>(licoup.stdio.v1 方法帧)"]:::box
        RPC_MOBILE["移动端 C-ABI FFI<br/>(Platform Bridges 命令)"]:::box
        RPC_OBSERVER["双向 Observer 事件流<br/>(实时 Parts · 状态机步进)"]:::box
    end

    subgraph TIER3["【第 3 层】Rust 领域核心与基础设施层 (Rust Functional Core & Infrastructure)"]
        direction TB
        subgraph T3_DOMAIN["3.1 对话领域业务核心 (Domain Core)"]
            direction LR
            D_INGEST["领域承接与预检<br/>(Domain Ingestion)"]:::box
            D_DISPATCH["调度门寻址<br/>(Dispatch Door)"]:::box
            D_FSM["核心会话状态机<br/>(Session State Machine)"]:::coreBox
            D_PARSER["L1 报文解析与终态仲裁<br/>(native_agent_parser)"]:::box
            D_INTERACT["L2 交互审批路由<br/>(Interaction Gate)"]:::box
            D_INGEST --> D_DISPATCH --> D_FSM
            D_FSM --> D_PARSER
            D_FSM --> D_INTERACT
        end

        subgraph T3_INFRA["3.2 基础设施与对外交互层 (Infrastructure Gateway)"]
            direction LR
            I_DB["数据库存储模块<br/>(SQLite WAL · 事务 · 索引)"]:::box
            I_PTY["PTY/TTY 伪终端模块<br/>(虚拟终端 · 窗口同步)"]:::box
            I_NET["网络通信模块<br/>(HTTP/SSE 流 · SSH 隧道)"]:::box
            I_CONF["动态配置系统<br/>(Agent 清单 · 环境变量)"]:::box
            I_SEC["密钥管理门面<br/>(会话建钥 · 加解密抽象)"]:::box
        end
        D_FSM -->|"持久化调用"| I_DB
        D_FSM -->|"终端与进程管道"| I_PTY
        D_FSM -->|"网络请求"| I_NET
        D_DISPATCH -->|"检索环境"| I_CONF
        D_FSM -->|"凭据操作"| I_SEC
        I_PTY -->|"原始帧"| D_PARSER
        I_NET -->|"流式帧"| D_PARSER
    end

    subgraph TIER4["【第 4 层】Native 原生系统适配层 (Native OS Layer)"]
        direction LR
        N_PTY["终端设备子系统<br/>(POSIX openpty / Win ConPTY)"]:::box
        N_PROC["操作系统进程与信号<br/>(SIGINT / SIGTERM / SIGKILL)"]:::box
        N_SEC["系统原生硬件凭据库<br/>(macOS Keychain / WinCred / Keystore / Enclave)"]:::box
    end

    %% 层与层之间的连接
    UI_CTRL ==>|"① 下行用户行为"| RPC_DESK
    UI_CTRL ==>|"① 移动端调用"| RPC_MOBILE
    RPC_DESK ==>|"② 传递 (Principal+Profile+Event)"| D_INGEST
    RPC_MOBILE ==>|"② 传递 (Principal+Profile+Event)"| D_INGEST

    I_PTY ==>|"③ 系统 PTY 挂接"| N_PTY
    I_PTY ==>|"③ 进程监督"| N_PROC
    I_SEC ==>|"③ 硬件存储"| N_SEC

    N_PTY ==>|"④ 终端输出"| I_PTY
    D_PARSER ==>|"⑤ 状态与增量推流"| RPC_OBSERVER
    RPC_OBSERVER ==>|"⑥ 进度反射刷新"| UI_CTRL

    class TIER1 t1;
    class TIER2 t2;
    class TIER3 t3_d;
    class TIER4 t4;
```

---

## 2. L1-L5 五层目标架构 (Target Architecture)

基于对 16 份审计报告、50+ 编目缺陷的系统性分析，对话流程中断的根因是**权威分裂——多个独立合成器争夺同一产物的写权限**（如"回合是否结束"、"当前进度前缀"、"是否正在等待用户交互"等）。目标架构引入 5 个专属层，每层独占特定产物的生产权威：

```mermaid
flowchart TB
    classDef l5 fill:#e7f5ff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef l4 fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef l3 fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef l2 fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f;
    classDef l1 fill:#fff5f5,stroke:#c92a2a,stroke-width:2px,color:#7c2d12;
    classDef box fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;

    subgraph L5_LAYER["L5: Flutter 证据消费归一层"]
        direction LR
        L5_P1["消除双枚举<br/>(单一 ConversationTurnProcessState)"]:::box
        L5_P2["单一 terminalTransition<br/>消费点"]:::box
        L5_P3["严禁：Dart 不得<br/>自造 Dispatch 事件"]:::box
    end

    subgraph L4_LAYER["L4: 连续性与会话身份层"]
        direction LR
        L4_P1["Exact Resume 校验<br/>(不匹配则 fail-closed)"]:::box
        L4_P2["AwaitSession 协商隔离<br/>(禁止提前活动注册)"]:::box
        L4_P3["诚实 In-flight 声明<br/>(崩溃 = interrupted 非 running)"]:::box
    end

    subgraph L3_LAYER["L3: 传输与进程监督层"]
        direction LR
        L3_P1["统一 ControlDisposition<br/>(Accepted / NoActiveTurn /<br/>SessionUnavailable / Unsupported)"]:::box
        L3_P2["DispatchDeadlinePolicy<br/>(替代硬编码 120s)"]:::box
        L3_P3["统一行/帧读取器<br/>与进程监督阶梯"]:::box
    end

    subgraph L2_LAYER["L2: 交互路由与终态结算层"]
        direction LR
        L2_P1["统一 Park-and-Wait<br/>(替代 4 份分散的等待循环)"]:::box
        L2_P2["终态 Fail-closed<br/>结算未决交互"]:::box
        L2_P3["激活 WaitingForHuman<br/>SQLite 写入路径"]:::box
    end

    subgraph L1_LAYER["L1: 报文解析与终态仲裁层"]
        direction LR
        L1_P1["TurnSettlementArbiter<br/>(唯一完成权威)"]:::box
        L1_P2["扩展 Transition 词汇表<br/>(ApprovalRequest / Usage / Progress)"]:::box
        L1_P3["Cancelled 与 Failed<br/>终态彻底分离"]:::box
    end

    L5_LAYER ==>|"消费规范证据"| L4_LAYER
    L4_LAYER ==>|"会话绑定证明"| L3_LAYER
    L3_LAYER ==>|"原始帧 (唯一边界)"| L1_LAYER
    L1_LAYER ==>|"InteractionRequested"| L2_LAYER
    L2_LAYER -.->|"审批响应回注"| L3_LAYER

    class L5_LAYER l5;
    class L4_LAYER l4;
    class L3_LAYER l3;
    class L2_LAYER l2;
    class L1_LAYER l1;
```

| 层 | 独占产物 | 设计原则 |
|:---|:---|:---|
| **L5** Flutter 证据消费归一 | 单一 `terminalTransition` 消费；统一状态枚举 | 前端仅消费后端产出的规范证据，严禁自行推导或合成任何生命周期事实 |
| **L4** 连续性与会话身份 | `RuntimeBinding` 真值；未校验时返回 `UnverifiedBinding` | 会话恢复必须经过真实原生身份校验，未经校验的绑定严禁报告为已绑定 |
| **L3** 传输与进程监督 | `ControlDisposition`; `DeadlinePolicy`; 监督阶梯 | 传输层仅负责帧边界与进程生命周期，严禁在传输层内嵌入业务语义判定 |
| **L2** 交互路由与终态结算 | `WaitingForHuman` + 一次性 Token；fail-closed 结算 | 轮次终结时必须以 fail-closed 方式结算所有未决交互，严禁遗弃挂起状态 |
| **L1** 报文解析与终态仲裁 | `TurnOutcome`；扩展 `Transition` 词汇表 | 轮次完成判定必须基于协议显式终结信号，严禁以静默推断或缺省超时替代 |

---

## 3. 九个独占合成器规范 (One Product, One Authority)

核心设计不变量：**对话管道中的每一个可观测产物，必须且只能由一个合成器独占生产**。任何两条代码路径都不允许竞争判定同一真值。

```mermaid
flowchart TD
    classDef synth fill:#ffffff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef input fill:#f8f9fa,stroke:#868e96,stroke-width:1px,color:#495057;
    classDef output fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;

    HE["Human Event<br/>(已定稿事实)"]:::input
    DOOR["调度门<br/>(准入计划)"]:::input

    HE --> DOOR
    DOOR --> S4["④ 身份登记<br/>RuntimeBinding 真值"]:::synth
    S4 --> S6["⑥ 准入投影<br/>Composer 交互状态"]:::synth
    S4 --> S7["⑦ 帧传输层<br/>标准帧 + EOF"]:::synth
    S7 --> S2["② 流式解析入口<br/>直播 Transition 流"]:::synth
    S2 -->|"Transition::Control"| S3["③ 交互门<br/>WaitingForHuman + Token"]:::synth
    S2 --> PT["PersistentTurn<br/>游标 EventParts"]:::input
    S2 --> S1["① 回合完成权威<br/>TurnOutcome"]:::synth
    S7 --> S1
    S1 --> CANON["Canonical 定稿<br/>Dispatch 终态 + Event 完成"]:::output

    S5["⑤ 活动回合控制面<br/>ControlDisposition"]:::synth -.->|"Cancel / Steer"| S4
    S8["⑧ 跟进策略<br/>忙时输入处理"]:::synth -.->|"Steer vs 排队 vs 拒绝"| DOOR
    S9["⑨ 能力真值<br/>动态能力矩阵"]:::synth -.->|"实测能力"| S6
```

| # | 合成器 | 独占产物 | 合成规则 | 设计原则 |
|:---|:---|:---|:---|:---|
| **①** | 回合完成权威 | `TurnOutcome ∈ {still-open, completed, failed, cancelled}` | 协议终结 × 传输 EOF × 取消确认 × 显式 deadline | 完成判定必须基于协议显式终结信号合成，不得以静默推断或缺省策略替代 |
| **②** | 流式解析入口 | 直播 `Transition` 流 + 游标 `EventPart` | Driver 字节 → Adapter Parser → 唯一发射器 → `PersistentTurn` | 直播与终态必须共享同一条 Transition 流，不得存在分裂的两套解析故事 |
| **③** | 交互门 | `TurnState::WaitingForHuman` + 一次性不透明 Token（无时钟过期） | 拦截 `Control` 动作，挂起回合保持 `still-open`；凭合法 Token 应答后恢复 | 未决交互不得被任何外部超时强行关闭，必须等待用户显式响应或轮次终结时 fail-closed 结算 |
| **④** | 身份登记 | `RuntimeBinding`（conversationId × membershipId × dispatchId ↔ adapter-private session） | 全键精确比对 control/steer/attach | 会话身份不得折叠或部分匹配，所有控制操作必须经过完整键校验 |
| **⑤** | 活动回合控制面 | `ControlDisposition ∈ {accepted, no-active-turn, unknown-session, unsupported, transport-unavailable}` | 接收 Cancel → 写入 `cancel-requested` → 等待完成权威收口 | 控制操作结果必须返回精确的处置类型，不得将不同失败原因折叠为同一错误 |
| **⑥** | 准入投影 | Composer 交互状态（`CanSend \| ReadOnlyLoading \| CanSteer`） | `open` 仅产生 `Prepared`（未绑定）；`send` 校验身份后产出 `Bound` | 准入状态必须反映真实绑定进度，不得将未校验的准备态报告为已绑定 |
| **⑦** | 帧传输层 | 标准帧 + EOF + 超限信号 | 从 Stdio/HTTP 字节流解出帧 | 传输层仅负责帧边界提取，不得混入有状态的业务语义解析 |
| **⑧** | 跟进策略 | 忙时输入处理（原生 Steer vs DirectTurn 排队 vs 拒绝） | 由适配器能力真值 + 当前 turn 状态决定 | 用户在轮次执行期间的输入必须有明确的处置路径，不得静默丢弃 |
| **⑨** | 能力真值 | 动态能力矩阵 | 依据控制面、解析器、Resume 实测生成 | 能力声明必须基于运行时实测结果，不得依赖静态配置猜测 |

---

## 4. 垂直四层逐层架构分解

Conversation 作为一个垂直业务切片，在水平四层中均有明确的职责与组件划分：

### ① 第 1 层：Flutter 用户外观与应用层（Presentation Layer）
- **视图交互与呈现**：
  - `Composer`（富文本输入、发送拦截、多模态附件暂存、防抖）；
  - `AgentConversationWorkspace`（会话气泡流式展示、历史翻页、Markdown 渲染）；
  - `ProcessBlackboard`（过程黑板，展示思考推理过程、工具调用进度条与诊断日志）；
  - `ApprovalModal`（一次性人机交互审批确认卡片，展示脱敏后的参数摘要）。
- **Profile 前端管理与回显**：
  - 支持 Human 与 Agent 专属 Profile 的实时展示、编辑、头像/名称设置与个性化 Prompt 配置。
- **响应式状态管理**：
  - `ClientConversationController` 维护前端交互状态机（`_sending`, `_liveTurns`, `draft`, `_dispatchPending`）。
- **进度与状态同步反射**：
  - 监听后端推送的状态机步进事件，**后端状态机前进一个阶段，前端进度条与黑板立即反射对应的阶段刷新**。

### ② 第 2 层：Bridging Contract 桥接协议层（Contract Layer）
- **通信通道**：
  - 桌面端：`licoup.stdio.v1` 强类型 JSON-RPC 方法帧；
  - 移动端：C-ABI 跨语言内存安全 FFI 命令。
- **数据穿透保证**：
  - 完整透传 `(Principal + Profile + Event)` 强类型数据载荷；
  - 建立双向 Observer 事件流通道，实时回传后端状态机步进、增量文本与终态证据。
- **边界约束**：
  - 严格阻断任意 CLI 参数数组（argv）穿透，防止未校验数据破坏后端状态机。

### ③ 第 3 层：Rust 领域与基础设施层（Rust Domain & Infrastructure Layer）
- **领域承接层（Domain Ingestion Layer）**：
  - 接收协议层传入的 `(Human/Agent + Profile + Event)` 组合；
  - 执行权限校验、Membership 席位匹配与 Dispatch 计划生成，将其转换为 Rust 内部可执行的领域任务。
- **会话管理模块与严格状态机（Session Manager & State Machine）**：
  - **核心调度与生命周期中枢**：独占驱动对话回合的状态演进；
  - **严格受控调用**：在状态机的严格约束下，将上游 RPC 事件转换为对 Rust 基础设施层与 Native 原生系统适配层的安全函数调用。
- **L1 报文解析与终态仲裁 (`native_agent_parser`)**：
  - 归约 13 个智能体的多源报文（ACP/RPC/PTY/Codex/OpenCode），输出规范的 `Typed Transitions`。
- **L2 交互路由与挂起结算 (`native_agent_interaction`)**：
  - 管理工具调用审批 Token 挂起与 Fail-Closed 结算。
- **Rust 基础设施底层支持**：
  - `ConversationStore`（SQLite WAL）：唯一持久化读写规范 Conversation 历史与 EventPart；
  - `ProviderHistoryAccess`（服务商中立核心边界）：仅通过服务商授权读取服务商管理的保留历史，并投影到客户端，不替代规范存储；
  - `DynamicConfig`：动态加载 Agent 扫描路径与自定义运行时环境；
  - `SecretCustody`：统一会话密钥管理；
  - `NetworkTransport` & `PTY/TTY`：流式 HTTP 接收与虚拟终端交互。

### ④ 第 4 层：Native 原生系统适配层（Native OS Layer）
- **底层终端与进程管道**：
  - macOS/Linux POSIX PTY (`openpty`, `termios`, `ioctl`) 与 Windows ConPTY / Named Pipes；
- **进程监督与信号回收**：
  - 操作系统级信号响应（`SIGINT`, `SIGTERM`, `SIGKILL`）与进程退出状态回收；
- **平台安全凭据**：
  - 系统硬件密钥库（Keychain / WinCred / D-Bus Secret / Keystore / Secure Enclave）存取。

---

## 5. 前后端双向绑定模型与分层职责划分

### 5.1 双向交互模型
前后端交互架构遵循严格的**双向绑定（Bidirectional Binding）**与单一事实源原则：
- **下行指令链路（Downlink Action Flow）**：前端（Flutter）负责捕获用户的交互意图并生成强类型请求（如：输入发送、取消轮次、审批确认/拒绝、切换会话等），作为交互发起方下发至协议层。
- **上行事实链路（Uplink Authoritative Flow）**：后端（Rust 核心）作为系统状态与数据的唯一权威，负责执行业务逻辑、安全持久化与底层管道调度，并通过 Observer 事件流向前端实时推送真实的状态机阶段迁移、增量流式数据块（Delta/Part）、终态证据与交互挂起请求。

### 5.2 分层职责与边界原则

| 架构分层 | 核心职责 | 数据与控制边界原则 |
|:---|:---|:---|
| **前端外观与应用层**<br>(Presentation Layer) | 1. 捕获用户键盘、手势与表单输入；<br>2. 前端交互状态防抖与锁（如发送期间锁住输入框、清空草稿）；<br>3. 将用户行为打包为 Dart 强类型请求；<br>4. 监听后端 Observer 事件流并响应式更新过程黑板与气泡；<br>5. 渲染安全摘要与一次性交互审批卡片。 | **状态镜像原则**：作为后端状态机的呈现镜像，完全基于后端推流的规范证据更新 UI；不持有本地 SQLite 副本，不自行推导或伪造生命周期事实。 |
| **桥接协议层**<br>(Contract Layer) | 1. 结构化 JSON-RPC 方法帧双向编码与解码；<br>2. 跨进程（stdio）与跨语言（FFI）边界内存安全传递；<br>3. 方法调用超时与通道断开的类型化错误保护。 | **无状态通道原则**：作为纯净的数据传输通道，透传类型化请求与事件流，不承载有状态业务逻辑。 |
| **Rust 领域与基础设施层**<br>(Functional Core & Infra) | 1. 规范 Conversation 事实的独占持久化权威（SQLite/WAL 唯一读写方）；<br>2. 独占调度门准入权（解析 `@mention`、群策略与 Membership）；<br>3. 独占生命周期裁决权（L1 统一完成谓词与终态仲裁）；<br>4. 独占进程生命周期管理（启动、输入管道、Grace Period 监督、SIGTERM/KILL）；<br>5. 维护交互路由（一次性 Token 挂起与 Fail-Closed 结算）。 | **事实权威原则**：作为规范 Conversation 事实的唯一权威源。服务商管理的历史仍是经授权的外部来源，不能静默合并进该权威；所有向前端暴露的规范事件均具备不可变持久化事实依据，并严格保证 Exact Resume 会话身份一致性。 |

---

## 6. 单聊独立入口与群聊协同编排架构 (Direct Chat & Group Orchestration)

在产品与领域架构设计上：
1. **单聊（Direct Chat）直接面向用户开放**：提供沉浸式的一对一智能体交互工作区（独立模型参数、单 Agent 提示词与直连会话状态机）；
2. **群聊（Group Chat）同样直接面向用户开放**：提供多人与多智能体协作工作区（成员列表、@补全、策略图绑定与 Assistant 卡片）；
3. **架构底层复用关系**：**单聊是底层执行引擎的核心原子基石，群聊是在多个底层单聊执行管道之上的「多方协同与编排封装层」**。

```mermaid
flowchart TB
    %% 样式定义
    classDef userFacing fill:#e7f5ff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e,font-weight:bold;
    classDef groupOrch fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef directPipeline fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef storeBox fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;
    classDef itemBox fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;

    subgraph USER_ENTRIES["【用户界面入口 (User-Facing Entry Points)】— 单聊与群聊均直接面向用户开放"]
        direction LR
        ENTRY_DIRECT["入口 A：一对一单聊工作区 (1:1 Direct Chat)<br/>• 直接面向用户的沉浸式单智能体界面<br/>• 专属模型参数 · 单 Agent 提示词 · 纯净直接交互"]:::userFacing
        ENTRY_GROUP["入口 B：多智能体群聊工作区 (Group Chat)<br/>• 直接面向用户的多人多智能体协作界面<br/>• 成员面板 · @补全菜单 · 群策略绑定 · Assistant 卡片"]:::userFacing
    end

    subgraph GROUP_LAYER["【群聊专属：多方协同与编排封装层 (Group Orchestration Layer)】"]
        direction TB
        subgraph G_MODULES["群聊特有的四重协同机制"]
            direction LR
            G_MEM["1. 多席位与准入管理<br/>(Multi-Membership)"]:::itemBox
            G_DISPATCH["2. 群调度门与寻址分发<br/>(@mention 扫描 / 目标切片)"]:::itemBox
            G_STRATEGY["3. 策略编排与目标治理<br/>(Flywheel Graph / Assistant)"]:::itemBox
            G_AGGREGATE["4. 多路事件汇聚投影<br/>(Shared Timeline 聚合)"]:::itemBox
            G_MEM --> G_DISPATCH --> G_STRATEGY --> G_AGGREGATE
        end
    end

    subgraph DIRECT_PIPELINES["【核心基石：原子单聊执行管道 (Atomic Direct Chat Pipelines)】"]
        direction LR
        subgraph PIPE_A["Agent Alpha 单聊执行管道"]
            direction TB
            A_FSM["独立状态机 (FSM)"]:::itemBox
            A_PROF["Alpha 专属 Profile"]:::itemBox
            A_PTY["PTY / 进程通道"]:::itemBox
            A_FSM --- A_PROF --- A_PTY
        end
        subgraph PIPE_B["Agent Beta 单聊执行管道"]
            direction TB
            B_FSM["独立状态机 (FSM)"]:::itemBox
            B_PROF["Beta 专属 Profile"]:::itemBox
            B_PTY["PTY / 进程通道"]:::itemBox
            B_FSM --- B_PROF --- B_PTY
        end
        subgraph PIPE_N["Agent N 单聊执行管道"]
            direction TB
            N_FSM["独立状态机 (FSM)"]:::itemBox
            N_PROF["Agent N 专属 Profile"]:::itemBox
            N_PTY["PTY / 进程通道"]:::itemBox
            N_FSM --- N_PROF --- N_PTY
        end
    end

    subgraph FACT_STORE["【统一持久化事实库 (Durable Fact Store)】"]
        DB["ConversationStore (SQLite WAL)<br/>同一张 conversations 实体表 · 统一 Events / EventParts 事实权威"]:::storeBox
    end

    %% 业务流向
    %% 1. 单聊直连链路：用户直接使用单聊，直通底层单聊管道
    ENTRY_DIRECT ==>|"【单聊直连路径】用户直接与单个 Agent 对话，绕过群编排"| PIPE_A

    %% 2. 群聊编排链路：用户进入群聊，先经群编排层，再分发到底层单聊管道
    ENTRY_GROUP ==>|"【群聊协同路径】用户在群聊发消息"| GROUP_LAYER
    G_DISPATCH -.->|"切片分发任务"| PIPE_A
    G_DISPATCH -.->|"切片分发任务"| PIPE_B
    G_DISPATCH -.->|"切片分发任务"| PIPE_N

    %% 3. 群聊结果汇聚
    PIPE_A -.->|"回传执行事件"| G_AGGREGATE
    PIPE_B -.->|"回传执行事件"| G_AGGREGATE
    PIPE_N -.->|"回传执行事件"| G_AGGREGATE
    G_AGGREGATE ==>|"投影为群可见时间线"| ENTRY_GROUP

    %% 4. 数据落盘
    PIPE_A ==>|"持久化事实"| DB
    PIPE_B ==>|"持久化事实"| DB
    PIPE_N ==>|"持久化事实"| DB
```

### 单聊直连路径 vs 群聊协同路径机制对比：

| 架构维度 | 一对一单聊（直接面向用户的独立服务） | 多智能体群聊（协同编排封装层） | 协同机制与设计原则 |
|:---|:---|:---|:---|
| **用户界面入口** | **独立单聊工作区**：沉浸式体验、单一 Agent 参数调优与系统 Prompt 设置。 | **独立群聊工作区**：多成员列表、@ 补全菜单、Assistant 卡片、策略版本绑定器。 | 两套界面均直接对用户提供交互服务，界面组件按需组合。 |
| **调度执行路径** | **直连执行路径**：用户发消息 $\to$ 绕过群编排 $\to$ 直接激活该 Agent 的单聊状态机与 PTY 通道。 | **编排分发路径**：用户发消息 $\to$ 经群调度门（@mention / Graph） $\to$ 分发给一个或多个底层单聊管道执行。 | 单聊追求最低延迟与直接交互；群聊追求上下文精准切片与多 Agent 协同。 |
| **底层管道复用** | 直接独占使用 1 条底层单聊执行管道（FSM + Profile + 进程/PTY）。 | 复用并并发调度 $N$ 条底层单聊执行管道，实现流水线交接或并行推理。 | **单聊执行引擎是全系统唯一的执行底座**，群聊绝不重新造轮子。 |
| **事件聚合投影** | 单一时间线直接写入 SQLite 并推流回单聊界面。 | 各底层管道产生的 `EventPart` 经群聚合器有序汇聚，投影为全局可见的群历史流。 | **统一持久化事实**：底层共用同一张 `conversations` 表与 `events` 索引。 |

---

群组名单是 active Agent Membership 的展示投影。`principal.agentId` 表示适配器，
只与 `TargetCandidate.target` 关联；候选记录的 `id` 不是 Membership 身份。
发现结果暂时缺失时，成员仍以不可用状态保留。已退休的 `kimi`、`kimi-desktop`
从当前名单隐藏，但不删除 Membership 或历史 Event；`kimi-code` 保持独立。
空名单不阻断历史阅读和消息编辑。点击名单与 mention 补全均使用 Membership
显示名，空名回退到 agent ID，与 native 寻址接受的别名保持一致。

## 7. Human / Agent 专属 Profile 抽象与全链路流向

在 Conversation 体系中，**无论是人类（Human）还是智能体（Agent），都必须且必然拥有一份专属的 Profile 数据封装抽象**。

```mermaid
sequenceDiagram
    autonumber
    actor User as 用户
    participant UI as Flutter 视图 (Profile UI)
    participant Ctrl as 前端控制器 (Controller)
    participant Bridge as 桥接层 (Contract)
    participant Ingestion as Rust 领域承接层
    participant SM as Rust 会话状态机

    User->>UI: 1. 配置或选择 Agent / Human Profile (模型/角色/设定)
    UI->>Ctrl: 2. 绑定当前 Membership 的 Profile
    Ctrl->>Bridge: 3. 下行穿透：(Principal + Profile + Event) 结构体
    Bridge->>Ingestion: 4. 强类型解构并执行领域准入预检
    Ingestion->>SM: 5. 注入会话状态机，作为该轮次执行的身份与策略上下文
    SM->>SM: 6. 按 Profile 声明的 Agent 路由与参数驱动底层进程
```

- **全链路贯穿能力**：
  - **前端**：具备完整的展示、编辑与回显能力（支持头像、显示名、上下文偏好、安全级别）；
  - **协议层**：作为结构化字段无损穿透 RPC / FFI 边界；
  - **后端**：由 Rust 领域承接层接纳，作为状态机驱动底层 Agent 执行的输入依据。

---

## 8. 配置定义的对话生命周期

`crates/licoup-conversation/resources/state-machines.json` 是对话持久化状态转换关系的唯一来源。构建过程将其中选定的六个状态机编译为 crate 私有 Rust 类型和转换表：直接轮次、发送、成员关系、派发、子智能体认领及派发投递。生成类型保留当前调用方使用的 kebab-case 持久化名称。

生成的转换表只判断给定状态和事件对应的后继状态，不执行数据库、进程、网络或调度操作。对话 store 在 `src/store/lifecycle.rs` 及其仓储中负责守卫条件、操作效果和事务；每次持久化状态变化都与相应的 store 操作一并提交。这样转换关系保持声明式，运行时仍负责执行。

| 持久化记录 | 生成状态类型 | 持久化生命周期 |
|:---|:---|:---|
| 直接轮次 | `TurnState` | `pending`、`claimed`、`running`、`waiting-for-human`，之后进入终态 |
| 发送 | `SendState` | `sending` 到 `delivered` 或 `failed` |
| 成员关系 | `MembershipStatus` | `active` 或 `left` |
| 派发 | `DispatchState` | `accepted`、`running`、取消请求及终态 |
| 子智能体认领 | `SubagentDispatchClaimState` | 认领、执行、对账及终态 |
| 派发投递 | `DispatchDeliveryState` | `pending`、`delivering`、`delivered` 或 `failed` |

## 9. 持久化状态的界面呈现

Rust 对话 store 是持久化记录和已提交事件的权威来源。界面呈现这些结果和投影；它不消费独立的生成式转换事件流，也不自行提交生命周期变化。草稿等本地展示状态仍由界面负责，持久化状态则来自对话权威。观察连接断开本身不会改变轮次的持久化状态；恢复流程会读取已保存的生命周期并应用对话恢复规则。

---

## 10. 端到端标准时序流

### ① 用户发消息与两阶段调度流

用户发消息采用「**第一阶段：落盘定稿；第二阶段：调度准入**」的双重保障机制，确保用户输入绝不丢失：

`conversation.dispatch.after-post` 还接收可选布尔值 `suppressAssistant`（默认
`false`），为该消息排除指定 Assistant 的直接派发和 steer。Rust 仍从已提交 Event
解析 mention、保留共享历史并返回已有可附着 turn。
[Assistant 控件](../functionality/ADAPTIVE-FLYWHEEL.md#group-conversation-start)
负责此界面选择；独立绑定的 Flywheel 执行保持原有语义。

```mermaid
sequenceDiagram
    autonumber
    actor User as 用户
    participant UI as Flutter 界面与控制器
    participant Bridge as 桥接协议层
    participant Domain as Rust 领域层 (Store)
    participant Dispatch as Rust 调度门 (Host)
    participant Driver as 智能体驱动与进程

    User->>UI: 1. 在 Composer 输入正文并点击“发送”
    Note over UI: 前端状态锁定：<br/>• _sending = true<br/>• 锁住输入框防重发

    UI->>Bridge: 2. 下行行为事件：conversation.message.post { conversationId, authorMembershipId, content }
    Bridge->>Domain: 3. 路由至 persist_posted_message
    Domain->>Domain: 4. 【后端落库】写入已 Finalize 的 Human Event
    Domain-->>Bridge: 5. 返回确认 { eventId }
    Bridge-->>UI: 6. 前端收到落库确认
    Note over UI: 前端释放草稿：<br/>• _draft = ''<br/>• 刷新本地事件列表

    UI->>Bridge: 7. 下行行为事件：conversation.dispatch.after-post { conversationId, eventId, suppressAssistant? }
    Bridge->>Dispatch: 8. 传入已持久化的 (conversationId, eventId)
    Dispatch->>Domain: 9. 从库中读取文本，解析 @mention / 绑定的 Flywheel Graph
    Dispatch->>Dispatch: 10. 登记 Dispatch(accepted) + 未定稿 Agent Event 槽位
    Dispatch->>Driver: 11. 启动 Agent 进程并建立监听
    Dispatch-->>Bridge: 12. 返回活动轮次句柄 { turns: [turnHandle] }
    Bridge-->>UI: 13. 前端挂接 _liveTurns，进入流式观察状态
```

---

### ② 实时流式数据生成与挂接流

```mermaid
sequenceDiagram
    autonumber
    participant Driver as 智能体进程
    participant L3 as L3 传输监督层
    participant L1 as L1 报文解析与终态仲裁
    participant Domain as 域持久化层
    participant UI as Flutter 前端

    Driver->>L3: 1. 输出原始字节/JSON 帧
    L3->>L1: 2. 投递原始帧 (传输与解析唯一边界)
    L1->>L1: 3. 归约出 Typed Transitions (Reasoning, ToolCall, ContentPart, Usage)
    L1->>Domain: 4. 向未 finalize 的 Agent Event 追加 EventPart
    Domain->>Domain: 5. 递增游标水位并持久化
    Domain-->>UI: 6. 上行推送真实增量事件 (Observer Stream)
    Note over UI: 前端响应式更新：<br/>• 流式追加气泡文字<br/>• 渲染过程黑板推理状态

    Driver->>L3: 7. 进程退出 / 协议显式 Finish
    L3->>L1: 8. 投递 EOF / 完成信号
    L1->>L1: 9. 终态仲裁器判定完成 (Terminal Transition)
    L1->>Domain: 10. Finalize 该 Agent Event (标记已完成)
    Domain-->>UI: 11. 上行推送完成态证据
    Note over UI: 前端解除 busy 锁定，轮次正常结束
```

---

### ③ 阻塞式人机审批交互流（Human-in-the-Loop）

```mermaid
sequenceDiagram
    autonumber
    participant Driver as 智能体进程
    participant L1 as L1 解析层
    participant L2 as L2 交互路由层
    participant Domain as 域持久化层
    participant UI as Flutter 前端
    actor User as 用户

    Driver->>L1: 1. 输出工具执行请求 (Tool Request)
    L1->>L2: 2. 识别为 InteractionRequested，生成一次性 Scoped Token
    L2->>Domain: 3. 激活轮次状态为 WaitingForHuman
    Domain-->>UI: 4. 上行推送审批卡片事件 (包含脱敏后的参数摘要)
    Note over UI: 前端展示审批确认弹窗/卡片

    User->>UI: 5. 点击“批准”或“拒绝”
    UI->>L2: 6. 下行行为事件：interaction.respond { token, approved: true/false }
    Note over L2: 校验 Token 合法性与单次使用约束
    L2->>Driver: 7. 将响应结果回注至 Agent 进程的 stdin
    L2->>Domain: 8. 恢复轮次状态为 Processing
    Domain-->>UI: 9. 上行推送恢复运行状态
```

---

### ④ 轮次取消与异常安全流

```mermaid
sequenceDiagram
    autonumber
    actor User as 用户
    participant UI as Flutter 前端
    participant Bridge as 桥接层
    participant L3 as L3 进程监督
    participant L1 as L1 仲裁器
    participant Domain as 域持久化层

    User->>UI: 1. 点击“停止 / 取消”按钮
    UI->>Bridge: 2. 下行行为事件：agent.conversation.cancel { turnHandle }
    Bridge->>L3: 3. 触发进程监督梯队
    Note over L3: 进程监督阶梯：<br/>① 发送 Graceful Cancel 协议包<br/>② 等待宽限期 (Grace Period)<br/>③ 发送 SIGTERM<br/>④ 仍未退出则 SIGKILL 强制回收

    L3->>L1: 4. 报告进程中断
    L1->>L1: 5. 仲裁器生成 Cancelled Terminal Transition (独立于 Failed)
    L1->>Domain: 6. 写入终态状态：Cancelled
    Domain-->>UI: 7. 上行推送已取消事实
    Note over UI: 前端恢复 Composer 可编辑状态
```

---

## 11. 异常处理与一致性保障

1. **观察者断开不等于终态失败**：
   - Flutter UI 页面切换或窗口最小化导致 Observer 流断开，**绝不代表底层 Agent 轮次被取消或失败**。
   - Rust 核心层保持进程内进度；当 Flutter 重新进入页面时，只需传入 `conversationId` 和游标即可无缝重放自最后可见水位以来的所有 EventPart。
2. **拒绝前端推导，一切以 Rust 提交的证据为准**：
   - 任何网络抖动、进程崩溃或超时，必须先由 Rust L1 / L3 产生明确类型化错误并写入 SQLite，Flutter 端只消费该错误码并呈现对应的本地化提示，杜绝 Dart 侧“猜测性报错”。

---

## 12. 可信历史与恢复边界

群聊侧边栏直接绑定该群关联会话形成的专属子集。
`conversation_native_sessions` 是关联关系的唯一依据，对已确认的
`(conversationId, membershipId, nativeSessionId)` 只保留一条关系；替换当前运行绑定、
切换模型或成员离群都不会删除旧关联。同一个智能体及原生会话只有分别留下明确绑定，
才可同时属于多个群。Schema 18 是当前对话布局。从已发布的 Schema 12 到 Schema 18
采用一次原子转换，补齐当前绑定和明确的派发来源，并记录旧运行来源链接中可确认的
所属群。未发布的 Schema 13 至 17 不是受支持的迁移输入；更新版本或缺失必要字段的
元数据会在修改 schema 前被拒绝。缺失所属证据时不会建立关系。

桌面端在 `conversation.get` 中显式传入 `includeNativeSessionReferences: true`，
取得仅限本地查表的 `membershipId`、`agentId`、`nativeSessionId`。默认 get、可移植
Conversation、导出及 MCP/协议契约均不变，结果不包含运行路径或 dispatch 句柄。
应用层由这些关联直接映射出群聊专属会话集合，列表直接消费该集合的投影。缺失的
元数据按原生会话身份精确读取，最多四路并发，每条就绪后立即更新投影。选择、刷新
和消息分页均使用这个群聊集合及精确读取器。禁止枚举或筛选成员 Agent 的浏览目录，
也不得把群聊子集写回普通浏览目录。普通浏览目录变化不应重建或替换群聊集合；切群
及绑定变化必须使过期响应失效。无关联即为空集合，禁止按产品名、项目、模型或时间
猜测归属，原生行不可读时也不编造会话。消息分页与执行仍复用现有机制。

规范 `conversation.events.page` 按序号升序返回事件，默认每页 20 条、最多 100 条。
`latest: true` 读取最新保留事件；`beforeSequence` 以独占序号锚点读取更早一页。
两者均使用索引降序查询，只反转已选中的当前页。`afterSequence` 继续独立支持尾部
增量与精确事件锚点读取；省略全部选择器时，从头向前读取。有效选择器互斥，组合
冲突返回 `invalid_request`。`hasEarlier` 与可空的 `nextBeforeSequence` 表示是否
还有更早的保留事件；仅存在更早事件时，后者才是当前页首条事件的序号。
`nextCursor` 继续返回当前页末条事件序号的字符串，供向前读取使用；空页返回 null。
`totalCount` 是保留行数，不能充当序号游标，删除事件可能留下序号空洞。桌面端打开
最新 20 条事件，向前滚动时每次读取更早 20 条；精确任务卡片锚点读取与这个连续分页
窗口独立。

规范 Conversation 历史仍由 LicoUp 本地的 `ConversationStore` 持有。服务商管理的
云端历史是另一条保留对象来源，只有在取得服务商授权后，才可以投影到会话体验中。

边界固定如下：

1. 服务商授权后，服务商管理的保留历史默认可读。默认历史读取不会调用恢复密钥。
2. 服务商管理历史的客户端加密必须显式选择，不会静默改变默认可读路径。
3. 历史恢复会还原在授权范围内仍然可用的全部保留对象。服务商访问规则继续生效；恢复
   不能绕过访问控制，也不能重新创建缺失、删除、过期或其他不可用的对象。
4. 身份恢复材料单独认证，且永远不会锁住默认读取。替换设备恢复会准备身份权威与完整
   可用历史，并交给调用方一次性原子提交；仅有身份材料不能重新创建缺失历史。
5. LicoUp 不会通过通讯站路由或存储这些历史。恶意通讯站始终位于历史路径之外；这条
   路径不作强制公证或端点证据承诺。

```mermaid
flowchart LR
    U["用户"] --> A["服务商授权"]
    A --> H["服务商管理的保留历史"]
    H --> L["LicoUp 本地投影"]
    R["历史恢复"] --> H
    E["显式客户端加密"] -.-> H
    I["身份恢复"] -.-> K["端点身份与密钥"]
    S["恶意通讯站<br/>仅运输"]
```


## 13. 本地执行过程查看

`agent.conversation.execution` 是单次执行的本地只读流，要求精确的
`conversationId`、`membershipId` 和 `turnHandle`。handle 是原生 dispatch ID；
适配器的 session ID 或晚到的 turn ID 不替代该身份。存储层在返回记录前解析并
验证对应的 Agent Event。实时气泡携带该执行引用；原生历史回读只有通过同一 Agent、
同一原生 session 已记录的明确服务商标识才能附加引用。这份小型来源关联归 dispatch
所有，冲突命中保持未绑定。Codex app-server 的轮次确认和 rollout 轮次上下文提供
这一关联。host 生成的 transport ID、文本相似度、时间戳或当前活跃 Agent 都不能
补出缺失的服务商标识。旧的未绑定历史仍可正常读取，不编造执行引用。

流先按顺序重放保留记录，报告历史读取已追平，再使用现有可唤醒的 turn observer
观察新记录。请求和完整终态 payload 归 dispatch 行所有；运行记录复用带 cursor
的 EventPart 分片，读取时直接拼接原字串，不经解码再编码。无需第二份对话历史
或独立的运行帧存储。

本地捕获观察者在 parser 投影之前记录实际可得的应用协议输入，在编码后的发送边界
记录输出，保留公开投影省略的未知字段、工具及中间过程内容。来源和方向属于记录
元数据，不作为前缀改写原文。文本保持原样；非 UTF-8 字节使用明确标注的可逆编码。
规范化运行记录与协议记录可区分。新增捕获不能恢复此前已丢弃的字节或元数据。

捕获归精确的 dispatch 所有。工作线程显式取得观察者；持久 transport 为当前 dispatch
绑定接收端，不把队列中的旧帧按后来消费它的轮次归属。未绑定流量不归给最近的任务。
捕获在终态提交前关闭；写入失败会在保留的终态证据中明确标记记录不完整，不改变
Agent 的真实结果，也不取消工作。

执行 cursor 是单调递增的不透明值，区别于公开运行帧 cursor 和传输 sequence。
它保留已捕获请求、运行记录（包括 user-speech 元数据）和终态 payload 的持久顺序，
允许间隙。`afterCursor` 是排他边界；大记录的传输分片完整重组后才推进游标，观察者
重连不会重复或截断记录。旧执行缺失的 payload 保持缺失，新读取接口不能恢复从未
保留的内容。

显式本地查看路径保留实际已有记录的原文，包括未知嵌套字段、数组、请求元数据、
中间输出和终态证据，不经过公开 assistant 摘要的脱敏处理。普通对话、批准、导出
和远程边界仍遵守各自契约。该方法不属于 MCP 能力，不读取受保护凭据，也不增加
运行数据的外部目的地。

观察与执行分离。关闭浮层或断开流不会 steer、取消、重启或结束任务。完整终态在
host 重启或 registry 淘汰后仍可读取。若保留的非终态 dispatch 已无可观察 runtime，
读取接口说明观察不可用并保留真实存储状态；不编造中断，也不无限等待不存在的观察者。

呈现绑定通过共享 projection 与 intent 契约暴露不可变的执行值。打开或关闭浮层时，
组合层连接或解除对应观察。前端组件不持有原生 reader 或 transport 生命周期端口。
增量更新保留记录列表的不可变前缀，不复制整份历史，也不建立第二套执行权威。

`agent.conversation.execution.detach` 使用原流的 request/workflow 身份及精确执行
作用域，释放该观察者、唤醒等待并结束该流，不结束共享对话传输或 Agent。晚到的帧
仍隔离在被关闭的请求内。

前端依据既有发送或 dispatch 事实，在正文到达前展示等待气泡；它不是新持久化的
assistant 消息，也不是第二套生命周期权威。思考或工具记录不算首段回复正文。
真实回复文字或终态结束等待。视觉与搜索交互由
[设计系统](../functionality/DESIGN-SYSTEM.md#conversation-loading-and-hierarchy) 定义。

## Peer 消息身份边界

已接受的端点会话目标区分原始消息身份、作者、代表主体、来源端点、转发者和本地顺序。
传输收讫、端点收讫、应用接纳、已读及约定验收不能互换。编辑/删除引用因果来源及修订，
同步和恢复不得复活删除内容，也不能使转发者取得原作者权限。

通信历史可合并，不代表执行权限、预算准入和撤权可最终一致；它们仍由既有属主裁决。
[工作流控制](ASSISTANT-WORKFLOW-CONTROL.zh-CN.md) 拥有效果接纳，
[安全边界](SECURITY-AND-DATA-BOUNDARY.zh-CN.md) 拥有撤权后的结果结算。
端点集成须验证这些区分，本地消息可用不能代替其证明。
