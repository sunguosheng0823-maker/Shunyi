//! UniRC-T 帧编解码。
//!
//! 一帧 = `type(1B) | json_len(u32 LE, 4B) | json(N B) | bin(M B)`。
//! 传输无关：WS 下一条二进制消息即一帧；QUIC/TCP 上由上层负责分帧边界时同样按此格式。

use serde::de::DeserializeOwned;
use serde::Serialize;

const MAX_JSON_LEN: u32 = 1 << 20; // 1 MiB，信令 JSON 上限
const MAX_BIN_LEN: u32 = 8 << 20; // 8 MiB，单帧二进制负载上限（文件块上限受此约束）

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub ty: u8,
    pub json: Vec<u8>,
    pub bin: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("帧过短")]
    TooShort,
    #[error("JSON 长度超限: {0}")]
    JsonTooLarge(u32),
    #[error("二进制负载超限: {0}")]
    BinTooLarge(u32),
}

impl Frame {
    pub fn new<T: Serialize>(ty: u8, json: &T, bin: Vec<u8>) -> Result<Self, serde_json::Error> {
        Ok(Frame {
            ty,
            json: serde_json::to_vec(json)?,
            bin,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let json_len = self.json.len() as u32;
        let mut out = Vec::with_capacity(5 + self.json.len() + self.bin.len());
        out.push(self.ty);
        out.extend_from_slice(&json_len.to_le_bytes());
        out.extend_from_slice(&self.json);
        out.extend_from_slice(&self.bin);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        if buf.len() < 5 {
            return Err(FrameError::TooShort);
        }
        let ty = buf[0];
        let json_len = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]);
        if json_len > MAX_JSON_LEN {
            return Err(FrameError::JsonTooLarge(json_len));
        }
        let json_len = json_len as usize;
        if buf.len() < 5 + json_len {
            return Err(FrameError::TooShort);
        }
        let json = buf[5..5 + json_len].to_vec();
        let bin = buf[5 + json_len..].to_vec();
        if bin.len() as u32 > MAX_BIN_LEN {
            return Err(FrameError::BinTooLarge(bin.len() as u32));
        }
        Ok(Frame { ty, json, bin })
    }

    /// 反序列化 JSON 负载；空/缺失字段由调用方的类型默认值兜底。
    pub fn json_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TestMsg {
        session_id: String,
        cols: u16,
    }

    #[test]
    fn roundtrip_with_bin() {
        let msg = TestMsg {
            session_id: "s-1".into(),
            cols: 120,
        };
        let f = Frame::new(9, &msg, b"hello pty".to_vec()).unwrap();
        let enc = f.encode();
        let dec = Frame::decode(&enc).unwrap();
        assert_eq!(dec.ty, 9);
        assert_eq!(dec.json_as::<TestMsg>().unwrap(), msg);
        assert_eq!(dec.bin, b"hello pty");
    }

    #[test]
    fn roundtrip_empty_bin() {
        let f = Frame::new(3, &serde_json::json!({"ts": 123}), vec![]).unwrap();
        let dec = Frame::decode(&f.encode()).unwrap();
        assert_eq!(dec.bin.len(), 0);
        let v: serde_json::Value = dec.json_as().unwrap();
        assert_eq!(v["ts"], 123);
    }

    #[test]
    fn reject_oversize_json_len() {
        let mut buf = vec![1u8];
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Frame::decode(&buf),
            Err(FrameError::JsonTooLarge(_))
        ));
    }

    #[test]
    fn reject_short() {
        assert!(matches!(Frame::decode(&[1, 2, 3]), Err(FrameError::TooShort)));
    }
}
