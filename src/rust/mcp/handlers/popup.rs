use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::AtomicBool as ReaperFlag;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;

use crate::mcp::types::PopupRequest;
use crate::mcp::utils::safe_truncate_clean;
use crate::{log_debug, log_important};

use super::response::is_cancel_signal;

/// 创建 Tauri 弹窗
///
/// 优先调用与 MCP 服务器同目录的 UI 命令，找不到时使用全局版本
pub fn create_tauri_popup(request: &PopupRequest) -> Result<String> {
    let start = Instant::now();

    // 创建临时请求文件 - 跨平台适配
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request.id));
    let request_json = serde_json::to_string_pretty(request)?;
    fs::write(&temp_file, request_json)?;

    log_important!(
        info,
        "[popup] 已写入MCP请求文件: request_id={}, file={}, message_len={}, message_preview={}, options_len={}, project={:?}, markdown={}",
        request.id,
        temp_file.display(),
        request.message.len(),
        safe_truncate_clean(&request.message, 200),
        request.predefined_options.as_ref().map(|v| v.len()).unwrap_or(0),
        request.project_root_path.as_deref(),
        request.is_markdown
    );

    // 尝试找到等一下命令的路径
    let command_path = find_ui_command()?;

    log_debug!(
        "[popup] 准备调用GUI进程: request_id={}, command_path={}",
        request.id,
        command_path
    );

    // 调用等一下命令
    let output = Command::new(&command_path)
        .arg("--mcp-request")
        .arg(temp_file.to_string_lossy().to_string())
        .output()?;

    // 清理临时文件
    let _ = fs::remove_file(&temp_file);

    let elapsed_ms = start.elapsed().as_millis();
    let exit_code = output.status.code();
    let stdout_len = output.stdout.len();
    let stderr_len = output.stderr.len();

    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout);
        let response = response.trim();

        log_important!(
            info,
            "[popup] GUI执行成功: request_id={}, exit_code={:?}, stdout_len={}, stderr_len={}, elapsed_ms={}",
            request.id,
            exit_code,
            stdout_len,
            stderr_len,
            elapsed_ms
        );
        if response.is_empty() {
            // 中文说明（2026-09-14）：真实取消会由 GUI 明确写出 "CANCELLED"；退出码 0 却
            // 一个字节都没写，说明 GUI 未走提交/取消链路就退出了（窗口被直接关闭、
            // 前端未初始化完成即退出、或异常退出但退出码为 0）。旧版把它当成用户取消，
            // 日志记「执行成功」、与真取消无法区分。现按异常上报，走 zhi 的重试指引分支。
            log_important!(
                warn,
                "[popup] GUI 以退出码 0 结束但 stdout 为空（未收到用户响应，也不是显式取消）: request_id={}, stderr_preview={}, elapsed_ms={}",
                request.id,
                safe_truncate_clean(&String::from_utf8_lossy(&output.stderr), 200),
                elapsed_ms
            );
            anyhow::bail!(
                "弹窗进程已退出但未返回任何响应（既非用户提交也非显式取消，可能是窗口被直接关闭或 GUI 异常退出）"
            );
        }
        Ok(response.to_string())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        log_important!(
            error,
            "[popup] GUI执行失败: request_id={}, exit_code={:?}, stdout_len={}, stderr_len={}, stderr_preview={}, elapsed_ms={}",
            request.id,
            exit_code,
            stdout_len,
            stderr_len,
            safe_truncate_clean(&error, 200),
            elapsed_ms
        );
        anyhow::bail!("UI进程失败: {}", error);
    }
}

// ============================================================================
// 短调用 + 重连（A′ 方案）：避免 zhi 长阻塞被客户端 ~30s 超时丢弃。
//
// 思路：MCP server 是长驻进程，把「未完成的弹窗」按 workspace 暂存到进程内注册表；
// 单次 zhi 调用最多阻塞 POPUP_POLL_WINDOW 就主动返回 Pending（弹窗保持开启、不丢用户输入），
// AI 收到「请再次调用」提示后重连同一弹窗，从而把「一次长调用」拆成「多次短调用」。
// ============================================================================

/// 弹窗轮询窗口：单次 zhi 调用最多阻塞这么久就主动返回 Pending（弹窗仍保持开启）。
///
/// 中文说明（方案A·根治重连风暴）：早期取 20s 是为了「即便客户端不认 progress 心跳，
/// 单次调用也稳稳低于 30s 超时」。但 20s 硬返回会把一次「等用户 N 分钟」的决策拆成
/// 大量 zhi 往返（实测等 4.5 分钟 → 10 次调用），每次重连 AI 还会重发整段 brief/choices，
/// 上下文按 N 倍膨胀、烧光 Cursor 迭代预算、触发后台新 request，最终把强约束规则挤出上下文、
/// 回退到原生 ask。现改为依赖 PROGRESS_HEARTBEAT_INTERVAL 的 progress 心跳在 30s 超时前
/// 反复重置客户端计时器，从而把窗口拉长到 900s（心跳每 10s 一次，实测可稳定支撑）。
/// 让「等 15 分钟」只需约 1 次重连。超过 900s 仍未响应才返回 Pending 让 AI 重连。
/// 配合 MAX_POPUP_RECONNECTS 上限，超过指定次数后自动挂起、不再消耗 token。
/// 另有 abort_flag 机制：心跳失败时立即通知轮询退出，避免客户端已断开后仍空等。
///
/// 中文说明（2026-06-07 调优）：经日志验证 Cursor 会下发 client_progress_token、心跳确实有效，
/// 故把窗口从 600s 上调到 900s，进一步减少 Pending→重连（每次重连都重发整段上下文、烧 token）。
pub const POPUP_POLL_WINDOW: Duration = Duration::from_secs(900);

/// 最大重连次数上限：超过此次数后不再返回 Pending 让 AI 重连，
/// 改为返回 Suspended 告知 AI 挂起等待、不再消耗 token。
/// 5 次 × 900s = 75 分钟持续等待后自动挂起。
///
/// 中文说明（2026-06-07 调优）：从 10 下调到 5，更早封顶"用户长时间离开"时的 token 消耗；
/// 75 分钟仍覆盖绝大多数"暂时离开"场景，弹窗不关、用户回来仍可操作。
pub const MAX_POPUP_RECONNECTS: u32 = 5;
/// 轮询 GUI 进程是否结束的间隔。
const POPUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Pending 放回注册表后，为「AI 重连」保留结果的时长。
///
/// 中文说明（2026-09-14）：返回 Pending 后 AI 会立刻重连（通常几秒到几十秒）。若用户恰在
/// 这个间隙提交、GUI 退出，弹窗条目已完成但仍应交给重连方而不是被回收成孤儿回复。
/// 保留期内 reap 一律跳过该条目；保留期过仍无人重连才回收。
/// 仅 Pending 放回的条目有保留期；Suspended/断连放回的条目 AI 已被告知收尾，不保留。
const RECONNECT_HOLD: Duration = Duration::from_secs(180);

/// 后台回收线程的巡检间隔。
///
/// 中文说明（2026-09-14）：Suspended / 断连后放回注册表的弹窗没有任何轮询方；用户此后
/// 才提交的回复要等「本进程下一次任何 zhi 调用取弹窗时顺带 reap」才会持久化为孤儿回复，
/// 本进程若再无 zhi 调用则永远滞留在管道里。后台线程定期 reap 整张注册表（跳过保留期内的
/// Pending 条目），回复提交后数秒内即落盘，下一次同 workspace 的 zhi 完成时就能附带提示。
const REAPER_INTERVAL: Duration = Duration::from_secs(3);

