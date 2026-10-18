# 原生 CLI

| 参考 | 权威文档 |
| --- | --- |
| 英文规范 | [NATIVE-CLI.md](NATIVE-CLI.md) |
| 架构 | [架构索引](README.zh-CN.md) |
| 原生帧与客户端传输 | [前后端交互](CLIENT-NATIVE-INTERACTION.md) |
| Canonical Conversation | [会话领域](CONVERSATION-DOMAIN.zh-CN.md) |
| Subagents MCP 适配器 | [Subagent MCP](../protocols/subagent-mcp.zh-CN.md) |

本文拥有本地原生 CLI 入口与发现行为。`licoup` 是原生核心的主要本地
门面。它准入命令、启动或连接原生宿主并呈现结果，不依赖 Flutter。
Flutter 与独立 MCP 进程均调用该原生边界。CLI 保留核心中的直接批准、
Membership 准入、平台认证与受保护效果检查。

## 发现与调用

```sh
licoup help
licoup commands
```

帮助与 `licoup.cli-catalog/v1` JSON 目录均从准入使用的同一命令注册表
生成。目录提供精确命令路径、位置参数类型、选项类型、必填与可重复选项、
选项约束及帮助。`rpc.methods` 来自生成的原生协议，不另行维护命令清单。

直接调用目录中的命令。携带请求体的命令可用 `--stdin-json true` 从私有
标准输入读取一个 JSON 对象。用户内容和凭据不应进入命令行参数或 shell 历史。

```sh
licoup conversation execute --stdin-json true < request.json
licoup strategy execute --stdin-json true < request.json
licoup subagents execute --stdin-json true < invocation.json
```

`conversation execute` 与 `strategy execute` 默认使用持久原生宿主，
包括调用 CLI 退出后仍需继续的工作。单次结果的 JSON 请求响应保留原生操作
结果。`conversation execute --require-running-host` 仅连接已有宿主，
没有宿主时拒绝且不创建本地状态。类型化的 `conversation list` 与
`conversation get` 使用同一宿主并保留应用信封。会话请求类型与动作仍由
会话领域拥有。

## 持久方法与流

`licoup commands` → `rpc.methods` 中的每个方法都可通过下列入口调用：

```sh
licoup rpc call METHOD --stdin-json true < params.json
```

请求体为方法的原生 `params` 对象。通用 `execute` 方法的请求体还包含
`args`，即已注册命令的参数数组。`rpc call` 先验证生成的方法，再连接
宿主。它输出原始 `licoup.stdio.v1` NDJSON 帧，事件到达即刷新输出，
收到响应或终态帧后结束。沿用协议既有的单帧大小边界，不增加总输出截断或
任务期限。观察者断开不会向 Agent 发送中断。

双向客户端使用 `licoup rpc conversation` 连接持久原生宿主，承载同一
生成的方法帧。`licoup rpc stdio` 提供本地命令桥。持久 Agent 派发、
attach、活跃 turn 查询、会话动作、目录观察和策略执行均保留为原生方法。
原生可执行程序拥有宿主的启动与生命周期，无需 Flutter 启动或操作它。
内部 `rpc conversation-host` 入口由该原生进程监督器使用。

本地 `agent.conversation.execution` 流查看一轮精确 dispatch。
记录归属、游标与观察契约由
[本地执行过程查看](CONVERSATION-DOMAIN.zh-CN.md#13-本地执行过程查看) 定义。

## 独立 MCP 进程

本地[模型注册表](MODEL-REGISTRY.zh-CN.md)拥有公开模型目录刷新与标准身份，
其命令不属于远程 MCP 操作。

`subagents catalog` 提供原生调用者与操作 schema；`subagents execute`
准入包含 `name`、`arguments`、`caller` 的本地调用。原生 Subagents
领域检查调用者作用域。`mcp start`、`stop`、`reload`、`status` 管理独立
MCP 进程。开发时 `start` 与 `reload` 可通过 `--binary` 明确指定 MCP
可执行文件。MCP 适配器收窄的远程能力面由其
[协议文档](../protocols/subagent-mcp.zh-CN.md)拥有。

## 实现权威

| 关注点 | 源码 |
| --- | --- |
| 准入、帮助与命令目录 | `crates/licoup-native/src/ffi/commands/mod.rs` |
| 生成的 RPC 方法权威 | `schemas/conversation_protocol/conversation_protocol.schema.json` |
| CLI 请求投影 | `crates/licoup-native/src/ffi/commands/native_rpc.rs` |
| 原生宿主帧与流式输出 | `crates/licoup-native/src/platform/conversation_host_client.rs` |
| 原生可执行程序与宿主启动 | `crates/licoup-native/src/bin/licoup.rs` 与 `bin/licoup/conversation_host.rs` |
| 本地 Subagents 与 MCP 生命周期命令 | `crates/licoup-native/src/ffi/commands/subagents.rs` |
| 本地 Subagents 调用与调用者准入 | `crates/licoup-native/src/domain/subagents/local.rs` |

## 自定义操作属性

已接受的自定义边界扩展原有 native catalog 与工具合同。属性具有命名空间、类型、
默认值及能力版本边界，不能覆盖 actor、授权、幂等身份或 canonical 状态。
未知或无效工具输入返回类型化准入错误；Agent 普通文本即使像 JSON 也保留原文。
本节约束后续目录扩展，不代表已增加自定义命令。
