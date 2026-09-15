use parking_lot::Mutex;
use rc_access::{
    client::{AccessClient, ConnectOptions, Credential, DesktopEvent},
    credentials::{self, AccessCertificate},
};
use rc_desktop::{Engine, Header, Input};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tauri::ipc::{Channel, Response};

#[derive(Default)]
pub struct Desktops {
    sessions: Mutex<HashMap<String, Session>>,
}
struct Session {
    client: Arc<AccessClient>,
    remote: String,
    control: bool,
    ack: tokio::sync::mpsc::Sender<u64>,
    _engine: Engine,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn engine_path() -> Result<PathBuf, String> {
    // Development override is process-local. Packaged builds only use their bundled helper.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("SHUNYI_DESKTOP_ENGINE") {
        let path = PathBuf::from(path);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
    }
    let path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(if cfg!(windows) { "shunyi-desktop-engine.exe" } else { "shunyi-desktop-engine" });
    if path.is_file() {
        Ok(path)
    } else {
        Err("当前安装包未包含远控引擎，请使用远控预览版".into())
    }
}

#[tauri::command]
pub async fn desktop_permissions(kind: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let pane = match kind.as_str() { "screen" => "Privacy_ScreenCapture", "input" => "Privacy_Accessibility", _ => return Err("无效权限类型".into()) };
        if kind == "screen" {
            let _ = tokio::process::Command::new(engine_path()?).arg("permissions")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
        }
        // Opening this pane cannot grant OS permission. The user chooses in System Settings.
        tokio::process::Command::new("/usr/bin/open").arg(format!("x-apple.systempreferences:com.apple.preference.security?{pane}"))
            .status().await.map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = kind; Err("此权限设置仅适用于 macOS".into()) }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    server: String,
    token: String,
    device_id: String,
    auth_type: String,
    certificate_path: String,
    temporary_password: String,
    control: bool,
    #[serde(default)]
    display: u32,
}

