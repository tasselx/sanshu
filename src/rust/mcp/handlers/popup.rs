use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;

use crate::mcp::types::PopupRequest;
use crate::mcp::utils::safe_truncate_clean;
use crate::{log_debug, log_important};

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
            Ok("用户取消了操作".to_string())
        } else {
            Ok(response.to_string())
        }
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
}

/// 全局「在飞弹窗」注册表，键为 workspace 绝对路径（同一 workspace 同时只允许一个 zhi 弹窗）。
static PENDING_POPUPS: Lazy<Mutex<HashMap<String, PendingPopup>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 正在被轮询中的弹窗 key 集合。
///
/// 弹窗在轮询期间会从 PENDING_POPUPS 取出（获取所有权），此时若另一个 Cursor request
/// 的 zhi 调用进来，会发现注册表为空而误创建重复弹窗。此集合记录「哪些 key 当前正在被
/// 轮询」，新调用发现同 key 正在轮询时会等待其释放后重连，而非新建弹窗。
static POLLING_IN_FLIGHT: Lazy<Mutex<HashSet<String>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

/// 弹窗轮询结果
pub enum PopupPoll {
    /// 用户已响应（GUI 进程已退出），携带响应文本
    Done(String),
    /// 仍在等待用户（本次轮询窗口已到），弹窗保持开启，需 AI 再次调用 zhi 重连
    Pending,
    /// 重连次数已达上限，弹窗仍开着但不再要求 AI 重连（节省 token）
    Suspended { reconnects: u32, waited_secs: u64 },
}

/// 启动或重连弹窗，并轮询至多 `wait` 时长。
///
/// 中文说明：
/// - 同一 workspace 已有在飞弹窗则复用（重连），否则 spawn 新弹窗；
/// - 后台线程持续抽干 stdout/stderr，主线程用 `is_finished()` 判断 GUI 进程是否已退出；
/// - 退出则收集输出作为结果；超过 `wait` 仍未退出则把子进程放回注册表，返回 Pending（不杀弹窗）。
/// - 若同 key 弹窗正在被另一个 zhi 调用轮询中（POLLING_IN_FLIGHT），等待其释放后重连，
///   而非创建重复弹窗。
/// - `abort_flag`：外部（如心跳任务）检测到客户端连接已断开时置 false，轮询将提前中止以避免空等。
pub fn poll_or_start_popup(request: &PopupRequest, wait: Duration, abort_flag: Option<Arc<AtomicBool>>) -> Result<PopupPoll> {
    let key = popup_key(request);

    let pending = acquire_popup(request, &key, wait)?;

    // 如果 acquire_popup 返回 None，说明用户已通过另一个活跃弹窗完成了响应
    let pending = match pending {
        Some(p) => p,
        None => return Ok(PopupPoll::Done(
            "用户已通过另一个活跃的 zhi 弹窗完成了响应，本次调用无需再等。".to_string()
        )),
    };

    // 标记正在轮询，防止并发 zhi 调用为同一 key 创建重复弹窗
    {
        let mut polling = POLLING_IN_FLIGHT
            .lock()
            .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
        polling.insert(key.clone());
    }

    let result = do_poll_loop(&key, pending, wait, abort_flag.as_ref());

    // 无论成功失败，都清除轮询标记
    {
        let mut polling = POLLING_IN_FLIGHT
            .lock()
            .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
        polling.remove(&key);
    }

    result
}

