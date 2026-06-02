// Tauri 命令入口
// 将提示词增强功能暴露给前端调用

use super::core::PromptEnhancer;
use super::history::ChatHistoryManager;
use super::types::*;
use crate::log_important;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter};

// 中文注释：保存增强请求的取消标记，用于前端主动取消
static ENHANCE_CANCEL_FLAGS: Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn register_cancel_flag(request_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = ENHANCE_CANCEL_FLAGS.lock() {
        map.insert(request_id.to_string(), flag.clone());
    }
    flag
}

fn remove_cancel_flag(request_id: &str) {
    if let Ok(mut map) = ENHANCE_CANCEL_FLAGS.lock() {
        map.remove(request_id);
    }
}

fn cancel_request(request_id: &str) -> bool {
    if let Ok(map) = ENHANCE_CANCEL_FLAGS.lock() {
        if let Some(flag) = map.get(request_id) {
            flag.store(true, Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// 流式增强提示词（主要入口）
/// 通过 Tauri Event 推送流式结果给前端
#[tauri::command]
pub async fn enhance_prompt_stream(
    app_handle: AppHandle,
    prompt: String,
    // 中文注释：原始用户输入（可选，用于历史记录与兜底）
    original_prompt: Option<String>,
    project_root_path: Option<String>,
    current_file_path: Option<String>,
    include_history: Option<bool>,
    selected_history_ids: Option<Vec<String>>,
    request_id: Option<String>,
) -> Result<EnhanceResponse, String> {
    let request_id = request_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let cancel_flag = register_cancel_flag(&request_id);

    log_important!(
        info,
        "收到增强请求: request_id={}, prompt_len={}, project={:?}",
        request_id,
        prompt.len(),
        project_root_path.as_ref().map(|p| p.len())
    );

    // 创建增强器
    let mut enhancer = PromptEnhancer::from_acemcp_config()
        .await
        .map_err(|e| format!("初始化增强器失败: {}", e))?;

    if let Some(ref path) = project_root_path {
        enhancer = enhancer.with_project_root(path);
    }

    let request = EnhanceRequest {
        prompt: prompt.clone(),
        original_prompt: original_prompt.clone(),
        project_root_path: project_root_path.clone(),
        current_file_path,
        include_history: include_history.unwrap_or(true),
        selected_history_ids,
        request_id: Some(request_id.clone()),
        cancel_flag: Some(cancel_flag.clone()),
    };

    // 使用流式增强
    let app = app_handle.clone();
    let result = enhancer
        .enhance_stream(request, move |event| {
            // 通过 Tauri Event 推送给前端
            if let Err(e) = app.emit("enhance-stream", &event) {
                log_important!(warn, "推送增强事件失败: {}", e);
            }
        })
        .await;

    // 中文注释：请求结束后释放取消标记，避免内存泄漏
    remove_cancel_flag(&request_id);

    match result {
        Ok(response) => {
            // 如果增强成功，记录到对话历史
            if response.success {
                if let Some(ref path) = project_root_path {
                    if let Ok(manager) = ChatHistoryManager::new(path) {
                        // 中文注释：优先记录“原始用户输入”，避免把规则/上下文拼接写入历史
                        let user_input = original_prompt.as_deref().unwrap_or(&prompt);
                        let _ = manager.add_entry(user_input, &response.enhanced_prompt, "enhance");
                    }
                }
            }
            Ok(response)
        }
        Err(e) => Err(format!("增强失败: {}", e)),
    }
}

/// 同步增强提示词（简化版，等待完成后返回）
#[tauri::command]
pub async fn enhance_prompt(
    prompt: String,
    // 中文注释：原始用户输入（可选，用于历史记录与兜底）
    original_prompt: Option<String>,
    project_root_path: Option<String>,
    current_file_path: Option<String>,
    include_history: Option<bool>,
    selected_history_ids: Option<Vec<String>>,
    request_id: Option<String>,
) -> Result<EnhanceResponse, String> {
    let request_id = request_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    log_important!(
        info,
        "收到同步增强请求: request_id={}, prompt_len={}",
        request_id,
        prompt.len()
    );

    // 创建增强器
    let mut enhancer = PromptEnhancer::from_acemcp_config()
        .await
        .map_err(|e| format!("初始化增强器失败: {}", e))?;

    if let Some(ref path) = project_root_path {
        enhancer = enhancer.with_project_root(path);
    }

    let request = EnhanceRequest {
        prompt: prompt.clone(),
        original_prompt,
        project_root_path: project_root_path.clone(),
        current_file_path,
        include_history: include_history.unwrap_or(true),
        selected_history_ids,
        request_id: Some(request_id),
        cancel_flag: None,
    };

    enhancer
        .enhance(request)
        .await
        .map_err(|e| format!("增强失败: {}", e))
}

/// 添加对话历史记录
#[tauri::command]
pub async fn add_chat_history(
    project_root_path: String,
    user_input: String,
    ai_response: String,
    source: Option<String>,
) -> Result<String, String> {
    let manager = ChatHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    manager
        .add_entry(
            &user_input,
            &ai_response,
            &source.unwrap_or_else(|| "popup".to_string()),
        )
        .map_err(|e| format!("添加历史记录失败: {}", e))
}

/// 获取对话历史
#[tauri::command]
pub async fn get_chat_history(
    project_root_path: String,
    count: Option<usize>,
) -> Result<Vec<super::history::ChatEntry>, String> {
    let manager = ChatHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    manager
        .get_recent_entries(count.unwrap_or(20))
        .map_err(|e| format!("读取历史记录失败: {}", e))
}

/// 清空对话历史
#[tauri::command]
pub async fn clear_chat_history(project_root_path: String) -> Result<(), String> {
    let manager = ChatHistoryManager::new(&project_root_path)
        .map_err(|e| format!("创建历史管理器失败: {}", e))?;

    manager.clear().map_err(|e| format!("清空历史失败: {}", e))
}

/// 取消正在进行的增强请求
#[tauri::command]
pub async fn cancel_enhance_request(request_id: String) -> Result<bool, String> {
    Ok(cancel_request(&request_id))
}
