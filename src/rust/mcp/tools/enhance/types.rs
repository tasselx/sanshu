// 提示词增强模块的类型定义

use serde::{Deserialize, Serialize};
use std::sync::{atomic::AtomicBool, Arc};

/// 增强请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhanceRequest {
    /// 要增强的原始提示词
    pub prompt: String,
    /// 原始用户输入（用于历史兜底与记录；可选）
    /// 中文注释：前端的 prompt 可能包含“规则/上下文拼接”，这里保留更干净的原始输入
    #[serde(default)]
    pub original_prompt: Option<String>,
    /// 项目根路径（用于加载 blob 上下文）
    #[serde(default)]
    pub project_root_path: Option<String>,
    /// 当前文件路径（可选，提供更精确的上下文）
    #[serde(default)]
    pub current_file_path: Option<String>,
    /// 是否包含对话历史
    #[serde(default = "default_include_history")]
    pub include_history: bool,
    /// 指定参与增强的历史记录 ID（为空时使用默认最近历史）
    #[serde(default)]
    pub selected_history_ids: Option<Vec<String>>,
    /// 请求 ID（用于前后端与流式事件关联）
    #[serde(default)]
    pub request_id: Option<String>,
    /// 取消标记（仅后端内部使用，前端不可见）
    #[serde(skip)]
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

fn default_include_history() -> bool {
    true
}

/// 增强响应结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhanceResponse {
    /// 增强后的提示词
    pub enhanced_prompt: String,
    /// 原始提示词
    pub original_prompt: String,
    /// 是否成功
    pub success: bool,
    /// 错误信息（如有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 使用的 blob 数量
    #[serde(default)]
    pub blob_count: usize,
    /// 使用的对话历史条数
    #[serde(default)]
    pub history_count: usize,
    /// 历史加载失败原因（用于区分“历史为空”与“历史加载失败”）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_load_error: Option<String>,
    /// 是否启用了“历史为空兜底”（即使 history_count 为 0，也会提供临时上下文）
    #[serde(default)]
    pub history_fallback_used: bool,
    /// 请求传入的项目根路径（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root_path: Option<String>,
    /// 实际匹配到的项目根路径（用于确认上下文来源）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_source_root: Option<String>,
    /// 请求 ID（用于前后端关联）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// 流式增强事件（通过 Tauri Event 推送给前端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhanceStreamEvent {
    /// 请求 ID（用于并发请求关联）
    pub request_id: String,
    /// 事件类型: "chunk" | "complete" | "error"
    pub event_type: String,
    /// 流式文本块（仅 chunk 类型有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk: Option<String>,
    /// 累积的完整文本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accumulated_text: Option<String>,
    /// 提取的增强结果（仅 complete 类型有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhanced_prompt: Option<String>,
    /// 错误信息（仅 error 类型有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 进度百分比（0-100）
    #[serde(default)]
    pub progress: u8,
}

impl EnhanceStreamEvent {
    /// 创建文本块事件
    pub fn chunk(request_id: &str, text: &str, accumulated: &str, progress: u8) -> Self {
        Self {
            request_id: request_id.to_string(),
            event_type: "chunk".to_string(),
            chunk: Some(text.to_string()),
            accumulated_text: Some(accumulated.to_string()),
            enhanced_prompt: None,
            error: None,
            progress,
        }
    }

    /// 创建完成事件
    pub fn complete(request_id: &str, enhanced_prompt: &str, full_text: &str) -> Self {
        Self {
            request_id: request_id.to_string(),
            event_type: "complete".to_string(),
            chunk: None,
            accumulated_text: Some(full_text.to_string()),
            enhanced_prompt: Some(enhanced_prompt.to_string()),
            error: None,
            progress: 100,
        }
    }

    /// 创建错误事件
    pub fn error(request_id: &str, message: &str) -> Self {
        Self {
            request_id: request_id.to_string(),
            event_type: "error".to_string(),
            chunk: None,
            accumulated_text: None,
            enhanced_prompt: None,
            error: Some(message.to_string()),
            progress: 0,
        }
    }
}

/// 对话历史消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatHistoryMessage {
    /// 角色: "user" | "assistant"
    pub role: String,
    /// 消息内容
    pub content: String,
    /// 时间戳
    pub timestamp: String,
}

/// chat-stream API 的对话历史节点格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatHistoryRequestNode {
    pub id: i32,
    #[serde(rename = "type")]
    pub node_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_node: Option<TextNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextNode {
    pub content: String,
}

/// chat-stream API 的历史条目格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatHistoryEntry {
    pub request_message: String,
    pub request_id: String,
    pub request_nodes: Vec<ChatHistoryRequestNode>,
    pub response_nodes: Vec<ChatHistoryResponseNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatHistoryResponseNode {
    pub id: i32,
    #[serde(rename = "type")]
    pub node_type: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<serde_json::Value>,
}
