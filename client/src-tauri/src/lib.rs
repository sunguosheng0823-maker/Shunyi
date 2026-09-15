//! 瞬移终端客户端：本地 PTY / SSH / UniRC-T 自有协议 三种会话引擎。
//!
//! 前端（xterm.js）通过 invoke 调用命令、通过事件接收终端输出；
//! 终端数据用 base64 编码传输，避免跨 IPC 的 UTF-8 边界截断。

mod desktop;
mod device;
pub mod fs;
mod pty;
mod ssh;
mod unirc;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

pub struct AppState {
    /// 本地 PTY 会话：session key → 句柄
    pub ptys: Mutex<HashMap<String, pty::PtyHandle>>,
    /// SSH 会话
    pub ssh: Mutex<HashMap<String, ssh::SshHandle>>,
    /// UniRC 自有协议连接（单服务器，多会话复用）
    pub unirc: Arc<unirc::UnircConn>,
    pub local_access: device::LocalAccess,
    pub desktops: std::sync::Arc<desktop::Desktops>,
    next_id: AtomicU32,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            ptys: Mutex::new(HashMap::new()),
            ssh: Mutex::new(HashMap::new()),
            unirc: Arc::new(unirc::UnircConn::default()),
            local_access: device::LocalAccess::default(),
            desktops: std::sync::Arc::new(desktop::Desktops::default()),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn next_session(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            import_hosts_text,
            export_hosts_text,
            fs::fs_list,
            fs::fs_read,
            fs::home_dir,
            pty_open,
            pty_write,
            pty_resize,
            pty_close,
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_close,
            unirc_connect,
            unirc::pick_access_certificate,
            unirc::inspect_access_certificate,
            desktop::desktop_permissions,
            desktop::desktop_connect,
            desktop::desktop_input,
            desktop::desktop_ack,
            desktop::desktop_close,
            device::device_status,
            device::device_start,
            device::device_stop,
            device::device_export_certificate,
            device::device_rotate_certificate,
            device::device_temporary_password,
            device::device_revoke_temporary,
            device::device_set_rotation,
            unirc_write,
            unirc_resize,
            unirc_close,
        ])
        .run(tauri::generate_context!())
        .expect("瞬移客户端启动失败");
}

// ---------- 系统目录选择 ----------

/// 弹出系统目录选择框；取消返回 None。Rust 侧调用，不占 capabilities 权限。
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog()
        .file()
        .set_title("打开项目文件夹")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    rx.await.map_err(|e| format!("目录选择失败：{e}"))
}

// ---------- 主机配置导入 / 导出 ----------

/// 弹出文件选择框，返回选中文件的文本内容；取消返回 None。
#[tauri::command]
async fn import_hosts_text(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog()
        .file()
        .set_title("导入主机配置（JSON）")
        .add_filter("JSON", &["json"])
        .pick_file(move |path| {
            let content = path.and_then(|p| std::fs::read_to_string(p.to_string()).ok());
            let _ = tx.send(content);
        });
    rx.await.map_err(|e| format!("对话框异常关闭：{e}"))
}

/// 弹出保存对话框，把主机配置 JSON 写入用户选择的文件；取消返回 None，成功返回保存路径。
#[tauri::command]
async fn export_hosts_text(
    app: tauri::AppHandle,
    content: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog()
        .file()
        .set_title("导出主机配置（JSON）")
        .add_filter("JSON", &["json"])
        .set_file_name("shunyi-hosts.json")
        .save_file(move |path| {
            let saved = path.and_then(|p| {
                let target = p.to_string();
                std::fs::write(&target, &content).ok()?;
                Some(target)
            });
            let _ = tx.send(saved);
        });
    rx.await.map_err(|e| format!("对话框异常关闭：{e}"))
}

// ---------- 本地终端 ----------

#[tauri::command]
async fn pty_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<String, String> {
    pty::open(app, state.inner(), cols.max(2), rows.max(2), cwd).await
}

#[tauri::command]
async fn pty_write(
    state: tauri::State<'_, AppState>,
    session: String,
    data: String,
) -> Result<(), String> {
    let handle = state.ptys.lock().get(&session).cloned();
    match handle {
        Some(h) => pty::write(&h, data.as_bytes()),
        None => Err(format!("会话 {session} 不存在")),
    }
}

#[tauri::command]
async fn pty_resize(
    state: tauri::State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let handle = state.ptys.lock().get(&session).cloned();
    match handle {
        Some(h) => pty::resize(&h, cols, rows),
        None => Err(format!("会话 {session} 不存在")),
    }
}

#[tauri::command]
async fn pty_close(state: tauri::State<'_, AppState>, session: String) -> Result<(), String> {
    state.ptys.lock().remove(&session);
    Ok(())
}

// ---------- SSH ----------

#[tauri::command]
async fn ssh_connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    password: String,
    key_path: String,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    ssh::connect(
        app,
        state.inner(),
        ssh::SshTarget {
            host,
            port: if port == 0 { 22 } else { port },
            username,
            auth_type,
            password,
            key_path,
        },
        cols.max(2),
        rows.max(2),
    )
    .await
}

#[tauri::command]
async fn ssh_write(
    state: tauri::State<'_, AppState>,
    session: String,
    data: String,
) -> Result<(), String> {
    let handle = state.ssh.lock().get(&session).cloned();
    match handle {
        Some(h) => h.write(data.as_bytes().to_vec()).await,
        None => Err(format!("会话 {session} 不存在")),
    }
}

#[tauri::command]
async fn ssh_resize(
    state: tauri::State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let handle = state.ssh.lock().get(&session).cloned();
    match handle {
        Some(h) => h.resize(cols, rows).await,
        None => Err(format!("会话 {session} 不存在")),
    }
}

#[tauri::command]
async fn ssh_close(state: tauri::State<'_, AppState>, session: String) -> Result<(), String> {
    let handle = state.ssh.lock().remove(&session);
    if let Some(h) = handle {
        h.close().await;
    }
    Ok(())
}

// ---------- UniRC 自有协议 ----------

#[tauri::command]
async fn unirc_connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: unirc::ConnectRequest,
) -> Result<String, String> {
    unirc::connect_and_open_session(app, state.inner(), request).await
}
#[tauri::command]
async fn unirc_write(
    state: tauri::State<'_, AppState>,
    session: String,
    data: String,
) -> Result<(), String> {
    state.unirc.write_session(&session, data.as_bytes()).await
}
#[tauri::command]
async fn unirc_resize(
    state: tauri::State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.unirc.resize_session(&session, cols, rows).await
}
#[tauri::command]
async fn unirc_close(state: tauri::State<'_, AppState>, session: String) -> Result<(), String> {
    state.unirc.close_session(&session).await
}
