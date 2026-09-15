//! Accountless agent. A shell is created only inside an authenticated TLS access session.
use anyhow::{bail, ensure, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rc_access::{
    credentials::CredentialStore,
    wire::{self, Access, Relay},
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinSet, time::timeout};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};

#[derive(Clone)]
pub struct AgentConfig {
    pub server_url: String,
    pub token: String,
    pub device_name: String,
    pub credentials: CredentialStore,
}
pub fn default_device_name() -> String {
    whoami::fallible::hostname().unwrap_or_else(|_| "Shunyi device".into())
}
pub fn default_shell() -> String {
    std::env::var("UNIRC_SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "powershell.exe".into()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
        }
    })
}
pub async fn run(config: AgentConfig) -> Result<()> {
    let _instance = config.credentials.lock_agent()?;
    let mut delay = 1;
    loop {
        let began = std::time::Instant::now();
        if let Err(error) = connection(&config).await {
            tracing::warn!(%error, "Agent 连接断开");
        }
        if began.elapsed() > Duration::from_secs(30) {
            delay = 1;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}
async fn connection(config: &AgentConfig) -> Result<()> {
    config.credentials.rotate_if_due()?;
    let limits = WebSocketConfig::default()
        .max_message_size(Some(wire::MAX_PACKET))
        .max_frame_size(Some(wire::MAX_PACKET));
    let (mut socket, _) = timeout(
        Duration::from_secs(15),
        connect_async_with_config(
            wire::endpoint(&config.server_url, "agent")?,
            Some(limits),
            false,
        ),
    )
    .await??;
    let nonce = match timeout(Duration::from_secs(10), socket.next())
        .await?
        .context("中继未响应")??
    {
        Message::Binary(bytes) => match Relay::decode(&bytes)?.0 {
            Relay::Challenge { nonce } => nonce,
            _ => bail!("中继不支持设备证书"),
        },
        _ => bail!("无效的中继握手"),
    };
    ensure!(nonce.len() <= 128, "中继挑战无效");
    let (device_id, ca_pem) = config.credentials.public_identity()?;
    let signature = config
        .credentials
        .sign_registration(&nonce, &config.device_name)?;
    socket
        .send(Message::Binary(
            Relay::Register {
                version: wire::VERSION,
                device_id: device_id.clone(),
                name: config.device_name.clone(),
                ca_pem,
                signature,
                token: config.token.clone(),
            }
            .packet(vec![])?
            .into(),
        ))
        .await?;
    match timeout(Duration::from_secs(10), socket.next())
        .await?
        .context("中继未响应")??
    {
        Message::Binary(bytes) => match Relay::decode(&bytes)?.0 {
            Relay::Registered => {}
            Relay::Error { message, .. } => bail!(message),
            _ => bail!("注册响应无效"),
        },
        _ => bail!("中继已关闭"),
    }
    tracing::info!(%device_id, "设备已上线，等待证书或临时密码认证");
    let (out, mut output) = mpsc::channel::<Vec<u8>>(wire::QUEUE);
    let (mut writer, mut reader) = socket.split();
    let sending = async {
        while let Some(bytes) = output.recv().await {
            writer.send(Message::Binary(bytes.into())).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(sending);
    let mut tunnels: HashMap<String, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut accesses = JoinSet::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last_message = std::time::Instant::now();
    loop {
        tokio::select! {
            result = &mut sending => return result,
            _ = heartbeat.tick() => {
                ensure!(last_message.elapsed() < Duration::from_secs(60), "中继心跳超时");
                config.credentials.rotate_if_due()?;
                out.try_send(Relay::Ping.packet(vec![])?).context("中继输出队列繁忙")?;
                tunnels.retain(|_, tx| !tx.is_closed());
            },
            completed = accesses.join_next(), if !accesses.is_empty() => { if let Some(Ok(id)) = completed { tunnels.remove(&id); } },
            next = reader.next() => {
                let message = next.context("中继已断开")??; last_message = std::time::Instant::now();
                if let Message::Binary(bytes) = message { match Relay::decode(&bytes)? {
                    (Relay::Incoming { tunnel }, _) => {
                        tunnels.retain(|_, tx| !tx.is_closed());
                        if tunnels.len() >= 32 || tunnels.contains_key(&tunnel) { out.try_send(Relay::Close { tunnel }.packet(vec![])?).context("中继输出队列繁忙")?; continue; }
                        let (incoming, receiver) = mpsc::channel(wire::QUEUE);
                        let stream = wire::tunnel_stream(tunnel.clone(), out.clone(), receiver);
                        tunnels.insert(tunnel.clone(), incoming); let store = config.credentials.clone();
                        accesses.spawn(async move {
                            if let Err(error) = serve_access(store, stream).await { tracing::debug!(%error, "设备访问已结束"); }
                            tunnel
                        });
                    },
                    (Relay::Data { tunnel }, bytes) => { if let Some(tx) = tunnels.get(&tunnel) { if tx.try_send(bytes).is_err() { tunnels.remove(&tunnel); out.try_send(Relay::Close { tunnel }.packet(vec![])?).context("中继输出队列繁忙")?; } } },
                    (Relay::Close { tunnel }, _) => { tunnels.remove(&tunnel); },
                    (Relay::Ping, _) => {},
                    (Relay::Error { message, .. }, _) => bail!(message),
                    _ => bail!("拒绝未认证的旧版终端消息"),
                }} else if matches!(message, Message::Close(_)) { bail!("中继已关闭"); }
            },
        }
    }
}

async fn serve_access(store: CredentialStore, stream: tokio::io::DuplexStream) -> Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(store.server_config()?);
    let mut tls = timeout(Duration::from_secs(10), acceptor.accept(stream))
        .await
        .context("设备 TLS 握手超时")??;
    let (auth, _) = timeout(Duration::from_secs(10), wire::read_access(&mut tls)).await??;
    let result = match auth {
        Access::Authenticate { method, password }
            if method == "certificate" && password.is_empty() =>
        {
            match tls.get_ref().1.peer_certificates().and_then(|c| c.first()) {
                Some(cert) => store.authorize_certificate(cert.as_ref()),
                None => Err(anyhow::anyhow!("缺少连接证书")),
            }
        }
        Access::Authenticate { method, password } if method == "temporary" => {
            store.claim_temporary(&password)
        }
        _ => Err(anyhow::anyhow!("需要证书或临时密码认证")),
    };
    if let Err(error) = result {
        // Bound password-guessing throughput without exposing whether a credential exists.
        tokio::time::sleep(Duration::from_millis(250)).await;
        wire::write_access(
            &mut tls,
            &Access::Error {
                terminal: None,
                message: error.to_string(),
            },
            vec![],
        )
        .await?;
        return Err(error);
    }
    wire::write_access(
        &mut tls,
        &Access::Authenticated {
            device_id: store.public_identity()?.0,
            user: whoami::username(),
        },
        vec![],
    )
    .await?;
    let (reader, mut writer) = tokio::io::split(tls);
    let (events, mut event_rx) = mpsc::channel::<(Access, Vec<u8>)>(wire::QUEUE);
    let mut terminals: HashMap<String, Pty> = HashMap::new();
    let mut opened = false;
    let mut idle = tokio::time::interval(Duration::from_secs(5));
    let began = std::time::Instant::now();
    let mut last_input = began;
    // Keep a read future alive across terminal output; cancelling read_exact would lose framing.
    let input = async_stream_read(reader);
    tokio::pin!(input);
    loop {
        tokio::select! {
            next = input.next() => {
                let (message, bytes) = next.context("设备访问已断开")??; last_input = std::time::Instant::now();
                match message {
                    Access::Open { terminal, cols, rows } => {
                        if terminals.len() >= 16 || terminals.contains_key(&terminal) || uuid::Uuid::parse_str(&terminal).is_err() {
                            wire::write_access(&mut writer, &Access::Error { terminal: Some(terminal), message: "终端数量已达到上限或请求无效".into() }, vec![]).await?; continue;
                        }
                        match Pty::open(&terminal, cols, rows, events.clone()) {
                            Ok(pty) => { terminals.insert(terminal.clone(), pty); opened = true; wire::write_access(&mut writer, &Access::Opened { terminal }, vec![]).await?; },
                            Err(error) => wire::write_access(&mut writer, &Access::Error { terminal: Some(terminal), message: format!("终端创建失败：{error}") }, vec![]).await?,
                        }
                    },
                    Access::Data { terminal } => { if let Some(pty) = terminals.get(&terminal) { pty.write.try_send(bytes).context("终端输入繁忙，连接已关闭以避免丢失输入")?; } },
                    Access::Resize { terminal, cols, rows } => { if let Some(pty) = terminals.get(&terminal) { pty.master.lock().unwrap().resize(size(cols, rows))?; } },
                    Access::Close { terminal } => {
                        terminals.remove(&terminal);
                        wire::write_access(&mut writer, &Access::Closed { terminal, reason: "终端已关闭".into() }, vec![]).await?;
                        if opened && terminals.is_empty() { return Ok(()); }
                    },
                    Access::Ping => wire::write_access(&mut writer, &Access::Pong, vec![]).await?,
                    _ => bail!("不支持的终端请求"),
                }
            },
            event = event_rx.recv() => {
                if let Some((message, bytes)) = event {
                    let id = match &message { Access::Data { terminal } | Access::Closed { terminal, .. } => terminal, _ => continue };
                    if !terminals.contains_key(id) { continue; }
                    if matches!(message, Access::Closed { .. }) { terminals.remove(id); }
                    wire::write_access(&mut writer, &message, bytes).await?;
                    if opened && terminals.is_empty() { return Ok(()); }
                }
            },
            _ = idle.tick() => { ensure!(last_input.elapsed() < Duration::from_secs(45), "设备访问心跳超时"); if !opened { ensure!(began.elapsed() < Duration::from_secs(20), "认证后未打开终端"); } },
        }
    }
}
// unfold owns its reader, so select! never discards a partially read TLS application frame.
fn async_stream_read<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
) -> impl futures_util::Stream<Item = Result<(Access, Vec<u8>)>> {
    futures_util::stream::unfold(reader, |mut reader| async {
        let next = wire::read_access(&mut reader).await;
        Some((next, reader))
    })
}
fn size(cols: u16, rows: u16) -> portable_pty::PtySize {
    portable_pty::PtySize {
        cols: cols.clamp(1, 500),
        rows: rows.clamp(1, 300),
        pixel_width: 0,
        pixel_height: 0,
    }
}
struct Pty {
    write: std::sync::mpsc::SyncSender<Vec<u8>>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}
impl Pty {
    fn open(id: &str, cols: u16, rows: u16, out: mpsc::Sender<(Access, Vec<u8>)>) -> Result<Self> {
        let pair = portable_pty::native_pty_system().openpty(size(cols, rows))?;
        let mut command = portable_pty::CommandBuilder::new(default_shell());
        command.env("TERM", "xterm-256color");
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut child = pair.slave.spawn_command(command)?;
        let killer = child.clone_killer();
        let (write, input) = std::sync::mpsc::sync_channel::<Vec<u8>>(wire::QUEUE);
        std::thread::spawn(move || {
            for bytes in input {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let terminal = id.to_owned();
        std::thread::spawn(move || {
            let mut bytes = [0u8; wire::CHUNK];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out
                            .blocking_send((
                                Access::Data {
                                    terminal: terminal.clone(),
                                },
                                bytes[..n].to_vec(),
                            ))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            let _ = out.blocking_send((
                Access::Closed {
                    terminal,
                    reason: "Shell 已退出".into(),
                },
                vec![],
            ));
        });
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(Self {
            write,
            master: Arc::new(Mutex::new(pair.master)),
            killer,
        })
    }
}
