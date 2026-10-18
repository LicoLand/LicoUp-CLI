# Lico Arc 候选通讯站 Adapter

[English（规范版本）](licoarc-station-adapter.md) ·
简体中文（本地化） · [协议索引](README.md)

本文描述 LicoUp 如何通过客户端自有 adapter，把端点保护内容映射到候选
`licoarc.relay.v1` 通讯站外层契约。线路权威仍属于 Lico Arc Protocol；
BadTower 仍是独立实现、明确不可信的通讯站。LicoUp 的运行或发布不依赖这两个
仓库。

## 协议与端点权威

Lico Arc Protocol 拥有所有稳定、线上可观测的 Pairwise Protection、Generic
Message、Reliable Exchange、协商与 Transport Profile 契约。LicoUp 拥有合规
本地执行、私钥、Provider 配置、明文、历史、备份、用户信任、审批和本地效果。

[当前正在退役的端点保护预览](../STATUS.zh-CN.md)是 LicoUp 实现，不是
Lico Arc Profile。它不承诺未来互操作；完整固定 Lico Arc Protocol Line
替换它时会直接退役。这与下文当前 `licoarc.relay.v1` 外层 adapter 相互独立；
后者仍是已经实现并在本机验证的候选 adapter。

## 封闭外层边界

Adapter 只生成并接受固定候选定义的五个字段：

- `contractVersion`；
- `envelopeId`；
- `mailboxId`；
- `ciphertext`；
- `expiresAt`。

未知字段、重复字段、不受支持的契约标识、无效标识、无效过期时间、畸形载体或
超限值都会关闭失败。外层对象没有明文字段。

端点保护载体是 `ciphertext` 内的一项规范值。LicoUp 把完整外层路由上下文
绑定为认证数据，并使用 XChaCha20-Poly1305 保护私有头。端点内容继续由当前
端点保护预览会话与棘轮保护。这些内层载体、会话与棘轮细节描述的是当前
LicoUp 预览，不是规范 Lico Arc Pairwise Protection 或 Transport Profile，
也不是 BadTower 算法。

## 四项通讯站操作

BadTower 运输 adapter 只暴露四项有界操作：

| 操作 | 通讯站本地效果 | 端点含义 |
| --- | --- | --- |
| 租约 mailbox | 请求临时 mailbox 工作资格 | 不可信运输提示 |
| 发送信封 | 提交一个封闭 Lico Arc 信封 | 通讯站接受不等于对端接收 |
| 接收信封 | 读取一组有界候选 | 每个值都必须经过严格端点校验 |
| 删除信封 | 请求移除一个已接收信封 | 通讯站确认不是端点证据 |

通讯站 URL 必须由客户端明确配置，目前没有填充官方网络默认值。Adapter 接受
HTTPS origin，也允许有界本机工作使用回环 HTTP；它不会发现通讯站、导入同级
仓库源码，也不接受通讯站提供的算法、密钥、信任根、身份、策略或可执行代码。

## 端点接受顺序

接收信封只有按顺序完成以下步骤后，才可以删除：

1. 严格校验五字段外层对象与加密载体；
2. 认证并解密私有头与受保护内容；
3. 检查预期端点、会话、方向、新鲜性与防重放状态；
4. 接受端点认证的命令或结果状态转换。

租约、HTTP 状态、通讯站时间、队列状态、接受标志、重复标志或删除确认都不能
绕过该顺序，也不能建立最终接收结论。

## 已验证候选场景

有界本机验收使用两套分别持有客户端状态的全新端点、固定的 Lico Arc 候选
bundle 和实际 BadTower 候选进程。它验证：

- 一次受保护命令与认证结果往返；
- 通讯站可见外层准确只有五个字段；
- 通讯站可见存储中没有端点明文；
- 不受支持或扩展信封被拒绝；
- 通讯站运输提示不会被提升为端点证据。

验收回执保持隐私最小化，不包含端点内容、密文、密钥材料、端点或机器身份、
私有地址或原始运行时记录。

该场景只是候选互操作证据。它不发布 Lico Arc Protocol，不发布 LicoUp 或
BadTower，不声明平台支持，也不证明官方网络正在运营。

## 迁移状态

退役的客户端专用通讯站信封/API、路由族、服务会话 scope、配置、夹具和文档均已
移除。不存在双通讯站线路兼容模式或通讯站翻译网关。这项已完成
的通讯站迁移不表示当前正在退役的端点保护预览已经移除或已被接受为 Lico Arc
Profile。
