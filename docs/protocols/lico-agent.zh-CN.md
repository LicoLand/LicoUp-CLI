# Lico Agent

[English](lico-agent.md) · 简体中文

权威实现：`licoup-agent-targets` 的 `domain/lico_agent/`、
`licoup-agent-lico-agent` 的 `driver/`、共享的 `platform/process_sandbox/`，
以及同级二进制 `lico-agent`。实现或验证变更时同步更新本文。

Lico Agent 是 LicoUp **自研**运行时，在智能体列表中作为普通一项
（`lico-agent`），与 Pi、Codex 等第三方适配器同级。它不是特殊 Conversation、
角色或编排权威。

## 能力

- Stdio JSONL RPC（`lico-agent-rpc-stdio-jsonl`）：`get_state`、`prompt`、
  `steer`、`abort`。
- 模型仅经本地回环 [LLM Gateway](llm-gateway.zh-CN.md)；非回环地址失败关闭。
- 配置档：`base` 与继承模式 `plan`（仅 `read` + `write_plan`）。
- Plan 模式在 macOS seatbelt 能力
  `platform-lico-agent-plan-isolated-v1` 下拉起：写权限仅限一个绝对路径计划
  文件；出站网络仅限 Gateway 端口。非 macOS 上 Plan 失败关闭。
- 会话状态由父进程保存在 `{portable}/client-state/lico-agent/`；子进程不落盘
  会话库。

## 与统一 Conversation 的关系

统一 Conversation 后端可以像接纳其它可运行集成一样，通过普通 Agent Membership
接纳 Lico Agent。用户定义的 Conversation Role 可以包含该 Membership，显式启动的
适应性飞轮可从 Role 的有序候选池中选择它。模型中没有内建主智能体槽、自动群聊轮转
或专用 Lico 群聊身份；当前交付也不宣称已经提供桌面群聊或飞轮编辑器。
