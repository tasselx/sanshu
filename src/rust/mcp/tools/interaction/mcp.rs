use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use rmcp::RoleServer;
use rmcp::service::Peer;
use rmcp::model::{ErrorData as McpError, CallToolResult, Content, ProgressNotificationParam, ProgressToken, NumberOrString};

use crate::mcp::{ZhiRequest, PopupRequest};
use crate::mcp::handlers::{poll_or_start_popup, parse_mcp_response, PopupPoll, POPUP_POLL_WINDOW};
use crate::mcp::utils::{generate_request_id, normalize_zhi_choices};
use crate::mcp::utils::safe_truncate_clean;
use crate::{log_important, log_debug};

/// 心跳进度通知周期：10 秒。
///
/// 中文说明：Cursor 等 MCP 客户端的工具调用超时多在 ~30 秒，zhi 等用户决策往往要数分钟。
/// 配合方案A 把 POPUP_POLL_WINDOW 拉长到 240s 后，单次 zhi 调用会真正长时间阻塞、
/// 完全依赖这里的 progress 心跳在 30s 超时前反复重置客户端计时器。
/// 取 10s（而非旧值 15s）是为了留足安全余量：30s 内能稳定发出约 3 次心跳，
/// 即使偶有一次丢包/抖动也不会逼近超时；从根上避免
/// 「长等待被客户端超时丢弃 → AI 误判失败 → 新开 request」。
const PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// 代码审阅记录工具
///
/// 汇总审阅内容、候选处理项与结构化反馈
#[derive(Clone)]
pub struct InteractionTool;

impl InteractionTool {
    pub async fn zhi(
        request: ZhiRequest,
    ) -> Result<CallToolResult, McpError> {
        // 默认生成 request_id（MCP server 会优先使用其 call_id 注入到 zhi_with_request_id）
        let request_id = generate_request_id();
        // 中文说明：无 peer 的入口（CLI/测试等）不发送 progress 心跳。
        Self::zhi_with_request_id(request, request_id, None, None).await
    }

