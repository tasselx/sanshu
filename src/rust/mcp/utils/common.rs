/// MCP 通用工具函数模块
///
/// 包含 MCP 相关的通用工具函数和辅助方法
use anyhow::Result;
use percent_encoding;
use regex::Regex;
use std::path::Path;

/// zhi 预设选项中的自定义兜底选项。
///
/// 中文说明：当模型给出的候选项不能覆盖用户真实意图时，用户可以明确选择该项，
/// 并在补充说明中给出最终要求，避免模型误把其他预设选项当作主决策。
pub const ZHI_CUSTOM_CHOICE: &str = "其他：自定义要求";

/// 规范化 zhi 预设选项。
///
/// 中文说明：仅在调用方已经提供候选项时追加自定义兜底；纯自由输入场景不新增选项，
/// 以保持原有交互的简单性。若调用方已提供同义自定义选项，则不重复追加。
pub fn normalize_zhi_choices(mut choices: Vec<String>) -> Vec<String> {
    if choices.is_empty() {
        return choices;
    }

    if !choices.iter().any(|choice| is_zhi_custom_choice(choice)) {
        choices.push(ZHI_CUSTOM_CHOICE.to_string());
    }

    choices
}

/// 判断选项是否表示 zhi 的自定义兜底语义。
pub fn is_zhi_custom_choice(choice: &str) -> bool {
    let normalized = choice.trim().to_lowercase();
    normalized == ZHI_CUSTOM_CHOICE.to_lowercase()
        || normalized == "其他"
        || normalized.starts_with("其他:")
        || normalized.starts_with("其他：")
        || normalized.starts_with("其他/")
        || normalized.starts_with("其他 /")
        || normalized.contains("自定义要求")
        || normalized.contains("以补充说明为准")
        || normalized == "custom"
        || normalized == "other"
        || normalized.starts_with("custom:")
        || normalized.starts_with("other:")
}

/// 解码并规范化路径
///
/// 处理 URL 编码、Windows 路径格式转换等问题
pub fn decode_and_normalize_path(path: &str) -> Result<String> {
    // 1. 先进行 URL 解码
    let decoded = decode_url_path(path);

    // 2. 规范化路径格式
    let normalized = normalize_path_format(&decoded)?;

    Ok(normalized)
}

/// 解码 URL 编码的路径
///
/// 在 Windows 下，路径中的冒号可能会被编码为 %3A，需要先解码
fn decode_url_path(path: &str) -> String {
    // 使用 percent_encoding 库进行 URL 解码
    match percent_encoding::percent_decode_str(path).decode_utf8() {
        Ok(decoded) => decoded.to_string(),
        Err(_) => {
            // 如果解码失败，返回原始路径
            path.to_string()
        }
    }
}

/// 规范化路径格式
///
/// 处理 Windows 下的路径格式问题，如 /c:/ -> C:\
fn normalize_path_format(path: &str) -> Result<String> {
    let path = path.trim();

    // 检查是否为 Windows 风格的路径（以 /盘符:/ 开头）
    if let Some(normalized) = normalize_windows_path(path) {
        return Ok(normalized);
    }

    // 检查是否为标准的 Windows 路径（C:\ 或 C:/）
    if is_windows_absolute_path(path) {
        return Ok(path.replace('/', "\\"));
    }

    // 其他情况直接返回
    Ok(path.to_string())
}

/// 规范化 Windows 路径格式
///
/// 将 /c:/path 或 /C:/path 格式转换为 C:\path
fn normalize_windows_path(path: &str) -> Option<String> {
    // 匹配 /盘符:/ 格式的路径
    let re = Regex::new(r"^/([a-zA-Z]):(.*)$").ok()?;

    if let Some(captures) = re.captures(path) {
        let drive = captures.get(1)?.as_str().to_uppercase();
        let rest = captures.get(2)?.as_str();

        // 转换为 Windows 格式
        let windows_path = format!("{}:{}", drive, rest.replace('/', "\\"));
        return Some(windows_path);
    }

    None
}

