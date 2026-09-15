# UniRC-T v2：免账号设备访问

版本字段为整数 2；应用版本仍为 0.1.0。v0 的明文终端请求被拒绝，需同时升级中继、Agent 和客户端。

## 外层中继

沿用 `type:u8 | json_len:u32 LE | JSON | binary` 帧封装。type=50 为中继消息，单帧上限 64 KiB。JSON 是 `kind` 标记的枚举；完整字段见 `crates/rc-access/src/wire.rs`。

1. Agent 连接 `/agent/ws`。中继发送随机 `challenge`，Agent 用设备 CA 私钥签署带域分隔的挑战和名称，发送 `register`（version/device_id/name/ca_pem/signature/token）。中继确认 `registered`。
2. 客户端连接 `/client/ws`，发送 `hello`（version/token），中继确认 `welcome`。没有全平台设备列表推送。
3. 客户端发送 `open`（device_id）。中继向对应 Agent 发送 `incoming`（tunnel），向客户端发送 `ready`（tunnel/device_id/ca_pem）。离线返回明确错误。
4. 两端用 `data`（tunnel）附加的二进制块承载 TLS 1.3 字节流。中继只允许已登记的客户端连接和 Agent 连接实例使用该 tunnel；不能凭 tunnel ID 操作他人连接。
5. `close` 结束隧道；一端断开时中继通知另一端。队列超限时关闭，不进行静默丢包。

客户端在发送 TLS 应用数据前检查 CA DER 的 SHA-256 是否等于设备 ID。服务端证书 SAN 为 `shunyi-device.local`，由此 CA 签发；临时访问也必须验证此证书。长期访问还提供客户端证书与私钥，经 TLS 验证后由 Agent 检查当前证书指纹。

## 内层设备访问

TLS 内的每条消息为 `frame_length:u32 BE` + 现有帧格式（type=51）。JSON 和二进制数据均位于 TLS 内。

首条消息必须是 `authenticate`（method=certificate 或 temporary，后者包含 password）。验证成功返回 `authenticated`（device_id/user），失败返回 `error` 并关闭。任何认证前的 open、data、resize 都不会创建 PTY。

认证后支持 `open/opened`（terminal/cols/rows）、`data`（terminal + binary）、`resize`、`close/closed`、`error`、`ping/pong`。客户端先登记对应 terminal 的等待槽，再发送 open；响应按 terminal UUID 分发，不使用全局单个等待槽。

一个 TLS 访问连接拥有自己的 PTY 表。关闭最后一个 PTY 即结束设备访问。临时密码首次认证成功时永久标记已使用，不能用来建立另一个 TLS 访问连接；同一访问里的多个 PTY 共用该次授权。

## 信任与重启

中继不持有设备私钥或临时密码校验值。密码消耗和证书轮换写入设备端持久化状态，并通过文件锁处理 CLI、图形客户端和 Agent 并发修改。Agent 自动重连中继只恢复设备可达性，不恢复旧设备访问与 PTY。

安全限制与部署边界见 [SECURITY.md](../SECURITY.md)。