/// 获取弹窗：从注册表取出已有弹窗（重连），或等待并发轮询释放后重连，或新建。
///
/// 返回 `None` 表示并发轮询已完成（用户已通过另一个弹窗响应），无需再等。
fn acquire_popup(request: &PopupRequest, key: &str, wait: Duration) -> Result<Option<PendingPopup>> {
    // 先尝试从注册表直接取出（最常见路径：首次或重连）
    {
        let mut map = PENDING_POPUPS
            .lock()
            .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
        reap_abandoned_popups(&mut map, key);
        if let Some(mut p) = map.remove(key) {
            p.reconnects = p.reconnects.saturating_add(1);
            log_important!(
                info,
                "[popup] 重连弹窗 #{}: key={}, request_id={}, 已等待={}s（重连越多越接近 Cursor 单轮预算上限→易被动新开 request）",
                p.reconnects,
                key,
                p.request_id,
                p.started.elapsed().as_secs()
            );
            return Ok(Some(p));
        }
    }

    // 注册表为空 → 检查是否有并发轮询正在进行
    let is_polling = {
        let polling = POLLING_IN_FLIGHT
            .lock()
            .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
        polling.contains(key)
    };

    if !is_polling {
        // 确实没有活跃弹窗 → 新建
        return Ok(Some(start_popup(request)?));
    }

    // 同 key 弹窗正在被另一个 zhi 调用轮询中 → 等待其释放后重连，避免创建重复弹窗
    log_important!(
        info,
        "[popup] 同 key 弹窗正在被另一个 zhi 调用轮询中，等待释放后重连: key={}",
        key
    );
    let deadline = Instant::now() + wait;
    loop {
        std::thread::sleep(Duration::from_millis(500));

        if Instant::now() >= deadline {
            log_important!(
                warn,
                "[popup] 等待并发轮询释放超时（同 key 弹窗仍被另一个 zhi 调用持有）: key={}",
                key
            );
            anyhow::bail!(
                "同 key 弹窗正在被另一个 zhi 调用轮询中且未在等待窗口内释放，请稍后重试"
            );
        }

        // 检查弹窗是否已被放回注册表（轮询方超时放回）
        {
            let mut map = PENDING_POPUPS
                .lock()
                .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
            if let Some(mut p) = map.remove(key) {
                p.reconnects = p.reconnects.saturating_add(1);
                log_important!(
                    info,
                    "[popup] 并发轮询已释放弹窗，成功重连 #{}: key={}, request_id={}",
                    p.reconnects,
                    key,
                    p.request_id
                );
                return Ok(Some(p));
            }
        }

        // 检查轮询是否已结束（弹窗被消费为 Done，即用户已通过另一个调用响应）
        let still_polling = {
            let polling = POLLING_IN_FLIGHT
                .lock()
                .map_err(|e| anyhow::anyhow!("轮询标记锁中毒: {}", e))?;
            polling.contains(key)
        };
        if !still_polling {
            log_important!(
                info,
                "[popup] 并发轮询已完成（用户已响应），无需新建弹窗: key={}",
                key
            );
            return Ok(None);
        }
    }
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
                let mut map = PENDING_POPUPS
                    .lock()
                    .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
                map.insert(key.to_string(), pending);
                return Ok(PopupPoll::Pending);
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
                let mut map = PENDING_POPUPS
                    .lock()
                    .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
                map.insert(key.to_string(), pending);
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
            let mut map = PENDING_POPUPS
                .lock()
                .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
            map.insert(key.to_string(), pending);
            return Ok(PopupPoll::Pending);
        }

        std::thread::sleep(POPUP_POLL_INTERVAL);
    }
}

/// 注册表关联键：用 workspace 绝对路径；缺失时回退到 request_id。
fn popup_key(request: &PopupRequest) -> String {
    request
        .project_root_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| request.id.clone())
}

/// 回收已遗弃的弹窗条目：GUI 进程已退出（stdout 已读到 EOF）但始终没人重连收集。
///
/// 中文说明：zhi 返回 Pending 后若对话彻底结束、再没有后续重连，对应注册表条目会一直留着；
/// 等用户事后关掉弹窗、进程退出，这条记录就变成「僵尸子进程 + 已结束的读取线程缓冲 + 临时文件」。
/// 在每次启动/重连前顺手扫一遍把它们 reap 掉。`skip` 是本次要重连的 key，跳过它
/// （它若已就绪，会在随后的 remove/重连里被正常收集为 Done，不能在这里提前回收）。
/// 仅回收 `is_finished()` 为真的条目：进程已退出，因此 child.wait()/join 都会立即返回，不会卡住锁。
fn reap_abandoned_popups(map: &mut HashMap<String, PendingPopup>, skip: &str) {
    let abandoned: Vec<String> = map
        .iter()
        .filter(|(k, p)| k.as_str() != skip && p.stdout_reader.is_finished())
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
            } = p;
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            let _ = fs::remove_file(&temp_file);
            log_important!(
                info,
                "[popup] 已回收遗弃弹窗: key={}, request_id={}, 存活={}s, 重连次数={}（对话很可能已被新开 request 打断，旧弹窗无人重连）",
                k,
                request_id,
                started.elapsed().as_secs(),
                reconnects
            );
        }
    }
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

    Ok(PendingPopup {
        child,
        stdout_reader,
        stderr_reader,
        temp_file,
        request_id: request.id.clone(),
        started: Instant::now(),
        reconnects: 0,
    })
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
        if response.is_empty() {
            Ok("用户取消了操作".to_string())
        } else {
            Ok(response.to_string())
        }
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
