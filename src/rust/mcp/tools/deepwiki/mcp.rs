use anyhow::Result;
use reqwest::Client;
use rmcp::model::{CallToolResult, Content, ErrorData as McpError, Tool};
use serde_json::json;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use super::types::{DeepwikiAction, DeepwikiRequest, JsonRpcRequest, JsonRpcResponse};
use crate::log_debug;
use crate::log_important;

/// DeepWiki MCP 服务端点
const DEEPWIKI_MCP_ENDPOINT: &str = "https://mcp.deepwiki.com/mcp";

/// DeepWiki 工具实现
pub struct DeepwikiTool;

impl DeepwikiTool {
    /// 执行 DeepWiki 查询
    pub async fn query(request: DeepwikiRequest) -> Result<CallToolResult, McpError> {
        log_important!(
            info,
            "DeepWiki 查询请求: repo={}, action={:?}, question={:?}, path={:?}",
            request.repo,
            request.action,
            request.question,
            request.path
        );

        match Self::execute_query(&request).await {
            Ok(result) => {
                log_important!(info, "DeepWiki 查询成功, 结果长度={}", result.len());
                Ok(CallToolResult {
                    content: vec![Content::text(result)],
                    is_error: Some(false),
                    meta: None,
                    structured_content: None,
                })
            }
            Err(e) => {
                let error_msg = format!("DeepWiki 查询失败: {}", e);
                log_important!(warn, "{}", error_msg);
                Ok(CallToolResult {
                    content: vec![Content::text(error_msg)],
                    is_error: Some(true),
                    meta: None,
                    structured_content: None,
                })
            }
        }
    }

    /// 获取工具定义
    pub fn get_tool_definition() -> Tool {
        let schema = json!({
            "type": "object",
            "properties": {
                "repo": {
                    "type": "string",
                    "description": "GitHub 仓库标识符，格式: owner/repo（如 tauri-apps/tauri、tokio-rs/tokio、vuejs/core）"
                },
                "action": {
                    "type": "string",
                    "enum": ["structure", "content", "ask"],
                    "description": "操作类型：structure（获取文档大纲）、content（读取文档内容）、ask（对仓库提问）"
                },
                "question": {
                    "type": "string",
                    "description": "提问内容（action=ask 时必填）。例如：'这个项目的架构设计是什么？'"
                },
                "path": {
                    "type": "string",
                    "description": "文档路径（action=content 时可选）。从 structure 返回的路径中选取。"
                }
            },
            "required": ["repo", "action"]
        });

        if let serde_json::Value::Object(schema_map) = schema {
            Tool {
                name: Cow::Borrowed("deepwiki"),
                description: Some(Cow::Borrowed(
                    "查询任意公开 GitHub 仓库的 AI 生成文档。支持获取文档结构、阅读文档内容、对仓库提问。免费无需认证。适合了解开源项目架构、设计决策和实现细节。"
                )),
                input_schema: Arc::new(schema_map),
                annotations: None,
                icons: None,
                meta: None,
                output_schema: None,
                title: Some("DeepWiki 仓库文档".to_string()),
            }
        } else {
            panic!("Schema creation failed");
        }
    }

    /// 执行查询（通过 MCP Streamable HTTP 协议）
    async fn execute_query(request: &DeepwikiRequest) -> Result<String> {
        let (tool_name, arguments) = match &request.action {
            DeepwikiAction::Structure => (
                "read_wiki_structure",
                json!({ "repoName": request.repo }),
            ),
            DeepwikiAction::Content => {
                let mut args = json!({ "repoName": request.repo });
                if let Some(path) = &request.path {
                    args["path"] = json!(path);
                }
                ("read_wiki_contents", args)
            }
            DeepwikiAction::Ask => {
                let question = request.question.as_deref().unwrap_or("What is this project about?");
                (
                    "ask_question",
                    json!({
                        "repoName": request.repo,
                        "question": question
                    }),
                )
            }
        };

        log_debug!(
            "DeepWiki MCP 调用: tool={}, args={}",
            tool_name,
            arguments
        );

        // 通过 MCP Streamable HTTP 调用 DeepWiki
        let result = Self::call_mcp_tool(tool_name, arguments).await?;
        Self::format_result(request, &result)
    }

    /// 通过 MCP Streamable HTTP 协议调用远程工具
    async fn call_mcp_tool(
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;

        // 1. 初始化 MCP 会话
        let init_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "sanshu-deepwiki",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        };

        let init_response = client
            .post(DEEPWIKI_MCP_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&init_request)
            .send()
            .await?;

        let init_status = init_response.status();
        // 提取 session ID（如果服务器返回的话）
        let session_id = init_response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        log_debug!(
            "DeepWiki 初始化: status={}, session_id={:?}",
            init_status,
            session_id
        );

