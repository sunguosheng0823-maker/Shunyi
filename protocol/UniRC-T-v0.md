> 历史协议。2026-09-15 起新版本拒绝此协议的明文终端请求；当前规范见 [UniRC-T v2](UniRC-T-v2.md)。

# UniRC-T 协议 v0（Terminal）

状态：**草案冻结（M0）** —— v0 期间只加不改；破坏性变更升 v1。

## 设计原则

1. **传输无关**：帧格式不绑定传输层。v0 用 WebSocket（二进制消息）；后续 QUIC 流、TCP 443 中继、WebSocket 子集（Web 端）承载同一帧格式。
2. **服务器不可见内容**（M1 起）：信令服务器只做注册/配对/转发；会话数据在两端点对点加密后对服务器不透明。
3. **最小可信面**：Agent 只做出站连接，无监听端口。
4. **可演进**：帧带版本协商；消息枚举可扩展，未知类型必须忽略不报错。

## 帧格式（传输层之上）

一条帧 = 一个二进制消息/一份数据报：

```
偏移  长度  字段
0     1    frame_type (u8)
1     4    json_len (u32, 小端)
5     N    JSON 负载（UTF-8，可为空对象 {}）
5+N   M    二进制附加负载（可选，PTY 数据等）
```

> M0 简化：会话数据暂不加密。M1 引入 E2E 加密后，`SessionData` 的二进制附加负载改为加密信封（X25519+ChaCha20-Poly1305 / 国密 SM2+SM4-GCM 双栈，模式在握手时协商）。信令（注册/配对）始终明文+TLS 承载。

## 消息类型

| type | 名称 | 方向 | JSON 字段 |
|---|---|---|---|
| 1 | Register | Agent→Server | device_id, name, os, version, token |
| 2 | RegisterAck | Server→Agent | ok, reason? |
| 3 | Heartbeat | 双向 | ts |
| 4 | DeviceList | Server→Client | devices: [{device_id, name, os, online}] |
| 5 | Hello | Client→Server | token |
| 6 | SessionOpen | Client→Server / Server→Agent | device_id / session_id, device_id |
| 7 | SessionAccept | Agent→Server | session_id |
| 8 | SessionReady | Server→Client | session_id |
| 9 | SessionData | Client↔Server↔Agent | session_id, stream(u8: 0=pty) + 二进制负载 |
| 10 | SessionClose | 双向 | session_id, reason? |
| 11 | SessionResize | Client→Agent | session_id, cols, rows |
| 12 | Error | 双向 | code, message, session_id? |

## 流程

### 注册（Agent）
```
Agent ──Register──▶ Server
Server ──RegisterAck(ok)──▶ Agent
之后每 20s Heartbeat；60s 无消息判掉线并广播 DeviceList
```

### 会话建立（v0 仅中继；P2P 打洞 v1 加入，信令已预留）
```
Client ──Hello──▶ Server ──▶ DeviceList（推送在线设备，增删都推）
Client ──SessionOpen{device_id}──▶ Server
Server ──SessionOpen{session_id, device_id}──▶ Agent（离线则回 Error 404）
Agent ──SessionAccept{session_id}──▶ Server ──▶ Client: SessionReady{session_id}
此后双向 SessionData / SessionResize 经服务器转发
任一端 SessionClose → 转发对端，会话销毁
```

## 安全（路线图）

- v0：共享 token 认证（`UNIRC_TOKEN`），TLS 由 wss:// 承载（生产必须）
- v1：E2E 会话加密（服务器不可见）；设备首次连接信任确认（TOFU）
- v2：国密模式 SM2 握手 + SM4-GCM 数据面（私有化/信创开关）
