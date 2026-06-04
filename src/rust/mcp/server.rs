use anyhow::Result;
use rmcp::model::*;
use rmcp::{
    model::ErrorData as McpError,
    service::{RequestContext, ServerInitializeError},
    transport::stdio,
    RoleServer, ServerHandler, ServiceExt,
};
use std::collections::HashMap;
use std::time::Instant;

use super::tools::{
    Context7Tool, EnhanceTool, IconTool, InteractionTool, MemoryTool, SkillsTool, SouTool,
    TavilyTool, UiuxTool,
};
use super::types::{JiyiRequest, SkillRunRequest, TuRequest, ZhiRequest};
use crate::config::load_standalone_config;
use crate::mcp::tools::context7::types::Context7Request;
use crate::mcp::tools::enhance::mcp::EnhanceMcpRequest;
use crate::mcp::tools::tavily::types::TavilyRequest;
use crate::mcp::utils::generate_request_id;
use crate::mcp::utils::safe_truncate_clean;
use crate::{log_debug, log_important};

const WINDSURF_ZHI_ALIAS: &str = "work_note";
const MCP_PROFILE_ENV: &str = "SANSHU_MCP_PROFILE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpClientProfile {
    Standard,
    Windsurf,
}

impl McpClientProfile {
    fn detect() -> Self {
        if let Ok(raw) = std::env::var(MCP_PROFILE_ENV) {
            match raw.trim().to_ascii_lowercase().as_str() {
                "windsurf" | "compat" | "alias" => return Self::Windsurf,
                "standard" | "zhi" => return Self::Standard,
                "" => {}
                other => {
                    log_important!(
                        warn,
                        "未知 MCP profile: {}={}，将继续按可执行文件名自动判断",
                        MCP_PROFILE_ENV,
                        other
                    );
                }
            }
        }

        // 中文说明：`sanshu` 是面向不支持中文命令的 MCP 客户端的 ASCII 入口。
        if std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .map(|stem| stem.eq_ignore_ascii_case("sanshu"))
            .unwrap_or(false)
        {
            return Self::Windsurf;
        }

        Self::Standard
    }

    fn is_windsurf(self) -> bool {
        matches!(self, Self::Windsurf)
    }
}

#[derive(Clone)]
pub struct ZhiServer {
    enabled_tools: HashMap<String, bool>,
    mcp_profile: McpClientProfile,
}

impl Default for ZhiServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ZhiServer {
    pub fn new() -> Self {
        // 尝试加载配置，如果失败则使用默认配置
        let enabled_tools = match load_standalone_config() {
            Ok(config) => config.mcp_config.tools,
            Err(e) => {
                log_important!(warn, "无法加载配置文件，使用默认工具配置: {}", e);
                crate::config::default_mcp_tools()
            }
        };
        let mcp_profile = McpClientProfile::detect();
        log_important!(info, "MCP profile: {:?}", mcp_profile);

        Self {
            enabled_tools,
            mcp_profile,
        }
    }

    /// 检查工具是否启用 - 动态读取最新配置
    fn is_tool_enabled(&self, tool_name: &str) -> bool {
        // 每次都重新读取配置，确保获取最新状态
        match load_standalone_config() {
            Ok(config) => {
                let enabled = config
                    .mcp_config
                    .tools
                    .get(tool_name)
                    .copied()
                    .unwrap_or(true);
                log_debug!("工具 {} 当前状态: {}", tool_name, enabled);
                enabled
            }
            Err(e) => {
                log_important!(warn, "读取配置失败，使用缓存状态: {}", e);
                // 如果读取失败，使用缓存的配置
                self.enabled_tools.get(tool_name).copied().unwrap_or(true)
            }
        }
    }

