// zhi 弹窗交互历史管理
// 仅保存最小必要信息（文本摘要与时间），不记录图片原始数据

use anyhow::Result;
use chrono::{DateTime, Utc};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;

use crate::{log_debug, log_important};

/// zhi 交互历史管理器
pub struct ZhiHistoryManager {
    /// 项目根路径的哈希值（用于文件名）
    project_hash: String,
    /// 原始项目路径
    project_path: String,
    /// 最大历史条数
    max_entries: usize,
}

/// 单条 zhi 交互历史
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZhiHistoryEntry {
    /// 唯一ID
    pub id: String,
    /// 请求ID（用于关联一次对话）
    pub request_id: String,
    /// zhi 弹窗展示的消息（AI 提问）
    pub prompt: String,
    /// 用户回复摘要（文本 + 选项摘要）
    pub user_reply: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 来源: "popup" | "telegram"
    pub source: String,
}

/// 历史文件结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ZhiHistoryFile {
    /// 项目路径
    project_path: String,
    /// 历史列表
    entries: VecDeque<ZhiHistoryEntry>,
    /// 最后更新时间
    last_updated: Option<DateTime<Utc>>,
}

impl ZhiHistoryManager {
    /// 最大历史条数默认值
    const DEFAULT_MAX_ENTRIES: usize = 20;

    /// 创建 zhi 历史管理器
    pub fn new(project_path: &str) -> Result<Self> {
        let project_hash = Self::hash_path(project_path);
        Ok(Self {
            project_hash,
            project_path: project_path.to_string(),
            max_entries: Self::DEFAULT_MAX_ENTRIES,
        })
    }

    /// 设置最大历史条数
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 计算路径哈希（使用 ring 库）
    fn hash_path(path: &str) -> String {
        let normalized = path.trim().to_lowercase().replace('\\', "/");
        let mut context = Context::new(&SHA256);
        context.update(normalized.as_bytes());
        let digest = context.finish();
        hex::encode(&digest.as_ref()[..8]) // 取前8字节作为短哈希
    }

    /// 获取历史文件路径
    fn history_file_path(&self) -> PathBuf {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".sanshu")
            .join("zhi_history");

        // 确保目录存在
        let _ = fs::create_dir_all(&data_dir);

        data_dir.join(format!("{}.json", self.project_hash))
    }

    /// 加载历史文件
    fn load_history(&self) -> ZhiHistoryFile {
        let path = self.history_file_path();
        if !path.exists() {
            return ZhiHistoryFile {
                project_path: self.project_path.clone(),
                entries: VecDeque::new(),
                last_updated: None,
            };
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                log_debug!("解析 zhi 历史文件失败: {}", e);
                ZhiHistoryFile {
                    project_path: self.project_path.clone(),
                    entries: VecDeque::new(),
                    last_updated: None,
                }
            }),
            Err(e) => {
                log_debug!("读取 zhi 历史文件失败: {}", e);
                ZhiHistoryFile {
                    project_path: self.project_path.clone(),
                    entries: VecDeque::new(),
                    last_updated: None,
                }
            }
        }
    }

    /// 保存历史文件
    fn save_history(&self, history: &ZhiHistoryFile) -> Result<()> {
        let path = self.history_file_path();
        let content = serde_json::to_string_pretty(history)?;
        fs::write(&path, content)?;
        log_debug!("zhi 历史已保存: {}", path.display());
        Ok(())
    }

    /// 添加一条历史记录
    pub fn add_entry(
        &self,
        request_id: &str,
        prompt: &str,
        user_reply: &str,
        source: &str,
    ) -> Result<String> {
        let mut history = self.load_history();

        // 生成唯一ID
        let id = format!(
            "{}_{}",
            chrono::Utc::now().timestamp_millis(),
            fastrand::u32(..)
        );

        let entry = ZhiHistoryEntry {
            id: id.clone(),
            request_id: request_id.to_string(),
            prompt: prompt.to_string(),
            user_reply: user_reply.to_string(),
            timestamp: Utc::now(),
            source: source.to_string(),
        };

        history.entries.push_back(entry);

        // 保持历史条数在限制内
        while history.entries.len() > self.max_entries {
            history.entries.pop_front();
        }

        history.last_updated = Some(Utc::now());
        self.save_history(&history)?;

        log_important!(
            info,
            "[ZhiHistory] 历史已记录: id={}, source={}",
            id,
            source
        );
        Ok(id)
    }

    /// 获取最近 N 条历史
    pub fn get_recent(&self, count: usize) -> Vec<ZhiHistoryEntry> {
        let history = self.load_history();
        history
            .entries
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// 获取所有历史
    pub fn get_all(&self) -> Vec<ZhiHistoryEntry> {
        let history = self.load_history();
        history.entries.into_iter().collect()
    }

    /// 清空历史
    pub fn clear(&self) -> Result<()> {
        let history = ZhiHistoryFile {
            project_path: self.project_path.clone(),
            entries: VecDeque::new(),
            last_updated: Some(Utc::now()),
        };
        self.save_history(&history)?;
        log_important!(
            info,
            "[ZhiHistory] 历史已清空: project={}",
            self.project_path
        );
        Ok(())
    }
}