        let init_body = init_response.text().await?;
        // 解析可能的 SSE 或 JSON 响应
        let _init_result = Self::parse_response(&init_body)?;

        // 2. 发送 initialized 通知（MCP 协议要求）
        let initialized_notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        let mut notify_req = client
            .post(DEEPWIKI_MCP_ENDPOINT)
            .header("Content-Type", "application/json")
            .json(&initialized_notification);

        if let Some(ref sid) = session_id {
            notify_req = notify_req.header("mcp-session-id", sid);
        }

        let _ = notify_req.send().await;

        // 3. 调用目标工具
        let tool_request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "tools/call",
            params: json!({
                "name": tool_name,
                "arguments": arguments
            }),
        };

        let mut tool_req = client
            .post(DEEPWIKI_MCP_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&tool_request);

        if let Some(ref sid) = session_id {
            tool_req = tool_req.header("mcp-session-id", sid);
        }

        let tool_response = tool_req.send().await?;

        let tool_status = tool_response.status();
        if !tool_status.is_success() {
            let error_body = tool_response.text().await.unwrap_or_default();
            anyhow::bail!(
                "DeepWiki API 请求失败 (HTTP {}): {}",
                tool_status,
                error_body
            );
        }

        let tool_body = tool_response.text().await?;
        let result = Self::parse_response(&tool_body)?;

        // 提取 content 字段
        if let Some(content) = result.get("content") {
            Ok(content.clone())
        } else {
            Ok(result)
        }
    }

    /// 解析 MCP 响应（支持 JSON 和 SSE 两种格式）
    fn parse_response(body: &str) -> Result<serde_json::Value> {
        let trimmed = body.trim();

        // 尝试直接解析为 JSON-RPC 响应
        if let Ok(rpc_response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
            if let Some(error) = rpc_response.error {
                anyhow::bail!("DeepWiki RPC 错误 ({}): {}", error.code, error.message);
            }
            return Ok(rpc_response.result.unwrap_or(json!(null)));
        }

        // 尝试解析 SSE 格式（event: message\ndata: {...}）
        for line in trimmed.lines() {
            let data_line = line.strip_prefix("data: ").unwrap_or(line);
            if let Ok(rpc_response) = serde_json::from_str::<JsonRpcResponse>(data_line) {
                if let Some(error) = rpc_response.error {
                    anyhow::bail!("DeepWiki RPC 错误 ({}): {}", error.code, error.message);
                }
                if rpc_response.result.is_some() {
                    return Ok(rpc_response.result.unwrap());
                }
            }
        }

        // 兜底：尝试解析为纯 JSON
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return Ok(value);
        }

        // 最终兜底：返回原始文本
        Ok(json!({ "text": trimmed }))
    }

    /// 格式化输出结果为 Markdown
    fn format_result(
        request: &DeepwikiRequest,
        result: &serde_json::Value,
    ) -> Result<String> {
        let mut output = String::new();

        // 标题
        output.push_str(&format!("# DeepWiki: {}\n\n", request.repo));

        match &request.action {
            DeepwikiAction::Structure => {
                output.push_str("## 文档结构\n\n");
            }
            DeepwikiAction::Content => {
                if let Some(path) = &request.path {
                    output.push_str(&format!("## 文档: {}\n\n", path));
                } else {
                    output.push_str("## 文档内容\n\n");
                }
            }
            DeepwikiAction::Ask => {
                if let Some(q) = &request.question {
                    output.push_str(&format!("**问题**: {}\n\n---\n\n", q));
                }
            }
        }

        // 提取文本内容
        let text = Self::extract_text_from_content(result);
        if text.is_empty() {
            output.push_str("未找到相关内容。请检查仓库名称是否正确，或尝试其他查询。\n");
        } else {
            output.push_str(&text);
        }

        // 来源标注
        output.push_str(&format!(
            "\n\n---\n🔗 来源: [DeepWiki - {}](https://deepwiki.com/{})\n",
            request.repo, request.repo
        ));

        Ok(output)
    }

    /// 从 MCP content 结构中提取纯文本
    fn extract_text_from_content(value: &serde_json::Value) -> String {
        // MCP content 格式：[{ "type": "text", "text": "..." }]
        if let Some(arr) = value.as_array() {
            let texts: Vec<String> = arr
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .map(String::from)
                })
                .collect();
            if !texts.is_empty() {
                return texts.join("\n\n");
            }
        }

        // 纯文本字符串
        if let Some(s) = value.as_str() {
            return s.to_string();
        }

        // 含 text 字段的对象
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        // 兜底：序列化为 JSON
        serde_json::to_string_pretty(value).unwrap_or_default()
    }
}
