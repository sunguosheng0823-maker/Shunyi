//! SSH 会话：russh 纯 Rust 实现（密码 / 私钥认证 + 交互式 shell）。
//!
//! 使用系统 known_hosts 验证服务器身份；未知或变化的密钥均拒绝。

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: String, // "password" | "key"
    pub password: String,
    pub key_path: String,
}

#[derive(Clone)]
pub struct SshHandle {
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    close_tx: mpsc::UnboundedSender<()>,
}

impl SshHandle {
    pub async fn write(&self, data: Vec<u8>) -> Result<(), String> {
        self.write_tx.send(data).map_err(|_| "会话已关闭".into())
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.resize_tx
            .send((cols, rows))
            .map_err(|_| "会话已关闭".into())
    }

    pub async fn close(&self) {
        let _ = self.close_tx.send(());
    }
}

struct KnownHostKey {
    host: String,
    port: u16,
}
impl russh::client::Handler for KnownHostKey {
    type Error = anyhow::Error;
    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        match russh::keys::check_known_hosts(&self.host, self.port, server_public_key) {
            Ok(true) => Ok(true),
            Ok(false) => anyhow::bail!("此 SSH 主机尚未受信任。请先在系统终端使用 ssh 连接并核对主机指纹，写入 ~/.ssh/known_hosts 后再连接。"),
            Err(error) => anyhow::bail!("SSH 主机密钥验证失败，已拒绝连接：{error}"),
        }
    }
}

pub async fn connect(
    app: tauri::AppHandle,
    state: &crate::AppState,
    target: SshTarget,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let session = state.next_session("ssh");

    let config = Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(
        config,
        (target.host.as_str(), target.port),
        KnownHostKey {
            host: target.host.clone(),
            port: target.port,
        },
    )
    .await
    .map_err(|e| {
        format!(
            "SSH 连接失败 {host}:{port}: {e}",
            host = target.host,
            port = target.port
        )
    })?;

    match target.auth_type.as_str() {
        "key" => {
            let secret = russh::keys::load_secret_key(&target.key_path, None)
                .map_err(|e| format!("读取私钥 {} 失败: {e}", target.key_path))?;
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(secret), None);
            let ok = handle
                .authenticate_publickey(&target.username, key)
                .await
                .map_err(|e| format!("密钥认证出错: {e}"))?;
            ensure_auth(ok.success(), "密钥认证被拒绝（用户名或密钥不匹配）")?;
        }
        _ => {
            let ok = handle
                .authenticate_password(&target.username, &target.password)
                .await
                .map_err(|e| format!("密码认证出错: {e}"))?;
            ensure_auth(ok.success(), "密码认证被拒绝")?;
        }
    }

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("打开 SSH 通道失败: {e}"))?;
    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(|e| format!("申请 PTY 失败: {e}"))?;
    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("请求 shell 失败: {e}"))?;

    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (resize_tx, mut resize_rx) = mpsc::unbounded_channel::<(u16, u16)>();
    let (close_tx, mut close_rx) = mpsc::unbounded_channel::<()>();

    let session_for_task = session.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        use base64::Engine;
        use russh::ChannelMsg;

        loop {
            tokio::select! {
                biased;
                _ = close_rx.recv() => break,
                msg = channel.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { data }) => {
                            use tauri::Emitter;
                            let _ = app2.emit(
                                "term:data",
                                serde_json::json!({
                                    "proto": "ssh",
                                    "session": session_for_task,
                                    "b64": base64::engine::general_purpose::STANDARD.encode(data.as_ref()),
                                }),
                            );
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            use tauri::Emitter;
                            let _ = app2.emit(
                                "term:data",
                                serde_json::json!({
                                    "proto": "ssh",
                                    "session": session_for_task,
                                    "b64": base64::engine::general_purpose::STANDARD.encode(data.as_ref()),
                                }),
                            );
                        }
                        Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                        _ => {}
                    }
                }
                Some(data) = write_rx.recv() => {
                    if let Err(e) = channel.data(&data[..]).await {
                        warn!("ssh 写入失败: {e}");
                        break;
                    }
                }
                Some((c, r)) = resize_rx.recv() => {
                    let _ = channel.window_change(c as u32, r as u32, 0, 0).await;
                }
            }
        }
        let _ = handle
            .disconnect(russh::Disconnect::ByApplication, "closed", "english")
            .await;
        use tauri::Emitter;
        let _ = app2.emit(
            "term:closed",
            serde_json::json!({"proto": "ssh", "session": session_for_task, "reason": "SSH 会话结束"}),
        );
    });

    state.ssh.lock().insert(
        session.clone(),
        SshHandle {
            write_tx,
            resize_tx,
            close_tx,
        },
    );
    Ok(session)
}

fn ensure_auth(ok: bool, msg: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(msg.into())
    }
}
