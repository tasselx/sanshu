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

    /// 单字段（prompt / user_reply）最大保存字符数。
    ///
    /// 中文说明（2026-06-11 实证修复）：曾出现单条 user_reply 达 10.1MB（用户在弹窗里
    /// 粘贴整份 spindump 报告），导致单个历史文件膨胀到 9.8MB——而 add_entry 每次都要
    /// 整文件 load+重写，enhance 的历史摘要也会加载它。本模块定位是「仅保存最小必要信息」
    /// （见文件头注释），完整回复本就已实时返回给模型，历史无需复制全文，故写入前截断。
    const MAX_FIELD_CHARS: usize = 4000;

    /// 保留换行的安全截断：超长时取前 max_chars 个字符并附截断标记。
    ///
    /// 中文说明：不复用 utils::safe_truncate_clean——它会把换行压成空格，
    /// 历史里的 markdown 结构（标题/列表）需要保留以便回看。
    fn truncate_field(text: &str, max_chars: usize) -> String {
        let total = text.chars().count();
        if total <= max_chars {
            return text.to_string();
        }
        let truncated: String = text.chars().take(max_chars).collect();
        format!(
            "{}\n…[已截断：原文 {} 字符，仅保留前 {} 字符]",
            truncated, total, max_chars
        )
    }

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

        // 中文说明：写入前截断超长字段，防止单条巨型粘贴撑爆历史文件（实证见 MAX_FIELD_CHARS 注释）
        let entry = ZhiHistoryEntry {
            id: id.clone(),
            request_id: request_id.to_string(),
            prompt: Self::truncate_field(prompt, Self::MAX_FIELD_CHARS),
            user_reply: Self::truncate_field(user_reply, Self::MAX_FIELD_CHARS),
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