/// 用户回复超长告警阈值（字节）。
///
/// 中文说明（2026-06-11 P1）：实证曾有用户在弹窗粘贴 10.1MB 文本（整份 spindump），
/// 原样回传模型 ≈ 百万级 token（单条 3700 万 token 会话的底层推手之一）。
/// 超过阈值时：server 打 warn 日志，zhi 返回额外附「超长提示」内容块引导 AI 不复述全文。
/// 仅告警与提示、**不截断**——截断用户输入有损语义，客户端侧另有 truncate-mcp-output
/// hook（50K 字符）可兜底。阈值与该 hook 对齐。
pub const RESPONSE_LEN_WARN_THRESHOLD: usize = 50_000;

/// 一个「在飞」的弹窗：GUI 子进程已启动、尚未拿到用户响应。
///
/// 中文说明：stdout/stderr 各用一个后台线程持续读取到 EOF，避免响应（可能含 base64 图片，
/// 体积超过管道缓冲）写满管道导致 GUI 进程阻塞、永不退出的死锁。
struct PendingPopup {
    child: Child,
    stdout_reader: JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_reader: JoinHandle<std::io::Result<Vec<u8>>>,
    temp_file: PathBuf,
    request_id: String,
    started: Instant,
    /// 本弹窗被 AI 重连（再次调用 zhi 续等）的次数。
    ///
    /// 中文说明：用于量化「重连风暴」——一次用户决策若触发大量重连，会烧光 Cursor 单轮
    /// iteration/tool-call 预算，进而被动新开 request。完成时打印该计数即可一眼看出严重程度。
    reconnects: u32,
    /// 以 Pending 放回注册表时设置的「为重连保留」截止；其它放回路径为 None。见 RECONNECT_HOLD。
    hold_until: Option<Instant>,
}

/// 后台回收线程是否已启动。
static REAPER_STARTED: ReaperFlag = ReaperFlag::new(false);

/// 全局「在飞弹窗」注册表，键为 workspace 绝对路径（同一 workspace 同时只允许一个 zhi 弹窗）。
static PENDING_POPUPS: Lazy<Mutex<HashMap<String, PendingPopup>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 正在被轮询中的弹窗 key 集合。
///
/// 弹窗在轮询期间会从 PENDING_POPUPS 取出（获取所有权），此时若另一个 Cursor request
/// 的 zhi 调用进来，会发现注册表为空而误创建重复弹窗。此集合记录「哪些 key 当前正在被
/// 轮询」，新调用发现同 key 正在轮询时会等待其释放后重连，而非新建弹窗。
/// 值为该弹窗的 request_id，供 resume_token 重连在「弹窗正被轮询」时识别归属。
static POLLING_IN_FLIGHT: Lazy<Mutex<HashMap<String, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 弹窗收口结果：真实提交 vs 用户取消/关窗。等待方据此区分「已回答」与「需重新确认」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecentOutcome {
    /// 用户提交了内容（含空提交，由解析层决定是否继续等）。
    Done,
    /// 用户取消或关闭弹窗。不是确认，等待方不得当成「已回答」。
    Cancelled,
}

/// 最近收口的弹窗 **request_id** → (结果, 时刻)。
///
/// 中文说明（2026-09-15）：等待方看到轮询标记消失时，需要区分三种收口：
/// 「用户已回答（Done）」「用户取消/关窗（Cancelled）」「持有方启动失败/出错后撤销标记」。
/// 前两种记录在此表；第三种不记录。
/// 按 request_id 而不是 key 记录：同一 workspace + brief 在 TTL 内会开出「下一代」弹窗，
/// 若按 key 记录，上一代的完成记录会让下一代失败时被误判为「已回答」。request_id 每个
/// 弹窗唯一，等待方拿着它所等的那个弹窗的 request_id 来查，不会串代。条目只保留 RECENT_OUTCOME_TTL。
///
/// 中文说明（2026-09-15）：CANCELLED 也曾被记成 Done——持有方正确走「请再次调用 zhi」，
/// 但另一路等待同一弹窗的调用会收到「用户已通过另一路完成响应」，把取消当成确认。
/// 取消必须单独记录，让等待方继续走重新确认。
static RECENT_OUTCOMES: Lazy<Mutex<HashMap<String, (RecentOutcome, Instant)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const RECENT_OUTCOME_TTL: Duration = Duration::from_secs(300);

/// 记录 `request_id` 对应弹窗刚收口；顺带清理过期条目。必须在撤销轮询标记**之前**
/// 调用，这样等待方一旦看到标记消失，就一定能查到这条记录。
fn record_outcome(request_id: &str, outcome: RecentOutcome) -> Result<()> {
    let mut map = RECENT_OUTCOMES
        .lock()
        .map_err(|e| anyhow::anyhow!("完成记录锁中毒: {}", e))?;
    let now = Instant::now();
    map.retain(|_, (_, t)| now.duration_since(*t) < RECENT_OUTCOME_TTL);
    map.insert(request_id.to_string(), (outcome, now));
    Ok(())
}

fn recent_outcome(request_id: &str) -> Result<Option<RecentOutcome>> {
    let map = RECENT_OUTCOMES
        .lock()
        .map_err(|e| anyhow::anyhow!("完成记录锁中毒: {}", e))?;
    Ok(map.get(request_id).and_then(|(outcome, t)| {
        if t.elapsed() < RECENT_OUTCOME_TTL {
            Some(*outcome)
        } else {
            None
        }
    }))
}

/// wait_for_polling_release 的结果。
enum Released {
    /// 持有方超时放回，本方已接手（并登记轮询）。
    Taken(PendingPopup),
    /// 持有方以 Done 收口，用户已通过那一路回答。
    Answered,
    /// 持有方收到取消/关窗：用户没有给出内容，等待方不得当成已回答，应继续重新确认。
    Cancelled,
    /// 标记消失但没有收口记录：持有方启动失败或出错退出，弹窗并不存在。
    Vanished,
}

/// 弹窗轮询结果
pub enum PopupPoll {
    /// 用户已响应（GUI 进程已退出），携带响应文本
    Done(String),
    /// 仍在等待用户（本次轮询窗口已到），弹窗保持开启，需 AI 再次调用 zhi 重连。
    /// `resume_token` 即弹窗的 request_id，AI 重连时原样回传以精确匹配本弹窗。
    Pending { resume_token: String },
    /// 重连次数已达上限，弹窗仍开着但不再要求 AI 重连（节省 token）
    Suspended { reconnects: u32, waited_secs: u64 },
    /// AI 携带的 resume_token 已不对应任何在飞/轮询中的弹窗（已完成、已回收或从未存在）。
    ///
    /// 中文说明（2026-09-15）：令牌是身份凭证，失效时不得降级到 key 匹配——那会接管同 brief 的
    /// 另一个弹窗、或静默再开一个。明确报错，由 zhi 层返回专门指引（不要再带此令牌重试）。
    TokenExpired { token: String },
}

/// acquire_popup 的结果。
enum Acquired {
    /// 取到弹窗及其在注册表中的实际 key。
    Popup(PendingPopup, String),
    /// 并发轮询已以 Done 收口，用户已通过另一路响应。
    AlreadyAnswered,
    /// 并发轮询收到取消/关窗：用户没有给出内容，本次调用应继续走重新确认。
    Cancelled,
    /// resume_token 未命中任何弹窗。
    TokenExpired(String),
}

