use crate::config::{load_config_and_apply_window_settings, AppState};
use crate::log_important;
use crate::ui::exit_handler::setup_exit_handlers;
use crate::ui::{initialize_audio_asset_manager, setup_window_event_listeners};
use tauri::{AppHandle, Manager};

/// 应用设置和初始化
pub async fn setup_application(app_handle: &AppHandle) -> Result<(), String> {
    let state = app_handle.state::<AppState>();

    // 加载配置并应用窗口设置
    if let Err(e) = load_config_and_apply_window_settings(&state, app_handle).await {
        log_important!(warn, "加载配置失败: {}", e);
    }

    // 中文说明：启动时自动清理附件工作目录中「超过 7 天」的旧文件（best-effort，失败不影响启动）
    match crate::attachments::cleanup_expired() {
        Ok(n) if n > 0 => log_important!(info, "已自动清理 {} 个过期附件（>7天）", n),
        Ok(_) => {}
        Err(e) => log_important!(warn, "自动清理过期附件失败: {}", e),
    }

    // 初始化音频资源管理器
    if let Err(e) = initialize_audio_asset_manager(app_handle) {
        log_important!(warn, "初始化音频资源管理器失败: {}", e);
    }

    // 设置窗口事件监听器
    setup_window_event_listeners(app_handle);

    // 设置退出处理器
    if let Err(e) = setup_exit_handlers(app_handle) {
        log_important!(warn, "设置退出处理器失败: {}", e);
    }

    // 中文说明：应用启动后延迟刷新 GitHub 代理站延迟缓存，避免阻塞主界面启动。
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        if let Err(e) = crate::network::refresh_github_proxy_cache().await {
            log_important!(warn, "刷新 GitHub 代理站缓存失败: {}", e);
        }
    });

    Ok(())
}
