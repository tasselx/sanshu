use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

// 微信历史与登录凭据分文件保存，避免查询记录时接触 token 等敏感字段。
const MAX_HISTORY_ENTRIES: usize = 200;
const MAX_CONTENT_CHARS: usize = 4000;
static HISTORY_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WechatHistoryEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub direction: String,
    pub kind: String,
    pub request_code: Option<String>,
    pub content: String,
    pub status: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WechatHistoryFile {
    #[serde(default)]
    entries: VecDeque<WechatHistoryEntry>,
}

pub fn history_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("获取系统配置目录失败")?
        .join("sanshu");
    fs::create_dir_all(&config_dir).context("创建三术配置目录失败")?;
    Ok(config_dir.join("wechat-history.json"))
}

fn load_file(path: &PathBuf) -> Result<WechatHistoryFile> {
    if !path.exists() {
        return Ok(WechatHistoryFile::default());
    }
    let content = fs::read_to_string(path).context("读取微信聊天记录失败")?;
    serde_json::from_str(&content).context("解析微信聊天记录失败")
}

fn save_file(path: &PathBuf, history: &WechatHistoryFile) -> Result<()> {
    let content = serde_json::to_string_pretty(history).context("序列化微信聊天记录失败")?;
    fs::write(path, content).context("保存微信聊天记录失败")
}

fn truncate_content(content: &str) -> String {
    let mut chars = content.chars();
    let truncated: String = chars.by_ref().take(MAX_CONTENT_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}\n…（内容已截断）")
    } else {
        truncated
    }
}

pub fn add_history_entry(
    direction: &str,
    kind: &str,
    request_code: Option<&str>,
    content: &str,
    status: &str,
) -> Result<WechatHistoryEntry> {
    let _guard = HISTORY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("微信聊天记录锁状态异常"))?;
    let path = history_path()?;
    let mut history = load_file(&path)?;
    let entry = WechatHistoryEntry {
        id: Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        direction: direction.to_string(),
        kind: kind.to_string(),
        request_code: request_code.map(str::to_string),
        content: truncate_content(content),
        status: status.to_string(),
    };
    history.entries.push_back(entry.clone());
    while history.entries.len() > MAX_HISTORY_ENTRIES {
        history.entries.pop_front();
    }
    save_file(&path, &history)?;
    Ok(entry)
}

pub fn get_history(limit: usize) -> Result<Vec<WechatHistoryEntry>> {
    let _guard = HISTORY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("微信聊天记录锁状态异常"))?;
    let history = load_file(&history_path()?)?;
    let limit = limit.clamp(1, MAX_HISTORY_ENTRIES);
    Ok(history.entries.into_iter().rev().take(limit).collect())
}

pub fn clear_history() -> Result<()> {
    let _guard = HISTORY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("微信聊天记录锁状态异常"))?;
    save_file(&history_path()?, &WechatHistoryFile::default())
}
