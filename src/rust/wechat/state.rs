use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredWechatCredentials {
    pub token: String,
    pub base_url: String,
    pub account_id: String,
    pub bot_user_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WechatRuntimeState {
    pub credentials: Option<StoredWechatCredentials>,
    pub target_user_id: String,
    pub context_token: String,
    pub cursor: String,
}

impl WechatRuntimeState {
    pub fn is_bound(&self) -> bool {
        self.credentials.is_some()
            && !self.target_user_id.trim().is_empty()
            && !self.context_token.trim().is_empty()
    }
}

fn state_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("获取系统配置目录失败")?
        .join("sanshu");
    fs::create_dir_all(&config_dir).context("创建三术配置目录失败")?;
    Ok(config_dir.join("wechat-state.json"))
}

pub fn load_wechat_state() -> Result<WechatRuntimeState> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(WechatRuntimeState::default());
    }
    let content = fs::read_to_string(&path).context("读取微信状态失败")?;
    serde_json::from_str(&content).context("解析微信状态失败")
}

pub fn save_wechat_state(state: &WechatRuntimeState) -> Result<()> {
    let path = state_path()?;
    let content = serde_json::to_string_pretty(state).context("序列化微信状态失败")?;
    fs::write(path, content).context("保存微信状态失败")
}

pub fn clear_wechat_state() -> Result<()> {
    let path = state_path()?;
    if path.exists() {
        fs::remove_file(path).context("清除微信状态失败")?;
    }
    Ok(())
}
