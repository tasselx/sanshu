//! 附件工作目录相关的 Tauri 命令
//!
//! 提供给前端弹窗与设置页调用：保存粘贴/拖入的附件、列出/删除/清空、
//! 获取与设置工作目录、打开目录、读取图片预览。

use tauri::{AppHandle, State};

use super::{AttachmentFileInfo, AttachmentInfo};
use crate::config::{save_config, AppState};

/// 保存「粘贴」进来的 base64 数据为附件，返回附件信息
#[tauri::command]
pub async fn save_pasted_attachment(
    data_base64: String,
    filename: Option<String>,
) -> Result<AttachmentInfo, String> {
    super::save_base64(&data_base64, filename.as_deref()).map_err(|e| e.to_string())
}

/// 复制「拖入」的若干文件到工作目录，返回成功保存的附件信息列表
#[tauri::command]
pub async fn save_dropped_attachments(paths: Vec<String>) -> Result<Vec<AttachmentInfo>, String> {
    let mut result = Vec::new();
    for p in paths {
        match super::save_dropped(&p) {
            Ok(info) => result.push(info),
            // 单个文件失败不阻断其它文件，仅记录日志
            Err(e) => log::warn!("[attachments] 保存拖入文件失败 {}: {}", p, e),
        }
    }
    Ok(result)
}

/// 列出工作目录中的全部附件
#[tauri::command]
pub async fn list_attachments() -> Result<Vec<AttachmentFileInfo>, String> {
    super::list().map_err(|e| e.to_string())
}

/// 删除工作目录中的单个附件
#[tauri::command]
pub async fn delete_attachment(filename: String) -> Result<(), String> {
    super::delete(&filename).map_err(|e| e.to_string())
}

/// 清空工作目录中的全部附件，返回删除数量
#[tauri::command]
pub async fn clear_attachments() -> Result<u32, String> {
    super::clear().map_err(|e| e.to_string())
}

/// 获取当前生效的工作目录绝对路径
#[tauri::command]
pub async fn get_attachment_workspace_dir() -> Result<String, String> {
    super::resolve_workspace_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

/// 设置自定义工作目录（传 None/空字符串则恢复为默认全局目录）
#[tauri::command]
pub async fn set_attachment_workspace_dir(
    dir: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    // 归一化：空白视为 None（恢复默认）
    let normalized = dir
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // 写入配置
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.mcp_config.attachment_workspace_dir = normalized;
    }
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 返回最终生效目录（并确保已创建）
    super::resolve_workspace_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

/// 在系统文件管理器中打开工作目录
#[tauri::command]
pub async fn open_attachment_workspace_dir() -> Result<(), String> {
    let dir = super::resolve_workspace_dir().map_err(|e| e.to_string())?;
    let dir_str = dir.to_string_lossy().to_string();

    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(&dir_str).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&dir_str).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&dir_str).spawn()
    };

    result.map_err(|e| format!("打开目录失败: {}", e))?;
    Ok(())
}

/// 读取图片附件并返回 data URL（仅供弹窗预览）
#[tauri::command]
pub async fn read_attachment_preview(path: String) -> Result<String, String> {
    super::read_image_data_url(&path).map_err(|e| e.to_string())
}

/// 弹出系统目录选择对话框，返回所选目录路径（取消则返回 None）
#[tauri::command]
pub async fn select_attachment_workspace_dir(
    app: AppHandle,
    default_path: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let mut builder = app.dialog().file();

    if let Some(path) = default_path {
        let path_buf = std::path::PathBuf::from(&path);
        if path_buf.exists() {
            builder = builder.set_directory(&path_buf);
        }
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_folder(move |folder_path| {
        let _ = tx.send(folder_path);
    });

    let result = rx.await.map_err(|_| "对话框选择被取消".to_string())?;
    Ok(result.map(|path| path.to_string()))
}
