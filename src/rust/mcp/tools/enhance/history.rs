// 对话历史管理模块
// 持久化存储用户与弹窗的交互历史，供提示词增强时使用

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ring::digest::{Context as ShaContext, SHA256};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::mcp::utils::safe_truncate;
use crate::{log_debug, log_important};

/// 对话历史管理器
pub struct ChatHistoryManager {
    /// 项目根路径的哈希值（用于文件名）
    project_hash: String,
    /// 旧规则 hash（用于兼容历史文件）
    legacy_hashes: Vec<String>,
    /// 原始项目路径
    project_path: String,
    /// 最大历史条数
    max_entries: usize,
}

/// 单条对话历史
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEntry {
    /// 唯一ID
    pub id: String,
    /// 用户输入
    pub user_input: String,
    /// AI响应摘要（仅保存前500字符）
    pub ai_response_summary: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 来源: "popup" | "mcp" | "telegram"
    #[serde(default)]
    pub source: String,
}

/// 历史文件结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ChatHistoryFile {
    /// 项目路径
    #[serde(default)]
    project_path: String,
    /// 对话历史列表
    #[serde(default)]
    entries: VecDeque<ChatEntry>,
    /// 最后更新时间
    #[serde(default)]
    last_updated: Option<DateTime<Utc>>,
}

impl ChatHistoryManager {
    /// 最大历史条数默认值
    const DEFAULT_MAX_ENTRIES: usize = 20;

    /// 创建对话历史管理器
    pub fn new(project_path: &str) -> Result<Self> {
        // 中文注释：新 hash 规则会清理 Windows 长路径前缀与末尾斜杠，避免同一项目出现多个 hash 文件
        let project_hash = Self::hash_path_v2(project_path);

        // 中文注释：兼容旧 hash 规则（历史文件可能已经以旧规则落盘）
        let legacy_hashes = Self::legacy_hashes(project_path, &project_hash);
        Ok(Self {
            project_hash,
            legacy_hashes,
            project_path: project_path.to_string(),
            max_entries: Self::DEFAULT_MAX_ENTRIES,
        })
    }

    /// 设置最大历史条数
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 旧规则：仅 trim + 小写 + 反斜杠转正斜杠
    fn normalize_path_v1(path: &str) -> String {
        path.trim().to_lowercase().replace('\\', "/")
    }

    /// 新规则：清理 Windows 长路径前缀 + 统一分隔符 + 去除末尾斜杠 + 小写
    fn normalize_path_v2(path: &str) -> String {
        let mut p = path.trim().to_string();

        // 处理 \\?\ 前缀（Windows 扩展路径语法）
        if p.starts_with(r"\\?\") {
            p = p[4..].to_string();
        }
        // 处理 //?/ 前缀（canonicalize 等场景可能返回）
        if p.starts_with("//?/") {
            p = p[4..].to_string();
        }

        // 统一使用正斜杠
        p = p.replace('\\', "/");

        // 再次处理 //?/（某些路径先以 \\?\\ 开头，替换后会变成 //?/）
        if p.starts_with("//?/") {
            p = p[4..].to_string();
        }

        // 去除末尾斜杠，避免同一路径 hash 不一致
        p = p.trim_end_matches('/').to_string();

        p.to_lowercase()
    }

    /// 计算短 hash（取 SHA256 前 8 字节）
    fn sha256_short_hex(input: &str) -> String {
        let mut ctx = ShaContext::new(&SHA256);
        ctx.update(input.as_bytes());
        let digest = ctx.finish();
        hex::encode(&digest.as_ref()[..8])
    }

    fn hash_path_v2(path: &str) -> String {
        Self::sha256_short_hex(&Self::normalize_path_v2(path))
    }

    /// 生成旧 hash 列表（去重且排除与 v2 相同的 hash）
    fn legacy_hashes(project_path: &str, v2_hash: &str) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();