/// 启动或重连弹窗，并轮询至多 `wait` 时长。
///
/// 中文说明：
/// - 同一 workspace 已有在飞弹窗则复用（重连），否则 spawn 新弹窗；
/// - 后台线程持续抽干 stdout/stderr，主线程用 `is_finished()` 判断 GUI 进程是否已退出；
/// - 退出则收集输出作为结果；超过 `wait` 仍未退出则把子进程放回注册表，返回 Pending（不杀弹窗）。
/// - 若同 key 弹窗正在被另一个 zhi 调用轮询中（POLLING_IN_FLIGHT），等待其释放后重连，
///   而非创建重复弹窗。
/// - `resume_token`：AI 从上一次 Pending 结果里拿到的弹窗 request_id。带上它时按身份精确
///   重连，不依赖 brief 指纹；未命中任何弹窗则返回 TokenExpired，**不**回落到 key 匹配。
///   这是唯一允许「brief 与首次不同」却复用弹窗的路径——身份由令牌证明，不靠猜。
/// - `abort_flag`：外部（如心跳任务）检测到客户端连接已断开时置 false，轮询将提前中止以避免空等。
pub fn poll_or_start_popup(
    request: &PopupRequest,
    wait: Duration,
    abort_flag: Option<Arc<AtomicBool>>,
    resume_token: Option<&str>,
) -> Result<PopupPoll> {
    let key = popup_key(request);

    // 中文说明（2026-09-15）：acquire_popup 在同一临界区内完成「从注册表取出 + 登记轮询中」，
    // 返回时该弹窗已经在 POLLING_IN_FLIGHT 里；这里不再二次登记，避免两步之间的空窗
    // 被并发调用误判为「无人持有」而新建/误报令牌失效。
    // 后续放回一律用 acquire_popup 返回的实际 key（令牌重连时是旧 key），不用本次参数算出的 key。
    let (pending, key) = match acquire_popup(request, &key, wait, resume_token)? {
        Acquired::Popup(p, k) => (p, k),
        Acquired::AlreadyAnswered => {
            return Ok(PopupPoll::Done(
                "用户已通过另一个活跃的 zhi 弹窗完成了响应，本次调用无需再等。".to_string(),
            ))
        }
        // 中文说明（2026-09-15）：取消不是确认。合成 CANCELLED 交给 zhi 解析层，
        // 走「请再次调用 zhi」的重新确认流程，而不是「已通过另一路完成响应」。
        Acquired::Cancelled => return Ok(PopupPoll::Done("CANCELLED".to_string())),
        Acquired::TokenExpired(token) => return Ok(PopupPoll::TokenExpired { token }),
    };
    let request_id = pending.request_id.clone();

    let result = do_poll_loop(&key, pending, wait, abort_flag.as_ref());

    // Pending / Suspended 已由 park_popup 在放回注册表的同一临界区内撤销轮询标记；
    // 只有 Done 与 Err（弹窗被消费或出错，不再放回）需要在这里撤销。
    // 真实提交记 Done、取消记 Cancelled，都必须在撤销标记之前写入，等待方看到标记
    // 消失时才能区分「已回答」与「需重新确认」。
    match &result {
        Ok(PopupPoll::Pending { .. }) | Ok(PopupPoll::Suspended { .. }) => {}
        Ok(PopupPoll::Done(response)) => {
            let outcome = if is_cancel_signal(response) {
                RecentOutcome::Cancelled
            } else {
                RecentOutcome::Done
            };
            record_outcome(&request_id, outcome)?;
            unmark_polling(&key, &request_id)?;
        }
        _ => unmark_polling(&key, &request_id)?,
    }

    result
}

/// 锁顺序约定：需要同时持有两张表时，**先 PENDING_POPUPS，后 POLLING_IN_FLIGHT**；全文件遵守。

/// 在调用方已持有注册表锁的前提下，把 `key` 登记为「正在被 request_id 轮询」。
fn mark_polling_locked(key: &str, request_id: &str) -> Result<()> {
    let mut polling = POLLING_IN_FLIGHT
        .lock()
        .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
    polling.insert(key.to_string(), request_id.to_string());
    Ok(())
}

/// 撤销 `key` 的轮询标记，但只在标记确属 `request_id` 时才撤——防止把后来接手同 key 的
/// 另一路调用的标记误删（那会让第三路调用误以为无人持有而新建重复弹窗）。
fn unmark_polling(key: &str, request_id: &str) -> Result<()> {
    let mut polling = POLLING_IN_FLIGHT
        .lock()
        .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
    if polling.get(key).map(|rid| rid == request_id).unwrap_or(false) {
        polling.remove(key);
    }
    Ok(())
}

/// 把弹窗放回注册表并撤销轮询标记，两步在注册表锁内完成，对外表现为原子切换。
fn park_popup(key: &str, pending: PendingPopup) -> Result<()> {
    let mut map = PENDING_POPUPS
        .lock()
        .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
    let request_id = pending.request_id.clone();
    map.insert(key.to_string(), pending);
    unmark_polling(key, &request_id)
}

