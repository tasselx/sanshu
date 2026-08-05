//! 微信通知待处理请求的跨进程轻量登记。
//!
//! 每个请求使用独立 JSON 文件，避免多个 MCP/GUI 进程同时改写一个总文件时丢失更新。

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const WECHAT_PENDING_EXPIRY_SECS: i64 = 300;
pub const WECHAT_PENDING_RETENTION_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WechatPendingStatus {
    Pending,
    Replied,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WechatPendingRequest {
    pub request_id: String,
    pub request_code: String,
    pub project_root_path: String,
    pub project_key: String,
    pub project_alias: String,
    pub agent_label: String,
    pub prompt_preview: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: WechatPendingStatus,
    pub completion_source: Option<String>,
}

pub fn normalize_project_path(path: &str) -> String {
    let mut normalized = path.trim().replace('\\', "/");
    if normalized.starts_with("//?/") {
        normalized = normalized[4..].to_string();
    }
    while normalized.ends_with('/') && normalized.len() > 1 {
        normalized.pop();
    }
    if cfg!(windows) {
        normalized.make_ascii_lowercase();
    }
    normalized
}

pub fn default_project_alias(path: &str) -> String {
    let normalized = normalize_project_path(path);
    normalized
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("未命名项目")
        .to_string()
}

pub fn project_alias(path: &str, aliases: &HashMap<String, String>) -> String {
    let key = normalize_project_path(path);
    aliases
        .get(&key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_project_alias(path))
}

fn pending_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("获取系统配置目录失败")?
        .join("sanshu")
        .join("wechat-pending");
    fs::create_dir_all(&dir).context("创建微信待处理目录失败")?;
    Ok(dir)
}

fn pending_path(request_id: &str) -> Result<PathBuf> {
    let safe_id: String = request_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    if safe_id.is_empty() {
        anyhow::bail!("微信待处理请求缺少安全请求 ID");
    }
    Ok(pending_dir()?.join(format!("{safe_id}.json")))
}

fn atomic_write(path: &Path, value: &WechatPendingRequest) -> Result<()> {
    let content = serde_json::to_string_pretty(value).context("序列化微信待处理请求失败")?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, content).context("写入微信待处理临时文件失败")?;
    if path.exists() {
        fs::remove_file(path).context("替换微信待处理文件失败")?;
    }
    fs::rename(&temp_path, path).context("提交微信待处理文件失败")?;
    Ok(())
}

pub fn register_pending(
    request_id: &str,
    request_code: &str,
    project_root_path: &str,
    aliases: &HashMap<String, String>,
    agent_label: &str,
    prompt: &str,
) -> Result<WechatPendingRequest> {
    let now = Utc::now();
    let project_key = normalize_project_path(project_root_path);
    let entry = WechatPendingRequest {
        request_id: request_id.to_string(),
        request_code: request_code.to_string(),
        project_root_path: project_root_path.to_string(),
        project_alias: project_alias(project_root_path, aliases),
        project_key,
        agent_label: if agent_label.trim().is_empty() {
            format!("AI-{request_code}")
        } else {
            agent_label.trim().to_string()
        },
        prompt_preview: prompt.chars().take(240).collect(),
        created_at: now,
        expires_at: now + Duration::seconds(WECHAT_PENDING_EXPIRY_SECS),
        updated_at: now,
        status: WechatPendingStatus::Pending,
        completion_source: None,
    };
    atomic_write(&pending_path(request_id)?, &entry)?;
    Ok(entry)
}

pub fn update_pending(
    request_id: &str,
    status: WechatPendingStatus,
    completion_source: Option<&str>,
) -> Result<()> {
    let path = pending_path(request_id)?;
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path).context("读取微信待处理请求失败")?;
    let mut entry: WechatPendingRequest =
        serde_json::from_str(&content).context("解析微信待处理请求失败")?;
    entry.status = status;
    entry.completion_source = completion_source.map(str::to_string);
    entry.updated_at = Utc::now();
    atomic_write(&path, &entry)
}

pub fn list_pending() -> Result<Vec<WechatPendingRequest>> {
    let dir = pending_dir()?;
    let now = Utc::now();
    let mut entries = Vec::new();
    for item in fs::read_dir(&dir).context("读取微信待处理目录失败")? {
        let item = item.context("读取微信待处理目录项失败")?;
        let path = item.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let mut entry: WechatPendingRequest = match serde_json::from_str(&content) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.status == WechatPendingStatus::Pending && entry.expires_at <= now {
            entry.status = WechatPendingStatus::Expired;
            entry.updated_at = now;
            let _ = atomic_write(&path, &entry);
        }
        if entry.status != WechatPendingStatus::Pending
            && entry.updated_at + Duration::seconds(WECHAT_PENDING_RETENTION_SECS) <= now
        {
            let _ = fs::remove_file(path);
            continue;
        }
        entries.push(entry);
    }
    entries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{default_project_alias, normalize_project_path, project_alias};
    use std::collections::HashMap;

    #[test]
    fn normalize_windows_extended_path() {
        assert_eq!(
            normalize_project_path("//?/E:\\Project\\sanshu\\"),
            "e:/project/sanshu"
        );
    }

    #[test]
    fn default_alias_uses_last_directory() {
        assert_eq!(default_project_alias("E:/Project/sanshu"), "sanshu");
    }

    #[test]
    fn custom_alias_wins() {
        let mut aliases = HashMap::new();
        aliases.insert("e:/project/sanshu".to_string(), "通知项目".to_string());
        assert_eq!(project_alias("E:/Project/sanshu", &aliases), "通知项目");
    }
}
