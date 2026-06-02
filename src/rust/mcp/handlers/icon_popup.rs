// 图标工坊弹窗处理器
// 负责调用 GUI 进程打开图标选择界面

use anyhow::Result;
use std::process::Command;
use std::time::Instant;

use crate::mcp::types::{IconSaveResponse, TuRequest};
use crate::mcp::utils::safe_truncate_clean;
use crate::{log_debug, log_important};

/// 创建图标选择弹窗
///
/// 调用 "等一下" GUI 进程，进入图标搜索模式
/// 用户可以搜索、预览、选择并保存图标
pub fn create_icon_popup(request: &TuRequest) -> Result<IconSaveResponse> {
    let start = Instant::now();

    log_important!(
        info,
        "[icon_popup] 启动图标弹窗: query={:?}, style={:?}, save_path={:?}, project_root={:?}",
        request
            .query
            .as_deref()
            .map(|s| safe_truncate_clean(s, 120)),
        request
            .style
            .as_deref()
            .map(|s| safe_truncate_clean(s, 120)),
        request
            .save_path
            .as_deref()
            .map(|s| safe_truncate_clean(s, 120)),
        request
            .project_root
            .as_deref()
            .map(|s| safe_truncate_clean(s, 120))
    );

    // 构建命令行参数
    let mut cmd = Command::new(find_ui_command()?);
    cmd.arg("--icon-search");

    // 添加可选参数
    if let Some(query) = &request.query {
        if !query.is_empty() {
            cmd.arg("--query").arg(query);
        }
    }
    if let Some(style) = &request.style {
        if !style.is_empty() {
            cmd.arg("--style").arg(style);
        }
    }
    if let Some(path) = &request.save_path {
        if !path.is_empty() {
            cmd.arg("--save-path").arg(path);
        }
    }
    if let Some(root) = &request.project_root {
        if !root.is_empty() {
            cmd.arg("--project-root").arg(root);
        }
    }

    // 执行命令并等待结果
    let output = cmd.output()?;
    let elapsed_ms = start.elapsed().as_millis();
    let exit_code = output.status.code();
    let stdout_len = output.stdout.len();
    let stderr_len = output.stderr.len();

    if output.status.success() {
        let response_str = String::from_utf8_lossy(&output.stdout);
        let response_str = response_str.trim();

        log_debug!(
            "[icon_popup] GUI执行成功: exit_code={:?}, stdout_len={}, stderr_len={}, elapsed_ms={}",
            exit_code,
            stdout_len,
            stderr_len,
            elapsed_ms
        );

        if response_str.is_empty() {
            // 用户取消了操作
            return Ok(IconSaveResponse {
                saved_count: 0,
                save_path: String::new(),
                saved_names: vec![],
                cancelled: true,
            });
        }

        // 解析 JSON 响应
        let response: IconSaveResponse = serde_json::from_str(response_str).map_err(|e| {
            log_important!(
                error,
                "[icon_popup] 解析响应失败: exit_code={:?}, stdout_preview={}, error={}",
                exit_code,
                safe_truncate_clean(response_str, 200),
                e
            );
            anyhow::anyhow!("解析图标保存响应失败: {}", e)
        })?;

        Ok(response)
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        log_important!(
            error,
            "[icon_popup] GUI执行失败: exit_code={:?}, stdout_len={}, stderr_len={}, stderr_preview={}, elapsed_ms={}",
            exit_code,
            stdout_len,
            stderr_len,
            safe_truncate_clean(&error, 200),
            elapsed_ms
        );
        anyhow::bail!("图标选择进程失败: {}", error);
    }
}

/// 查找 UI 命令路径
///
/// 复用 popup.rs 中的逻辑
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

    // 2. 尝试全局命令
    if test_command_available("等一下") {
        return Ok("等一下".to_string());
    }

    // 3. 返回错误
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
fn is_executable(path: &std::path::Path) -> bool {
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
