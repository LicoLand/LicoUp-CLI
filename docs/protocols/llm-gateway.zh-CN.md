# LicoUp 本机 LLM Gateway

[English（规范版本）](llm-gateway.md)

LLM Gateway 是 Gateway Runtime 的**下层**。本层权威实现位于
`crates/licoup-private-gateway/src/model/llm_gateway.rs`、
`crates/licoup-private-gateway/src/http/transport.rs`，以及统一进程 `lico-gateway`
（`crates/licoup-private-gateway/src/runtime`）。下层只监听回环地址，并按
“客户端协议 + 请求模型”精确路由，不使用全局当前供应商。消息 channel 见
[`gateway-runtime.zh-CN.md`](gateway-runtime.zh-CN.md)。

## 能力

- Codex 的 `/v1/responses` 可以转接到 Kimi 等 OpenAI Chat Completions 上游，
  包括工具调用、流事件以及有界 `previous_response_id` 历史。
- Claude Code 的 `/v1/messages` 可以按模型分别路由到 Anthropic Messages 或
  OpenAI Chat Completions 上游。
- Kimi、DeepSeek 与 Kilo API Key 分别存为独立的系统密钥库项目。原生宿主在释放
  密钥内容前要求 macOS 机主授权；清单不返回密钥尾号或内容。
- 机主授权（Touch ID 可用时必须使用，仅生物识别不可用时允许系统密码）在长驻的
  `licoup-cli` 进程中解锁密钥。
  冷启动 Gateway 时，通过继承的文件描述符把已解锁会话交给 sidecar；sidecar
  自身从不读取 Keychain。若托管 Gateway 已在运行，授权与撤销会通过 Gateway
  状态目录下的私有 Unix 控制套接字（权限 0600、同 uid）热加载更新后的租约，
  无需重启进程。不带交接 fd 独立启动的 sidecar 以未连接状态运行，直到热加载
  或之后带交接的启动。密钥只进入可清零的进程内租约，退出即销毁。
  7/30/60/90/180/365 天只是单次运行进程的最长有效期，不会跳过下次机主授权。
- 修改授权有效期不会撤销正在运行的 Gateway。当前进程继续沿用原租约；新周期在
  下次重建交接的机主授权（热加载）或冷启动时生效。
- Codex 与 Claude Code 的托管配置只指向本机回环 Gateway，上游 API Key 绝不复制到
  智能体配置文件。
- 经过本机客户端认证的 `GET /v1/models` 与 `GET /models` 会使用当前进程内租约中
  各厂商的凭据，实时请求对应上游 `/models`。没有已授权厂商凭据时返回空列表；
  已授权凭据无效时返回上游错误，不再回退到产品内置模型目录。

## 配置格式

```json
{
  "schemaVersion": 1,
  "providers": [
    {
      "id": "kimi-chat",
      "baseUrl": "https://provider.example/v1",
      "protocol": "open_ai_chat_completions",
      "credentialProvider": "kimi",
      "credentialStyle": "bearer"
    }
  ],
  "routes": [
    {
      "clientProtocol": "open_ai_responses",
      "requestedModel": "kimi-for-codex",
      "providerId": "kimi-chat",
      "upstreamModel": "provider-model-id"
    }
  ]
}
```

内置配置只定义 Kimi、DeepSeek 与 Kilo 三个固定厂商边界，不再包含产品维护的
模型目录。模型请求使用 `{provider}:{upstream-model-id}`，例如
`kimi:kimi-k3`、`deepseek:deepseek-v4-flash` 或
`kilo:anthropic/claude-sonnet-4.5`；Gateway 转发时只移除第一个厂商前缀。模型列表
保留上游返回的模型对象，并把 `id` 改为带厂商命名空间的值，同时补充
`upstream_id` 与 `gateway_provider`，从而让多个厂商的合并目录保持无歧义。
若一个已授权厂商不可用而其他厂商成功，Gateway 会返回健康模型，并附带
`partial: true` 与经过收敛的 `failed_provider_count`；模型发现总时限为 45 秒。

OpenCode 与 Pi 的自定义厂商配置要求显式模型项，因此其 agent-config plan/apply
会从运行中的 Gateway 快照实时列表。Gateway 停止时快照为空；Gateway 运行时，
上游目录或凭据错误会使计划失败，而不会替换为固定模型名。Codex 与 Claude Code
继续直接使用 Gateway 端点，不嵌入模型列表。
OpenCode/Pi 一键脚本拒绝应用空快照。

配置文件必须是绝对路径、普通文件且不超过 1 MiB。可先用
`lico-llm-gateway --config <绝对路径> --check` 校验，再启动 sidecar；默认监听
`127.0.0.1:15722`。非回环 HTTP 端点、重定向、未知厂商、没有厂商命名空间的动态
模型和未知字段都会关闭失败。

## 密钥托管