#[tauri::command]
pub async fn desktop_connect(
    state: tauri::State<'_, crate::AppState>,
    request: Request,
    frames: Channel<Response>,
    status: Channel<serde_json::Value>,
) -> Result<String, String> {
    let path = engine_path()?;
    let conn = state.unirc.clone();
    let _serial = conn.connecting.lock().await;
    let (credential, fingerprint) = match request.auth_type.as_str() {
        "certificate" => {
            let bundle = AccessCertificate::read(std::path::Path::new(&request.certificate_path))
                .map_err(|e| e.to_string())?;
            if bundle.device_id != request.device_id {
                return Err("所选证书不属于此设备".into());
            }
            let fingerprint = credentials::digest(bundle.certificate_pem.as_bytes());
            (Credential::Certificate(bundle), fingerprint)
        }
        "temporary" => (
            Credential::Temporary(request.temporary_password),
            "temporary".into(),
        ),
        _ => return Err("请选择连接证书或临时密码".into()),
    };
    let key = format!("{}|{}|{}", request.server, request.device_id, fingerprint);
    let existing = conn
        .devices
        .lock()
        .get(&key)
        .and_then(std::sync::Weak::upgrade)
        .filter(|c| c.supports_desktop() && !c.is_closed());
    let client = match existing {
        Some(client) => client,
        None => {
            let client = AccessClient::connect_desktop(ConnectOptions {
                server: request.server,
                token: request.token,
                device_id: request.device_id,
                credential,
            })
            .await
            .map_err(|e| e.to_string())?;
            conn.devices.lock().insert(key, Arc::downgrade(&client));
            client
        }
    };
    let (remote, mut events) = client
        .open_desktop(request.display, request.control)
        .await
        .map_err(|e| e.to_string())?;
    let first = tokio::time::timeout(Duration::from_secs(16), events.recv()).await;
    let display = match first {
        Ok(Some(DesktopEvent::Opened { display, control })) if control == request.control => {
            display
        }
        other => {
            let _ = client.close_desktop(&remote).await;
            return Err(match other {
                Ok(Some(DesktopEvent::Closed(reason))) => reason,
                _ => "桌面启动超时或设备未返回有效权限".into(),
            });
        }
    };
    let (engine, mut decoded) = match Engine::spawn(&path, "decode") {
        Ok(v) => v,
        Err(error) => {
            let _ = client.close_desktop(&remote).await;
            return Err(error.to_string());
        }
    };
    let session = state.next_session("desktop");
    let id = session.clone();
    let desktops = state.desktops.clone();
    let remote_id = remote.clone();
    let access = client.clone();
    let commands = engine.commands.clone();
    let (ack, mut acks) = tokio::sync::mpsc::channel(1);
    let (start, started) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _ = started.await;
        let _ = status.send(
            serde_json::json!({"phase":"connected","display":display,"control":request.control}),
        );
        let result: anyhow::Result<()> = async {
            while let Some(event) = events.recv().await {
                match event {
                    DesktopEvent::Frame {
                        sequence,
                        width,
                        height,
                        data,
                    } => {
                        commands
                            .send((
                                Header::Decode {
                                    sequence,
                                    width,
                                    height,
                                },
                                data,
                            ))
                            .await?;
                        let output = tokio::time::timeout(Duration::from_secs(10), decoded.recv())
                            .await?
                            .ok_or_else(|| anyhow::anyhow!("解码引擎已退出"))?;
                        match output {
                            (
                                Header::Image {
                                    sequence,
                                    width,
                                    height,
                                },
                                pixels,
                            ) => {
                                let mut data = Vec::with_capacity(16 + pixels.len());
                                data.extend_from_slice(&sequence.to_be_bytes());
                                data.extend_from_slice(&width.to_be_bytes());
                                data.extend_from_slice(&height.to_be_bytes());
                                data.extend_from_slice(&pixels);
                                frames.send(Response::new(data))?;
                                let received =
                                    tokio::time::timeout(Duration::from_secs(10), acks.recv())
                                        .await?;
                                anyhow::ensure!(
                                    received == Some(sequence),
                                    "画面确认超时或次序无效"
                                );
                            }
                            (Header::Error { message }, _) => anyhow::bail!(message),
                            _ => anyhow::bail!("解码引擎返回了无效画面"),
                        }
                    }
                    DesktopEvent::Closed(reason) => anyhow::bail!(reason),
                    DesktopEvent::Opened { .. } => anyhow::bail!("重复的桌面启动消息"),
                }
            }
            anyhow::bail!("设备连接已断开")
        }
        .await;
        let _ = access.close_desktop(&remote_id).await;
        let _ = status
            .send(serde_json::json!({"phase":"closed","reason":result.unwrap_err().to_string()}));
        desktops.sessions.lock().remove(&id);
    });
    state.desktops.sessions.lock().insert(
        session.clone(),
        Session {
            client,
            remote,
            control: request.control,
            ack,
            _engine: engine,
            task,
        },
    );
    let _ = start.send(());
    Ok(session)
}

#[tauri::command]
pub async fn desktop_input(
    state: tauri::State<'_, crate::AppState>,
    session: String,
    event: Input,
) -> Result<(), String> {
    let (client, remote) = {
        let sessions = state.desktops.sessions.lock();
        let session = sessions.get(&session).ok_or("桌面已断开")?;
        if !session.control {
            return Err("此桌面仅允许查看".into());
        }
        (session.client.clone(), session.remote.clone())
    };
    client
        .desktop_input(&remote, event)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn desktop_ack(
    state: tauri::State<'_, crate::AppState>,
    session: String,
    sequence: u64,
) -> Result<(), String> {
    let sessions = state.desktops.sessions.lock();
    let session = sessions.get(&session).ok_or("桌面已断开")?;
    session
        .ack
        .try_send(sequence)
        .map_err(|_| "画面确认队列繁忙".into())
}
#[tauri::command]
pub async fn desktop_close(
    state: tauri::State<'_, crate::AppState>,
    session: String,
) -> Result<(), String> {
    let session = state.desktops.sessions.lock().remove(&session);
    if let Some(session) = session {
        let _ = session.client.close_desktop(&session.remote).await;
    }
    Ok(())
}
