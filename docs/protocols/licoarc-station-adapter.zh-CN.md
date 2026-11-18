# Lico Arc 通讯站 Adapter

[English（规范版本）](licoarc-station-adapter.md) ·
简体中文（本地化） · [协议索引](README.md)

本文描述 LicoUp 对已发布 `licoarc-client` adapter 的边界。线路权威仍属于
Lico Arc Protocol；面向通讯站的传输契约、信封校验、Pairwise Protection、
会话与防重放策略由固定的 `licoarc` / `licoarc-client` 发布版本实现，不属于
本仓库。通讯站为独立实现且明确不可信；任何通讯站仓库都不是 LicoUp 的运行
或发布依赖。

## 协议与端点权威

Lico Arc Protocol 拥有所有稳定、线上可观测的 Pairwise Protection、Generic
Message、Reliable Exchange、协商与 Transport Profile 契约。LicoUp 拥有合规
本地执行、私钥、Provider 配置、明文、历史、备份、用户信任、审批和本地效果。

仓库内已退役的端点保护预览——本地 `licoarc.relay.v1` 外层 adapter、其
OpenMLS 成对会话、棘轮、密钥透明性与 HTTP BadTower 通讯站传输——均已移除。
已发布的 LicoArc 客户端栈是本仓库执行的唯一通讯站 adapter；不存在双通讯站
线路兼容模式或翻译网关。

## 本地 adapter 边界

LicoUp 在 adapter 一侧承担四项有界职责：

- 凭据。通讯站 URL 与访问令牌必须由客户端明确配置，不填充官方网络默认值。
  设备签名密钥不离开托管：`SessionCustody` 经 `KeyCustody` 在操作系统机密
  存储中解析密钥句柄，并只向 SDK 交付托管句柄，绝不导出密钥材料。
- 持久状态。配对 offer/claim、对端会话、游标与出站 outbox 由
  `SessionState` / `DevicesState` 在设备数据目录下原子持久化。
  `devices sync` 会重放每个对端会话的持久出站工作，包括当前进程未触碰的
  会话。
- 接收循环。设备注册期间，宿主按 SDK 的重连语义运行 `MailboxClient`：
  带抖动退避的无界重试；被取代、被吊销、设备不匹配或被拒绝时终止。每个
  投递都会转交处理线程；只有处理器报告 `Done` 后才发送 ACK，`Defer` 会
  保留投递供下一次连接重投。
- 桥接分派。会话协议工作在同步处理线程上运行；每个出站报文经桥接
  `Transport` 阻塞等待异步侧回报通讯站结果——接受、拒绝、暂时或歧义。

## 不可信通讯站边界

通讯站上报的状态只是运输提示，绝不是端点证据。租约、接受标志、重复标志、
删除确认、HTTP 状态、通讯站时间或队列状态都不能确立投递、准入、已读或
接受——SDK 的 `StationHint` 类型将这一点设为常量
（`is_endpoint_evidence()` 恒为 `false`）。

Adapter 接受 HTTPS origin，也允许有界本机工作使用回环 HTTP；它不会发现
通讯站、导入同级仓库源码，也不接受通讯站提供的算法、密钥、信任根、身份、
策略或可执行代码。

## 接受顺序

接收报文只有按顺序完成以下步骤后，才可以 ACK 并移除：

1. 通过 SDK 的封闭外层校验接受；
2. 在 LicoArc 会话引擎内完成认证与校验——预期对端、会话、方向、新鲜性与
   防重放状态；
3. 处理器应用端点认证的命令或结果状态转换并报告 `Done`。

任何通讯站提示都不能绕过该顺序，也不能建立最终接收结论。

## 本机验证

确定性的进程内覆盖端到端验证两套数据目录：配对 offer/claim/cancel、已配对
消息发送与结果取回、重复幂等、存在活动工作时拒绝解绑、不能销毁活动会话的
已完成配对取消，以及 `devices sync` 排空所有对端会话的持久出站工作。真实
通讯站验收属于另行指派的工作流，不是此边界的工程证明。

诊断保持有界：命令输出与证据中不出现私有路径、存储值、凭据材料、原始机密
或密文。