    fn zhi_public_tool_name(&self) -> &'static str {
        if self.mcp_profile.is_windsurf() {
            WINDSURF_ZHI_ALIAS
        } else {
            "zhi"
        }
    }

    fn zhi_public_title(&self) -> &'static str {
        if self.mcp_profile.is_windsurf() {
            "Work Note"
        } else {
            "代码审阅记录"
        }
    }

    fn zhi_public_description(&self) -> &'static str {
        // 中文说明：把「强制交互 / 不可结束对话」硬约束内嵌到 description，
        // 这样即使 MCP 客户端没有加载 sanshu 提示词模板，AI 也能从工具说明里看到，
        // 避免因为弹窗取消/空响应/超时把对话提前结束、导致客户端开新 request。
        // 第一句开门见山声明「本工具=向用户提问/确认的唯一通道」：上下文被压缩、
        // sanshu 规则掉出有效上下文时，模型仍能仅凭描述首句认出这是提问工具，
        // 而不是退回到客户端原生的「ask questions」（治标缓解，配合方案A 一起用）。
        "向用户提问、确认方案、收尾确认并等待用户回复的【唯一交互工具】：\
         任何需要用户决策、澄清、方案确认或完成确认的场景，都必须调用本工具，\
         不要在正文里列选项，也不要改用其它提问方式。\
         本工具记录方案摘要、候选项与处理结果，返回结构化数据。方案选择场景应提供候选项；\
         已有候选项时系统会自动追加“其他：自定义要求”兜底。\n\
         【强制交互约束】\n\
         1) 任何方案确认、计划变更、任务收尾必须通过本工具向用户展示并等待回复；\n\
         2) 未收到用户明确的「完成/结束/不需要再问」类指令前，禁止主动结束本轮对话；\n\
         3) 【保活·最重要】若本次返回含「用户仍在思考中」「本次等待已达上限」「弹窗仍开着」\
         「请再次调用」「继续等待」「用户暂未给出回应」「用户未提供任何内容」等任一字样，\
         这是正常的防超时保活信号（弹窗还开着、用户没结束），你必须**立即无条件再次调用本工具**\
         继续等待，禁止把它当作失败或结束理由，禁止输出任何收尾/告别/总结类文本作为本轮最后一条消息；\n\
         4) 若本工具返回错误（参数错误、弹窗失败等），先按错误提示修正后立刻重试，\
         不要因一次失败就结束对话。"
    }

    fn is_zhi_entry(tool_name: &str) -> bool {
        tool_name == "zhi" || tool_name == WINDSURF_ZHI_ALIAS
    }
}

