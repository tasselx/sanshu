// Exa Tauri 命令入口
// 提供配置读写和连接测试功能

use super::types::ExaTestConnectionResponse;
use crate::config::AppState;
use tauri::State;

/// 获取 Exa 配置
#[tauri::command]
pub async fn get_exa_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    Ok(serde_json::json!({
        "api_key": config.mcp_config.exa_api_key.as_deref().unwrap_or(""),
    }))
}

/// 保存 Exa 配置
#[tauri::command]
pub async fn save_exa_config(
    api_key: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 空字符串视为清除 API Key
        config.mcp_config.exa_api_key = if api_key.trim().is_empty() {
            None
        } else {
            Some(api_key.trim().to_string())
        };
    }

    crate::config::save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 测试 Exa 连接
#[tauri::command]
pub async fn test_exa_connection(api_key: String) -> Result<ExaTestConnectionResponse, String> {
    use reqwest::Client;
    use std::time::Duration;

    if api_key.trim().is_empty() {
        return Ok(ExaTestConnectionResponse {
            success: false,
            message: "API Key 不能为空".to_string(),
            preview: None,
        });
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    // 使用一次最小成本的 keyword search 测试（不请求正文）
    let body = serde_json::json!({
        "query": "test",
        "type": "keyword",
        "numResults": 1,
    });

    match client
        .post("https://api.exa.ai/search")
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key.trim())
        .json(&body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                let text = response.text().await.unwrap_or_default();
                // 尝试解析获取结果数量和费用
                let preview = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    let results_count = parsed["results"].as_array().map(|a| a.len()).unwrap_or(0);
                    let cost = parsed["costDollars"]["total"].as_f64().unwrap_or(0.0);
                    Some(format!(
                        "获取到 {} 条结果，本次费用 ${:.4}",
                        results_count, cost
                    ))
                } else {
                    None
                };

                Ok(ExaTestConnectionResponse {
                    success: true,
                    message: "连接成功！API Key 有效。".to_string(),
                    preview,
                })
            } else {
                let error_text = response.text().await.unwrap_or_default();
                let msg = match status.as_u16() {
                    401 => "API Key 无效或已过期".to_string(),
                    402 => "账户额度已耗尽".to_string(),
                    429 => "请求频率超限，请稍后重试".to_string(),
                    _ => {
                        // 按字符截断，避免多字节字符导致越界
                        let preview: String = error_text.chars().take(200).collect();
                        format!("HTTP {} - {}", status.as_u16(), preview)
                    }
                };

                Ok(ExaTestConnectionResponse {
                    success: false,
                    message: format!("连接失败: {}", msg),
                    preview: None,
                })
            }
        }
        Err(e) => {
            let msg = if e.is_timeout() {
                "连接超时（15s）".to_string()
            } else if e.is_connect() {
                "无法连接到 Exa API，请检查网络".to_string()
            } else {
                format!("请求失败: {}", e)
            };

            Ok(ExaTestConnectionResponse {
                success: false,
                message: msg,
                preview: None,
            })
        }
    }
}
