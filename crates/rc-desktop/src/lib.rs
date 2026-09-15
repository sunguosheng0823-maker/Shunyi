//! Shunyi's desktop envelope. The optional RustDesk engine supplies the media payload.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

pub const MAX_ENCODED: usize = 8 * 1024 * 1024;
pub const MAX_PIXELS: usize = 16 * 1024 * 1024;
pub const MAX_LOCAL: usize = MAX_PIXELS * 4 + 8192;
pub const FRAGMENT: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    #[default]
    Denied,
    View,
    Control,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub scale: f32,
    pub cursor_embedded: bool,
}
impl DisplayInfo {
    pub fn validate(&self) -> Result<()> {
        validate_size(self.width, self.height)?;
        ensure!(
            self.scale.is_finite() && self.scale > 0.0 && self.scale <= 8.0,
            "无效屏幕缩放"
        );
        Ok(())
    }
}
pub fn validate_size(width: u32, height: u32) -> Result<()> {
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && (width as usize) * (height as usize) <= MAX_PIXELS,
        "屏幕分辨率超出预览版支持范围"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Pointer { x: f32, y: f32 },
    Button { button: Button, down: bool },
    Scroll { x: i32, y: i32 },
    Key { key: String, down: bool },
    Text { text: String },
    ReleaseAll,
}
impl Input {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Pointer { x, y } => ensure!(
                x.is_finite()
                    && y.is_finite()
                    && (0.0..=1.0).contains(x)
                    && (0.0..=1.0).contains(y),
                "无效鼠标坐标"
            ),
            Self::Scroll { x, y } => ensure!(
                x.unsigned_abs() <= 100 && y.unsigned_abs() <= 100,
                "滚动超出范围"
            ),
            Self::Key { key, .. } => ensure!(!key.is_empty() && key.len() <= 32, "无效按键"),
            Self::Text { text } => ensure!(
                text.len() <= 4096 && !text.contains('\0'),
                "输入文本过长或无效"
            ),
            _ => (),
        }
        Ok(())
    }
}

/// Local helper protocol, never interpreted as shell commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Header {
    Start {
        display: u32,
        control: bool,
    },
    Ready {
        display: DisplayInfo,
    },
    Frame {
        sequence: u64,
        width: u32,
        height: u32,
    },
    Decode {
        sequence: u64,
        width: u32,
        height: u32,
    },
    Image {
        sequence: u64,
        width: u32,
        height: u32,
    },
    Input {
        event: Input,
    },
    Stop,
    Error {
        message: String,
    },
}
pub type Packet = (Header, Vec<u8>);

