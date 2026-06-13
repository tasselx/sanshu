// Exa AI 搜索 MCP 工具实现
// 支持 Search（神经语义搜索）和 Contents（按 URL 提取正文）双端点
// 请求行为对齐官方 exa-mcp-server（web_search_exa：默认携带正文，maxCharacters 截断）

use anyhow::Result;
use reqwest::Client;
use rmcp::model::{CallToolResult, Content, ErrorData as McpError, Tool};
use serde_json::json;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use super::types::*;
use crate::{log_debug, log_important};

/// 正文默认最大字符数（与官方 exa-mcp-server 默认值一致）
const DEFAULT_MAX_CHARACTERS: u32 = 3000;

/// Exa AI 搜索工具
pub struct ExaTool;

impl ExaTool {
    /// 执行 Exa 工具调用（根据 action 分发到 search 或 contents）
    pub async fn execute(request: ExaRequest) -> Result<CallToolResult, McpError> {
        let action = request.action.to_lowercase();
        log_important!(
            info,
            "Exa 请求: action={}, query={:?}",
            action,
            // 按字符截断，避免多字节字符导致越界
            request
                .query
                .as_deref()
                .map(|s| s.chars().take(100).collect::<String>())
        );

        // 获取配置
        let config = Self::get_config()
            .await
            .map_err(|e| McpError::internal_error(format!("获取 Exa 配置失败: {}", e), None))?;

        // 验证 API Key
        let api_key = config.api_key.as_ref().ok_or_else(|| {
            McpError::internal_error(
                "Exa API Key 未配置。请在设置中配置 Exa API Key（dashboard.exa.ai 注册，新账号赠送 $10 额度）。".to_string(),
                None,
            )
        })?;

        match action.as_str() {
            "search" => Self::search(&config, api_key, &request).await,
            "contents" => Self::contents(&config, api_key, &request).await,
            _ => Err(McpError::invalid_params(
                format!(
                    "未知的 action: {}。支持 'search'（默认）或 'contents'",
                    action
                ),
                None,
            )),
        }
    }

    /// 构建 contents 子对象（控制是否返回正文及截断长度）
    fn build_contents_field(request: &ExaRequest) -> Option<serde_json::Value> {
        // include_text 显式传 false 时不请求正文
        if request.include_text == Some(false) {
            return None;
        }
        let max_chars = request.max_characters.unwrap_or(DEFAULT_MAX_CHARACTERS);
        let mut contents = json!({
            "text": { "maxCharacters": max_chars }
        });
        if let Some(ref livecrawl) = request.livecrawl {
            contents["livecrawl"] = json!(livecrawl);
        }
        Some(contents)
    }