macOS 密钥托管执行体采用[app-like helper 与永久公开 CLI 入口](../platforms/MACOS-DIRECT-DISTRIBUTION.zh-CN.md#本机密钥托管验证)。Profile 和 access-group 授权属于 helper 执行体，Gateway 不获得钥匙串托管权。


日常显式授权只准入一个精确操作或有界批次，最多进行一次原生认证。所有选中
密钥共用保留的 `LAContext`；已有有效的精确授权不额外弹窗。取消、生物识别失败
或锁定不会触发密码回退。Sidecar 接收已授权交接，不再次认证。
等待认证期间，以原生结果或明确取消为准；客户端不另设响应超时。

Data Protection Keychain 调用复用该上下文并禁止交互。Classic Keychain ACL
使用独立的授权机制：适配器串行地临时禁止旧钥匙串 UI，每次效果结束后恢复
原设置，失败时也恢复。旧 ACL 或锁定的钥匙串阻止静默读取时返回
`secure_mesh_keychain_classic_access_requires_user_action`；批次不会发布部分交接，也不会
修改被拒绝项目的 ACL。授权不会逐项重写凭据。该错误本身不能区分钥匙串锁定、
ACL 不匹配或签名身份变化。原 ACL 允许静默访问的旧项目仍可使用；读取或迁移
受阻不等于单提示解锁成功。
原生授权结果只通过 `authorized: false` 和白名单原因码投影这类失败，既有授权
保持不变。桌面端提供受保护迁移操作并保留授权失败状态；读取失败不会授予权限。

`SecAccessControl` 不能转换旧 ACL。Data Protection 访问要求实际保管进程具有
有效的访问组 entitlement 和 provisioning；仅对本地可执行文件签名并不满足要求。
显式 `llm-gateway credentials migrate` 是旧项目的一次性升级入口，涵盖过期凭据。
它绑定独立迁移范围，允许 macOS 请求旧条目的原生 ACL 授权，系统可能要求密码。
这个例外只用于迁移，日常授权仍遵守单次指纹要求。无需重新输入 API Key，不放宽 ACL，
不导出明文文件。应用逐项复制到 Data Protection，读回验证后才删除 classic 源记录。
失败保留源记录；重试继续清理，不覆盖已验证的新记录。批次由凭据清单和存活原生会话
限定，等待用户认证期间没有客户端响应超时。
[状态迁移权威](../architecture/CLIENT-UPDATE-AND-STATE-MIGRATION.zh-CN.md#启动准入)
负责记录完成。迁移本身不启用凭据，也不发起供应商请求。参见 Apple 的
[钥匙串实现说明](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains)
与 [provisioning 要求](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles)。

桌面设置页可保存多个 Kimi、DeepSeek 与 Kilo 密钥。输入框默认模糊，内容通过私有 stdin
传给原生 CLI。保存后没有查看、复制和修改操作，只能删除；删除会移除对应 Keychain
项目，并通过本机 epoch 标记撤销已运行进程的租约。

原生接口仅开放以下闭合操作：

- `llm-gateway credentials status`
- `llm-gateway credentials list`
- `llm-gateway credentials migrate`
- `llm-gateway credentials create --stdin-json true`
- `llm-gateway credentials delete <credential-id>`
- `llm-gateway credentials lease <days>`
- `llm-gateway service status`
- `llm-gateway service start [--port]`
- `llm-gateway service stop [--port]`
- `llm-gateway agent-config plan <codex|claude-code|opencode|pi> <绝对配置根目录>`
- `llm-gateway agent-config apply <codex|claude-code|opencode|pi> <绝对配置根目录> --confirmation <digest> --confirmed [--stdin-json true]`

智能体配置命令只生成可审核且不含上游密钥的计划。Codex 使用官方自定义
`model_providers` profile 与 Responses API 基础地址；Claude Code 使用官方
`ANTHROPIC_BASE_URL`/`apiKeyHelper` Gateway 字段。OpenCode 只写入 sidecar
`opencode.licoup-gateway.json`（OpenAI Compatible Chat Completions），不会改写
`opencode.json` / `opencode.jsonc`；通过 `OPENCODE_CONFIG` 加载该 sidecar，由
OpenCode 与现有全局配置合并。Pi 只写入 sidecar `models.licoup-gateway.json`
（OpenAI Completions），不会整文件覆盖 `models.json`；一键脚本仅把
`providers.licoup-gateway` 合并进 `~/.pi/agent/models.json`。未知智能体在其精确
适配器加入统一运行时注册表之前一律关闭失败。OpenCode 与 Pi 的 apply 通过私有
stdin 接收 plan 中的精确模型快照，因此预览后的上游目录变化不会改变已确认内容。

开发者一键脚本（同样的 sidecar 语义）：

```bash
npm run client:opencode:add-gateway
npm run client:pi:add-gateway
```