impl ServerHandler for ZhiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            // 中文说明：MCP 初始化元数据也可能被客户端侧规则扫描，这里保持中性表述。
            server_info: Implementation {
                name: "sanshu-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                title: None,
                website_url: None,
            },
            instructions: Some(
                "Sanshu MCP 服务，提供项目记录、上下文检索与辅助处理能力。".to_string(),
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ServerInfo, McpError> {
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        use std::borrow::Cow;
        use std::sync::Arc;

        let mut tools = Vec::new();

        // 三术工具始终可用（必需工具）
        // 中文说明：对外 schema 使用中性字段名与描述，降低部分 MCP 客户端的内容级误判风险。
        let zhi_tool_name = self.zhi_public_tool_name();
        let zhi_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "brief": {
                    "type": "string",
                    "description": "审阅内容或方案摘要"
                },
                "choices": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "候选处理项列表（可选）。方案选择或确认场景建议提供 2-5 个明确选项；已有候选项时系统会自动追加“其他：自定义要求”兜底。"
                },
                "render_markdown": {
                    "type": "boolean",
                    "description": "是否按 Markdown 格式处理内容，默认 true"
                },
                "workspace": {
                    "type": "string",
                    "description": "工作区根目录绝对路径（必填）"
                }
            },
            "required": ["brief", "workspace"]
        });

        if let serde_json::Value::Object(schema_map) = zhi_schema {
            tools.push(Tool {
                name: Cow::Borrowed(zhi_tool_name),
                description: Some(Cow::Borrowed(self.zhi_public_description())),
                input_schema: Arc::new(schema_map),
                annotations: None,
                icons: None,
                meta: None,
                output_schema: None,
                title: Some(self.zhi_public_title().to_string()),
            });
        }

        // 记忆管理工具 - 仅在启用时添加
        if self.is_tool_enabled("ji") {
            let ji_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "操作类型：记忆(添加) | 回忆(查询) | 整理(去重) | 列表(全部记忆) | 预览相似(检测相似度) | 配置(获取/更新) | 删除(移除记忆)"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "项目路径（必需）"
                    },
                    "content": {
                        "type": "string",
                        "description": "记忆内容（记忆/预览相似操作时必需）"
                    },
                    "category": {
                        "type": "string",
                        "description": "记忆分类：rule(规范规则), preference(用户偏好), pattern(最佳实践), context(项目上下文)"
                    },
                    "config": {
                        "type": "object",
                        "description": "配置参数（配置操作时使用）",
                        "properties": {
                            "similarity_threshold": {
                                "type": "number",
                                "description": "相似度阈值 (0.5~0.95)，超过此值视为重复"
                            },
                            "dedup_on_startup": {
                                "type": "boolean",
                                "description": "启动时自动去重"
                            },
                            "enable_dedup": {
                                "type": "boolean",
                                "description": "启用去重检测"
                            }
                        }
                    },
                    "memory_id": {
                        "type": "string",
                        "description": "记忆ID（删除操作时必需）"
                    }
                },
                "required": ["action", "project_path"]
            });

            if let serde_json::Value::Object(schema_map) = ji_schema {
                tools.push(Tool {
                    name: Cow::Borrowed("ji"),
                    description: Some(Cow::Borrowed(
                        "全局记忆管理工具，用于存储和管理重要的开发规范、用户偏好和最佳实践",
                    )),
                    input_schema: Arc::new(schema_map),
                    annotations: None,
                    icons: None,
                    meta: None,
                    output_schema: None,
                    title: None,
                });
            }
        }

        // 代码搜索工具 - 仅在启用时添加
        if self.is_tool_enabled("sou") {
            tools.push(SouTool::get_tool_definition());
        }

        // Context7 文档查询工具 - 仅在启用时添加
        if self.is_tool_enabled("context7") {
            tools.push(Context7Tool::get_tool_definition());
        }

        // 图标工坊工具 - 仅在启用时添加
        if self.is_tool_enabled("icon") {
            tools.push(IconTool::get_tool_definition());
        }

        // UI/UX 工具 - 仅在启用时添加
        if self.is_tool_enabled("uiux") {
            tools.extend(UiuxTool::get_tool_definitions());
        }

        // 提示词增强工具 - 仅在启用时添加
        if self.is_tool_enabled("enhance") {
            tools.push(EnhanceTool::get_tool_definition());
        }

        // Tavily AI 搜索工具 - 仅在启用时添加
        if self.is_tool_enabled("tavily") {
            tools.push(TavilyTool::get_tool_definition());
        }

        // 技能运行时工具 - 动态发现 skills 并追加工具
        let project_root =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        tools.extend(SkillsTool::list_dynamic_tools(&project_root));

        log_debug!(
            "返回给客户端的工具列表: {:?}",
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        Ok(ListToolsResult {
            meta: None,
            next_cursor: None,
            tools,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let call_id = generate_request_id();
        let start = Instant::now();

        let tool_name = request.name.to_string();
        let arg_keys: Vec<String> = request
            .arguments
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();

        // 解析参数（保持与旧逻辑一致：None -> 空对象）
        let arguments_value = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        // 统一入口日志（全链路追踪用）
        log_important!(
            info,
            "[MCP] 调用开始: call_id={}, tool={}, arg_keys={:?}",
            call_id,
            tool_name,
            arg_keys
        );

        // 常见字段摘要（避免打印完整内容导致日志膨胀/泄露）
        if let Some(obj) = arguments_value.as_object() {
            for k in ["brief", "message", "prompt", "query", "content"] {
                if let Some(s) = obj.get(k).and_then(|v| v.as_str()) {
                    log_debug!(
                        "[MCP] 参数摘要: call_id={}, tool={}, {}_len={}, {}_preview={}",
                        call_id,
                        tool_name,
                        k,
                        s.len(),
                        k,
                        safe_truncate_clean(s, 200)
                    );
                }
            }
        }

        let result: Result<CallToolResult, McpError> = match tool_name.as_str() {
            tool if Self::is_zhi_entry(tool) => {
                match serde_json::from_value::<ZhiRequest>(arguments_value) {
                    Ok(zhi_request) => {
                        // 调用三术工具（将 call_id 作为 request.id 贯穿到 GUI/响应）
                        // 中文说明：把 peer 与客户端下发的 progressToken 一并传入，
                        // 让 zhi 在等待用户期间按周期推送 progress 心跳，规避客户端 ~30s 工具超时。
                        let progress_token = _context.meta.get_progress_token();
                        InteractionTool::zhi_with_request_id(
                            zhi_request,
                            call_id.clone(),
                            Some(_context.peer.clone()),
                            progress_token,
                        )
                        .await
                    }
                    Err(e) => {
                        log_important!(
                            warn,
                            "[MCP] 参数解析失败: call_id={}, tool={}, error={}",
                            call_id,
                            tool,
                            e
                        );
                        // 中文说明：zhi 是强制交互入口，参数错误不能以 hard Err 返回——
                        // 否则 Cursor 等客户端会直接结束本轮、把后续输入计为新一轮 request。
                        // 改为 success + is_error + 修正指引，让 AI 立刻在同一轮内重试。
                        let guidance = format!(
                            "zhi 参数解析失败: {}\n\n\
                             【处理指引】\n\
                             1) 必填字段：`brief`（string）+ `workspace`（绝对路径 string）；\n\
                             2) 可选字段：`choices`（string 数组）、`render_markdown`（bool）；\n\
                             3) 请按以上 schema 修正后立即重新调用 `zhi`，禁止因此结束本轮对话。",
                            e
                        );
                        Ok(CallToolResult {
                            content: vec![Content::text(guidance)],
                            is_error: Some(true),
                            meta: None,
                            structured_content: None,
                        })
                    }
                }
            }
            "ji" => {
                if !self.is_tool_enabled("ji") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=ji", call_id);
                    Err(McpError::internal_error(
                        "记忆管理工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<JiyiRequest>(arguments_value) {
                        Ok(ji_request) => MemoryTool::jiyi(ji_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=ji, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            "sou" => {
                if !self.is_tool_enabled("sou") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=sou", call_id);
                    Err(McpError::internal_error(
                        "代码搜索工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<crate::mcp::tools::sou::SouRequest>(
                        arguments_value,
                    ) {
                        Ok(sou_request) => SouTool::search_context(sou_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=sou, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            "context7" => {
                if !self.is_tool_enabled("context7") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=context7", call_id);
                    Err(McpError::internal_error(
                        "Context7 文档查询工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<Context7Request>(arguments_value) {
                        Ok(context7_request) => Context7Tool::query_docs(context7_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=context7, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            "tu" => {
                if !self.is_tool_enabled("icon") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=tu(icon)", call_id);
                    Err(McpError::internal_error(
                        "图标工坊工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<TuRequest>(arguments_value) {
                        Ok(tu_request) => IconTool::tu(tu_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=tu, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            "uiux" => {
                if !self.is_tool_enabled("uiux") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=uiux", call_id);
                    Err(McpError::internal_error(
                        "UI/UX 工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    UiuxTool::call_tool("uiux", arguments_value).await
                }
            }
            name if name == "skill_run" || name.starts_with("skill_") => {
                match serde_json::from_value::<SkillRunRequest>(arguments_value) {
                    Ok(skill_request) => {
                        let project_root = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        SkillsTool::call_tool(name, skill_request, &project_root).await
                    }
                    Err(e) => {
                        log_important!(
                            warn,
                            "[MCP] 参数解析失败: call_id={}, tool={}, error={}",
                            call_id,
                            name,
                            e
                        );
                        Err(McpError::invalid_params(
                            format!("参数解析失败: {}", e),
                            None,
                        ))
                    }
                }
            }
            "enhance" => {
                if !self.is_tool_enabled("enhance") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=enhance", call_id);
                    Err(McpError::internal_error(
                        "提示词增强工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<EnhanceMcpRequest>(arguments_value) {
                        Ok(enhance_request) => EnhanceTool::enhance(enhance_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=enhance, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            "tavily" => {
                if !self.is_tool_enabled("tavily") {
                    log_important!(warn, "[MCP] 工具已禁用: call_id={}, tool=tavily", call_id);
                    Err(McpError::internal_error(
                        "Tavily AI 搜索工具已被禁用".to_string(),
                        None,
                    ))
                } else {
                    match serde_json::from_value::<TavilyRequest>(arguments_value) {
                        Ok(tavily_request) => TavilyTool::execute(tavily_request).await,
                        Err(e) => {
                            log_important!(
                                warn,
                                "[MCP] 参数解析失败: call_id={}, tool=tavily, error={}",
                                call_id,
                                e
                            );
                            Err(McpError::invalid_params(
                                format!("参数解析失败: {}", e),
                                None,
                            ))
                        }
                    }
                }
            }
            _ => Err(McpError::invalid_request(
                format!("未知的工具: {}", tool_name),
                None,
            )),
        };

        // 统一出口日志（全链路追踪用）
        let elapsed_ms = start.elapsed().as_millis();
        match &result {
            Ok(r) => {
                let is_error = r.is_error.unwrap_or(false);
                log_important!(
                    info,
                    "[MCP] 调用结束: call_id={}, tool={}, is_error={}, content_items={}, elapsed_ms={}",
                    call_id,
                    tool_name,
                    is_error,
                    r.content.len(),
                    elapsed_ms
                );
            }
            Err(e) => {
                log_important!(
                    error,
                    "[MCP] 调用失败: call_id={}, tool={}, elapsed_ms={}, error={}",
                    call_id,
                    tool_name,
                    elapsed_ms,
                    e
                );
            }
        }

        result
    }
}

/// 启动MCP服务器
pub async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    // 中文说明：先打印「启动水印」——记录本二进制的版本/提交/构建时间/关键保活窗口，
    // 便于排查「源码已更新但 Cursor 仍在跑旧 MCP 二进制」这类问题（旧二进制会因 20s 窗口
    // 频繁重连、烧光单轮预算而被动新开 request）。
    log_startup_watermark();

    // 中文说明：MCP 进程负责长期维护代码监听；GUI 只写入配置中的监听意图。
    start_acemcp_watch_config_sync();

    // 创建并运行服务器
    let service = match ZhiServer::new().serve(stdio()).await {
        Ok(service) => service,
        Err(e) => {
            match &e {
                ServerInitializeError::ConnectionClosed(_) => {
                    log_important!(
                        error,
                        "启动服务器失败：初始化阶段连接已关闭。通常是未通过 MCP 客户端以 stdio 管道启动，或客户端启动后立即退出。请检查 MCP 客户端配置（command/args/stdio），不要直接双击运行。"
                    );
                }
                _ => {
                    log_important!(error, "启动服务器失败: {}", e);
                }
            }
            return Err(Box::new(e));
        }
    };

    // 等待服务器关闭
    service.waiting().await?;
    Ok(())
}

/// 打印 MCP「启动水印」：版本 / git 提交 / 构建时间 / 关键保活窗口（POPUP_POLL_WINDOW）。
///
/// 中文说明：`SANSHU_GIT_SHA`、`SANSHU_BUILD_EPOCH` 由 build.rs 在编译期注入；
/// 非 git 环境或裁剪构建下回退为 unknown/0，不影响启动。
/// 排查「跑的是不是最新二进制」时，对照日志这一行的 git 与 POPUP_POLL_WINDOW 即可一眼确认。
fn log_startup_watermark() {
    let version = env!("CARGO_PKG_VERSION");
    let git_sha = option_env!("SANSHU_GIT_SHA").unwrap_or("unknown");
    let build_time = option_env!("SANSHU_BUILD_EPOCH")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&e| e > 0)
        .and_then(|e| chrono::DateTime::<chrono::Utc>::from_timestamp(e, 0))
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    log_important!(
        info,
        "[启动水印] version={} git={} built={} POPUP_POLL_WINDOW={}s MAX_RECONNECTS={}",
        version,
        git_sha,
        build_time,
        crate::mcp::handlers::POPUP_POLL_WINDOW.as_secs(),
        crate::mcp::handlers::MAX_POPUP_RECONNECTS
    );
}

fn start_acemcp_watch_config_sync() {
    tokio::spawn(async {
        let watcher_manager = crate::mcp::tools::acemcp::watcher::get_watcher_manager();
        watcher_manager.sync_with_persisted_watch_projects().await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            interval.tick().await;
            watcher_manager.sync_with_persisted_watch_projects().await;
        }
    });
}