    /// 搜索端点
    async fn search(
        config: &ExaConfig,
        api_key: &str,
        request: &ExaRequest,
    ) -> Result<CallToolResult, McpError> {
        let query = request.query.as_deref().ok_or_else(|| {
            McpError::invalid_params("search 操作需要 query 参数".to_string(), None)
        })?;

        if query.trim().is_empty() {
            return Err(McpError::invalid_params("query 不能为空".to_string(), None));
        }

        let client = Self::create_client()?;
        let url = format!("{}/search", config.base_url);

        // 构建请求体（Exa API 使用 camelCase 字段）
        let mut body = json!({
            "query": query,
            "type": request.search_type.as_deref().unwrap_or("auto"),
            "numResults": request.num_results.unwrap_or(5).min(100),
        });

        // 填充可选参数
        if let Some(ref category) = request.category {
            body["category"] = json!(category);
        }
        if let Some(ref domains) = request.include_domains {
            if !domains.is_empty() {
                body["includeDomains"] = json!(domains);
            }
        }
        if let Some(ref domains) = request.exclude_domains {
            if !domains.is_empty() {
                body["excludeDomains"] = json!(domains);
            }
        }
        if let Some(ref date) = request.start_published_date {
            body["startPublishedDate"] = json!(date);
        }
        if let Some(ref date) = request.end_published_date {
            body["endPublishedDate"] = json!(date);
        }
        if let Some(contents) = Self::build_contents_field(request) {
            body["contents"] = contents;
        }

        log_debug!("Exa Search 请求 URL: {}", url);

        // 发送请求
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let msg = if e.is_timeout() {
                    "Exa 搜索请求超时（30s）".to_string()
                } else if e.is_connect() {
                    "无法连接到 Exa API，请检查网络连接".to_string()
                } else {
                    format!("Exa 搜索请求失败: {}", e)
                };
                McpError::internal_error(msg, None)
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "无法读取错误信息".to_string());
            let msg = Self::format_http_error(status.as_u16(), &error_text);
            return Ok(CallToolResult {
                content: vec![Content::text(msg)],
                is_error: Some(true),
                meta: None,
                structured_content: None,
            });
        }

        // 解析响应
        let response_text = response
            .text()
            .await
            .map_err(|e| McpError::internal_error(format!("读取响应失败: {}", e), None))?;

        let search_response: ExaSearchResponse = serde_json::from_str(&response_text)
            .map_err(|e| McpError::internal_error(format!("解析搜索响应失败: {}", e), None))?;

        // 格式化输出
        let formatted = Self::format_search_result(&search_response);
        log_important!(
            info,
            "Exa Search 完成: results={}, resolved_type={:?}, request_id={:?}",
            search_response.results.len(),
            search_response.resolved_search_type,
            search_response.request_id
        );

        Ok(CallToolResult {
            content: vec![Content::text(formatted)],
            is_error: Some(false),
            meta: None,
            structured_content: None,
        })
    }

    /// 内容提取端点（按 URL 获取网页正文）
    async fn contents(
        config: &ExaConfig,
        api_key: &str,
        request: &ExaRequest,
    ) -> Result<CallToolResult, McpError> {
        let urls = request.urls.as_ref().ok_or_else(|| {
            McpError::invalid_params("contents 操作需要 urls 参数".to_string(), None)
        })?;

        // 将 urls 统一为数组格式
        let urls_array = match urls {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => {
                return Err(McpError::invalid_params(
                    "urls 参数必须是字符串或字符串数组".to_string(),
                    None,
                ));
            }
        };

        if urls_array.is_empty() {
            return Err(McpError::invalid_params("urls 不能为空".to_string(), None));
        }

        let client = Self::create_client()?;
        let url = format!("{}/contents", config.base_url);

        // 构建请求体：contents 端点的正文参数平铺在顶层
        let max_chars = request.max_characters.unwrap_or(DEFAULT_MAX_CHARACTERS);
        let mut body = json!({
            "urls": urls_array,
            "text": { "maxCharacters": max_chars },
        });
        if let Some(ref livecrawl) = request.livecrawl {
            body["livecrawl"] = json!(livecrawl);
        }

        log_debug!("Exa Contents 请求 URL: {}", url);

        // 发送请求
        let response = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| McpError::internal_error(format!("Exa 提取请求失败: {}", e), None))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "无法读取错误信息".to_string());
            let msg = Self::format_http_error(status.as_u16(), &error_text);
            return Ok(CallToolResult {
                content: vec![Content::text(msg)],
                is_error: Some(true),
                meta: None,
                structured_content: None,
            });
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| McpError::internal_error(format!("读取响应失败: {}", e), None))?;

        let contents_response: ExaSearchResponse = serde_json::from_str(&response_text)
            .map_err(|e| McpError::internal_error(format!("解析提取响应失败: {}", e), None))?;

        let formatted = Self::format_contents_result(&contents_response);
        log_important!(
            info,
            "Exa Contents 完成: results={}, statuses={}, request_id={:?}",
            contents_response.results.len(),
            contents_response.statuses.len(),
            contents_response.request_id
        );

        Ok(CallToolResult {
            content: vec![Content::text(formatted)],
            is_error: Some(false),
            meta: None,
            structured_content: None,
        })
    }

    /// 获取工具定义
    pub fn get_tool_definition() -> Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索查询关键词或问题（search 时必填）"
                },
                "action": {
                    "type": "string",
                    "enum": ["search", "contents"],
                    "description": "操作类型：search（神经语义搜索，默认）或 contents（按 URL 提取正文）"
                },
                "search_type": {
                    "type": "string",
                    "enum": ["auto", "neural", "keyword", "fast"],
                    "description": "搜索类型：auto（默认，自动选择）、neural（语义）、keyword（关键词）、fast（低延迟）"
                },
                "category": {
                    "type": "string",
                    "enum": ["company", "research paper", "news", "pdf", "github", "tweet", "personal site", "linkedin profile", "financial report"],
                    "description": "内容类别过滤（如 github 仓库、news 新闻、research paper 论文）"
                },
                "num_results": {
                    "type": "integer",
                    "description": "最大搜索结果数量（默认5，最大100）"
                },
                "include_domains": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "域名白名单（仅包含这些域名的结果）"
                },
                "exclude_domains": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "域名黑名单（排除这些域名的结果）"
                },
                "start_published_date": {
                    "type": "string",
                    "description": "发布时间下限（ISO 8601，如 2024-01-01）"
                },
                "end_published_date": {
                    "type": "string",
                    "description": "发布时间上限（ISO 8601）"
                },
                "include_text": {
                    "type": "boolean",
                    "description": "是否返回网页正文（默认 true）"
                },
                "max_characters": {
                    "type": "integer",
                    "description": "每条结果正文最大字符数（默认3000）"
                },
                "livecrawl": {
                    "type": "string",
                    "enum": ["never", "fallback", "always", "preferred"],
                    "description": "实时抓取策略（默认由服务端决定，需要最新内容时用 always/preferred）"
                },
                "urls": {
                    "description": "提取目标 URL（contents 时必填，支持单个字符串或字符串数组）"
                }
            },
            "required": ["query"]
        });

        if let serde_json::Value::Object(schema_map) = schema {
            Tool {
                name: Cow::Borrowed("exa"),
                description: Some(Cow::Borrowed(
                    "Exa AI 神经语义搜索与网页正文获取工具。search：基于嵌入的实时网页搜索（适合语义化发现网页/论文/GitHub 仓库），默认返回正文；contents：按 URL 提取网页正文。新注册账号赠送 $10 额度。"
                )),
                input_schema: Arc::new(schema_map),
                annotations: None,
                icons: None,
                meta: None,
                output_schema: None,
                title: Some("Exa AI 搜索".to_string()),
            }
        } else {
            panic!("Schema creation failed");
        }
    }

    /// 获取配置
    async fn get_config() -> Result<ExaConfig> {
        let config = crate::config::load_standalone_config()
            .map_err(|e| anyhow::anyhow!("读取配置文件失败: {}", e))?;

        Ok(ExaConfig {
            api_key: config.mcp_config.exa_api_key,
            base_url: "https://api.exa.ai".to_string(),
        })
    }

    /// 创建 HTTP 客户端
    fn create_client() -> Result<Client, McpError> {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| McpError::internal_error(format!("创建 HTTP 客户端失败: {}", e), None))
    }

    /// 格式化单条结果（搜索与提取共用）
    fn format_result_item(output: &mut String, index: usize, result: &ExaResult) {
        let title = result.title.as_deref().unwrap_or("无标题");
        output.push_str(&format!("### {}. {}\n", index + 1, title));
        output.push_str(&format!("**URL:** {}\n", result.url));
        if let Some(ref date) = result.published_date {
            output.push_str(&format!("**发布时间:** {}\n", date));
        }
        if let Some(ref author) = result.author {
            if !author.is_empty() {
                output.push_str(&format!("**作者:** {}\n", author));
            }
        }
        if let Some(score) = result.score {
            output.push_str(&format!("**相关度:** {:.2}\n", score));
        }
        if let Some(ref summary) = result.summary {
            if !summary.is_empty() {
                output.push_str(&format!("\n**摘要:** {}\n", summary));
            }
        }
        if !result.highlights.is_empty() {
            output.push_str("\n**高亮片段:**\n");
            for h in &result.highlights {
                output.push_str(&format!("- {}\n", h));
            }
        }
        if let Some(ref text) = result.text {
            if !text.is_empty() {
                // 正文长度已由请求中的 maxCharacters 在服务端截断
                output.push_str(&format!("\n{}\n", text));
            }
        }
        output.push('\n');
    }

    /// 格式化搜索结果为可读文本
    fn format_search_result(response: &ExaSearchResponse) -> String {
        let mut output = String::new();

        if response.results.is_empty() {
            output.push_str("未找到相关搜索结果。\n");
        } else {
            output.push_str(&format!(
                "## 搜索结果（共 {} 条）\n\n",
                response.results.len()
            ));
            for (i, result) in response.results.iter().enumerate() {
                Self::format_result_item(&mut output, i, result);
            }
        }

        // 元信息
        if let Some(ref resolved) = response.resolved_search_type {
            output.push_str(&format!("\n_搜索类型: {}_", resolved));
        }
        if let Some(ref cost) = response.cost_dollars {
            if let Some(total) = cost.total {
                output.push_str(&format!(" _本次费用: ${:.4}_", total));
            }
        }

        output
    }

    /// 格式化提取结果为可读文本
    fn format_contents_result(response: &ExaSearchResponse) -> String {
        let mut output = String::new();

        // 统计失败项
        let failed: Vec<&ExaContentStatus> = response
            .statuses
            .iter()
            .filter(|s| s.status.as_deref() == Some("error"))
            .collect();

        if response.results.is_empty() && failed.is_empty() {
            output.push_str("未获取到任何提取结果。\n");
            return output;
        }

        if !response.results.is_empty() {
            output.push_str(&format!(
                "## 提取结果（共 {} 条）\n\n",
                response.results.len()
            ));
            for (i, result) in response.results.iter().enumerate() {
                Self::format_result_item(&mut output, i, result);
            }
        }

        if !failed.is_empty() {
            output.push_str(&format!("## 提取失败（{} 条）\n\n", failed.len()));
            for status in failed {
                let id = status.id.as_deref().unwrap_or("未知 URL");
                let error = status
                    .error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "未知错误".to_string());
                output.push_str(&format!("- {} — {}\n", id, error));
            }
        }

        if let Some(ref cost) = response.cost_dollars {
            if let Some(total) = cost.total {
                output.push_str(&format!("\n_本次费用: ${:.4}_", total));
            }
        }

        output
    }

    /// 格式化 HTTP 错误信息
    fn format_http_error(status: u16, body: &str) -> String {
        match status {
            401 => "❌ Exa API Key 无效或已过期。请在设置中检查并更新 API Key。".to_string(),
            402 => "❌ Exa 账户额度已耗尽。请前往 dashboard.exa.ai 充值。".to_string(),
            429 => "❌ Exa API 请求频率超限。请稍后重试。".to_string(),
            _ => {
                // 按字符截断，避免多字节字符导致越界
                let preview: String = body.chars().take(300).collect();
                format!("❌ Exa API 错误 (HTTP {}): {}", status, preview)
            }
        }
    }
}
