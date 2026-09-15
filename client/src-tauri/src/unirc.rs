//! Native adapter for the shared TLS access client. Pool by device and credential identity.
use base64::Engine;
use parking_lot::Mutex;
use rc_access::{
    client::{AccessClient, ConnectOptions, Credential, TerminalEvent},
    credentials::{self, AccessCertificate},
};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use tauri::Emitter;

#[derive(Clone)]
struct Terminal {
    remote: String,
    client: Arc<AccessClient>,
}
#[derive(Default)]
pub struct UnircConn {
    pub(crate) connecting: tokio::sync::Mutex<()>,
    pub(crate) devices: Mutex<HashMap<String, Weak<AccessClient>>>,
    terminals: Mutex<HashMap<String, Terminal>>,
}
impl UnircConn {
    pub async fn write_session(&self, session: &str, data: &[u8]) -> Result<(), String> {
        let terminal = self
            .terminals
            .lock()
            .get(session)
            .cloned()
            .ok_or("终端已关闭")?;
        terminal
            .client
            .write(&terminal.remote, data)
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn resize_session(&self, session: &str, cols: u16, rows: u16) -> Result<(), String> {
        let terminal = self
            .terminals
            .lock()
            .get(session)
            .cloned()
            .ok_or("终端已关闭")?;
        terminal
            .client
            .resize(&terminal.remote, cols, rows)
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn close_session(&self, session: &str) -> Result<(), String> {
        let terminal = self.terminals.lock().remove(session);
        if let Some(terminal) = terminal {
            terminal
                .client
                .close_terminal(&terminal.remote)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub server: String,
    pub token: String,
    pub device_id: String,
    pub auth_type: String,
    pub certificate_path: String,
    pub temporary_password: String,
    pub cols: u16,
    pub rows: u16,
}
pub async fn connect_and_open_session(
    app: tauri::AppHandle,
    state: &crate::AppState,
    request: ConnectRequest,
) -> Result<String, String> {
    let conn = state.unirc.clone();
    // Serialize creation so two simultaneous tabs cannot consume the same temporary password twice.
    let _connecting = conn.connecting.lock().await;
    let (credential, credential_id) = match request.auth_type.as_str() {
        "certificate" => {
            let bundle = AccessCertificate::read(std::path::Path::new(&request.certificate_path))
                .map_err(|e| format!("无法读取连接证书：{e}"))?;
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
    let key = format!("{}|{}|{}", request.server, request.device_id, credential_id);
    let existing = conn
        .devices
        .lock()
        .get(&key)
        .and_then(Weak::upgrade)
        .filter(|client| !client.is_closed());
    let client = match existing {
        Some(client) => client,
        None => {
            let client = AccessClient::connect(ConnectOptions {
                server: request.server,
                token: request.token,
                device_id: request.device_id,
                credential,
            })
            .await
            .map_err(|e| e.to_string())?;
            conn.devices
                .lock()
                .retain(|_, weak| weak.strong_count() > 0);
            conn.devices.lock().insert(key, Arc::downgrade(&client));
            client
        }
    };
    let (remote, mut events) = client
        .open_terminal(request.cols, request.rows)
        .await
        .map_err(|e| e.to_string())?;
    let session = state.next_session("unirc");
    conn.terminals
        .lock()
        .insert(session.clone(), Terminal { remote, client });
    let id = session.clone();
    let manager = conn.clone();
    tokio::spawn(async move {
        let mut closed = false;
        while let Some(event) = events.recv().await {
            match event {
                TerminalEvent::Data(bytes) => {
                    let _ = app.emit("term:data", serde_json::json!({"session":id,"b64":base64::engine::general_purpose::STANDARD.encode(bytes)}));
                }
                TerminalEvent::Closed(reason) => {
                    closed = true;
                    let _ = app.emit(
                        "term:closed",
                        serde_json::json!({"session":id,"reason":reason}),
                    );
                    break;
                }
            }
        }
        manager.terminals.lock().remove(&id);
        if !closed {
            let _ = app.emit(
                "term:closed",
                serde_json::json!({"session":id,"reason":"设备连接已断开，临时密码需重新生成"}),
            );
        }
    });
    Ok(session)
}
#[derive(serde::Serialize)]
pub struct CertificateInfo {
    path: String,
    device_id: String,
    fingerprint: String,
}
#[tauri::command]
pub async fn inspect_access_certificate(path: String) -> Result<CertificateInfo, String> {
    let bundle = AccessCertificate::read(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    Ok(CertificateInfo {
        path,
        device_id: bundle.device_id,
        fingerprint: credentials::digest(bundle.certificate_pem.as_bytes()),
    })
}
#[tauri::command]
pub async fn pick_access_certificate(
    app: tauri::AppHandle,
) -> Result<Option<CertificateInfo>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择瞬移连接证书")
        .add_filter("瞬移连接证书", &["shunyi-cert"])
        .pick_file(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    let path = rx.await.map_err(|e| e.to_string())?;
    match path {
        Some(path) => inspect_access_certificate(path).await.map(Some),
        None => Ok(None),
    }
}