/// 获取弹窗：从注册表取出已有弹窗（重连），或等待并发轮询释放后重连，或新建。
///
/// 中文说明（2026-09-15）：所有「查注册表 → 查轮询表 → 决定取出/新建」的判断都在持有
/// 注册表锁的同一临界区内完成，取出/预留的同时登记轮询标记。返回 Popup 时该弹窗已登记为
/// 轮询中，调用方无需再登记。令牌重连返回的是该弹窗在注册表中的原始 key；令牌未命中任何
/// 弹窗时返回 TokenExpired，绝不降级到 key 匹配。
fn acquire_popup(
    request: &PopupRequest,
    key: &str,
    wait: Duration,
    resume_token: Option<&str>,
) -> Result<Acquired> {
    // 0) 带 resume_token：按弹窗身份精确重连（允许 brief 已改写）。
    if let Some(token) = resume_token.map(str::trim).filter(|t| !t.is_empty()) {
        // 注册表与轮询表在同一临界区内查，令牌不可能在两次查之间被移走。
        let polled_key = {
            let mut map = PENDING_POPUPS
                .lock()
                .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
            let hit = map
                .iter()
                .find(|(_, p)| p.request_id == token)
                .map(|(k, _)| k.clone());
            if let Some(old_key) = hit {
                if let Some(mut p) = map.remove(&old_key) {
                    p.reconnects = p.reconnects.saturating_add(1);
                    p.hold_until = None;
                    mark_polling_locked(&old_key, &p.request_id)?;
                    log_important!(
                        info,
                        "[popup] 按 resume_token 重连弹窗 #{}: request_id={}, key={}（本次参数 key={}，保留原 key）, 已等待={}s",
                        p.reconnects,
                        p.request_id,
                        old_key,
                        key,
                        p.started.elapsed().as_secs()
                    );
                    return Ok(Acquired::Popup(p, old_key));
                }
            }
            POLLING_IN_FLIGHT
                .lock()
                .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?
                .iter()
                .find(|(_, rid)| rid.as_str() == token)
                .map(|(k, _)| k.clone())
        };
        if let Some(polled_key) = polled_key {
            log_important!(
                info,
                "[popup] resume_token 对应弹窗正被另一个调用轮询，等待释放后重连: token={}, key={}",
                token,
                polled_key
            );
            // strict=true：令牌等待固定 request_id，同 key 换成别的弹窗时绝不跟随、绝不取走。
            return Ok(match wait_for_polling_release(&polled_key, token, wait, true)? {
                Released::Taken(p) => Acquired::Popup(p, polled_key),
                Released::Answered => Acquired::AlreadyAnswered,
                Released::Cancelled => Acquired::Cancelled,
                // 持有方出错退出、弹窗已不存在：令牌指向的弹窗没了，按失效处理。
                Released::Vanished => Acquired::TokenExpired(token.to_string()),
            });
        }
        // 令牌既不在注册表也不在轮询中：已完成/已回收/伪造。明确报错，不降级。
        log_important!(
            warn,
            "[popup] resume_token 未命中任何弹窗（已完成或已回收），拒绝降级到 key 匹配: token={}",
            token
        );
        return Ok(Acquired::TokenExpired(token.to_string()));
    }

    // 1) 精确 key：取出即登记；未命中则在同一临界区内判断是否正被轮询，否则预留后新建。
    //    若等待到的是「持有方启动失败/出错、标记消失」，回到这里重新获取（届时由本方新建）。
    let deadline = Instant::now() + wait;
    loop {
        let polling_holder = {
            let mut map = PENDING_POPUPS
                .lock()
                .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
            reap_abandoned_popups(&mut map, key);
            if let Some(mut p) = map.remove(key) {
                p.hold_until = None;
                p.reconnects = p.reconnects.saturating_add(1);
                mark_polling_locked(key, &p.request_id)?;
                log_important!(
                    info,
                    "[popup] 重连弹窗 #{}: key={}, request_id={}, 已等待={}s（重连越多越接近 Cursor 单轮预算上限→易被动新开 request）",
                    p.reconnects,
                    key,
                    p.request_id,
                    p.started.elapsed().as_secs()
                );
                return Ok(Acquired::Popup(p, key.to_string()));
            }
            let mut polling = POLLING_IN_FLIGHT
                .lock()
                .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
            match polling.get(key) {
                // 记下当前持有者的 request_id，等待方凭它区分「已回答」与「持有方失败退出」。
                Some(holder) => Some(holder.clone()),
                None => {
                    // 先预留轮询标记再 spawn：并发同 key 调用在 spawn 期间到来会看到「正被轮询」而等待，
                    // 不会再各自新建一个弹窗。request.id 在 spawn 前已知。
                    polling.insert(key.to_string(), request.id.clone());
                    None
                }
            }
        };

        let Some(holder_rid) = polling_holder else {
            return match start_popup(request) {
                Ok(p) => Ok(Acquired::Popup(p, key.to_string())),
                Err(e) => {
                    // spawn 失败要撤销预留，否则后续同 key 调用会一直误以为有人在轮询。
                    // 不写 RECENT_OUTCOMES，等待方会据此判为 Vanished 并自行重试新建。
                    let _ = unmark_polling(key, &request.id);
                    Err(e)
                }
            };
        };

        // 同 key 弹窗正在被另一个 zhi 调用轮询中 → 等待其释放后重连，避免创建重复弹窗
        log_important!(
            info,
            "[popup] 同 key 弹窗正在被另一个 zhi 调用轮询中，等待释放后重连: key={}",
            key
        );
        let remaining = deadline.saturating_duration_since(Instant::now());
        // strict=false：普通 key 重连只关心「同 key 有没有弹窗可接手」，持有者换人就跟随。
        match wait_for_polling_release(key, &holder_rid, remaining, false)? {
            Released::Taken(p) => return Ok(Acquired::Popup(p, key.to_string())),
            Released::Answered => return Ok(Acquired::AlreadyAnswered),
            Released::Cancelled => return Ok(Acquired::Cancelled),
            Released::Vanished => {
                if Instant::now() >= deadline {
                    anyhow::bail!("同 key 弹窗的持有方已退出且未产生结果，等待窗口耗尽");
                }
                log_important!(
                    warn,
                    "[popup] 同 key 弹窗的持有方未产生结果即退出（启动失败/出错），本方重新获取: key={}",
                    key
                );
                continue;
            }
        }
    }
}

/// 等待正被另一路调用轮询的 `key` 弹窗释放。
///
/// `Taken` 表示轮询方超时放回、本方成功接手（已登记轮询）；`Answered` 表示轮询方已以 Done
/// 收口（RECENT_OUTCOMES 记为 Done）；`Cancelled` 表示用户取消/关窗（不是确认）；
/// `Vanished` 表示标记消失却无收口记录——持有方启动失败或出错退出，弹窗并不存在，
/// 调用方应自行重新获取；等满 `wait` 仍未释放则报错。
/// 「放回注册表」与「仍在轮询」两个判断在同一临界区内完成，不会看到中间态。
fn wait_for_polling_release(
    key: &str,
    expected_rid: &str,
    wait: Duration,
    strict: bool,
) -> Result<Released> {
    let deadline = Instant::now() + wait;
    // 中文说明：非 strict（普通 key 重连）时持有者可能换人（A 放回、B 接手），跟随最近一次看到的
    // 持有者；strict（resume_token 精确重连）时 request_id 固定不变，见 decide_release。
    let mut expected_rid = expected_rid.to_string();
    loop {
        std::thread::sleep(Duration::from_millis(500));

        if Instant::now() >= deadline {
            log_important!(
                warn,
                "[popup] 等待并发轮询释放超时（弹窗仍被另一个 zhi 调用持有）: key={}, request_id={}",
                key,
                expected_rid
            );
            anyhow::bail!(
                "同一弹窗正在被另一个 zhi 调用轮询中且未在等待窗口内释放，请稍后重试"
            );
        }

        // 注册表、轮询表、完成记录三者在同一临界区内读取并决策，取走也在同一临界区内完成。
        let mut map = PENDING_POPUPS
            .lock()
            .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
        let in_map_rid = map.get(key).map(|p| p.request_id.clone());
        let holder_rid = POLLING_IN_FLIGHT
            .lock()
            .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?
            .get(key)
            .cloned();
        let outcome_for_expected = recent_outcome(&expected_rid)?;
        let decision = decide_release(
            in_map_rid.as_deref(),
            holder_rid.as_deref(),
            &mut expected_rid,
            strict,
            outcome_for_expected,
        );
        match decision {
            ReleaseDecision::Take => {
                let mut p = map
                    .remove(key)
                    .ok_or_else(|| anyhow::anyhow!("决策为接手但注册表已无该弹窗（不可能的状态）"))?;
                p.reconnects = p.reconnects.saturating_add(1);
                p.hold_until = None;
                mark_polling_locked(key, &p.request_id)?;
                log_important!(
                    info,
                    "[popup] 并发轮询已释放弹窗，成功重连 #{}: key={}, request_id={}",
                    p.reconnects,
                    key,
                    p.request_id
                );
                return Ok(Released::Taken(p));
            }
            ReleaseDecision::KeepWaiting => {}
            ReleaseDecision::Answered => {
                log_important!(
                    info,
                    "[popup] 并发轮询已完成（用户已响应），无需新建弹窗: key={}, request_id={}",
                    key,
                    expected_rid
                );
                return Ok(Released::Answered);
            }
            ReleaseDecision::Cancelled => {
                log_important!(
                    info,
                    "[popup] 并发轮询收到取消/关窗，用户未给出内容，继续重新确认: key={}, request_id={}",
                    key,
                    expected_rid
                );
                return Ok(Released::Cancelled);
            }
            ReleaseDecision::Vanished => {
                log_important!(
                    info,
                    "[popup] 所等弹窗已不存在且无完成记录: key={}, request_id={}, strict={}",
                    key,
                    expected_rid,
                    strict
                );
                return Ok(Released::Vanished);
            }
        }
    }
}

