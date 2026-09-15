use crate::{
    credentials::{self, AccessCertificate},
    wire::{self, Access, Relay},
};
use anyhow::{bail, ensure, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

pub enum Credential {
    Certificate(AccessCertificate),
    Temporary(String),
}
pub struct ConnectOptions {
    pub server: String,
    pub token: String,
    pub device_id: String,
    pub credential: Credential,
}
#[derive(Debug)]
pub enum TerminalEvent {
    Data(Vec<u8>),
    Closed(String),
}

#[derive(Debug)]
pub enum DesktopEvent {
    Opened {
        display: rc_desktop::DisplayInfo,
        control: bool,
    },
    Frame {
        sequence: u64,
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    Closed(String),
}
type OpenReply = oneshot::Sender<std::result::Result<(), String>>;
struct Inner {
    out: mpsc::Sender<(Access, Vec<u8>)>,
    pending: Mutex<HashMap<String, OpenReply>>,
    terminals: Mutex<HashMap<String, mpsc::Sender<TerminalEvent>>>,
    desktops: Mutex<HashMap<String, mpsc::Sender<DesktopEvent>>>,
    version: u8,
    closed: AtomicBool,
}
pub struct AccessClient {
    inner: Arc<Inner>,
    task: JoinHandle<()>,
    pub remote_user: String,
}
impl Drop for AccessClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl AccessClient {
    pub async fn connect(options: ConnectOptions) -> Result<Arc<Self>> {
        timeout(
            Duration::from_secs(20),
            Self::connect_inner(options, wire::VERSION),
        )
        .await
        .context("连接超时，请检查设备是否在线")?
    }
    pub async fn connect_desktop(options: ConnectOptions) -> Result<Arc<Self>> {
        timeout(
            Duration::from_secs(20),
            Self::connect_inner(options, wire::DESKTOP_VERSION),
        )
        .await
        .context("远程桌面连接超时")?
    }
    async fn connect_inner(options: ConnectOptions, version: u8) -> Result<Arc<Self>> {
        ensure!(
            options.device_id.len() == 67,
            "请输入新版本证书对应的完整设备 ID"
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(wire::MAX_PACKET))
            .max_frame_size(Some(wire::MAX_PACKET));
        let (mut socket, _) = connect_async_with_config(
            wire::endpoint(&options.server, "client")?,
            Some(config),
            false,
        )
        .await?;
        socket
            .send(Message::Binary(
                Relay::Hello {
                    version,
                    token: options.token,
                }
                .packet(vec![])?
                .into(),
            ))
            .await?;
        match next_relay(&mut socket).await?.0 {
            Relay::Welcome => {}
            Relay::Error { message, .. } => bail!(message),
            _ => bail!("中继握手响应无效"),
        }
        socket
            .send(Message::Binary(
                Relay::Open {
                    device_id: options.device_id.clone(),
                }
                .packet(vec![])?
                .into(),
            ))
            .await?;
        let (tunnel, ca) = match next_relay(&mut socket).await?.0 {
            Relay::Ready {
                tunnel,
                device_id,
                ca_pem,
            } if device_id == options.device_id => (tunnel, ca_pem),
            Relay::Error { message, .. } => bail!(message),
            _ => bail!("中继设备响应无效"),
        };
        credentials::validate_identity(&options.device_id, &ca)?;
        let (tls_config, method, password) = match options.credential {
            Credential::Certificate(bundle) => {
                ensure!(
                    bundle.device_id == options.device_id && bundle.ca_pem == ca,
                    "连接证书不属于此设备"
                );
                (bundle.client_config()?, "certificate", String::new())
            }
            Credential::Temporary(password) => (
                credentials::temporary_client_config(&options.device_id, &ca)?,
                "temporary",
                password,
            ),
        };
        let (relay_out, mut relay_rx) = mpsc::channel::<Vec<u8>>(wire::QUEUE);
        let (incoming, incoming_rx) = mpsc::channel(wire::QUEUE);
        let stream = wire::tunnel_stream(tunnel.clone(), relay_out, incoming_rx);
        let (mut ws_write, mut ws_read) = socket.split();
        tokio::spawn(async move {
            let writer = async {
                while let Some(bytes) = relay_rx.recv().await {
                    ws_write.send(Message::Binary(bytes.into())).await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            let reader = async {
                while let Some(message) = ws_read.next().await {
                    if let Message::Binary(bytes) = message? {
                        match Relay::decode(&bytes)? {
                            (Relay::Data { tunnel: id }, bytes) if id == tunnel => {
                                incoming.send(bytes).await?;
                            }
                            (Relay::Close { tunnel: id }, _) if id == tunnel => break,
                            (Relay::Error { .. }, _) => break,
                            (Relay::Ping, _) => {}
                            _ => bail!("中继返回了不属于此连接的数据"),
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { _ = reader => {}, _ = writer => {} }
        });
        let name = rustls::pki_types::ServerName::try_from(credentials::SERVER_NAME)?;
        let mut tls = tokio_rustls::TlsConnector::from(tls_config)
            .connect(name, stream)
            .await
            .context("设备 TLS 身份验证失败")?;
        wire::write_access(
            &mut tls,
            &Access::Authenticate {
                method: method.into(),
                password,
            },
            vec![],
        )
        .await?;
        let remote_user = match wire::read_access(&mut tls).await?.0 {
            Access::Authenticated { device_id, user } if device_id == options.device_id => user,
            Access::Error { message, .. } => bail!(message),
            _ => bail!("设备认证响应无效"),
        };
        let (out, mut output) = mpsc::channel::<(Access, Vec<u8>)>(wire::QUEUE);
        let inner = Arc::new(Inner {
            out,
            pending: Mutex::default(),
            terminals: Mutex::default(),
            desktops: Mutex::default(),
            version,
            closed: AtomicBool::new(false),
        });
        let state = inner.clone();
        let task = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(tls);
            let sending = async {
                let mut ping = tokio::time::interval(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        packet = output.recv() => match packet { Some((message, bytes)) => wire::write_access(&mut writer, &message, bytes).await?, None => break },
                        _ = ping.tick() => wire::write_access(&mut writer, &Access::Ping, vec![]).await?,
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            let receiving = async {
                let mut frames: HashMap<String, rc_desktop::Assembler> = HashMap::new();
                loop {
                    let (message, bytes) =
                        timeout(Duration::from_secs(45), wire::read_access(&mut reader)).await??;
                    match message {
                        Access::DesktopOpened {
                            desktop,
                            display,
                            control,
                        } => {
                            display.validate()?;
                            let target = state.desktops.lock().unwrap().get(&desktop).cloned();
                            if let Some(target) = target {
                                target
                                    .send(DesktopEvent::Opened { display, control })
                                    .await
                                    .context("桌面已关闭")?;
                            }
                        }
                        Access::DesktopFrame {
                            desktop,
                            sequence,
                            offset,
                            total,
                            width,
                            height,
                        } => {
                            let target = state.desktops.lock().unwrap().get(&desktop).cloned();
                            if let Some(target) = target {
                                if let Some(data) = frames
                                    .entry(desktop)
                                    .or_default()
                                    .push(sequence, offset, total, width, height, &bytes)?
                                {
                                    target
                                        .send(DesktopEvent::Frame {
                                            sequence,
                                            width,
                                            height,
                                            data,
                                        })
                                        .await
                                        .context("桌面已关闭")?;
                                }
                            }
                        }
                        Access::DesktopClosed { desktop, reason } => {
                            frames.remove(&desktop);
                            let target = state.desktops.lock().unwrap().remove(&desktop);
                            if let Some(target) = target {
                                let _ = target.send(DesktopEvent::Closed(reason)).await;
                            }
                        }
                        Access::Opened { terminal } => {
                            if let Some(reply) = state.pending.lock().unwrap().remove(&terminal) {
                                let _ = reply.send(Ok(()));
                            }
                        }
                        Access::Data { terminal } => {
                            let target = state.terminals.lock().unwrap().get(&terminal).cloned();
                            if let Some(target) = target {
                                target
                                    .send(TerminalEvent::Data(bytes))
                                    .await
                                    .context("终端接收器已关闭")?;
                            }
                        }
                        Access::Closed { terminal, reason } => {
                            if let Some(reply) = state.pending.lock().unwrap().remove(&terminal) {
                                let _ = reply.send(Err(reason.clone()));
                            }
                            let target = state.terminals.lock().unwrap().remove(&terminal);
                            if let Some(target) = target {
                                let _ = target.send(TerminalEvent::Closed(reason)).await;
                            }
                        }
                        Access::Error {
                            terminal: Some(terminal),
                            message,
                        } => {
                            if let Some(reply) = state.pending.lock().unwrap().remove(&terminal) {
                                let _ = reply.send(Err(message.clone()));
                            }
                            let target = state.terminals.lock().unwrap().remove(&terminal);
                            if let Some(target) = target {
                                let _ = target.send(TerminalEvent::Closed(message)).await;
                            }
                        }
                        Access::Pong => {}
                        Access::Error { message, .. } => bail!(message),
                        _ => bail!("设备返回了不支持的终端消息"),
                    }
                }
            };
            let reason =
                tokio::select! { result = sending => result, result = receiving => result }
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "连接已关闭".into());
            state.closed.store(true, Ordering::SeqCst);
            let pending = std::mem::take(&mut *state.pending.lock().unwrap());
            for (_, reply) in pending {
                let _ = reply.send(Err(reason.clone()));
            }
            let terminals = std::mem::take(&mut *state.terminals.lock().unwrap());
            for (_, target) in terminals {
                let _ = target.try_send(TerminalEvent::Closed(reason.clone()));
            }
            let desktops = std::mem::take(&mut *state.desktops.lock().unwrap());
            for (_, target) in desktops {
                let _ = target.try_send(DesktopEvent::Closed(reason.clone()));
            }
        });
        Ok(Arc::new(Self {
            inner,
            task,
            remote_user,
        }))
    }
    pub fn supports_desktop(&self) -> bool {
        self.inner.version >= wire::DESKTOP_VERSION
    }
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst) || self.task.is_finished()
    }
    pub async fn open_terminal(
        &self,
        cols: u16,
        rows: u16,
    ) -> Result<(String, mpsc::Receiver<TerminalEvent>)> {
        ensure!(!self.is_closed(), "设备连接已断开");
        let id = uuid::Uuid::new_v4().to_string();
        let (reply, response) = oneshot::channel();
        let (events, receiver) = mpsc::channel(wire::QUEUE);
        self.inner.pending.lock().unwrap().insert(id.clone(), reply);
        self.inner
            .terminals
            .lock()
            .unwrap()
            .insert(id.clone(), events);
        let result = async {
            self.send(
                Access::Open {
                    terminal: id.clone(),
                    cols,
                    rows,
                },
                vec![],
            )
            .await?;
            timeout(Duration::from_secs(15), response)
                .await
                .context("终端启动超时")?
                .context("设备已断开")?
                .map_err(anyhow::Error::msg)
        }
        .await;
        if let Err(error) = result {
            self.inner.pending.lock().unwrap().remove(&id);
            self.inner.terminals.lock().unwrap().remove(&id);
            let _ = self.close_terminal(&id).await;
            return Err(error);
        }
        Ok((id, receiver))
    }
    pub async fn open_desktop(
        &self,
        display: u32,
        control: bool,
    ) -> Result<(String, mpsc::Receiver<DesktopEvent>)> {
        ensure!(
            self.inner.version >= wire::DESKTOP_VERSION && !self.is_closed(),
            "请建立远程桌面连接"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let (events, receiver) = mpsc::channel(2);
        {
            let mut desktops = self.inner.desktops.lock().unwrap();
            ensure!(desktops.is_empty(), "每个设备通道只允许一个桌面");
            desktops.insert(id.clone(), events);
        }
        if let Err(error) = self
            .send(
                Access::DesktopOpen {
                    desktop: id.clone(),
                    display,
                    control,
                },
                vec![],
            )
            .await
        {
            self.inner.desktops.lock().unwrap().remove(&id);
            return Err(error);
        }
        Ok((id, receiver))
    }
    pub async fn desktop_input(&self, desktop: &str, event: rc_desktop::Input) -> Result<()> {
        event.validate()?;
        self.send(
            Access::DesktopInput {
                desktop: desktop.into(),
                event,
            },
            vec![],
        )
        .await
    }
    pub async fn close_desktop(&self, desktop: &str) -> Result<()> {
        self.inner.desktops.lock().unwrap().remove(desktop);
        self.send(
            Access::DesktopClose {
                desktop: desktop.into(),
            },
            vec![],
        )
        .await
    }
    async fn send(&self, message: Access, data: Vec<u8>) -> Result<()> {
        ensure!(!self.is_closed(), "连接已断开");
        self.inner
            .out
            .send((message, data))
            .await
            .context("连接已断开")
    }
    pub async fn write(&self, terminal: &str, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(wire::CHUNK) {
            self.send(
                Access::Data {
                    terminal: terminal.into(),
                },
                chunk.to_vec(),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn resize(&self, terminal: &str, cols: u16, rows: u16) -> Result<()> {
        self.send(
            Access::Resize {
                terminal: terminal.into(),
                cols,
                rows,
            },
            vec![],
        )
        .await
    }
    pub async fn close_terminal(&self, terminal: &str) -> Result<()> {
        self.send(
            Access::Close {
                terminal: terminal.into(),
            },
            vec![],
        )
        .await
    }
}
async fn next_relay<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<(Relay, Vec<u8>)> {
    loop {
        match socket.next().await.context("中继已断开")?? {
            Message::Binary(bytes) => return Relay::decode(&bytes),
            Message::Ping(_) | Message::Pong(_) => {}
            _ => bail!("中继已关闭连接"),
        }
    }
}