/// 检查是否为 Windows 绝对路径
fn is_windows_absolute_path(path: &str) -> bool {
    let re = Regex::new(r"^[a-zA-Z]:[/\\]").unwrap();
    re.is_match(path)
}

/// 验证项目路径是否存在
pub fn validate_project_path(path: &str) -> Result<()> {
    // 先对路径进行解码和规范化
    let normalized_path = decode_and_normalize_path(path)?;

    // 验证路径格式
    validate_path_format(&normalized_path)?;

    // 检查路径是否存在
    let path_obj = Path::new(&normalized_path);
    if !path_obj.exists() {
        anyhow::bail!("项目路径不存在: {}", normalized_path);
    }

    // 检查是否为目录
    if !path_obj.is_dir() {
        anyhow::bail!("项目路径不是目录: {}", normalized_path);
    }

    Ok(())
}

/// 验证路径格式是否合法
fn validate_path_format(path: &str) -> Result<()> {
    // 检查路径是否包含非法字符
    let illegal_chars = ['<', '>', '"', '|', '?', '*'];
    for ch in illegal_chars.iter() {
        if path.contains(*ch) {
            anyhow::bail!("路径包含非法字符 '{}': {}", ch, path);
        }
    }

    // 检查路径长度（Windows 限制）
    if cfg!(windows) && path.len() > 260 {
        anyhow::bail!("路径过长（超过260字符）: {}", path);
    }

    Ok(())
}

/// 生成唯一的请求 ID
pub fn generate_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ============================================================================
// UTF-8 安全字符串截断工具
// ============================================================================

/// 安全截断字符串（UTF-8 友好）
///
/// 按字符数（而非字节数）截断字符串，避免在多字节字符边界处截断导致 panic。
/// 如果截断发生，自动添加 "..." 省略号。
///
/// # 参数
/// - `text`: 要截断的字符串
/// - `max_chars`: 最大字符数（不包括省略号）
///
/// # 示例
/// ```rust
/// use sanshu::mcp_utils::safe_truncate;
/// let chinese = "你好世界，这是一段很长的中文文本";
/// let truncated = safe_truncate(chinese, 5);
/// assert_eq!(truncated, "你好世界，...");
/// ```
pub fn safe_truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{}...", truncated)
}

/// 安全截断并清理字符串（UTF-8 友好，移除换行符）
///
/// 与 `safe_truncate` 类似，但会额外进行文本清理：
/// - 将换行符（\r, \n）替换为空格
/// - 去除首尾空白
///
/// # 参数
/// - `text`: 要截断的字符串
/// - `max_chars`: 最大字符数（不包括省略号）
pub fn safe_truncate_clean(text: &str, max_chars: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(max_chars).collect();
    format!("{}...", truncated)
}

#[cfg(test)]
mod tests {
    use super::{is_zhi_custom_choice, normalize_zhi_choices, ZHI_CUSTOM_CHOICE};

    #[test]
    fn normalize_zhi_choices_adds_custom_choice_once() {
        let choices = normalize_zhi_choices(vec!["方案 A".to_string(), "方案 B".to_string()]);

        assert_eq!(choices.len(), 3);
        assert_eq!(choices[2], ZHI_CUSTOM_CHOICE);

        let normalized_again = normalize_zhi_choices(choices);
        assert_eq!(
            normalized_again
                .iter()
                .filter(|choice| is_zhi_custom_choice(choice))
                .count(),
            1
        );
    }

    #[test]
    fn normalize_zhi_choices_keeps_free_input_empty() {
        assert!(normalize_zhi_choices(Vec::new()).is_empty());
    }

    #[test]
    fn is_zhi_custom_choice_recognizes_common_labels() {
        assert!(is_zhi_custom_choice("其他：自定义要求"));
        assert!(is_zhi_custom_choice("其他 / 自定义要求"));
        assert!(is_zhi_custom_choice("其他"));
        assert!(is_zhi_custom_choice("custom"));
        assert!(is_zhi_custom_choice("other: write my own plan"));
        assert!(!is_zhi_custom_choice("方案 A"));
    }
}
