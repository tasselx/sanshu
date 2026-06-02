use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
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
/// 反复重置客户端计时器，从而把窗口拉长到 240s（< Cursor ~5 分钟硬上限），
/// 让「等 4.5 分钟」从 10 次往返降到约 1~2 次。超过 240s 仍未响应才返回 Pending 让 AI 重连，
/// 因此任意长的等待仍被覆盖，只是往返次数下降约一个数量级。
pub const POPUP_POLL_WINDOW: Duration = Duration::from_secs(240);
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

/// 弹窗轮询结果
pub enum PopupPoll {
    /// 用户已响应（GUI 进程已退出），携带响应文本
    Done(String),
    /// 仍在等待用户（本次轮询窗口已到），弹窗保持开启，需 AI 再次调用 zhi 重连
    Pending,
}

/// 启动或重连弹窗，并轮询至多 `wait` 时长。
///
/// 中文说明：
/// - 同一 workspace 已有在飞弹窗则复用（重连），否则 spawn 新弹窗；
/// - 后台线程持续抽干 stdout/stderr，主线程用 `is_finished()` 判断 GUI 进程是否已退出；
/// - 退出则收集输出作为结果；超过 `wait` 仍未退出则把子进程放回注册表，返回 Pending（不杀弹窗）。
pub fn poll_or_start_popup(request: &PopupRequest, wait: Duration) -> Result<PopupPoll> {
    let key = popup_key(request);

    // 仅在 remove/insert 时短暂持锁；轮询期间不持锁，避免阻塞其它工具。
    let pending = {
        let mut map = PENDING_POPUPS
            .lock()
            .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
        // 顺手回收已遗弃的陈旧弹窗（进程已退出但一直没人重连收集），避免僵尸进程/线程/临时文件堆积。
        reap_abandoned_popups(&mut map, &key);
        match map.remove(&key) {
            Some(mut p) => {
                // 重连同一弹窗：累加计数并升到 info，便于直接从日志统计「重连风暴」强度。
                p.reconnects = p.reconnects.saturating_add(1);
                log_important!(
                    info,
                    "[popup] 重连弹窗 #{}: key={}, request_id={}, 已等待={}s（重连越多越接近 Cursor 单轮预算上限→易被动新开 request）",
                    p.reconnects,
                    key,
                    p.request_id,
                    p.started.elapsed().as_secs()
                );
                p
            }
            None => start_popup(request)?,
        }
    };

    let deadline = Instant::now() + wait;
    loop {
        // stdout_reader 完成 == GUI 关闭了 stdout == 进程已退出且输出已收齐。
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
            // 进程已退出，wait() 立即返回退出状态。
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

        if Instant::now() >= deadline {
            // 仍在等用户：放回注册表，下次 zhi 调用重连，不杀弹窗、不丢已输入内容。
            log_important!(
                info,
                "[popup] 等待窗口({}s)到，弹窗仍开启→返回 Pending 待 AI 重连: key={}, request_id={}, 已等待={}s, 当前重连次数={}",
                wait.as_secs(),
                key,
                pending.request_id,
                pending.started.elapsed().as_secs(),
                pending.reconnects
            );
            let mut map = PENDING_POPUPS
                .lock()
                .map_err(|e| anyhow::anyhow!("弹窗注册表锁中毒: {}", e))?;
            map.insert(key, pending);
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
