//! 本地文件浏览：项目文件树（按目录懒加载）与只读预览。

use serde::Serialize;

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct FileContent {
    pub content: String,
    pub size: u64,
    pub lines: usize,
}

/// 列出单个目录（懒加载：前端展开哪个目录就读哪个），目录在前、名称排序。
#[tauri::command]
pub async fn fs_list(dir: String) -> Result<Vec<FsEntry>, String> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(&dir).map_err(|e| format!("无法读取目录 {dir}：{e}"))?;
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".DS_Store" || name.starts_with("._") {
            continue; // macOS 元数据噪音
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(FsEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir: ft.is_dir(),
            size,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// 读取文本文件用于只读预览：512KB / 8000 行上限，拒绝二进制。
#[tauri::command]
pub async fn fs_read(path: String) -> Result<FileContent, String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("无法读取 {path}：{e}"))?;
    if meta.is_dir() {
        return Err("这是一个目录，不能作为文件预览。".into());
    }
    let size = meta.len();
    if size > 512 * 1024 {
        return Err("文件超过 512 KB，暂不加载全文。".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("无法读取 {path}：{e}"))?;
    if bytes.contains(&0) {
        return Err("此文件包含二进制内容，无法以源码预览。".into());
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| "该文件不是有效的 UTF-8 文本，暂不提供编码转换。".to_string())?;
    let lines = content.split('\n').count();
    if lines > 8000 {
        return Err("文件超过 8,000 行，暂不加载全文。".into());
    }
    Ok(FileContent {
        content,
        size,
        lines,
    })
}

/// 当前用户主目录（首个默认项目的根）。Windows 用 USERPROFILE，unix 用 HOME。
#[tauri::command]
pub async fn home_dir() -> Result<String, String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "无法获取主目录".into())
}