/// 等待方在一次观察后的决策。
#[derive(Debug, PartialEq, Eq)]
enum ReleaseDecision {
    /// 注册表里的弹窗可以接手。
    Take,
    /// 所等弹窗仍被持有，继续等。
    KeepWaiting,
    /// 所等弹窗已以 Done 收口。
    Answered,
    /// 所等弹窗被用户取消/关闭，不是确认。
    Cancelled,
    /// 所等弹窗不存在且无完成记录（持有方失败退出，或 strict 模式下已被别的弹窗替代）。
    Vanished,
}

/// 纯决策函数：根据「注册表里是谁、轮询表里是谁、我在等谁」决定下一步。
///
/// 中文说明（2026-09-15）：
/// - `strict=false`（普通 key 重连）：只关心同 key 有没有弹窗可接手。注册表里有就接手；
///   持有者换人就把 expected 跟过去；标记消失时按 expected 查完成记录。
/// - `strict=true`（resume_token 精确重连）：request_id 固定。注册表里若是**别的** request_id，
///   或持有者是**别的** request_id，都说明令牌指向的弹窗已经不在了——不跟随、不取走，
///   按 expected 的完成记录判 Answered / Cancelled / Vanished。这是「旧令牌不得接管新弹窗」的落点。
fn decide_release(
    in_map_rid: Option<&str>,
    holder_rid: Option<&str>,
    expected: &mut String,
    strict: bool,
    outcome_for_expected: Option<RecentOutcome>,
) -> ReleaseDecision {
    let finished = |outcome: Option<RecentOutcome>| match outcome {
        Some(RecentOutcome::Done) => ReleaseDecision::Answered,
        Some(RecentOutcome::Cancelled) => ReleaseDecision::Cancelled,
        None => ReleaseDecision::Vanished,
    };
    if let Some(rid) = in_map_rid {
        if !strict || rid == expected.as_str() {
            return ReleaseDecision::Take;
        }
        // strict 且注册表里是别的弹窗：令牌那个已不存在
        return finished(outcome_for_expected);
    }
    if let Some(rid) = holder_rid {
        if rid == expected.as_str() {
            return ReleaseDecision::KeepWaiting;
        }
        if strict {
            return finished(outcome_for_expected);
        }
        *expected = rid.to_string();
        return ReleaseDecision::KeepWaiting;
    }
    finished(outcome_for_expected)
}

/// 轮询弹窗直到用户响应（Done）或超时（Pending）。
/// `abort_flag` 为 false 时表示客户端连接已断开，应立即停止等待。
fn do_poll_loop(key: &str, pending: PendingPopup, wait: Duration, abort_flag: Option<&Arc<AtomicBool>>) -> Result<PopupPoll> {
    let deadline = Instant::now() + wait;
    loop {
        if pending.stdout_reader.is_finished() {
            let PendingPopup {
                mut child,
                stdout_reader,
                stderr_reader,
                temp_file,
                request_id,
                started,
                reconnects,
                ..
            } = pending;
            log_important!(
                info,
                "[popup] 弹窗完成: key={}, request_id={}, 总等待={}s, 重连次数={}（重连次数即本次决策额外消耗的 zhi 工具调用数）",
                key,
                request_id,
                started.elapsed().as_secs(),
                reconnects
            );
            let status = child.wait()?;
            let stdout = stdout_reader
                .join()
                .map_err(|_| anyhow::anyhow!("读取 GUI stdout 的线程 panic"))??;
            let stderr = stderr_reader
                .join()
                .map_err(|_| anyhow::anyhow!("读取 GUI stderr 的线程 panic"))??;
            let _ = fs::remove_file(&temp_file);
            let response = collect_response(
                &request_id,
                status.success(),
                status.code(),
                &stdout,
                &stderr,
                started.elapsed().as_millis(),
            )?;
            return Ok(PopupPoll::Done(response));
        }

        // 心跳失败 → 客户端连接已断开，继续等待无意义，立即返回 Pending 让弹窗留存
        if let Some(flag) = abort_flag {
            if !flag.load(Ordering::Relaxed) {
                log_important!(
                    warn,
                    "[popup] 心跳检测到客户端已断开，提前结束轮询: key={}, request_id={}, 已等待={}s",
                    key,
                    pending.request_id,
                    pending.started.elapsed().as_secs()
                );
                let mut pending = pending;
                pending.hold_until = None;
                let resume_token = pending.request_id.clone();
                park_popup(key, pending)?;
                return Ok(PopupPoll::Pending { resume_token });
            }
        }

        if Instant::now() >= deadline {
            let reconnects = pending.reconnects;
            let waited_secs = pending.started.elapsed().as_secs();

            if reconnects >= MAX_POPUP_RECONNECTS {
                // 重连次数已达上限，挂起而非继续要求 AI 重连（节省 token）
                log_important!(
                    warn,
                    "[popup] 重连次数已达上限({}/{})，挂起弹窗不再要求 AI 重连: key={}, request_id={}, 已等待={}s",
                    reconnects,
                    MAX_POPUP_RECONNECTS,
                    key,
                    pending.request_id,
                    waited_secs
                );
                let mut pending = pending;
                pending.hold_until = None;
                park_popup(key, pending)?;
                return Ok(PopupPoll::Suspended { reconnects, waited_secs });
            }

            log_important!(
                info,
                "[popup] 等待窗口({}s)到，弹窗仍开启→返回 Pending 待 AI 重连: key={}, request_id={}, 已等待={}s, 当前重连次数={}",
                wait.as_secs(),
                key,
                pending.request_id,
                waited_secs,
                reconnects
            );
            let mut pending = pending;
            pending.hold_until = Some(Instant::now() + RECONNECT_HOLD);
            let resume_token = pending.request_id.clone();
            park_popup(key, pending)?;
            return Ok(PopupPoll::Pending { resume_token });
        }

        std::thread::sleep(POPUP_POLL_INTERVAL);
    }
}

/// 注册表关联键：workspace 绝对路径 + 弹窗内容指纹；workspace 缺失时回退到 request_id。
///
/// 中文说明（2026-06-11 修复·跨会话串弹窗）：旧版仅用 workspace 作键——同一 workspace
/// 开两个会话时，B 会话的 zhi 会「重连」到 A 会话仍在等待的弹窗，用户回答的是 A 的问题、
/// 答案却被 B 拿走（Done 错配），与同日 stop-hook 跨窗口误拦问题同构。现把 brief 内容
/// 指纹并入键：同一问题的保活重连（参数不变）仍精确复用弹窗；不同问题（哪怕同 workspace）
/// 各开各的弹窗，互不窜扰。指纹仅在本进程内存注册表中使用，无需跨进程稳定。
/// 边界：同一会话重开时若改写 brief（如附注「上次弹窗超时」），指纹变化会使旧弹窗不被
/// 复用而残留——由 reap + 孤儿回复持久化兜底，用户提交的内容不会丢。
fn popup_key(request: &PopupRequest) -> String {
    use std::hash::{Hash, Hasher};
    let base = request
        .project_root_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| request.id.clone());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.message.hash(&mut hasher);
    format!("{}#{:016x}", base, hasher.finish())
}

