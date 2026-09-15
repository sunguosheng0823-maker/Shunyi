//! rc-core：UniRC-T 协议帧编解码与消息定义。
//!
//! 所有端（客户端/Agent/服务器）共享此 crate，保证线上一致。
//! 帧格式见 protocol/UniRC-T-v0.md：type(1B) + json_len(4B LE) + JSON + 二进制负载。


pub mod frame;
pub mod message;

pub use frame::Frame;
pub use message::*;
