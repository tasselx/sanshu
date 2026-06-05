use serde::{Deserialize, Serialize};

/// DeepWiki 操作类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepwikiAction {
    /// 获取仓库文档结构（目录大纲）
    Structure,
    /// 读取指定主题的文档内容
    Content,
    /// 对仓库提问（AI 回答）
    Ask,
}

/// DeepWiki MCP 工具请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepwikiRequest {
    /// GitHub 仓库，格式: owner/repo（如 "tauri-apps/tauri"）
    pub repo: String,
    /// 操作类型
    pub action: DeepwikiAction,
    /// 提问内容（action=ask 时必填）
    #[serde(default)]
    pub question: Option<String>,
    /// 文档路径/主题（action=content 时可选，不传则返回首页）
    #[serde(default)]
    pub path: Option<String>,
}

/// DeepWiki MCP JSON-RPC 请求体
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    pub params: serde_json::Value,
}

/// DeepWiki MCP JSON-RPC 响应体
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 错误
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}
