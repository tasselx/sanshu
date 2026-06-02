use serde::{Deserialize, Serialize};
use std::time::Duration;

// 安全限制常量
const MAX_EXECUTION_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_BYTES: usize = 100_000; // 100KB
const MAX_CODE_LENGTH: usize = 50_000; // 50KB

#[derive(Debug, Serialize, Deserialize)]
pub struct CodeExecutionRequest {
    pub language: String,
    pub code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodeExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
}

// 语言到命令的映射
fn resolve_command(language: &str) -> Result<(&str, Vec<String>, &str), String> {
    match language {
        "python" | "py" => Ok(("python", vec![], ".py")),
        "node" | "javascript" | "js" => Ok(("node", vec![], ".js")),
        "go" | "golang" => Ok(("go", vec!["run".to_string()], ".go")),
        "java" => Ok(("java", vec![], ".java")),
        _ => Err(format!("不支持的语言: {}", language)),
    }
}

/// 执行代码片段
#[tauri::command]
pub async fn execute_code_snippet(
    request: CodeExecutionRequest,
) -> Result<CodeExecutionResult, String> {
    // 输入校验
    if request.code.is_empty() {
        return Err("代码不能为空".into());
    }
    if request.code.len() > MAX_CODE_LENGTH {
        return Err(format!(
            "代码长度超过限制（最大 {}KB）",
            MAX_CODE_LENGTH / 1024
        ));
    }

    let (cmd, extra_args, file_ext) = resolve_command(&request.language)?;

    // 创建临时目录和文件
    let temp_dir = std::env::temp_dir().join("sanshu_code_exec");
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;

    let file_name = format!("snippet_{}{}", uuid::Uuid::new_v4().simple(), file_ext);
    let file_path = temp_dir.join(&file_name);

    std::fs::write(&file_path, &request.code).map_err(|e| format!("写入临时文件失败: {}", e))?;

    // 构建命令
    let mut command = tokio::process::Command::new(cmd);
    for arg in &extra_args {
        command.arg(arg);
    }
    command.arg(&file_path);
    command.current_dir(&temp_dir);

    // 执行，带超时
    let result = tokio::time::timeout(
        Duration::from_secs(MAX_EXECUTION_TIMEOUT_SECS),
        command.output(),
    )
    .await;

    // 清理临时文件
    let _ = std::fs::remove_file(&file_path);

    // 处理结果
    match result {
        Ok(Ok(output)) => {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            stdout.truncate(MAX_OUTPUT_BYTES);
            stderr.truncate(MAX_OUTPUT_BYTES);

            Ok(CodeExecutionResult {
                stdout,
                stderr,
                exit_code: output.status.code(),
                timed_out: false,
                error: None,
            })
        }
        Ok(Err(e)) => {
            // 命令执行失败（如找不到解释器）
            let error_msg = format!("{}", e);
            let hint = if error_msg.contains("not found") || error_msg.contains("找不到") {
                format!("执行失败: 未找到 {} 解释器，请确认已安装并配置到 PATH", cmd)
            } else {
                format!("执行失败: {}", e)
            };

            Ok(CodeExecutionResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                timed_out: false,
                error: Some(hint),
            })
        }
        Err(_) => {
            // 超时
            Ok(CodeExecutionResult {
                stdout: String::new(),
                stderr: format!("执行超时（{}秒）", MAX_EXECUTION_TIMEOUT_SECS),
                exit_code: None,
                timed_out: true,
                error: None,
            })
        }
    }
}
