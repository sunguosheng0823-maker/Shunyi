//! UniRC-T 消息类型定义（protocol/UniRC-T-v0.md 的代码化）。
//!
//! 约定：新增消息只能追加枚举值；接收方遇到未知 type 必须忽略。

use serde::{Deserialize, Serialize};

pub mod ty {
    pub const REGISTER: u8 = 1;
    pub const REGISTER_ACK: u8 = 2;
    pub const HEARTBEAT: u8 = 3;
    pub const DEVICE_LIST: u8 = 4;
    pub const HELLO: u8 = 5;
    pub const SESSION_OPEN: u8 = 6;
    pub const SESSION_ACCEPT: u8 = 7;
    pub const SESSION_READY: u8 = 8;
    pub const SESSION_DATA: u8 = 9;
    pub const SESSION_CLOSE: u8 = 10;
    pub const SESSION_RESIZE: u8 = 11;
    pub const ERROR: u8 = 12;


}

/// 会话内流编号：v0 仅 PTY。
pub mod stream {
    pub const PTY: u8 = 0;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub device_id: String,
    pub name: String,
    pub os: String,
    pub version: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAck {
    pub ok: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOpen {
    /// Client→Server 时为目标设备；Server→Agent 时附带分配的会话 id。
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAccept {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReady {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    /// v0 固定为 stream::PTY
    #[serde(default = "default_stream")]
    pub stream: u8,
}

fn default_stream() -> u8 {
    stream::PTY
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClose {
    pub session_id: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResize {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: u16,
    pub message: String,
    #[serde(default)]
    pub session_id: String,
}

pub mod error_code {
    pub const DEVICE_OFFLINE: u16 = 404;
    pub const UNAUTHORIZED: u16 = 401;
    pub const AGENT_REJECT: u16 = 503;
    pub const INTERNAL: u16 = 500;
    pub const ENCRYPTION_FAILED: u16 = 510;
}