/// 回收注册表里「已退出且不在重连保留期内」的弹窗；`skip` 为调用方自己的 key。
///
/// 中文说明（2026-09-14）：保留期内的 Pending 条目即使 GUI 已退出也不回收——那份回复
/// 属于即将重连的 AI 调用，重连后 do_poll_loop 会立即以 Done 交付。
fn reap_abandoned_popups(map: &mut HashMap<String, PendingPopup>, skip: &str) {
    let now = Instant::now();
    let abandoned: Vec<String> = map
        .iter()
        .filter(|(k, p)| k.as_str() != skip && p.stdout_reader.is_finished())
        .filter(|(_, p)| !p.hold_until.map(|t| t > now).unwrap_or(false))
        .map(|(k, _)| k.clone())
        .collect();
    for k in abandoned {
        if let Some(p) = map.remove(&k) {
            let PendingPopup {
                mut child,
                stdout_reader,
                stderr_reader,
                temp_file,
                request_id,
                started,
                reconnects,
                ..
            } = p;
            let status = child.wait();
            let stdout = stdout_reader.join().ok().and_then(|r| r.ok()).unwrap_or_default();
            let _ = stderr_reader.join();
            let _ = fs::remove_file(&temp_file);

            // 中文说明（2026-06-11 修复·黑洞回复）：旧版在这里直接丢弃 stdout——
            // 挂起/断流后用户才在弹窗里提交的回答会静默消失（UI 还显示提交成功）。
            // 现把有效回复持久化到 ~/.sanshu/orphan_replies/，下次同 workspace 的 zhi
            // 完成时会附带提示，AI/用户可按路径取回。
            let exited_ok = status.map(|s| s.success()).unwrap_or(false);
            let response_text = String::from_utf8_lossy(&stdout);
            let response_text = response_text.trim();
            if exited_ok && !response_text.is_empty() {
                save_orphan_reply(&k, &request_id, response_text);
            }

            log_important!(
                info,
                "[popup] 已回收遗弃弹窗: key={}, request_id={}, 存活={}s, 重连次数={}, 有回复待送达={}（对话很可能已被新开 request 打断，旧弹窗无人重连）",
                k,
                request_id,
                started.elapsed().as_secs(),
                reconnects,
                exited_ok && !response_text.is_empty()
            );
        }
    }
}

/// 孤儿回复持久化目录：~/.sanshu/orphan_replies/
fn orphan_replies_dir() -> PathBuf {
    // 中文说明：允许用环境变量覆盖，供测试落到临时目录，不污染真实 ~/.sanshu。
    if let Some(dir) = std::env::var_os("SANSHU_ORPHAN_REPLIES_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sanshu")
        .join("orphan_replies")
}

/// 持久化「无人轮询时用户才提交」的弹窗回复，避免静默丢失。
fn save_orphan_reply(key: &str, request_id: &str, response: &str) {
    let dir = orphan_replies_dir();
    if fs::create_dir_all(&dir).is_err() {
        log_important!(warn, "[popup] 孤儿回复目录创建失败: key={}", key);
        return;
    }
    let file = dir.join(format!("{}.json", request_id));
    let payload = serde_json::json!({
        "key": key,
        "request_id": request_id,
        "saved_at": chrono::Utc::now().to_rfc3339(),
        "response": response,
    });
    let write_result = serde_json::to_string_pretty(&payload).map(|s| fs::write(&file, s));
    match write_result {
        Ok(Ok(())) => log_important!(
            warn,
            "[popup] 用户回复无人接收，已持久化为孤儿回复: file={}, key={}, response_len={}",
            file.display(),
            key,
            response.len()
        ),
        _ => log_important!(warn, "[popup] 孤儿回复持久化失败: key={}", key),
    }
}

/// 取走 workspace 下「未送达孤儿回复」的一次性提示文本。
///
/// 中文说明（2026-06-11 新增）：在下一次同 workspace 的 zhi 正常完成时调用；
/// 命中的文件改名为 `.seen.json` 防止重复提示，文件本体保留供 AI/用户按路径读取。
pub fn take_orphan_reply_notice(workspace: &str) -> Option<String> {
    if workspace.trim().is_empty() {
        return None;
    }
    let dir = orphan_replies_dir();
    let read_dir = fs::read_dir(&dir).ok()?;

    let mut hits: Vec<PathBuf> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.ends_with(".json") || name.ends_with(".seen.json") {
            continue;
        }
        // 中文说明：只去掉最后一个 # 后的指纹，再精确比较完整路径，避免 /app 误领 /app-old，
        // 同时保留 workspace 路径本身可能包含的 #。
        let belongs = fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| {
                v.get("key")
                    .and_then(|k| k.as_str())
                    .and_then(|k| k.rsplit_once('#'))
                    .map(|(root, _)| root == workspace)
            })
            .unwrap_or(false);
        if belongs {
            hits.push(path);
        }
    }
    if hits.is_empty() {
        return None;
    }

    // 中文说明（2026-09-15）：改名即领取；改名失败说明已被并发的另一路领走，不再列出。
    let mut listed: Vec<String> = Vec::new();
    for path in &hits {
        let seen = path.with_extension("seen.json");
        if fs::rename(path, &seen).is_ok() {
            listed.push(seen.display().to_string());
        }
    }
    if listed.is_empty() {
        return None;
    }
    log_important!(
        info,
        "[popup] 发现 {} 条未送达的孤儿回复，已随本次 zhi 返回提示: workspace={}",
        listed.len(),
        workspace
    );
    Some(format!(
        "ℹ️ 另有 {} 条历史弹窗回复未送达（用户在 AI 停止轮询后才提交，与本次提问无关）。\
         如可能与当前任务相关，可读取以下文件查看：\n{}",
        listed.len(),
        listed.join("\n")
    ))
}

/// 按 request_id 领取到的一条孤儿回复：原始响应文本 + 领取后的文件路径。
pub struct OrphanReply {
    /// 领取后（已改名为 `.seen.json`）的文件路径。
    pub path: PathBuf,
    /// GUI 当时写出的原始响应文本（未解析），交给 parse_mcp_response_with_structured 处理。
    pub response: String,
}

/// 按 request_id 精确领取一条孤儿回复（用于 resume_token 失效场景）。
///
/// 中文说明（2026-09-15）：孤儿文件以弹窗 request_id 命名，令牌即 request_id，可精确定位。
/// 中文说明：先读取并校验，失败时保留未读文件供后续重试；成功后以原子改名取得领取权。
/// 并发请求可以同时读取，但只有改名成功的一方能返回回复，失败的一方不能交付已读到的内容。
/// 返回原始响应而不是拼好的文本，由调用方走正常解析路径区分「有效回复 / 取消 / 空」。
pub fn take_orphan_reply_for_request(request_id: &str) -> Option<OrphanReply> {
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.contains(['/', '\\', '.']) {
        return None;
    }
    let path = orphan_replies_dir().join(format!("{}.json", request_id));
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log_important!(
                warn,
                "[popup] 孤儿回复读取失败，未标记已读: request_id={}, file={}, error={}",
                request_id,
                path.display(),
                e
            );
            return None;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(e) => {
            log_important!(
                warn,
                "[popup] 孤儿回复 JSON 解析失败，未标记已读: request_id={}, file={}, error={}",
                request_id,
                path.display(),
                e
            );
            return None;
        }
    };
    let Some(response) = value.get("response").and_then(|v| v.as_str()) else {
        log_important!(
            warn,
            "[popup] 孤儿回复缺少字符串 response，未标记已读: request_id={}, file={}",
            request_id,
            path.display()
        );
        return None;
    };
    let response = response.to_string();
    // 中文说明：改名是唯一的领取成功条件，必须在校验后执行；失败时不能返回已读取的回复。
    let seen = path.with_extension("seen.json");
    if let Err(e) = fs::rename(&path, &seen) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log_important!(
                warn,
                "[popup] 孤儿回复领取失败，未交付: request_id={}, file={}, error={}",
                request_id,
                path.display(),
                e
            );
        }
        return None;
    }
    log_important!(
        info,
        "[popup] 按 request_id 领取孤儿回复: request_id={}, file={}, response_len={}",
        request_id,
        seen.display(),
        response.len()
    );
    Some(OrphanReply { path: seen, response })
}

