use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;
use rmcp::model::{
    CallToolResult, Content, ErrorData as McpError, NumberOrString, ProgressNotificationParam,
    ProgressToken,
};
use rmcp::service::Peer;
use rmcp::RoleServer;

use crate::mcp::handlers::{
    parse_mcp_response, poll_or_start_popup, take_orphan_reply_notice, PopupPoll,
    POPUP_POLL_WINDOW, RESPONSE_LEN_WARN_THRESHOLD,
};
use crate::mcp::utils::safe_truncate_clean;
use crate::mcp::utils::{generate_request_id, normalize_zhi_choices};
use crate::mcp::{PopupRequest, ZhiRequest};
use crate::{log_debug, log_important};

/// 心跳进度通知周期：10 秒。
///
/// 中文说明：Cursor 等 MCP 客户端的工具调用超时多在 ~30 秒，zhi 等用户决策往往要数分钟。
/// 配合方案A 把 POPUP_POLL_WINDOW 拉长到 900s 后，单次 zhi 调用会真正长时间阻塞、
/// 完全依赖这里的 progress 心跳在 30s 超时前反复重置客户端计时器。
/// 取 10s（而非旧值 15s）是为了留足安全余量：30s 内能稳定发出约 3 次心跳，
/// 即使偶有一次丢包/抖动也不会逼近超时；从根上避免
/// 「长等待被客户端超时丢弃 → AI 误判失败 → 新开 request」。
const PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// brief 过长告警阈值（字符数）。
///
/// 中文说明（2026-06-07 调优）：每次 zhi 调用模型都会重发整段上下文，brief 本身越大、
/// 单次往返固定开销越高。超过此阈值仅打 warn 日志做诊断提示，**不截断**——
/// 截断会破坏用户在弹窗里看到的内容（UX），且无法减少模型侧已生成的 token。
/// 真正省 token 要靠"少调 zhi / 合并 zhi"（见 强制交互规则 一·补充）。
const BRIEF_LEN_WARN_THRESHOLD: usize = 4000;

/// 每 workspace 的 zhi 调用节流统计：workspace → (累计调用次数, 上次调用时刻)。
///
/// 中文说明（2026-06-07 新增·节流监控）：用于直接观测"频繁新建弹窗 / 保活空转"——
/// 这是长会话 token 的大头。改了「强制交互·一·补充」节流规则后，对照本统计输出的
/// 「第 N 次调用 / 距上次 Xs」即可验证弹窗是否真的变少、间隔是否拉长。
/// 注：计数是 MCP server 进程生命周期内累计（不按对话重置），关键看「距上次间隔」。
static ZHI_CALL_CADENCE: Lazy<Mutex<HashMap<String, (u64, Instant)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 代码审阅记录工具
///
/// 汇总审阅内容、候选处理项与结构化反馈
#[derive(Clone)]
pub struct InteractionTool;

