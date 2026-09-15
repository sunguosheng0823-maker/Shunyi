//! 本地终端：portable-pty 起本地 shell。

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use base64::Engine;

#[derive(Clone)]
pub struct PtyHandle {
    /// 写入经有界通道交给专属 std 线程，异步路径永不阻塞在 PTY I/O 上。
    write_tx: Arc<std::sync::mpsc::SyncSender<Vec<u8>>>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

pub async fn open(
    app: tauri::AppHandle,
    state: &crate::AppState,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<String, String> {
    let session = state.next_session("local");

    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    let mut cmd = portable_pty::CommandBuilder::new(default_shell());
    cmd.env("TERM", "xterm-256color");
    // 起始目录：用户在设置里选定的本地位置，作为 shell 的根目录
    if let Some(dir) = cwd.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
        let path = std::path::PathBuf::from(&dir);
        if !path.is_dir() {
            return Err(format!("起始目录不存在或不可访问：{dir}"));
        }
        cmd.cwd(path);
    }
    let _child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // 专属写线程：从通道取数据写 PTY
    let (write_tx, write_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(128);
    std::thread::spawn(move || {
        let mut w = writer;
        for buf in write_rx.iter() {
            if w.write_all(&buf).and_then(|_| w.flush()).is_err() {
                break;
            }
        }
    });

    let handle = PtyHandle {
        write_tx: Arc::new(write_tx),
        master: Arc::new(Mutex::new(pair.master)),
    };

    // 读线程：PTY 输出 → 前端事件
    let session2 = session.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let payload = serde_json::json!({
                        "proto": "local",
                        "session": session2,
                        "b64": base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
                    });
                    use tauri::Emitter;
                    if app2.emit("term:data", payload).is_err() {
                        break;
                    }
                }
            }
        }
        use tauri::Emitter;
        let _ = app2.emit(
            "term:closed",
            serde_json::json!({"proto": "local", "session": session2, "reason": "shell 退出"}),
        );
    });

    state.ptys.lock().insert(session.clone(), handle);
    Ok(session)
}

pub fn write(handle: &PtyHandle, data: &[u8]) -> Result<(), String> {
    handle
        .write_tx
        .try_send(data.to_vec())
        .map_err(|_| "PTY 写入通道满或已关闭".to_string())
}

pub fn resize(handle: &PtyHandle, cols: u16, rows: u16) -> Result<(), String> {
    handle
        .master
        .lock()
        .unwrap()
        .resize(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    /// 起始目录必须是 shell 实际所在目录：spawn `sh -c pwd` 读回输出验证。
    #[test]
    fn pty_cwd_is_honored() {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "pwd"]);
        cmd.cwd("/tmp");
        pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        // macOS 上 /tmp 是 /private/tmp 的符号链接，按规范化路径比较
        let expected = std::fs::canonicalize("/tmp").unwrap();
        assert_eq!(std::path::PathBuf::from(out.trim()), expected);
    }
}