/// 启动一个 GUI 弹窗子进程，并起后台线程持续读取 stdout/stderr。
fn start_popup(request: &PopupRequest) -> Result<PendingPopup> {
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request.id));
    let request_json = serde_json::to_string_pretty(request)?;
    fs::write(&temp_file, request_json)?;

    log_important!(
        info,
        "[popup] 已写入MCP请求文件: request_id={}, file={}, message_len={}, message_preview={}, options_len={}, project={:?}, markdown={}",
        request.id,
        temp_file.display(),
        request.message.len(),
        safe_truncate_clean(&request.message, 200),
        request.predefined_options.as_ref().map(|v| v.len()).unwrap_or(0),
        request.project_root_path.as_deref(),
        request.is_markdown
    );

    let command_path = find_ui_command()?;
    log_debug!(
        "[popup] 启动GUI子进程: request_id={}, command_path={}",
        request.id,
        command_path
    );

    let mut child = Command::new(&command_path)
        .arg("--mcp-request")
        .arg(temp_file.to_string_lossy().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // 取出管道，交给后台线程持续读取到 EOF，避免大响应写满管道阻塞 GUI 进程。
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取 GUI 子进程 stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法获取 GUI 子进程 stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).map(|_| buf)
    });

    log_important!(
        info,
        "[popup] 新建弹窗: request_id={}, project={:?}（首次展示，reconnects=0）",
        request.id,
        request.project_root_path.as_deref()
    );

    ensure_reaper_running();

    Ok(PendingPopup {
        child,
        stdout_reader,
        stderr_reader,
        temp_file,
        request_id: request.id.clone(),
        started: Instant::now(),
        reconnects: 0,
        hold_until: None,
    })
}

/// 启动（至多一次）后台回收线程：定期 reap 注册表里已退出却无人轮询的弹窗。
///
/// 中文说明（2026-09-14）：见 REAPER_INTERVAL。线程空转成本极低（每 3s 取一次锁）。
fn ensure_reaper_running() {
    if REAPER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("popup-reaper".into())
        .spawn(|| loop {
            std::thread::sleep(REAPER_INTERVAL);
            if let Ok(mut map) = PENDING_POPUPS.lock() {
                if !map.is_empty() {
                    reap_abandoned_popups(&mut map, "");
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| {
            REAPER_STARTED.store(false, Ordering::SeqCst);
            log_important!(warn, "[popup] 后台回收线程启动失败: {}", e);
        });
}

/// 把 GUI 进程的退出输出转成响应文本（语义与 create_tauri_popup 保持一致）。
fn collect_response(
    request_id: &str,
    success: bool,
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    elapsed_ms: u128,
) -> Result<String> {
    if success {
        let response = String::from_utf8_lossy(stdout);
        let response = response.trim();
        log_important!(
            info,
            "[popup] GUI执行成功: request_id={}, exit_code={:?}, stdout_len={}, elapsed_ms={}",
            request_id,
            exit_code,
            stdout.len(),
            elapsed_ms
        );
        // 中文说明（2026-06-11 P1）：巨型回复告警——见 RESPONSE_LEN_WARN_THRESHOLD 注释
        if response.len() > RESPONSE_LEN_WARN_THRESHOLD {
            log_important!(
                warn,
                "[popup] 用户回复超长: request_id={}, len={}（阈值={}）——将原样回传模型，token 消耗巨大，疑似大段粘贴",
                request_id,
                response.len(),
                RESPONSE_LEN_WARN_THRESHOLD
            );
        }
        if response.is_empty() {
            // 中文说明（2026-09-14）：同 create_tauri_popup——退出码 0 且 stdout 为空不是取消，
            // 是 GUI 未走提交/取消链路就退出了，按异常上报以便与真取消区分。
            log_important!(
                warn,
                "[popup] GUI 以退出码 0 结束但 stdout 为空（未收到用户响应，也不是显式取消）: request_id={}, stderr_preview={}, elapsed_ms={}",
                request_id,
                safe_truncate_clean(&String::from_utf8_lossy(stderr), 200),
                elapsed_ms
            );
            anyhow::bail!(
                "弹窗进程已退出但未返回任何响应（既非用户提交也非显式取消，可能是窗口被直接关闭或 GUI 异常退出）"
            );
        }
        Ok(response.to_string())
    } else {
        let error = String::from_utf8_lossy(stderr);
        log_important!(
            error,
            "[popup] GUI执行失败: request_id={}, exit_code={:?}, stderr_preview={}, elapsed_ms={}",
            request_id,
            exit_code,
            safe_truncate_clean(&error, 200),
            elapsed_ms
        );
        anyhow::bail!("UI进程失败: {}", error);
    }
}

/// 查找等一下 UI 命令的路径
///
/// 按优先级查找：同目录 -> 全局版本 -> 开发环境
fn find_ui_command() -> Result<String> {
    // 1. 优先尝试与当前 MCP 服务器同目录的等一下命令
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let local_ui_path = exe_dir.join("等一下");
            if local_ui_path.exists() && is_executable(&local_ui_path) {
                return Ok(local_ui_path.to_string_lossy().to_string());
            }
        }
    }

    // 2. 尝试全局命令（最常见的部署方式）
    if test_command_available("等一下") {
        return Ok("等一下".to_string());
    }

    // 3. 如果都找不到，返回详细错误信息
    anyhow::bail!(
        "找不到等一下 UI 命令。请确保：\n\
         1. 已编译项目：cargo build --release\n\
         2. 或已全局安装：./install.sh\n\
         3. 或等一下命令在同目录下"
    )
}

/// 测试命令是否可用
fn test_command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 检查文件是否可执行
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        // Windows 上检查文件扩展名
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod popup_registry_tests {
    use super::*;

    fn req(project: Option<&str>, message: &str, id: &str) -> PopupRequest {
        PopupRequest {
            id: id.to_string(),
            message: message.to_string(),
            predefined_options: None,
            is_markdown: false,
            project_root_path: project.map(|p| p.to_string()),
            agent_label: None,
            uiux_intent: None,
            uiux_context_policy: None,
            uiux_reason: None,
        }
    }

    #[test]
    fn key_isolates_different_briefs_in_same_project() {
        assert!(popup_key(&req(Some("/ws"), "m", "rid")).starts_with("/ws#"));
        assert!(popup_key(&req(None, "m", "rid")).starts_with("rid#"));
        assert_ne!(
            popup_key(&req(Some("/ws"), "m1", "rid")),
            popup_key(&req(Some("/ws"), "m2", "rid"))
        );
        assert_eq!(
            popup_key(&req(Some("/ws"), "m", "a")),
            popup_key(&req(Some("/ws"), "m", "b"))
        );
    }

    #[test]
    fn exit_zero_with_empty_stdout_is_an_error_not_a_cancel() {
        let r = collect_response("rid", true, Some(0), b"", b"", 10);
        assert!(r.is_err());
        let r = collect_response("rid", true, Some(0), b"\"CANCELLED\"", b"", 10).unwrap();
        assert_eq!(r, "\"CANCELLED\"");
    }
}

#[cfg(test)]
mod orphan_reply_tests {
    use super::*;

    #[test]
    fn take_for_request_rejects_path_like_ids() {
        assert!(take_orphan_reply_for_request("").is_none());
        assert!(take_orphan_reply_for_request("../x").is_none());
        assert!(take_orphan_reply_for_request("a/b").is_none());
        assert!(take_orphan_reply_for_request("evil.json").is_none());
    }

    #[test]
    fn take_for_request_returns_only_that_request_and_marks_seen() {
        /// 作用域结束（含 panic 展开）时恢复环境变量，不污染同进程其它测试。
        struct EnvGuard(Option<std::ffi::OsString>);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => std::env::set_var("SANSHU_ORPHAN_REPLIES_DIR", v),
                    None => std::env::remove_var("SANSHU_ORPHAN_REPLIES_DIR"),
                }
            }
        }
        let _guard = EnvGuard(std::env::var_os("SANSHU_ORPHAN_REPLIES_DIR"));
        let dir = std::env::temp_dir().join(format!("sanshu-orphan-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        std::env::set_var("SANSHU_ORPHAN_REPLIES_DIR", &dir);
        assert_eq!(orphan_replies_dir(), dir);
        let rid_a = format!("test-a-{}", std::process::id());
        let rid_b = format!("test-b-{}", std::process::id());
        save_orphan_reply("/ws#k", &rid_a, "answer A");
        save_orphan_reply("/ws#k", &rid_b, "answer B");

        let got = take_orphan_reply_for_request(&rid_a).expect("hit");
        assert_eq!(got.response, "answer A");
        assert!(got.path.to_string_lossy().ends_with(&format!("{}.seen.json", rid_a)));
        // A 已改名为 .seen.json，B 原样保留
        assert!(!dir.join(format!("{}.json", rid_a)).exists());
        assert!(dir.join(format!("{}.seen.json", rid_a)).exists());
        assert!(dir.join(format!("{}.json", rid_b)).exists());
        // 再取 A 应为 None（领取权只有一次）
        assert!(take_orphan_reply_for_request(&rid_a).is_none());

        // 并发领取：多个线程同时领同一条，恰好一个成功
        let rid_c = format!("test-c-{}", std::process::id());
        save_orphan_reply("/ws#k", &rid_c, "answer C");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let rid = rid_c.clone();
                std::thread::spawn(move || take_orphan_reply_for_request(&rid).is_some())
            })
            .collect();
        let wins = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|claimed| *claimed)
            .count();
        assert_eq!(wins, 1, "同一孤儿回复只能被领取一次");

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod recent_outcome_tests {
    use super::*;

    #[test]
    fn done_record_is_per_request_not_per_key() {
        let rid_old = format!("gen1-{}", std::process::id());
        let rid_new = format!("gen2-{}", std::process::id());
        record_outcome(&rid_old, RecentOutcome::Done).unwrap();
        assert_eq!(recent_outcome(&rid_old).unwrap(), Some(RecentOutcome::Done));
        // 同 key 的下一代弹窗有不同 request_id：上一代的完成记录不得让它被判为「已回答」
        assert_eq!(recent_outcome(&rid_new).unwrap(), None);
    }

    #[test]
    fn cancel_is_not_recorded_as_done() {
        let rid = format!("cancel-{}", std::process::id());
        record_outcome(&rid, RecentOutcome::Cancelled).unwrap();
        assert_eq!(
            recent_outcome(&rid).unwrap(),
            Some(RecentOutcome::Cancelled)
        );
        assert_ne!(recent_outcome(&rid).unwrap(), Some(RecentOutcome::Done));
    }
}

