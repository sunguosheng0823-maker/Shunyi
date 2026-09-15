//! Only Relay messages cross the relay in cleartext. Access messages travel inside TLS.
use anyhow::{ensure, Context, Result};
use rc_core::Frame;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::mpsc,
};

pub const MAX_PACKET: usize = 64 * 1024;
pub const CHUNK: usize = 16 * 1024;
pub const QUEUE: usize = 32;
pub const VERSION: u8 = 2;
pub const DESKTOP_VERSION: u8 = 3;
pub fn supported(version: u8) -> bool {
    matches!(version, VERSION | DESKTOP_VERSION)
}
const RELAY_TYPE: u8 = 50;
const ACCESS_TYPE: u8 = 51;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Relay {
    Challenge {
        nonce: String,
    },
    Register {
        version: u8,
        device_id: String,
        name: String,
        ca_pem: String,
        signature: String,
        token: String,
    },
    Registered,
    Hello {
        version: u8,
        token: String,
    },
    Welcome,
    Open {
        device_id: String,
    },
    Incoming {
        tunnel: String,
    },
    Ready {
        tunnel: String,
        device_id: String,
        ca_pem: String,
    },
    Data {
        tunnel: String,
    },
    Close {
        tunnel: String,
    },
    Error {
        code: String,
        message: String,
    },
    Ping,
}
impl Relay {
    pub fn packet(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        encode(RELAY_TYPE, self, data)
    }
    pub fn decode(bytes: &[u8]) -> Result<(Self, Vec<u8>)> {
        decode(RELAY_TYPE, bytes)
    }
}

// Do not derive Debug: Authenticate contains a one-time secret.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Access {
    Authenticate {
        method: String,
        #[serde(default)]
        password: String,
    },
    Authenticated {
        device_id: String,
        user: String,
    },
    Open {
        terminal: String,
        cols: u16,
        rows: u16,
    },
    Opened {
        terminal: String,
    },
    Data {
        terminal: String,
    },
    Resize {
        terminal: String,
        cols: u16,
        rows: u16,
    },
    Close {
        terminal: String,
    },
    Closed {
        terminal: String,
        reason: String,
    },
    Error {
        terminal: Option<String>,
        message: String,
    },
    DesktopOpen {
        desktop: String,
        display: u32,
        control: bool,
    },
    DesktopOpened {
        desktop: String,
        display: rc_desktop::DisplayInfo,
        control: bool,
    },
    DesktopFrame {
        desktop: String,
        sequence: u64,
        offset: usize,
        total: usize,
        width: u32,
        height: u32,
    },
    DesktopInput {
        desktop: String,
        event: rc_desktop::Input,
    },
    DesktopClose {
        desktop: String,
    },
    DesktopClosed {
        desktop: String,
        reason: String,
    },
    Ping,
    Pong,
}
fn encode<T: Serialize>(ty: u8, message: &T, data: Vec<u8>) -> Result<Vec<u8>> {
    let bytes = Frame::new(ty, message, data)?.encode();
    ensure!(bytes.len() <= MAX_PACKET, "协议帧过大");
    Ok(bytes)
}
fn decode<T: for<'de> Deserialize<'de>>(ty: u8, bytes: &[u8]) -> Result<(T, Vec<u8>)> {
    ensure!(bytes.len() <= MAX_PACKET, "协议帧过大");
    let frame = Frame::decode(bytes)?;
    ensure!(frame.ty == ty, "协议版本不兼容：请升级到证书认证版本");
    Ok((frame.json_as()?, frame.bin))
}
pub async fn read_access<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(Access, Vec<u8>)> {
    let length = reader.read_u32().await? as usize;
    ensure!((5..=MAX_PACKET).contains(&length), "加密帧长度无效");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    decode(ACCESS_TYPE, &bytes)
}
pub async fn write_access<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Access,
    data: Vec<u8>,
) -> Result<()> {
    let bytes = encode(ACCESS_TYPE, message, data)?;
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
/// Bounded adapter from relayed chunks to the ordered byte stream expected by TLS.
/// Dropping either side closes the tunnel and cancels its other half.
pub fn tunnel_stream(
    id: String,
    out: mpsc::Sender<Vec<u8>>,
    mut incoming: mpsc::Receiver<Vec<u8>>,
) -> DuplexStream {
    let (tls, transport) = tokio::io::duplex(MAX_PACKET);
    tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(transport);
        let send = async {
            let mut bytes = vec![0; CHUNK];
            loop {
                let count = reader.read(&mut bytes).await?;
                if count == 0 {
                    break;
                }
                out.send(Relay::Data { tunnel: id.clone() }.packet(bytes[..count].to_vec())?)
                    .await
                    .context("中继已关闭")?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let receive = async {
            while let Some(bytes) = incoming.recv().await {
                writer.write_all(&bytes).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! { _ = send => {}, _ = receive => {} }
        // Never block cleanup behind a full queue.
        if let Ok(bytes) = (Relay::Close { tunnel: id }).packet(vec![]) {
            let _ = out.try_send(bytes);
        }
    });
    tls
}
pub fn endpoint(server: &str, role: &str) -> Result<String> {
    let mut url = url::Url::parse(server)?;
    ensure!(
        matches!(url.scheme(), "ws" | "wss")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "中继地址需要是 ws:// 或 wss://，不能包含密码、查询参数或片段"
    );
    let base = url
        .path()
        .trim_end_matches('/')
        .trim_end_matches("/client/ws")
        .trim_end_matches("/agent/ws");
    let path = format!("{base}/{role}/ws");
    url.set_path(&path);
    Ok(url.into())
}
