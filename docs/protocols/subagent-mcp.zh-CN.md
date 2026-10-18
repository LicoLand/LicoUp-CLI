# LicoUp Subagent MCP

Updated: 2026-09-30

| 参考 | 文档 |
| --- | --- |
| 规范版本 | [English](subagent-mcp.md) |
| 原生门面 | [原生 CLI](../architecture/NATIVE-CLI.zh-CN.md) |
| Provider 执行与注册 | [Agent 适配器](../architecture/AGENT-ADAPTERS-ARCHITECTURE.zh-CN.md) |
| 公共模式 | [Subagent MCP Schema](../../schemas/subagent_mcp/subagent_mcp.schema.json) |

## 模块边界

`crates/licoup-public-gateway` 独立拥有可选公共 MCP 服务和 stdio 连接器。它使用公开
Rust 依赖及中立的 `licoup-foundation` crate 独立构建，后者负责共享数据根目录
解析和进程访问租约。它不依赖 native、Flutter 或领域 crate；运行时操作
通过已安装原生 CLI 的公开 `licoup.stdio.v1` 进程契约调用 LicoUp。
原生出站 MCP 客户端适配器仍是独立能力。

MCP 传输凭据继续由独立的 `private_state` 模块负责，不是 Foundation 通用
私有文件工具的门面。尤其是 Windows 凭据文件，必须属于当前用户，具有仅含
当前用户授权的受保护 DACL，并拒绝重解析点。共享数据根目录协调能力不替代
或放宽这些凭据保护。

原生 `domain/subagents` 拥有调用方 Membership 检查、Provider 执行准入、
持久化分派声明、继续、取消与回执。Canonical Conversation 与 PersistentTurn
保留既有存储、调度器、历史、运行绑定及受保护操作权限。MCP 模块不复制这些权限。

## 远程接口

服务名为 `lico-up-subagents`，版本 `0.14.0`，支持协议版本 `2025-06-18`
和 `2025-11-25`。完整有序工具许可列表如下。

| 工具 | 操作 |
| --- | --- |
| `lico_subagents_list` | 读取已准入的目标清单 |
| `lico_subagent_probe` | 读取目标就绪及能力投影 |
| `lico_subagent_delegate` | 准入新的 Membership 轮次 |
| `lico_subagent_continue` | 通过私有原生绑定继续轮次 |
| `lico_subagent_cancel` | 请求取消准确的活动分派声明 |

工具模式来自 `licoup subagents catalog`，独立适配器仅选择上述五个名字并拒绝
其余操作。输入模式全部封闭。连接器通过 `--caller` 或
`LICOUP_MCP_CALLER_PROVIDER` 声明 Provider；准入调用方集合来自原生适配器注册表，
服务及连接器不维护第二份 Provider 清单。

Assistant Profiles、Assistant 工作流、完整 Conversation 及其他原生能力仍通过
[本地 CLI](../architecture/NATIVE-CLI.zh-CN.md) 使用，不由 MCP 远程公开。
捆绑的 `licoup-guide` 负责把调用方引导到相应接口。

## 独立生命周期与开发

可执行文件 `lico-subagent-mcp` 是可选包 `org.licoland.feature.mcp` 的负载，
不是客户端捆绑的一部分。客户端既不分发副本，也不自行启动该服务：包声明
`on-demand` 激活，因此由调用方启动；包缺失或关闭时不启动任何进程，也不发布
任何端点。`licoup mcp start`、`stop`、`status`、`reload` 仍是该进程的本地原生
门面；`start` 和 `reload` 支持 `--binary` 选择独立构建的模块。命令确保原生宿主
可用，无需 Flutter。

```sh
node tools/scripts/cargo-client.mjs build -p licoup-public-gateway
licoup mcp reload --binary <built-lico-subagent-mcp>
```

发布时会在打包该目录前填充包声明的原生入口，因此仓库中提交的入口是暂存占位符，
而不是程序：

```sh
node tools/distribution/client-release-package-stage.mjs stage
node tools/distribution/client-release-package-stage.mjs verify
npm run client:release:package-index:build
```

模块也可直接执行 `service start|stop|status|reload`，通过 `LICOUP_CLI_BINARY`
明确指定原生 CLI，通过 `LICOUP_HOME` 限定状态目录。已发布安装仍可使用优先级更低的
`LICOUP_PORTABLE_DIR` 别名。可复用、有界的
公开 CLI 会话池承载准入请求；取消具有独立保留通道，不被缓慢清单或准入请求占用。

停止操作先认证私有控制请求，停止接受新帧，并在释放服务租约前排空已准入请求。
重新加载随后启动所选文件。发现文档变化后，连接器更新 MCP 握手；绝不重放
效果不确定的工具调用。原生轮次、声明、工作流和历史不受模块停止或更新影响，
没有模块计时器取消轮次。服务崩溃后使用操作系统租约存活性清理旧发现记录，
不会凭不可信 PID 杀死进程。

## 权限与谱系

每项效果都要求已认证调用方，以及准确、同一 Conversation 内的活动 Agent
Membership。原生存储在启动 Provider 工作前提交持久化声明，并拒绝自调用、
重复活动边、跨 Conversation、重复祖先、环和超过四级深度的调用。
继续操作解析私有适配器持有的原生身份；调用方不提交或接收原生会话和路径。
结果不确定的取消保持 `reconciliation-required`。

委派、继续和取消将 `subagent_mcp_inbound` 证据与分派声明、所属 PersistentTurn
共同记录到 Canonical Conversation。这些持久事件名仍是原生数据格式契约。
只读清单和探测不启动 Provider、不注入提示、不刷新历史，也不另建运行权限。

## 本地安全与隐私

服务只绑定数字回环地址。每个请求必须具有准确 Host、没有浏览器 Origin，
并在查询会话或执行效果前完成认证。私有发现文档为每个准入调用方提供短期 bearer
令牌，并为控制端点提供独立令牌。工具调用方不能使用控制端点；控制令牌不会进入
公共工具结果或连接器诊断。发现文档私有、原子写入；关闭只删除自身代次。

连接、会话及准入请求数有界。HTTP 输入帧和健康检查可以约束传输 I/O，已准入
原生工作没有适配器执行期限。协议取消只结束观察，只有显式
`lico_subagent_cancel` 操作可以中断 Agent。

Provider 注册继续要求既有摘要绑定、一次性批准。命名空间条目、外部内容检查、
平台认证、操作系统权限、原生密钥保管和受保护操作批准仍由原生模块负责。
适合远程调用不构成开放公共监听器、修改端点、弱化认证或扩大数据传输的权限。
已配置连接器在既有授权内使用现有已认证本地传输。

## 验证

| 范围 | 维护入口 |
| --- | --- |
| 公共传输与工具契约 | [互操作清单](../../tests/product-e2e/cli/subagent-mcp/interop-manifest.yaml) |
| 调用方协议验收 | [上游互操作](../../tests/product-e2e/cli/subagent-mcp/upstream.mjs) |
| 原生目标执行与控制 | [下游互操作](../../tests/product-e2e/cli/subagent-mcp/downstream.mjs) |
| 独立进程生命周期、恢复及取消隔离 | [模块生命周期测试](../../crates/licoup-public-gateway/tests/lifecycle.rs) |