#[cfg(test)]
mod release_decision_tests {
    use super::*;

    fn exp(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn strict_never_follows_or_takes_a_different_request() {
        // 等旧令牌 A；同 key 新弹窗 B 已放回注册表 → 不能取走 B
        let mut e = exp("A");
        assert_eq!(
            decide_release(Some("B"), None, &mut e, true, None),
            ReleaseDecision::Vanished
        );
        assert_eq!(e, "A");
        // A 已 Done：判 Answered，而不是把 B 当 A
        let mut e = exp("A");
        assert_eq!(
            decide_release(Some("B"), None, &mut e, true, Some(RecentOutcome::Done)),
            ReleaseDecision::Answered
        );
        // 持有者变成 B：不跟随
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, Some("B"), &mut e, true, None),
            ReleaseDecision::Vanished
        );
        assert_eq!(e, "A");
    }

    #[test]
    fn strict_takes_or_waits_only_for_the_same_request() {
        let mut e = exp("A");
        assert_eq!(
            decide_release(Some("A"), None, &mut e, true, None),
            ReleaseDecision::Take
        );
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, Some("A"), &mut e, true, None),
            ReleaseDecision::KeepWaiting
        );
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, None, &mut e, true, Some(RecentOutcome::Done)),
            ReleaseDecision::Answered
        );
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, None, &mut e, true, None),
            ReleaseDecision::Vanished
        );
    }

    #[test]
    fn cancel_is_not_answered_for_waiting_caller() {
        // 标记消失且记录为 Cancelled：等待方必须判 Cancelled，不得当成已回答
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, None, &mut e, false, Some(RecentOutcome::Cancelled)),
            ReleaseDecision::Cancelled
        );
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, None, &mut e, true, Some(RecentOutcome::Cancelled)),
            ReleaseDecision::Cancelled
        );
        // strict 且注册表里是别的弹窗、A 被取消：同样不得接管 B
        let mut e = exp("A");
        assert_eq!(
            decide_release(
                Some("B"),
                None,
                &mut e,
                true,
                Some(RecentOutcome::Cancelled)
            ),
            ReleaseDecision::Cancelled
        );
        assert_eq!(e, "A");
    }

    #[test]
    fn non_strict_follows_holder_and_takes_whatever_is_parked() {
        // 普通 key 重连：注册表里是谁都接手
        let mut e = exp("A");
        assert_eq!(
            decide_release(Some("B"), None, &mut e, false, None),
            ReleaseDecision::Take
        );
        // 持有者换成 B：跟随，继续等
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, Some("B"), &mut e, false, None),
            ReleaseDecision::KeepWaiting
        );
        assert_eq!(e, "B");
        // 之后 B 的标记消失且 B 已 Done → Answered（按 B 查，而不是 A）
        assert_eq!(
            decide_release(None, None, &mut e, false, Some(RecentOutcome::Done)),
            ReleaseDecision::Answered
        );
        // 启动失败场景：标记消失且无 Done → Vanished
        let mut e = exp("A");
        assert_eq!(
            decide_release(None, None, &mut e, false, None),
            ReleaseDecision::Vanished
        );
    }
}
