# Agent 真实对话端到端测试

本目录是 LicoUp 每个 Agent 的**真实对话端到端测试**所在位置。

## 测试要求（验收标准）

每个 Agent 目录对应**一个完整的端到端对话测试用例**，链路必须从前端界面开始、到 Agent 返回消息回显为止：

1. **起点：前端界面发送**
   测试必须从 LicoUp 前端界面（Composer）开始——在界面中向指定 Agent 发送一条消息，而不是绕过界面直接调用底层通道。

2. **真实对话**
   消息必须真实到达指定 Agent，Agent 真实执行并返回消息。不使用 mock、假后端或 fixture 替身。

3. **终点：界面回显**
   Agent 返回的消息必须在 LicoUp 前端界面中回显（界面看到 Agent 的回复），链路才算走通。

即：**界面输入 → 发送 → Agent 真实回复 → 回复回显到界面**，整条链路一次跑通，缺一不可。

## 当前状态（不合格）

⚠️ 现有 `*/conversation.test.mjs` 用例**未达到上述标准**：

- 当前用例只通过 CLI sidecar（`licoup-cli agent conversation send`）发送消息并断言 Agent 回复非空
- 这仅覆盖了链路的中段（发送 → Agent 返回），起点没有经过前端界面，终点没有验证界面回显
- 因此**所有现有用例均视为不合格**，需按本 README 要求重写为完整端到端测试

## 目录组织

```
tests/product-e2e/cli/agent-conversations/
├── <agent-id>/conversation.test.mjs   # 每个 Agent 一个目录、一个端到端用例
├── support/                           # 公共测试工具（固定逻辑）
└── README.md
```

- **一个 Agent 一个目录**，目录内只做这一件事：该 Agent 的前端界面到回显的端到端真实对话测试。
- 公共的固定工具（界面驱动、sidecar 解析、环境准备、清理、断言等）放在 `support/`，供各 Agent 用例复用。

## Agent 清单

| Agent | 目录 |
| --- | --- |
| openclaw | `openclaw/` |
| claude-code | `claude-code/` |
| codex | `codex/` |
| antigravity | `antigravity/` |
| opencode | `opencode/` |
| copilot | `copilot/` |
| kilo-code | `kilo-code/` |
| cursor | `cursor/` |
| hermes | `hermes/` |
| kimi-code | `kimi-code/` |
| pi | `pi/` |
| lico-agent | `lico-agent/` |

## 验收标准

一个用例只有同时满足以下条件才算通过：

- [ ] 从前端界面（Composer）发送消息，非 CLI/底层直连
- [ ] 消息真实到达指定 Agent，Agent 返回真实回复
- [ ] 回复在界面中回显，且与 Agent 实际返回内容一致
- [ ] 不依赖 mock / fixture / 假后端
- [ ] 一个 Agent 一个用例，可独立运行、独立验收
