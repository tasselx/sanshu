// Exa AI 搜索工具类型定义

use serde::{Deserialize, Serialize};

/// Exa MCP 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExaRequest {
    /// 搜索查询（search 时必填）
    #[serde(default)]
    pub query: Option<String>,
    /// 操作类型：search（默认）或 contents
    #[serde(default = "default_action")]
    pub action: String,
    /// 搜索类型："auto"（默认）、"neural"、"keyword" 或 "fast"
    #[serde(default)]
    pub search_type: Option<String>,
    /// 内容类别过滤（如 "github"、"news"、"research paper" 等）
    #[serde(default)]
    pub category: Option<String>,
    /// 最大结果数（默认 5，最大 100）
    #[serde(default)]
    pub num_results: Option<u32>,
    /// 域名白名单
    #[serde(default)]
    pub include_domains: Option<Vec<String>>,
    /// 域名黑名单
    #[serde(default)]
    pub exclude_domains: Option<Vec<String>>,
    /// 发布时间下限（ISO 8601，如 "2024-01-01"）
    #[serde(default)]
    pub start_published_date: Option<String>,
    /// 发布时间上限（ISO 8601）
    #[serde(default)]
    pub end_published_date: Option<String>,
    /// 是否返回网页正文（默认 true，与官方 exa-mcp-server 行为一致）
    #[serde(default)]
    pub include_text: Option<bool>,
    /// 正文最大字符数（默认 3000）
    #[serde(default)]
    pub max_characters: Option<u32>,
    /// 实时抓取策略："never"、"fallback"、"always" 或 "preferred"
    #[serde(default)]
    pub livecrawl: Option<String>,
    /// 提取目标 URL（contents 时必填，支持单个字符串或字符串数组）
    #[serde(default)]
    pub urls: Option<serde_json::Value>,
}

fn default_action() -> String {
    "search".to_string()
}

/// Exa 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExaConfig {
    /// API 密钥（必填）
    pub api_key: Option<String>,
    /// API 基础 URL
    pub base_url: String,
}

impl Default for ExaConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: "https://api.exa.ai".to_string(),
        }
    }
}

// ============ Search / Contents API 响应结构 ============
// Exa API 响应字段为 camelCase，统一通过 rename_all 映射

/// Exa Search API 响应（/search 与 /contents 共用该结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaSearchResponse {
    #[serde(default)]
    pub request_id: Option<String>,
    /// auto 模式下服务端实际采用的搜索类型
    #[serde(default)]
    pub resolved_search_type: Option<String>,
    #[serde(default)]
    pub results: Vec<ExaResult>,
    /// 内容提取状态（仅 /contents 返回）
    #[serde(default)]
    pub statuses: Vec<ExaContentStatus>,
    /// 本次请求费用（美元）
    #[serde(default)]
    pub cost_dollars: Option<ExaCostDollars>,
}

/// 搜索/提取结果项
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaResult {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub published_date: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    /// 网页正文（请求 contents.text 时返回）
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub highlights: Vec<String>,
}

/// 内容提取状态项（/contents 端点）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaContentStatus {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// 失败原因（结构不固定，保留原始 JSON）
    #[serde(default)]
    pub error: Option<serde_json::Value>,
}

/// 请求费用明细
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExaCostDollars {
    #[serde(default)]
    pub total: Option<f64>,
}

// ============ 测试连接响应 ============

/// 测试连接响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ExaTestConnectionResponse {
    pub success: bool,
    pub message: String,
    /// 搜索结果预览（可选）
    pub preview: Option<String>,
}