        // 旧规则原样
        let v1_norm = Self::normalize_path_v1(project_path);
        candidates.push(Self::sha256_short_hex(&v1_norm));

        // 旧规则 + 去末尾斜杠（覆盖用户输入包含尾斜杠的情况）
        let v1_trim = v1_norm.trim_end_matches('/').to_string();
        candidates.push(Self::sha256_short_hex(&v1_trim));

        // 中文注释：兼容“历史文件曾用 //?/ 前缀路径参与 hash”的旧情况
        // 典型场景：某些路径来自 canonicalize 后携带 \\?\ 或 //?/ 前缀，旧规则会把它们纳入 hash
        let v2_norm = Self::normalize_path_v2(project_path);
        if !v2_norm.is_empty() {
            // drive path: e:/xxx -> //?/e:/xxx
            // unc path: //server/share -> //?/unc/server/share
            let prefixed = if v2_norm.starts_with("//") {
                let without = v2_norm.trim_start_matches('/');
                format!("//?/unc/{}", without)
            } else {
                format!("//?/{}", v2_norm)
            };
            candidates.push(Self::sha256_short_hex(&prefixed));
            // 兼容旧规则未去除末尾斜杠的情况
            candidates.push(Self::sha256_short_hex(&(prefixed + "/")));
        }

