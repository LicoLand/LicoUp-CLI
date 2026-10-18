# LicoUp Gateway Runtime

[English（规范版本）](gateway-runtime.md) · 简体中文（本地化）

Private Gateway Runtime 是单一本机进程（`lico-gateway`），只拥有一层：

1. **LLM Gateway** — 回环 HTTP 模型协议路由与凭证交接。见
   [`llm-gateway.zh-CN.md`](llm-gateway.zh-CN.md)。

Communication Channel 层已不在此进程内：Telegram 通道是
`org.licoland.channel.telegram` 包自身的进程（`lico-telegram-channel`），
按需安装，由宿主管理。

## 进程

- 二进制：`lico-gateway`（遗留名 `lico-llm-gateway` 仍运行同一运行时）。两者都是可选包自有的程序
  （`crates/licoup-private-gateway/entry`），由包清单以 `runtime.entry = "bin/lico-gateway"` 声明；
  内核不声明任何 gateway 目标。
- 启动：由程序自身启动。它接受 `--port`、`--config`、`--usage`、`--state-root`、
  `--credentials-fd`、`--credentials-control`、`--client-token-file` 与 `--check`；未指定路径时自行解析自身状态布局，在尚无配置时物化默认 provider 配置，
  并生成其鉴权端点所需的客户端令牌。登录项正是以仅带 `--port` 的方式启动它，登录时不再经由 CLI 转接。
- 生命周期 CLI：`gateway service {status,start,stop,initialize}`，以及别名
  `llm-gateway service …`。
- 清单 CLI：`gateway inventory reload --stdin-json true` 只做 **局部** 热加载：
  更新已验收 conversation readiness（LLM gateway 状态目录下的私有
  `inventory.sock` + 持久 overlay）。绝不因清单变更重启 Gateway。控制套接字
  不可用时仅写入 overlay（`mode: overlay_pending`），运行中进程保持不动。
- Channel CLI：`gateway channel status`，
  `gateway channel telegram credentials|pairing|service …`。channel 命令管理
  Telegram 包的托管文件与其自有进程；gateway 进程不含 channel 代码。

## Telegram channel

- 传输：默认 Telegram Bot API long polling。
- 准入：私聊 pairing；批准后会话级委托。
- 控制：`/start`、`/pair`、`/unpair`、`/whoami`、`/status`、`/agent`、
  `/session`、`/new`、`/reset`、`/stop`、`/help`、`/commands`（别名：`/id`、
  `/agents`、`/sessions`、`/revoke`）。
- 入站：文本、caption、图片/文档/视频/动画/语音/音频/贴纸/圆视频、位置/
  场所/联系人/投票、引用回复、转发，以及 `edited_message`；归一化为带媒体
  占位符的智能体信封；斜杠命令从 text 或 caption 解析。
- 桥接：已配对普通内容 → `conversation_lane` send → `sendMessage` 回写
  （回复挂到触发消息）。
- 智能体准入：`/agent` 仅列出并绑定 conversation readiness 为 `ready` 且已
  检测到可执行文件的智能体，经宿主 lane 端口实时应答；未验收库存不会出现在
  通道上。
- 状态文件仍在 portable 的 `telegram-gateway/` 目录；通道以
  `org.licoland.channel.telegram` 包进程（`lico-telegram-channel`）运行，按需
  下载安装，生命周期由 `gateway channel telegram service status|start|stop`
  管理。

## 状态模式

`gateway service status` 在 `licoup.gateway-runtime.v1` 下报告 `layers.llm` 与
`layers.channels`；channel 层反映 Telegram 包的托管文件与就绪标记，而非进程内
线程。清单与 status 永不返回 bot token 或模型 API key。
