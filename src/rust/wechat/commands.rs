use crate::config::{save_config, AppState, WechatConfig};
use crate::log_important;
use crate::wechat::history::{
    add_history_entry, clear_history, get_history, history_path, WechatHistoryEntry,
};
use crate::wechat::parser::{parse_wechat_reply, request_short_code};
use crate::wechat::pending::{
    list_pending, normalize_project_path, register_pending, update_pending, WechatPendingRequest,
    WechatPendingStatus, WECHAT_PENDING_EXPIRY_SECS,
};
use crate::wechat::state::{
    clear_wechat_state, load_wechat_state, save_wechat_state, state_path, StoredWechatCredentials,
    WechatRuntimeState,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use qrcode::{render::svg, QrCode};
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout, Duration};
use uuid::Uuid;
use wechatbot::protocol::{
    build_cdn_upload_url, build_media_message, build_text_message, GetUploadUrlParams, ILinkClient,
    CDN_BASE_URL, DEFAULT_BASE_URL,
};
use wechatbot::{
    encode_aes_key_base64, encode_aes_key_hex, encrypt_aes_ecb, generate_aes_key, IncomingMessage,
};

static BINDING_ACTIVE: AtomicBool = AtomicBool::new(false);
static VERIFY_CODE_SENDER: Lazy<Mutex<Option<oneshot::Sender<String>>>> =
    Lazy::new(|| Mutex::new(None));