        // 去重并移除 v2 hash
        let mut seen: HashSet<String> = HashSet::new();
        candidates
            .into_iter()
            .filter(|h| h != v2_hash)
            .filter(|h| seen.insert(h.clone()))
            .collect()
    }

    /// 获取历史目录
    fn history_dir() -> PathBuf {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".sanshu")
            .join("chat_history");
        // 确保目录存在
        let _ = fs::create_dir_all(&data_dir);
        data_dir
    }

    fn history_file_path_for_hash(hash: &str) -> PathBuf {
        Self::history_dir().join(format!("{}.json", hash))
    }

    /// 新规则文件路径（v2 hash）
    fn primary_history_file_path(&self) -> PathBuf {
        Self::history_file_path_for_hash(&self.project_hash)
    }

    /// 返回所有可能的历史文件路径（新规则优先）
    fn history_file_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        out.push(Self::history_file_path_for_hash(&self.project_hash));
        for h in &self.legacy_hashes {
            out.push(Self::history_file_path_for_hash(h));
        }
        out
    }

    fn empty_history(&self) -> ChatHistoryFile {
        ChatHistoryFile {
            project_path: self.project_path.clone(),
            entries: VecDeque::new(),
            last_updated: None,
        }
    }

    /// 加载单个历史文件（失败时返回 Err，便于上层区分“空/失败”）
    fn load_history_from_path(&self, path: &Path) -> Result<ChatHistoryFile> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("读取对话历史文件失败: {}", path.display()))?;
        let parsed: ChatHistoryFile = serde_json::from_str(&content)
            .with_context(|| format!("解析对话历史文件失败: {}", path.display()))?;
        Ok(parsed)
    }

    /// 加载并合并历史（兼容旧 hash 文件）
    ///
    /// - **无文件**：返回空历史 (Ok)\n
    /// - **有文件但全部读取/解析失败**：返回 Err（用于 UI 明确提示）\n
    /// - **部分成功**：合并成功结果并忽略失败文件（仅 debug 日志）
    fn load_history_merged(&self) -> Result<ChatHistoryFile> {
        let mut found_any_file = false;
        let mut loaded_files: Vec<ChatHistoryFile> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for path in self.history_file_paths() {
            if !path.exists() {
                continue;
            }
            found_any_file = true;
            match self.load_history_from_path(&path) {
                Ok(file) => loaded_files.push(file),
                Err(e) => {
                    log_debug!("{}", e);
                    errors.push(e.to_string());
                }
            }
        }

        if loaded_files.is_empty() {
            if !found_any_file {
                return Ok(self.empty_history());
            }
            // 有文件但都失败：显式返回 Err，便于前端区分“空/失败”
            let msg = errors
                .first()
                .cloned()
                .unwrap_or_else(|| "对话历史文件读取/解析失败".to_string());
            return Err(anyhow::anyhow!(msg));
        }

        // 合并 entries（按 id 去重，按 timestamp 排序）
        let mut map: HashMap<String, ChatEntry> = HashMap::new();
        for file in loaded_files {
            for entry in file.entries {
                map.entry(entry.id.clone()).or_insert(entry);
            }
        }

        let mut entries: Vec<ChatEntry> = map.into_values().collect();
        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));

        Ok(ChatHistoryFile {
            project_path: self.project_path.clone(),
            entries: VecDeque::from(entries),
            last_updated: Some(Utc::now()),
        })
    }

    /// 保存历史文件到指定路径
    fn save_history_to_path(&self, path: &Path, history: &ChatHistoryFile) -> Result<()> {
        let content = serde_json::to_string_pretty(history)?;
        fs::write(path, content)
            .with_context(|| format!("写入对话历史文件失败: {}", path.display()))?;
        log_debug!("对话历史已保存: {}", path.display());
        Ok(())
    }

    /// 保存历史文件到 v2 hash 路径
    fn save_history_v2(&self, history: &ChatHistoryFile) -> Result<()> {
        let path = self.primary_history_file_path();
        self.save_history_to_path(&path, history)
    }

    /// 添加一条对话记录
    pub fn add_entry(&self, user_input: &str, ai_response: &str, source: &str) -> Result<String> {
        // 中文注释：写入时只维护 v2 文件；读取时会合并展示（兼容旧文件）
        let primary_path = self.primary_history_file_path();
        let mut history = if primary_path.exists() {
            match self.load_history_from_path(&primary_path) {
                Ok(h) => h,
                Err(e) => {
                    log_debug!("加载对话历史失败，将创建新历史文件: {}", e);
                    self.empty_history()
                }
            }
        } else {
            self.empty_history()
        };

        // 生成唯一ID
        let id = format!(
            "{}_{}",
            chrono::Utc::now().timestamp_millis(),
            fastrand::u32(..)
        );

        // 截取AI响应摘要（最多500字符）
        // 使用 safe_truncate 确保在 UTF-8 字符边界安全截断，避免多字节字符被截断导致 panic
        let ai_summary = safe_truncate(ai_response, 500);

        let entry = ChatEntry {
            id: id.clone(),
            user_input: user_input.to_string(),
            ai_response_summary: ai_summary,
            timestamp: Utc::now(),
            source: source.to_string(),
        };

        history.entries.push_back(entry);

        // 保持历史条数在限制内
        while history.entries.len() > self.max_entries {
            history.entries.pop_front();
        }

        history.last_updated = Some(Utc::now());
        self.save_history_v2(&history)?;

        log_important!(info, "对话历史已记录: id={}, source={}", id, source);
        Ok(id)
    }

    /// 获取最近N条对话历史
    pub fn get_recent(&self, count: usize) -> Result<Vec<ChatEntry>> {
        let history = self.load_history_merged()?;
        let entries: Vec<ChatEntry> = history.entries.into_iter().collect();
        if entries.len() <= count {
            return Ok(entries);
        }
        Ok(entries
            .into_iter()
            .rev()
            .take(count)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect())
    }

    /// 获取最近N条对话历史（别名，便于外部调用语义统一）
    pub fn get_recent_entries(&self, count: usize) -> Result<Vec<ChatEntry>> {
        self.get_recent(count)
    }

    /// 获取所有对话历史
    pub fn get_all(&self) -> Result<Vec<ChatEntry>> {
        let history = self.load_history_merged()?;
        Ok(history.entries.into_iter().collect())
    }

    /// 根据 ID 列表获取历史（保持传入顺序）
    pub fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChatEntry>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let history = self.load_history_merged()?;
        let mut map: HashMap<String, ChatEntry> = HashMap::new();
        for entry in history.entries {
            map.insert(entry.id.clone(), entry);
        }

        Ok(ids.iter().filter_map(|id| map.get(id).cloned()).collect())
    }

    /// 清空对话历史
    pub fn clear(&self) -> Result<()> {
        let history = ChatHistoryFile {
            project_path: self.project_path.clone(),
            entries: VecDeque::new(),
            last_updated: Some(Utc::now()),
        };

        // 中文注释：清空所有可能的历史文件（新旧 hash），避免“清空后仍然有历史”
        let mut wrote_any = false;
        for path in self.history_file_paths() {
            if path.exists() {
                self.save_history_to_path(&path, &history)?;
                wrote_any = true;
            }
        }
        if !wrote_any {
            // 无文件时也写一份 v2 空文件，保证后续读取稳定
            self.save_history_v2(&history)?;
        }
        log_important!(info, "对话历史已清空: project={}", self.project_path);
        Ok(())
    }

    /// 删除指定ID的历史条目
    pub fn remove_entry(&self, entry_id: &str) -> Result<bool> {
        let mut removed_any = false;

        // 中文注释：尽量从所有可能的历史文件中删除，避免旧文件残留导致“删除后又出现”
        for path in self.history_file_paths() {
            if !path.exists() {
                continue;
            }
            match self.load_history_from_path(&path) {
                Ok(mut history) => {
                    let original_len = history.entries.len();
                    history.entries.retain(|e| e.id != entry_id);
                    if history.entries.len() < original_len {
                        history.last_updated = Some(Utc::now());
                        self.save_history_to_path(&path, &history)?;
                        removed_any = true;
                    }
                }
                Err(e) => {
                    // 删除失败不阻断主流程，但输出 debug 方便排查
                    log_debug!("删除历史条目时读取文件失败: {}", e);
                }
            }
        }

        Ok(removed_any)
    }

    /// 转换为 chat-stream API 所需的格式
    pub fn to_api_format(&self, count: usize) -> Result<Vec<super::types::ChatHistoryEntry>> {
        let entries = self.get_recent(count)?;

        Ok(entries
            .into_iter()
            .map(|entry| super::types::ChatHistoryEntry {
                request_message: entry.user_input.clone(),
                request_id: entry.id.clone(),
                request_nodes: vec![super::types::ChatHistoryRequestNode {
                    id: 0,
                    node_type: 0,
                    text_node: Some(super::types::TextNode {
                        content: entry.user_input,
                    }),
                }],
                response_nodes: vec![super::types::ChatHistoryResponseNode {
                    id: 1,
                    node_type: 0,
                    content: Some(entry.ai_response_summary),
                    tool_use: None,
                    thinking: None,
                    billing_metadata: None,
                    metadata: None,
                    token_usage: None,
                }],
            })
            .collect())
    }

    /// 按指定 ID 转换为 chat-stream API 格式
    pub fn to_api_format_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<super::types::ChatHistoryEntry>> {
        let entries = self.get_by_ids(ids)?;

        Ok(entries
            .into_iter()
            .map(|entry| super::types::ChatHistoryEntry {
                request_message: entry.user_input.clone(),
                request_id: entry.id.clone(),
                request_nodes: vec![super::types::ChatHistoryRequestNode {
                    id: 0,
                    node_type: 0,
                    text_node: Some(super::types::TextNode {
                        content: entry.user_input,
                    }),
                }],
                response_nodes: vec![super::types::ChatHistoryResponseNode {
                    id: 1,
                    node_type: 0,
                    content: Some(entry.ai_response_summary),
                    tool_use: None,
                    thinking: None,
                    billing_metadata: None,
                    metadata: None,
                    token_usage: None,
                }],
            })
            .collect())
    }
}