pub async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Packet> {
    let len = reader.read_u32().await? as usize;
    ensure!((4..=MAX_LOCAL).contains(&len), "引擎消息长度无效");
    let json_len = reader.read_u32().await? as usize;
    ensure!(
        json_len <= 8192 && json_len <= len - 4,
        "引擎消息头长度无效"
    );
    let mut json = vec![0; json_len];
    reader.read_exact(&mut json).await?;
    let header: Header = serde_json::from_slice(&json)?;
    let data_len = len - 4 - json_len;
    match &header {
        Header::Frame { width, height, .. } | Header::Decode { width, height, .. } => {
            validate_size(*width, *height)?;
            ensure!(data_len > 0 && data_len <= MAX_ENCODED, "编码画面长度无效");
        }
        Header::Image { width, height, .. } => {
            validate_size(*width, *height)?;
            ensure!(
                data_len == *width as usize * *height as usize * 4,
                "解码画面长度无效"
            );
        }
        _ => ensure!(data_len == 0, "控制消息不接受二进制数据"),
    }
    let mut data = vec![0; data_len];
    reader.read_exact(&mut data).await?;
    Ok((header, data))
}
pub async fn write_packet<W: AsyncWrite + Unpin>(
    writer: &mut W,
    (header, data): &Packet,
) -> Result<()> {
    let json = serde_json::to_vec(header)?;
    let len = 4 + json.len() + data.len();
    ensure!(json.len() <= 8192 && len <= MAX_LOCAL, "引擎消息过大");
    writer.write_u32(len as u32).await?;
    writer.write_u32(json.len() as u32).await?;
    writer.write_all(&json).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

/// One in-flight frame per desktop; contiguous fragments avoid allocating from peer offsets.
#[derive(Default)]
pub struct Assembler {
    current: Option<(u64, usize, u32, u32, Vec<u8>)>,
}
impl Assembler {
    pub fn push(
        &mut self,
        sequence: u64,
        offset: usize,
        total: usize,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        validate_size(width, height)?;
        ensure!(
            total > 0 && total <= MAX_ENCODED && !data.is_empty() && data.len() <= FRAGMENT,
            "画面分片长度无效"
        );
        if offset == 0 {
            ensure!(self.current.is_none(), "上一帧尚未完成");
            self.current = Some((sequence, total, width, height, Vec::with_capacity(total)));
        }
        let (seq, expected, w, h, bytes) = self.current.as_mut().context("缺少画面首分片")?;
        ensure!(
            *seq == sequence
                && *expected == total
                && *w == width
                && *h == height
                && bytes.len() == offset,
            "画面分片次序无效"
        );
        ensure!(data.len() <= total - bytes.len(), "画面分片越界");
        bytes.extend_from_slice(data);
        if bytes.len() == total {
            Ok(self.current.take().map(|v| v.4))
        } else {
            Ok(None)
        }
    }
}

pub struct Engine {
    pub commands: mpsc::Sender<Packet>,
    stop: Option<oneshot::Sender<()>>,
}
impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
impl Engine {
    pub fn spawn(path: &Path, mode: &str) -> Result<(Self, mpsc::Receiver<Packet>)> {
        ensure!(path.is_absolute() && path.is_file(), "远控引擎尚未安装");
        ensure!(matches!(mode, "capture" | "decode"), "无效引擎模式");
        let mut child = tokio::process::Command::new(path)
            .arg(mode)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("无法启动远控引擎")?;
        let mut stdin = child.stdin.take().context("引擎输入管道不可用")?;
        let mut stdout = child.stdout.take().context("引擎输出管道不可用")?;
        let (commands, mut input) = mpsc::channel::<Packet>(32);
        let (events, output) = mpsc::channel::<Packet>(2);
        let (stop, stopped) = oneshot::channel();
        tokio::spawn(async move {
            let receiving = async {
                loop {
                    let packet = read_packet(&mut stdout).await?;
                    events.send(packet).await.context("画面接收已结束")?;
                }
                #[allow(unreachable_code)]
                Ok::<(), anyhow::Error>(())
            };
            let sending = async {
                while let Some(packet) = input.recv().await {
                    write_packet(&mut stdin, &packet).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            let result = tokio::select! { result = receiving => result, result = sending => result, _ = stopped => Ok(()) };
            if let Err(error) = result {
                let _ = events.try_send((
                    Header::Error {
                        message: format!("远控引擎已停止：{error}"),
                    },
                    vec![],
                ));
            }
            // EOF gives the helper a chance to release every key/button before we reap it.
            drop(stdin);
            if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        });
        Ok((
            Self {
                commands,
                stop: Some(stop),
            },
            output,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_fragment_gaps_overflow_and_interleaving() {
        let mut a = Assembler::default();
        assert!(a.push(1, 1, 2, 2, 2, &[1]).is_err());
        assert!(a.push(1, 0, MAX_ENCODED + 1, 2, 2, &[1]).is_err());
        assert!(a.push(1, 0, 3, 2, 2, &[1, 2]).unwrap().is_none());
        assert!(a.push(2, 0, 3, 2, 2, &[1]).is_err());
        assert!(a.push(1, 2, 3, 2, 2, &[3, 4]).is_err());
        assert_eq!(a.push(1, 2, 3, 2, 2, &[3]).unwrap(), Some(vec![1, 2, 3]));
    }
    #[tokio::test]
    async fn rejects_untrusted_allocation_and_image_dimensions() {
        let bytes = (MAX_LOCAL as u32 + 1).to_be_bytes();
        assert!(read_packet(&mut bytes.as_slice()).await.is_err());
        let mut output = vec![];
        write_packet(
            &mut output,
            &(
                Header::Image {
                    sequence: 0,
                    width: 100,
                    height: 100,
                },
                vec![0; 4],
            ),
        )
        .await
        .unwrap();
        assert!(read_packet(&mut output.as_slice()).await.is_err());
        assert!(Input::Pointer {
            x: f32::NAN,
            y: 0.0
        }
        .validate()
        .is_err());
        assert!(Input::Scroll { x: i32::MIN, y: 0 }.validate().is_err());
    }
}