#[derive(Debug, Serialize)]
pub struct WechatStatus {
    pub enabled: bool,
    pub bound: bool,
    pub binding: bool,
    pub target_user: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WechatDiagnostics {
    pub base_url: Option<String>,
    pub account_id: Option<String>,
    pub bot_user_id: Option<String>,
    pub context_ready: bool,
    pub cursor_ready: bool,
    pub state_file: String,
    pub history_file: String,
    pub history_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WechatEvent {
    Submit {
        selected_options: Vec<String>,
        user_input: Option<String>,
    },
    Continue,
}

// 历史写入只用于查询，不应阻断已经成功的微信收发主流程。
fn record_history(
    direction: &str,
    kind: &str,
    request_code: Option<&str>,
    content: &str,
    status: &str,
) {
    if let Err(error) = add_history_entry(direction, kind, request_code, content, status) {
        log_important!(warn, "[wechat] history: write_failed error={}", error);
    }
}

#[tauri::command]
pub async fn get_wechat_config(state: State<'_, AppState>) -> Result<WechatConfig, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {e}"))?;
    Ok(config.wechat_config.clone())
}

#[tauri::command]
pub async fn set_wechat_config(
    wechat_config: WechatConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {e}"))?;
        config.wechat_config = wechat_config;
    }
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {e}"))
}

/// 获取跨进程微信待处理请求，供管理页展示项目、AI 和短码。
#[tauri::command]
pub fn get_wechat_pending_requests() -> Result<Vec<WechatPendingRequest>, String> {
    list_pending().map_err(|error| error.to_string())
}

/// 保存项目别名；路径只作为本地配置键，不会发送到微信。
#[tauri::command]
pub async fn set_wechat_project_alias(
    project_root_path: String,
    alias: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let normalized = normalize_project_path(&project_root_path);
    if normalized.is_empty() {
        return Err("项目路径不能为空".to_string());
    }
    let alias = alias.trim().to_string();
    if alias.chars().count() > 40 {
        return Err("项目别名不能超过 40 个字符".to_string());
    }
    if alias.chars().any(|ch| ch.is_control()) {
        return Err("项目别名不能包含控制字符".to_string());
    }
    {
        let mut config = state
            .config
            .lock()
            .map_err(|error| format!("获取配置失败: {error}"))?;
        if alias.is_empty() {
            config.wechat_config.project_aliases.remove(&normalized);
        } else {
            config
                .wechat_config
                .project_aliases
                .insert(normalized, alias);
        }
    }
    save_config(&state, &app)
        .await
        .map_err(|error| format!("保存项目别名失败: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn get_wechat_status(state: State<'_, AppState>) -> Result<WechatStatus, String> {
    let enabled = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {e}"))?
        .wechat_config
        .enabled;
    let runtime = load_wechat_state().map_err(|e| e.to_string())?;
    Ok(WechatStatus {
        enabled,
        bound: runtime.is_bound(),
        binding: BINDING_ACTIVE.load(Ordering::SeqCst),
        target_user: mask_user_id(&runtime.target_user_id),
    })
}

#[tauri::command]
pub fn get_wechat_diagnostics() -> Result<WechatDiagnostics, String> {
    let runtime = load_wechat_state().map_err(|e| e.to_string())?;
    let history_count = get_history(200).map_err(|e| e.to_string())?.len();
    Ok(WechatDiagnostics {
        base_url: runtime
            .credentials
            .as_ref()
            .map(|credentials| credentials.base_url.clone()),
        account_id: runtime
            .credentials
            .as_ref()
            .and_then(|credentials| mask_user_id(&credentials.account_id)),
        bot_user_id: runtime
            .credentials
            .as_ref()
            .and_then(|credentials| mask_user_id(&credentials.bot_user_id)),
        context_ready: !runtime.context_token.trim().is_empty(),
        cursor_ready: !runtime.cursor.trim().is_empty(),
        state_file: state_path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string(),
        history_file: history_path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string(),
        history_count,
    })
}

#[tauri::command]
pub fn get_wechat_history(limit: Option<usize>) -> Result<Vec<WechatHistoryEntry>, String> {
    get_history(limit.unwrap_or(200)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_wechat_history() -> Result<(), String> {
    clear_history().map_err(|e| e.to_string())?;
    log_important!(info, "[wechat] history: cleared");
    Ok(())
}

#[tauri::command]
pub async fn start_wechat_binding(app: AppHandle) -> Result<(), String> {
    if BINDING_ACTIVE.swap(true, Ordering::SeqCst) {
        return Err("微信绑定正在进行中".to_string());
    }

    tauri::async_runtime::spawn(async move {
        let result = run_binding_flow(&app).await;
        BINDING_ACTIVE.store(false, Ordering::SeqCst);
        if let Err(error) = result {
            log_important!(error, "[wechat] 绑定失败: {}", error);
            let _ = app.emit("wechat-binding-error", error);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn submit_wechat_verify_code(code: String) -> Result<(), String> {
    let sender = VERIFY_CODE_SENDER
        .lock()
        .map_err(|_| "验证码通道状态异常".to_string())?
        .take()
        .ok_or_else(|| "当前没有待提交的配对码".to_string())?;
    sender
        .send(code.trim().to_string())
        .map_err(|_| "配对码提交已过期".to_string())
}

#[tauri::command]
pub async fn clear_wechat_binding() -> Result<(), String> {
    clear_wechat_state().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_wechat_connection() -> Result<String, String> {
    let runtime = require_bound_state()?;
    send_text(&runtime, "三术微信通知连接测试成功").await?;
    record_history("outgoing", "test", None, "三术微信通知连接测试成功", "sent");
    log_important!(info, "[wechat] notification: test_sent");
    Ok("微信通知连接正常".to_string())
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "User32")]
extern "system" {
    fn GetLastInputInfo(last_input_info: *mut LastInputInfo) -> i32;
}

/// 读取系统最近一次键盘或鼠标输入时间，用于判断用户是否仍在电脑前操作。
#[tauri::command]
pub fn get_system_last_input_tick() -> Result<u32, String> {
    #[cfg(target_os = "windows")]
    {
        let mut info = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        let succeeded = unsafe { GetLastInputInfo(&mut info) };
        return (succeeded != 0)
            .then_some(info.dw_time)
            .ok_or_else(|| "读取系统输入状态失败".to_string());
    }

    #[cfg(not(target_os = "windows"))]
    Err("当前平台未实现系统输入检测".to_string())
}

#[tauri::command]
pub async fn start_wechat_sync(
    request_id: String,
    message: String,
    predefined_options: Vec<String>,
    image_pages: Vec<String>,
    project_root_path: Option<String>,
    agent_label: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let wechat_config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {e}"))?
        .wechat_config
        .clone();
    if !wechat_config.enabled {
        return Ok(());
    }

    let runtime = require_bound_state()?;
    let code = request_short_code(&request_id);
    let project_root_path = project_root_path.unwrap_or_default();
    let project_alias = if project_root_path.trim().is_empty() {
        "未命名项目".to_string()
    } else {
        crate::wechat::pending::project_alias(&project_root_path, &wechat_config.project_aliases)
    };
    let agent_label = agent_label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("AI-{code}"));
    let start_time_ms = now_millis();
    log_important!(
        info,
        "[wechat] notification: sending code={} pages={} options={}",
        code,
        image_pages.len(),
        predefined_options.len()
    );
    for (index, image_page) in image_pages.iter().enumerate() {
        let bytes = STANDARD
            .decode(image_page)
            .map_err(|e| format!("解析通知图片失败: {e}"))?;
        let caption = format!(
            "三术 zhi · {} · {} · #{code} ({}/{})",
            project_alias,
            agent_label,
            index + 1,
            image_pages.len()
        );
        send_image(&runtime, bytes, Some(caption)).await?;
    }
    send_text(
        &runtime,
        &build_reply_guide(&code, &project_alias, &agent_label, &predefined_options),
    )
    .await?;
    record_history("outgoing", "zhi", Some(&code), &message, "sent");
    if let Err(error) = register_pending(
        &request_id,
        &code,
        &project_root_path,
        &wechat_config.project_aliases,
        &agent_label,
        &message,
    ) {
        log_important!(warn, "[wechat] pending: register_failed error={error}");
    }
    log_important!(info, "[wechat] notification: sent code={}", code);

    tauri::async_runtime::spawn(async move {
        if let Err(error) = listen_for_reply(
            runtime,
            code,
            predefined_options,
            start_time_ms,
            request_id,
            project_alias,
            agent_label,
            app.clone(),
        )
        .await
        {
            log_important!(warn, "[wechat] 回复监听结束: {}", error);
        }
    });
    Ok(())
}

async fn run_binding_flow(app: &AppHandle) -> Result<(), String> {
    log_important!(info, "[wechat] binding: start");
    let client = ILinkClient::with_bot_agent(Some("Sanshu/WechatNotification"));
    let existing = load_wechat_state().map_err(|e| e.to_string())?;
    let local_tokens = existing
        .credentials
        .as_ref()
        .map(|credentials| vec![credentials.token.clone()])
        .unwrap_or_default();

    let mut credentials = None;
    for attempt in 1..=3 {
        log_important!(info, "[wechat] binding: requesting_qr attempt={}", attempt);
        app.emit("wechat-binding-status", "requesting_qr")
            .map_err(|e| format!("推送二维码请求状态失败: {e}"))?;
        let qr = timeout(
            Duration::from_secs(15),
            client.get_qr_code(DEFAULT_BASE_URL, &local_tokens),
        )
        .await
        .map_err(|_| "获取登录二维码超时，请检查网络或代理后重试".to_string())?
        .map_err(|e| format!("获取登录二维码失败: {e}"))?;
        let qr_image = render_qr_data_url(&qr.qrcode_img_content)?;
        log_important!(
            info,
            "[wechat] binding: qr_ready content_length={}",
            qr.qrcode_img_content.len()
        );
        app.emit("wechat-qrcode", &qr_image)
            .map_err(|e| format!("推送二维码到界面失败: {e}"))?;
        app.emit("wechat-binding-status", "qr_ready")
            .map_err(|e| format!("推送二维码就绪状态失败: {e}"))?;
        let mut poll_base_url = DEFAULT_BASE_URL.to_string();
        let mut verify_code = None;
        let mut last_status = String::new();

        loop {
            let status = client
                .poll_qr_status(&poll_base_url, &qr.qrcode, verify_code.as_deref())
                .await
                .map_err(|e| format!("查询扫码状态失败: {e}"))?;
            verify_code = None;
            if status.status != last_status {
                log_important!(info, "[wechat] binding: status={}", status.status);
                last_status = status.status.clone();
            }
            let _ = app.emit("wechat-binding-status", &status.status);
            match status.status.as_str() {
                "confirmed" => {
                    credentials = Some(StoredWechatCredentials {
                        token: status
                            .bot_token
                            .ok_or_else(|| "登录结果缺少 bot_token".to_string())?,
                        base_url: status.baseurl.unwrap_or_else(|| poll_base_url.clone()),
                        account_id: status.ilink_bot_id.unwrap_or_default(),
                        bot_user_id: status.ilink_user_id.unwrap_or_default(),
                    });
                    break;
                }
                "binded_redirect" => {
                    credentials = existing.credentials.clone();
                    break;
                }
                "need_verifycode" => {
                    let (sender, receiver) = oneshot::channel();
                    *VERIFY_CODE_SENDER
                        .lock()
                        .map_err(|_| "创建配对码通道失败".to_string())? = Some(sender);
                    let _ = app.emit("wechat-verify-code-required", ());
                    verify_code = Some(
                        timeout(Duration::from_secs(180), receiver)
                            .await
                            .map_err(|_| "等待配对码超时".to_string())?
                            .map_err(|_| "配对码通道已关闭".to_string())?,
                    );
                    continue;
                }
                "scaned_but_redirect" => {
                    if let Some(host) = status.redirect_host {
                        poll_base_url = format!("https://{host}");
                    }
                }
                "expired" | "verify_code_blocked" => break,
                _ => {}
            }
            if credentials.is_some() {
                break;
            }
            sleep(Duration::from_secs(2)).await;
        }
        if credentials.is_some() {
            break;
        }
    }

    let credentials = credentials.ok_or_else(|| "微信扫码登录未完成".to_string())?;
    let mut runtime = WechatRuntimeState {
        credentials: Some(credentials),
        cursor: existing.cursor,
        ..WechatRuntimeState::default()
    };
    save_wechat_state(&runtime).map_err(|e| e.to_string())?;
    let _ = app.emit("wechat-binding-status", "waiting_message");

    loop {
        let credentials = runtime.credentials.as_ref().expect("凭据已设置");
        let updates = client
            .get_updates(&credentials.base_url, &credentials.token, &runtime.cursor)
            .await
            .map_err(|e| format!("等待绑定消息失败: {e}"))?;
        if !updates.get_updates_buf.is_empty() {
            runtime.cursor = updates.get_updates_buf;
        }
        if let Some(message) = updates.msgs.iter().find_map(IncomingMessage::from_wire) {
            record_history("incoming", "system", None, &message.text, "received");
            runtime.target_user_id = message.user_id.clone();
            runtime.context_token = message.context_token().to_string();
            save_wechat_state(&runtime).map_err(|e| e.to_string())?;
            send_text(
                &runtime,
                "微信通知绑定成功，后续 zhi 请求会发送到当前会话。",
            )
            .await?;
            record_history(
                "outgoing",
                "system",
                None,
                "微信通知绑定成功，后续 zhi 请求会发送到当前会话。",
                "sent",
            );
            log_important!(info, "[wechat] binding: complete");
            let _ = app.emit("wechat-binding-complete", mask_user_id(&message.user_id));
            return Ok(());
        }
        save_wechat_state(&runtime).map_err(|e| e.to_string())?;
    }
}

/// 将接口返回的扫码 URL 编码为本地 SVG，避免把文本 URL 误作图片地址。
fn render_qr_data_url(content: &str) -> Result<String, String> {
    let code = QrCode::new(content.as_bytes()).map_err(|e| format!("生成登录二维码失败: {e}"))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(256, 256)
        .quiet_zone(true)
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg.as_bytes())
    ))
}

async fn listen_for_reply(
    mut runtime: WechatRuntimeState,
    code: String,
    options: Vec<String>,
    start_time_ms: i64,
    request_id: String,
    project_alias: String,
    agent_label: String,
    app: AppHandle,
) -> Result<(), String> {
    let client = ILinkClient::with_bot_agent(Some("Sanshu/WechatNotification"));
    let expires_at = start_time_ms + WECHAT_PENDING_EXPIRY_SECS * 1000;
    loop {
        if now_millis() >= expires_at {
            let _ = update_pending(&request_id, WechatPendingStatus::Expired, None);
            let expiration = format!(
                "项目 {project_alias} · AI {agent_label} · #{code} 已过期，请回到对应 zhi 重新发起。"
            );
            if let Err(error) = send_text(&runtime, &expiration).await {
                log_important!(
                    warn,
                    "[wechat] reply: expiration_notice_failed error={error}"
                );
            }
            log_important!(info, "[wechat] reply: expired code={code}");
            return Ok(());
        }
        let credentials = runtime
            .credentials
            .as_ref()
            .ok_or_else(|| "微信凭据缺失".to_string())?;
        let updates = client
            .get_updates(&credentials.base_url, &credentials.token, &runtime.cursor)
            .await
            .map_err(|e| format!("获取微信回复失败: {e}"))?;
        if !updates.get_updates_buf.is_empty() {
            runtime.cursor = updates.get_updates_buf;
        }

        for message in updates.msgs.iter().filter_map(IncomingMessage::from_wire) {
            if message.user_id != runtime.target_user_id
                || message.raw.create_time_ms < start_time_ms
            {
                continue;
            }
            runtime.context_token = message.context_token().to_string();
            save_wechat_state(&runtime).map_err(|e| e.to_string())?;
            record_history("incoming", "reply", Some(&code), &message.text, "received");
            log_important!(info, "[wechat] reply: received code={}", code);
            if let Some(reply) = parse_wechat_reply(&message.text, &code, &options) {
                let event = if reply.continue_requested {
                    WechatEvent::Continue
                } else {
                    WechatEvent::Submit {
                        selected_options: reply.selected_options,
                        user_input: reply.user_input,
                    }
                };
                app.emit("wechat-event", &event)
                    .map_err(|e| format!("发送微信回复事件失败: {e}"))?;
                send_text(&runtime, "已收到，本次 zhi 回复正在提交。").await?;
                update_pending(&request_id, WechatPendingStatus::Replied, Some("wechat"))
                    .map_err(|error| format!("更新微信待处理状态失败: {error}"))?;
                return Ok(());
            }
        }
        save_wechat_state(&runtime).map_err(|e| e.to_string())?;
    }
}

async fn send_text(runtime: &WechatRuntimeState, text: &str) -> Result<(), String> {
    let credentials = runtime
        .credentials
        .as_ref()
        .ok_or_else(|| "微信凭据缺失".to_string())?;
    let client = ILinkClient::with_bot_agent(Some("Sanshu/WechatNotification"));
    let payload = build_text_message(&runtime.target_user_id, &runtime.context_token, text);
    client
        .send_message(&credentials.base_url, &credentials.token, &payload)
        .await
        .map_err(|e| format!("发送微信文字失败: {e}"))
}

async fn send_image(
    runtime: &WechatRuntimeState,
    data: Vec<u8>,
    caption: Option<String>,
) -> Result<(), String> {
    let credentials = runtime
        .credentials
        .as_ref()
        .ok_or_else(|| "微信凭据缺失".to_string())?;
    let client = ILinkClient::with_bot_agent(Some("Sanshu/WechatNotification"));
    let aes_key = generate_aes_key();
    let ciphertext = encrypt_aes_ecb(&data, &aes_key);
    let filekey = Uuid::new_v4().simple().to_string();
    let raw_md5 = hex::encode(Md5::digest(&data));
    let upload = client
        .get_upload_url(
            &credentials.base_url,
            &credentials.token,
            &GetUploadUrlParams {
                filekey: filekey.clone(),
                media_type: 1,
                to_user_id: runtime.target_user_id.clone(),
                rawsize: data.len(),
                rawfilemd5: raw_md5,
                filesize: ciphertext.len(),
                no_need_thumb: true,
                aeskey: encode_aes_key_hex(&aes_key),
            },
        )
        .await
        .map_err(|e| format!("获取微信图片上传地址失败: {e}"))?;
    let upload_url = match upload.upload_full_url.filter(|url| !url.trim().is_empty()) {
        Some(url) => url,
        None => {
            let upload_param = upload
                .upload_param
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "微信图片上传结果缺少上传地址".to_string())?;
            build_cdn_upload_url(CDN_BASE_URL, &upload_param, &filekey)
        }
    };
    let encrypted_param = client
        .upload_to_cdn(&upload_url, &ciphertext)
        .await
        .map_err(|e| format!("上传微信通知图片失败: {e}"))?;

    let mut items = Vec::new();
    if let Some(caption) = caption {
        items.push(json!({"type": 1, "text_item": {"text": caption}}));
    }
    items.push(json!({
        "type": 2,
        "image_item": {
            "media": {
                "encrypt_query_param": encrypted_param,
                "aes_key": encode_aes_key_base64(&aes_key),
                "encrypt_type": 1
            },
            "mid_size": ciphertext.len()
        }
    }));
    let payload = build_media_message(&runtime.target_user_id, &runtime.context_token, items);
    client
        .send_message(&credentials.base_url, &credentials.token, &payload)
        .await
        .map_err(|e| format!("发送微信通知图片失败: {e}"))
}

fn build_reply_guide(
    code: &str,
    project_alias: &str,
    agent_label: &str,
    options: &[String],
) -> String {
    if options.is_empty() {
        return format!(
            "三术 zhi #{code}\n项目：{project_alias}\nAI：{agent_label}\n\n复制并修改：\n#{code}\n项目：{project_alias}\nAI：{agent_label}\n回复：在这里填写回复"
        );
    }
    let option_lines = options
        .iter()
        .enumerate()
        .map(|(index, option)| format!("{}. {option}", (b'A' + index as u8) as char))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "三术 zhi #{code}\n项目：{project_alias}\nAI：{agent_label}\n{option_lines}\n\n复制并修改：\n#{code}\n项目：{project_alias}\nAI：{agent_label}\n选择：A\n补充："
    )
}

fn require_bound_state() -> Result<WechatRuntimeState, String> {
    let runtime = load_wechat_state().map_err(|e| e.to_string())?;
    if runtime.is_bound() {
        Ok(runtime)
    } else {
        Err("微信通知尚未完成绑定".to_string())
    }
}

fn mask_user_id(user_id: &str) -> Option<String> {
    if user_id.is_empty() {
        return None;
    }
    let prefix: String = user_id.chars().take(6).collect();
    let suffix: String = user_id
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Some(format!("{prefix}...{suffix}"))
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