    /// 带 request_id 的 zhi 调用入口
    ///
    /// 中文说明：用于将 MCP 分发层生成的 call_id 贯穿到 GUI 进程与响应，便于全链路日志关联。
    /// `peer` / `client_progress_token` 由 MCP 分发层传入，用于在等待用户期间推送 progress 心跳。
    pub async fn zhi_with_request_id(
        request: ZhiRequest,
        request_id: String,
        // 中文说明：MCP server 分发时传入，用于在等待用户期间向客户端推送 progress 心跳；
        // CLI/测试等无 peer 场景传 None。
        peer: Option<Peer<RoleServer>>,
        // 中文说明：客户端在 tools/call 的 _meta.progressToken 中提供的进度令牌（可能为空）。
        client_progress_token: Option<ProgressToken>,
    ) -> Result<CallToolResult, McpError> {
        // 记录 UI/UX 上下文控制信号，便于审计排查
        if request.uiux_intent.is_some()
            || request.uiux_context_policy.is_some()
            || request.uiux_reason.is_some()
        {
            log::info!(
                "UI/UX 上下文信号: intent={:?}, policy={:?}, reason={:?}",
                request.uiux_intent.as_deref(),
                request.uiux_context_policy.as_deref(),
                request.uiux_reason.as_deref()
            );
        }

        log_important!(
            info,
            "[zhi] 记录请求: request_id={}, brief_len={}, brief_preview={}, choices_len={}, workspace={:?}",
            request_id,
            request.brief.len(),
            safe_truncate_clean(&request.brief, 200),
            request.choices.len(),
            request.workspace.as_str()
        );

        // 中文说明：MCP 对外字段采用中性命名，内部仍映射到既有弹窗协议以保持 UI 链路稳定。
        let choices = normalize_zhi_choices(request.choices);

        let popup_request = PopupRequest {
            id: request_id.clone(),
            message: request.brief,
            predefined_options: if choices.is_empty() {
                None
            } else {
                Some(choices)
            },
            is_markdown: request.render_markdown,
            project_root_path: Some(request.workspace),
            // 透传 UI/UX 上下文控制信号
            uiux_intent: request.uiux_intent,
            uiux_context_policy: request.uiux_context_policy,
            uiux_reason: request.uiux_reason,
        };

        // 中文说明：在等待用户响应期间，按固定周期向客户端推送 progress 心跳，
        // 让 Cursor 等客户端在 ~30s 工具超时前不断重置计时器，从根上解决「长等待被超时丢弃」。
        // token 优先用客户端 _meta.progressToken 下发的（最规范、最可能被客户端关联到本次调用）；
        // 缺失时回退为本次 request_id 自生成，兼容「只认 notifications/progress、不校验 token 来源」的客户端。
        let heartbeat = peer.map(|peer| {
            let token = client_progress_token.unwrap_or_else(|| {
                ProgressToken(NumberOrString::String(Arc::from(request_id.as_str())))
            });
            let heartbeat_request_id = request_id.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(PROGRESS_HEARTBEAT_INTERVAL);
                // 跳过 interval 立即触发的首个 tick，让第一次心跳在一个周期后再发。
                ticker.tick().await;
                let mut elapsed_secs: f64 = 0.0;
                loop {
                    ticker.tick().await;
                    elapsed_secs += PROGRESS_HEARTBEAT_INTERVAL.as_secs() as f64;
                    if let Err(e) = peer
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token.clone(),
                            progress: elapsed_secs,
                            total: None,
                            message: Some(format!("等待用户响应中（已等待 {} 秒）", elapsed_secs as u64)),
                        })
                        .await
                    {
                        // 通知失败通常意味着连接已关闭，停止心跳即可，无需上报。
                        log_debug!(
                            "[zhi] progress 心跳发送失败，停止心跳: request_id={}, error={}",
                            heartbeat_request_id,
                            e
                        );
                        break;
                    }
                }
            })
        });

        // 中文说明：poll_or_start_popup 是同步阻塞调用（spawn/重连 GUI 子进程并轮询至多 POPUP_POLL_WINDOW≈240s），
        // 放入阻塞线程池，避免阻塞 tokio 运行时，也保证上面的心跳任务能并行推进。
        let popup_outcome =
            tokio::task::spawn_blocking(move || poll_or_start_popup(&popup_request, POPUP_POLL_WINDOW)).await;

        // 中文说明：无论弹窗成功、失败还是 join 异常，都要先停掉心跳任务，避免任务泄漏。
        if let Some(handle) = heartbeat {
            handle.abort();
        }

        let popup_result = match popup_outcome {
            Ok(result) => result,
            Err(join_err) => {
                // spawn_blocking 任务异常（如 panic）：按弹窗失败软兜底处理，禁止结束本轮。
                log_important!(warn, "[zhi] 弹窗任务异常: request_id={}, error={}", request_id, join_err);
                Err(anyhow::anyhow!("弹窗任务异常: {}", join_err))
            }
        };

        match popup_result {
            Ok(PopupPoll::Done(response)) => {
                log_debug!(
                    "[zhi] 弹窗响应已收到: request_id={}, response_len={}",
                    request_id,
                    response.len()
                );
                // 解析响应内容，支持文本和图片
                let content = parse_mcp_response(&response)?;
                Ok(CallToolResult::success(content))
            }
            Ok(PopupPoll::Pending) => {
                // 中文说明：本次等待窗口（≈240s）已到但用户还没操作——在 Cursor ~5 分钟硬上限前主动返回，
                // 弹窗仍开着。回写「继续等待」语义，触发 AI 立即重连同一弹窗，不结束本轮。
                log_debug!(
                    "[zhi] 等待窗口到，弹窗仍开启，提示 AI 重连: request_id={}",
                    request_id
                );
                Ok(CallToolResult::success(vec![Content::text(
                    "用户仍在思考中，尚未在弹窗中作出选择（本次等待已达上限，弹窗仍开着）。\
                     请再次调用 `zhi` 工具继续等待用户回复，禁止结束本轮对话或生成最终答复。"
                        .to_string(),
                )]))
            }
            Err(e) => {
                log_important!(warn, "[zhi] 弹窗失败: request_id={}, error={}", request_id, e);
                // 中文说明：不再以 Err 返回给 MCP 客户端（Cursor 等收到 hard error 极易直接收尾、
                // 把后续消息计为新一轮 request）；改为成功响应 + is_error=true + 重试指引，
                // 让 AI 在本轮内自我修复后立刻再次调用 zhi。
                let guidance = format!(
                    "弹窗调用失败（request_id={}）: {}\n\n\
                     【处理指引】\n\
                     1) 若错误信息提示找不到 UI 命令，请提醒用户检查 sanshu/等一下 是否已安装；\n\
                     2) 修正后请再次调用 `zhi` 工具继续等待用户回复；\n\
                     3) 禁止结束本轮对话或生成最终答复（不要因为这次失败就收尾）。",
                    request_id, e
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
}
