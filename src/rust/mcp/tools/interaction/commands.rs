// zhi 弹窗交互历史相关命令
// 提供添加、查询、清空历史的 Tauri 接口

use super::zhi_history::{ZhiHistoryEntry, ZhiHistoryManager};

/// 添加 zhi 交互历史
#[tauri::command]
pub async fn add_zhi_history(
    project_root_path: String,
    request_id: String,
    prompt: String,
    user_reply: String,
    source: Option<String>,
) -> Result<String, String> {
    let manager = ZhiHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    manager
        .add_entry(
            &request_id,
            &prompt,
            &user_reply,
            &source.unwrap_or_else(|| "popup".to_string()),
        )
        .map_err(|e| format!("添加历史记录失败: {}", e))
}

/// 获取 zhi 交互历史
#[tauri::command]
pub async fn get_zhi_history(
    project_root_path: String,
    count: Option<usize>,
) -> Result<Vec<ZhiHistoryEntry>, String> {
    let manager = ZhiHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    Ok(manager.get_recent(count.unwrap_or(20)))
}

/// 清空 zhi 交互历史
#[tauri::command]
pub async fn clear_zhi_history(project_root_path: String) -> Result<(), String> {
    let manager = ZhiHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    manager.clear().map_err(|e| format!("清空历史失败: {}", e))
}