impl InteractionTool {
    pub async fn zhi(request: ZhiRequest) -> Result<CallToolResult, McpError> {
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

        // 中文说明（2026-06-07 调优）：brief 过长时打 warn 诊断，提示上游"精简 brief / 合并 zhi"；
        // 仅告警不截断，避免破坏弹窗展示内容。
        if request.brief.len() > BRIEF_LEN_WARN_THRESHOLD {
            log_important!(
                warn,
                "[zhi] brief 偏长: request_id={}, brief_len={}（阈值={}）——每次 zhi 都会重发整段上下文，建议精简 brief 或合并多次 zhi 以省 token",
                request_id,
                request.brief.len(),
                BRIEF_LEN_WARN_THRESHOLD
            );
        }

        // 中文说明（2026-06-07 新增·节流监控）：按 workspace 记录 zhi 调用序号与距上次间隔，
        // 直接暴露"频繁新建弹窗/保活空转"。间隔越短越像无谓往返；改节流规则后应看到调用变疏。
        {
            let now = Instant::now();
            if let Ok(mut map) = ZHI_CALL_CADENCE.lock() {
                let entry = map.entry(request.workspace.clone()).or_insert((0, now));
                entry.0 = entry.0.saturating_add(1);
                let gap_secs = now.duration_since(entry.1).as_secs();
                entry.1 = now;
                let nth = entry.0;
                log_important!(
                    info,
                    "[zhi] 节流监控: workspace={:?}, 第 {} 次 zhi 调用, 距上次={}s（间隔越短=越像保活/汇报空转，每次都重发整段上下文烧 token）",
                    request.workspace.as_str(),
                    nth,
                    gap_secs
                );
            }
        }

        // 中文说明：MCP 对外字段采用中性命名，内部仍映射到既有弹窗协议以保持 UI 链路稳定。
        let choices = normalize_zhi_choices(request.choices);

        // 中文说明（2026-06-11）：workspace 随后会移交给 popup_request，先留一份用于
        // Done 时检索本 workspace 的「孤儿回复」（无人轮询时用户才提交的历史回答）。
        let workspace_for_notice = request.workspace.clone();

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
        // 中文说明：client_progress_token 是否由客户端下发，是排查「心跳是否真的有效」的关键信号——
        // 若为 None，我们只能自造 token，部分客户端（如 Cursor）可能不认、不重置超时计时器，
        // 导致长等待仍被 ~30s 超时丢弃 → AI 误判失败 → 新开 request。这里在 info 级显式标注。
        let has_client_token = client_progress_token.is_some();
        if peer.is_some() {
            log_important!(
                info,
                "[zhi] 心跳已启用: request_id={}, 间隔={}s, client_progress_token={}（token=自造时部分客户端可能不认而失效）",
                request_id,
                PROGRESS_HEARTBEAT_INTERVAL.as_secs(),
                if has_client_token { "客户端下发" } else { "缺失/自造" }
            );
        } else {
            log_important!(info, "[zhi] 未启用心跳（无 peer，CLI/测试场景）: request_id={}", request_id);
        }

        // 连接存活标志：心跳失败时置 false，通知轮询线程提前退出以避免空等
        let connection_alive = Arc::new(AtomicBool::new(true));
        let abort_flag_for_poll = connection_alive.clone();

        let heartbeat = peer.map(|peer| {
            let token = client_progress_token.unwrap_or_else(|| {
                ProgressToken(NumberOrString::String(Arc::from(request_id.as_str())))
            });
            let heartbeat_request_id = request_id.clone();
            let alive_flag = connection_alive.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(PROGRESS_HEARTBEAT_INTERVAL);
                // 跳过 interval 立即触发的首个 tick，让第一次心跳在一个周期后再发。
                ticker.tick().await;
                let mut elapsed_secs: f64 = 0.0;
                let mut beat_no: u64 = 0;
                loop {
                    ticker.tick().await;
                    elapsed_secs += PROGRESS_HEARTBEAT_INTERVAL.as_secs() as f64;
                    beat_no += 1;
                    if let Err(e) = peer
                        .notify_progress(ProgressNotificationParam {
                            progress_token: token.clone(),
                            progress: elapsed_secs,
                            total: None,
                            message: Some(format!("等待用户响应中（已等待 {} 秒）", elapsed_secs as u64)),
                        })
                        .await
                    {
                        // 客户端连接已关闭 → 通知轮询线程立即退出
                        alive_flag.store(false, Ordering::Relaxed);
                        log_important!(
                            warn,
                            "[zhi] progress 心跳#{}发送失败→连接已关闭，设置 abort 信号: request_id={}, 已等待={}s, error={}",
                            beat_no,
                            heartbeat_request_id,
                            elapsed_secs as u64,
                            e
                        );
                        break;
                    }
                    log_debug!(
                        "[zhi] progress 心跳#{}已发送: request_id={}, 已等待={}s",
                        beat_no,
                        heartbeat_request_id,
                        elapsed_secs as u64
                    );
                }
            })
        });

        // 中文说明：poll_or_start_popup 是同步阻塞调用（spawn/重连 GUI 子进程并轮询至多 POPUP_POLL_WINDOW≈900s），
        // 放入阻塞线程池，避免阻塞 tokio 运行时，也保证上面的心跳任务能并行推进。
        // abort_flag 在心跳检测到连接断开时置 false，使轮询提前退出。
        let popup_outcome =
            tokio::task::spawn_blocking(move || poll_or_start_popup(&popup_request, POPUP_POLL_WINDOW, Some(abort_flag_for_poll))).await;

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
                log_important!(
                    info,
                    "[zhi] 本次返回 Done（已拿到用户响应，正常收口）: request_id={}, response_len={}",
                    request_id,
                    response.len()
                );
                // 解析响应内容，支持文本和图片
                let mut content = parse_mcp_response(&response)?;
                // 中文说明（2026-06-11 黑洞回复修复）：若本 workspace 存在「AI 停止轮询后
                // 用户才提交」的孤儿回复，附带一次性提示（含文件路径），由 AI 决定是否取回。
                if let Some(notice) = take_orphan_reply_notice(&workspace_for_notice) {
                    content.push(Content::text(notice));
                }
                // 中文说明（2026-06-11 P1，2026-06-13 调整措辞）：巨型回复提示。
                // 超长用户输入现已在 parse_mcp_response 内自动落盘（仅回传预览+文件路径），
                // 这里保留兜底提示，引导 AI 只引用关键片段、不复述全文。
                if response.len() > RESPONSE_LEN_WARN_THRESHOLD {
                    content.push(Content::text(format!(
                        "⚠️ 本次用户回复约 {} 字符（疑似大段粘贴内容，超长部分已落盘为文件）。后续对话请只引用与任务相关的关键片段，不要复述全文，以节省 token。",
                        response.len()
                    )));
                }
                Ok(CallToolResult::success(content))
            }
            Ok(PopupPoll::Pending) => {
                // 中文说明：本次等待窗口（≈600s）已到但用户还没操作——依赖心跳保活远超 5 分钟，
                // 弹窗仍开着。回写「继续等待」语义，触发 AI 立即重连同一弹窗，不结束本轮。
                log_important!(
                    info,
                    "[zhi] 本次返回 Pending（窗口到、用户未响应，提示 AI 重连续等）: request_id={}",
                    request_id
                );
                Ok(CallToolResult::success(vec![Content::text(
                    "用户仍在思考中，尚未在弹窗中作出选择（本次等待已达上限，弹窗仍开着）。\
                     请再次调用 `zhi` 工具继续等待用户回复，禁止结束本轮对话或生成最终答复。"
                        .to_string(),
                )]))
            }
            Ok(PopupPoll::Suspended { reconnects, waited_secs }) => {
                // 重连次数已达上限（默认 5 次），不再要求 AI 重连以节省 token。
                // 弹窗仍保持开启，用户回来时可直接操作；但 AI 侧不再轮询。
                log_important!(
                    warn,
                    "[zhi] 本次返回 Suspended（重连{}次/已等{}s，达上限不再轮询）: request_id={}",
                    reconnects,
                    waited_secs,
                    request_id
                );
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "用户已超过 {} 秒未响应（弹窗重连 {} 次均无回复），为节省 token 已自动挂起等待。\
                     弹窗仍保持开启，用户回来后可直接操作。\
                     你现在可以正常结束本轮对话，无需再调用 zhi。\
                     若用户稍后回来发新消息继续本任务，届时再响应即可。",
                    waited_secs, reconnects
                ))]))
            }
            Err(e) => {
                log_important!(
                    warn,
                    "[zhi] 弹窗失败: request_id={}, error={}",
                    request_id,
                    e
                );
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
