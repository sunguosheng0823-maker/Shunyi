//! Local device controls are explicit. Merely opening the app never enables remote access.
use parking_lot::Mutex;
use rc_access::credentials::{CredentialStatus, CredentialStore};
use std::{path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct LocalAccess {
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    error: Arc<Mutex<Option<String>>>,
    desktop: Mutex<rc_desktop::Permission>,
}
fn store() -> Result<CredentialStore, String> {
    let path = std::env::var_os("UNIRC_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            // Windows 没有 HOME，取 USERPROFILE（与 rc-agent 主程序保持一致）
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
            Some(PathBuf::from(home).join(if cfg!(feature = "desktop-preview") { ".shunyi-desktop-preview" } else { ".shunyi" }))
        })
        .ok_or("无法确定本机凭据目录")?;
    CredentialStore::open(path).map_err(|e| e.to_string())
}
#[derive(serde::Serialize)]
pub struct DeviceStatus {
    credentials: CredentialStatus,
    running: bool,
    error: Option<String>,
    user: String,
    desktop_permission: rc_desktop::Permission,
    desktop_available: bool,
    desktop_platform: &'static str,
}
#[tauri::command]
pub async fn device_status(
    state: tauri::State<'_, crate::AppState>,
) -> Result<DeviceStatus, String> {
    Ok(DeviceStatus {
        credentials: store()?.status().map_err(|e| e.to_string())?,
        running: state
            .local_access
            .task
            .lock()
            .as_ref()
            .is_some_and(|t| !t.is_finished()),
        error: state.local_access.error.lock().clone(),
        user: std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
            .unwrap_or_else(|_| "当前系统用户".into()),
        desktop_permission: *state.local_access.desktop.lock(),
        desktop_available: crate::desktop::engine_path().is_ok(),
        desktop_platform: std::env::consts::OS,
    })
}
#[tauri::command]
pub async fn device_start(
    state: tauri::State<'_, crate::AppState>,
    server: String,
    token: String,
    desktop_permission: Option<rc_desktop::Permission>,
) -> Result<(), String> {
    rc_access::wire::endpoint(&server, "agent").map_err(|e| e.to_string())?;
    let permission = desktop_permission.unwrap_or_default();
    let desktop = if permission == rc_desktop::Permission::Denied {
        None
    } else {
        Some(
            rc_agent::desktop::DesktopConfig::new(crate::desktop::engine_path()?, permission)
                .map_err(|e| e.to_string())?,
        )
    };
    let credentials = store()?;
    // Fail before returning success if a standalone service already owns this identity.
    drop(credentials.lock_agent().map_err(|e| e.to_string())?);
    let mut task = state.local_access.task.lock();
    if task.as_ref().is_some_and(|t| !t.is_finished()) {
        return Ok(());
    }
    *state.local_access.error.lock() = None;
    *state.local_access.desktop.lock() = permission;
    let error = state.local_access.error.clone();
    *task = Some(tokio::spawn(async move {
        if let Err(failure) = rc_agent::run(rc_agent::AgentConfig {
            desktop,
            server_url: server,
            token,
            device_name: rc_agent::default_device_name(),
            credentials,
        })
        .await
        {
            *error.lock() = Some(failure.to_string());
        }
    }));
    Ok(())
}
#[tauri::command]
pub async fn device_stop(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    if let Some(task) = state.local_access.task.lock().take() {
        task.abort();
    }
    Ok(())
}
#[tauri::command]
pub async fn device_export_certificate(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("导出本机连接证书")
        .add_filter("瞬移连接证书", &["shunyi-cert"])
        .set_file_name("device.shunyi-cert")
        .save_file(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    if let Some(path) = rx.await.map_err(|e| e.to_string())? {
        store()?
            .export_certificate(std::path::Path::new(&path))
            .map_err(|e| format!("导出失败，请选择尚不存在的新文件：{e}"))?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}
#[tauri::command]
pub async fn device_rotate_certificate() -> Result<(), String> {
    store()?.rotate_certificate().map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn device_temporary_password(minutes: u64) -> Result<String, String> {
    store()?
        .create_temporary(minutes.checked_mul(60).ok_or("有效期过长")?)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn device_revoke_temporary() -> Result<(), String> {
    store()?.revoke_temporary().map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn device_set_rotation(hours: Option<u64>) -> Result<(), String> {
    store()?.set_rotation(hours).map_err(|e| e.to_string())
}
