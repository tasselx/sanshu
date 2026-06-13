use anyhow::Result;
use rmcp::model::{Content, ErrorData as McpError};

use crate::log_debug;
use crate::log_important;
use crate::mcp::handlers::popup::RESPONSE_LEN_WARN_THRESHOLD;
use crate::mcp::types::{McpResponse, McpResponseContent};
use crate::mcp::utils::is_zhi_custom_choice;

/// 将字节数格式化为人类可读的大小字符串（B / KB / MB）
fn format_byte_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// 超长输入落盘后回传给模型的预览字符数。
const OVERFLOW_PREVIEW_CHARS: usize = 2000;

/// 超长用户输入落盘目录：~/.sanshu/overflow_replies/
fn overflow_replies_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".sanshu")
        .join("overflow_replies")
}

/// 超长用户输入自动落盘，回传「预览 + 文件路径」替代全文。
///
/// 中文说明（2026-06-13 实证修复）：用户曾在 zhi 弹窗粘贴 35.5 万字符文本，原样回传模型
/// 导致单次上下文超限、触发 Cursor 历史压缩并多计一条 request（之前仅 WARN 不处理）。
/// 改为超过 RESPONSE_LEN_WARN_THRESHOLD 时写入 ~/.sanshu/overflow_replies/，
/// 模型按需用文件读取工具分段查看。落盘失败时退回原样回传，保证内容不丢。
fn spill_long_user_input(text: &str) -> String {
    if text.len() <= RESPONSE_LEN_WARN_THRESHOLD {
        return text.to_string();
    }
    let dir = overflow_replies_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        log_important!(warn, "[parse_mcp_response] 超长输入落盘目录创建失败，退回原样回传");
        return text.to_string();
    }
    let file = dir.join(format!(
        "reply_{}.txt",
        chrono::Utc::now().timestamp_millis()
    ));
    if std::fs::write(&file, text).is_err() {
        log_important!(warn, "[parse_mcp_response] 超长输入落盘写入失败，退回原样回传");
        return text.to_string();
    }
    let preview: String = text.chars().take(OVERFLOW_PREVIEW_CHARS).collect();
    log_important!(
        info,
        "[parse_mcp_response] 用户超长输入已落盘: file={}, len={}",
        file.display(),
        text.len()
    );
    format!(
        "⚠️ 用户本次输入超长（共 {} 字符），完整内容已保存到文件：{}\n\
         请按需用文件读取工具分段查看该文件，不要一次性读入全文。\n\n\
         === 内容预览（前 {} 字符）===\n{}",
        text.chars().count(),
        file.display(),
        OVERFLOW_PREVIEW_CHARS,
        preview
    )
}

/// 解析 MCP 响应内容
///
/// 支持新的结构化格式和旧格式的兼容性，并生成适当的 Content 对象
pub fn parse_mcp_response(response: &str) -> Result<Vec<Content>, McpError> {
    if response.trim() == "CANCELLED" || response.trim() == "用户取消了操作" {
        log_debug!("[parse_mcp_response] 收到取消信号");
        // 中文说明：把「取消/弹窗关闭」改写为「继续等待」语义，避免 AI 把它当成对话结束信号
        // 而提前收尾（从而导致 IDE/客户端把下一条消息计为新一轮对话次数）。
        return Ok(vec![Content::text(
            "用户暂未给出回应（可能误关闭弹窗或临时离开），未表达任何结束意图。\
             请再次调用 `zhi` 工具等待用户明确回复，禁止在未获得用户完成指令前主动结束本轮对话。"
                .to_string(),
        )]);
    }

    // 首先尝试解析为新的结构化格式
    if let Ok(structured_response) = serde_json::from_str::<McpResponse>(response) {
        log_debug!(
            "[parse_mcp_response] 结构化响应: selected_options={}, attachments={}, images(legacy)={}, request_id={:?}, source={:?}",
            structured_response.selected_options.len(),
            structured_response.attachments.len(),
            structured_response.images.len(),
            structured_response.metadata.request_id.as_deref(),
            structured_response.metadata.source.as_deref()
        );
        return parse_structured_response(structured_response);
    }

    // 回退到旧格式兼容性解析
    match serde_json::from_str::<Vec<McpResponseContent>>(response) {
        Ok(content_array) => {
            log_debug!(
                "[parse_mcp_response] 旧格式响应数组: items={}",
                content_array.len()
            );
            let mut result = Vec::new();
            let mut image_count = 0;

            // 分别收集用户文本和图片信息
            let mut user_text_parts = Vec::new();
            let mut image_info_parts = Vec::new();

            for content in content_array {
                match content.content_type.as_str() {
                    "text" => {
                        if let Some(text) = content.text {
                            user_text_parts.push(text);
                        }
                    }
                    "image" => {
                        if let Some(source) = content.source {
                            if source.source_type == "base64" {
                                image_count += 1;

                                // 先添加图片到结果中（图片在前）
                                result.push(Content::image(
                                    source.data.clone(),
                                    source.media_type.clone(),
                                ));

                                // 添加图片信息到图片信息部分
                                let base64_len = source.data.len();
                                let preview = if base64_len > 50 {
                                    format!("{}...", &source.data[..50])
                                } else {
                                    source.data.clone()
                                };

                                // 计算图片大小（base64解码后的大小）
                                let estimated_size = (base64_len * 3) / 4; // base64编码后大约增加33%
                                let size_str = if estimated_size < 1024 {
                                    format!("{} B", estimated_size)
                                } else if estimated_size < 1024 * 1024 {
                                    format!("{:.1} KB", estimated_size as f64 / 1024.0)
                                } else {
                                    format!("{:.1} MB", estimated_size as f64 / (1024.0 * 1024.0))
                                };

                                let image_info = format!(
                                    "=== 图片 {} ===\n类型: {}\n大小: {}\nBase64 预览: {}\n完整 Base64 长度: {} 字符",
                                    image_count, source.media_type, size_str, preview, base64_len
                                );
                                image_info_parts.push(image_info);
                            }
                        }
                    }
                    _ => {
                        // 未知类型，作为文本处理
                        if let Some(text) = content.text {
                            user_text_parts.push(text);
                        }
                    }
                }
            }

            // 构建文本内容：用户文本 + 图片信息 + 注意事项
            let mut all_text_parts = Vec::new();

            // 1. 用户输入的文本
            if !user_text_parts.is_empty() {
                all_text_parts.extend(user_text_parts);
            }

            // 2. 图片详细信息
            if !image_info_parts.is_empty() {
                all_text_parts.extend(image_info_parts);
            }

            // 3. 兼容性说明
            if image_count > 0 {
                all_text_parts.push(format!(
                    "💡 注意：用户提供了 {} 张图片。如果 AI 助手无法显示图片，图片数据已包含在上述 Base64 信息中。",
                    image_count
                ));
            }

            // 将所有文本内容合并并添加到结果末尾（图片后面）
            if !all_text_parts.is_empty() {
                let combined_text = all_text_parts.join("\n\n");
                result.push(Content::text(combined_text));
            }

            if result.is_empty() {
                // 中文说明：空内容同样不能让 AI 把本轮当成结束，统一回写「继续等待」指令。
                result.push(Content::text(
                    "用户本次未在弹窗中提供任何内容（既未选择选项，也未输入文本/图片），\
                     未表达完成或结束意图。请再次调用 `zhi` 工具等待用户回复，\
                     禁止主动结束本轮对话。"
                        .to_string(),
                ));
            }

            log_debug!(
                "[parse_mcp_response] 旧格式解析完成: images={}, content_items={}",
                image_count,
                result.len()
            );
            Ok(result)
        }
        Err(_) => {
            // 如果不是JSON格式，作为纯文本处理
            log_debug!(
                "[parse_mcp_response] 非JSON响应，按纯文本处理: len={}",
                response.len()
            );
            // 中文说明：纯文本回退分支也要兜底空/纯空白响应，否则 AI 收到空文本会按
            // 「无内容」直接收尾，从而导致下一条消息被客户端计为新一轮 request。
            if response.trim().is_empty() {
                return Ok(vec![Content::text(
                    "用户本次未在弹窗中提供任何内容（响应为空），未表达完成或结束意图。\
                     请再次调用 `zhi` 工具等待用户回复，禁止主动结束本轮对话。"
                        .to_string(),
                )]);
            }
            // 中文说明：纯文本回退分支同样可能携带大段粘贴，超长时一并落盘
            Ok(vec![Content::text(spill_long_user_input(response))])
        }
    }
}

/// 解析新的结构化响应格式
fn parse_structured_response(response: McpResponse) -> Result<Vec<Content>, McpError> {
    let mut result = Vec::new();
    let mut text_parts = Vec::new();

    let custom_selected = response
        .selected_options
        .iter()
        .any(|option| is_zhi_custom_choice(option));

    // 1. 处理选择的选项。自定义选项需要明确表达“以补充说明为准”，降低模型误读风险。
    if custom_selected {
        text_parts
            .push("用户选择了自定义要求：不采用以上预设选项，以补充说明为最终要求。".to_string());
        let non_custom_options: Vec<&str> = response
            .selected_options
            .iter()
            .filter(|option| !is_zhi_custom_choice(option))
            .map(String::as_str)
            .collect();
        if !non_custom_options.is_empty() {
            text_parts.push(format!(
                "同时选中的其他选项仅供参考，不应优先于自定义要求: {}",
                non_custom_options.join(", ")
            ));
        }
    } else if !response.selected_options.is_empty() {
        text_parts.push(format!(
            "选择的选项: {}",
            response.selected_options.join(", ")
        ));
    }

    // 2. 处理用户输入文本（超长时自动落盘，只回传预览+文件路径，见 spill_long_user_input）
    if let Some(user_input) = response.user_input {
        if !user_input.trim().is_empty() {
            let input_text = spill_long_user_input(user_input.trim());
            if custom_selected {
                text_parts.push(format!("用户最终要求: {}", input_text));
            } else {
                text_parts.push(input_text);
            }
        }
    }

    // 3. 处理附件
    //    新版：附件以「本地绝对路径」形式传递，AI 用文件读取工具按需查看，彻底避免超长 base64 内联。
    //    旧版：保留 base64 内联图片的兼容处理（历史响应仍可解析）。
    let mut attachment_text_parts: Vec<String> = Vec::new();

    if !response.attachments.is_empty() {
        let mut lines = Vec::new();
        for (index, att) in response.attachments.iter().enumerate() {
            let kind = att.kind.as_deref().unwrap_or("file");
            let label = if kind == "image" { "图片" } else { "文件" };
            let size_str = att
                .size
                .map(format_byte_size)
                .unwrap_or_else(|| "未知大小".to_string());
            lines.push(format!(
                "{}. [{}] {}（{}）\n   路径: {}",
                index + 1,
                label,
                att.filename,
                size_str,
                att.path
            ));
        }
        attachment_text_parts.push(format!(
            "用户附带了 {} 个本地文件（已保存到附件工作目录）。请使用你的文件读取工具按需查看以下绝对路径，不要凭空猜测内容：\n{}",
            response.attachments.len(),
            lines.join("\n")
        ));
    } else if !response.images.is_empty() {
        // 旧格式兼容：base64 内联图片（图片在前，文本在后）
        for (index, image) in response.images.iter().enumerate() {
            result.push(Content::image(image.data.clone(), image.media_type.clone()));

            let base64_len = image.data.len();
            let preview = if base64_len > 50 {
                format!("{}...", &image.data[..50])
            } else {
                image.data.clone()
            };
            let estimated_size = (base64_len * 3) / 4;
            let size_str = format_byte_size(estimated_size as u64);
            let filename_info = image
                .filename
                .as_ref()
                .map(|f| format!("\n文件名: {}", f))
                .unwrap_or_default();

            attachment_text_parts.push(format!(
                "=== 图片 {} ==={}\n类型: {}\n大小: {}\nBase64 预览: {}\n完整 Base64 长度: {} 字符",
                index + 1,
                filename_info,
                image.media_type,
                size_str,
                preview,
                base64_len
            ));
        }
        attachment_text_parts.push(format!(
            "💡 注意：用户提供了 {} 张图片（旧格式 Base64 内联）。如果 AI 助手无法显示图片，图片数据已包含在上述 Base64 信息中。",
            response.images.len()
        ));
    }

    // 4. 合并所有文本内容
    let mut all_text_parts = text_parts;
    all_text_parts.extend(attachment_text_parts);

    // 6. 将文本内容添加到结果中（图片后面）
    if !all_text_parts.is_empty() {
        let combined_text = all_text_parts.join("\n\n");
        result.push(Content::text(combined_text));
    }

    // 7. 如果没有任何内容，添加默认响应
    if result.is_empty() {
        // 中文说明：与旧格式分支保持一致——空响应改写为「继续等待」语义，避免 AI 提前结束对话。
        result.push(Content::text(
            "用户本次未在弹窗中提供任何内容（既未选择选项，也未输入文本/图片），\
             未表达完成或结束意图。请再次调用 `zhi` 工具等待用户回复，\
             禁止主动结束本轮对话。"
                .to_string(),
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::parse_mcp_response;
    use rmcp::model::RawContent;

    fn extract_text(response: &str) -> String {
        let content = parse_mcp_response(response).expect("响应应可解析");
        content
            .iter()
            .find_map(|item| match &item.raw {
                RawContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .expect("响应应包含文本内容")
    }

    #[test]
    fn custom_choice_promotes_user_input_as_final_requirement() {
        let response = serde_json::json!({
            "user_input": "不要按选项一执行，改为先补需求访谈。",
            "selected_options": ["其他：自定义要求"],
            "images": [],
            "metadata": {
                "timestamp": "2026-05-17T00:00:00Z",
                "request_id": "test",
                "source": "popup"
            }
        });

        let text = extract_text(&response.to_string());

        assert!(text.contains("用户选择了自定义要求"));
        assert!(text.contains("用户最终要求: 不要按选项一执行，改为先补需求访谈。"));
        assert!(!text.contains("选择的选项: 其他：自定义要求"));
    }

    #[test]
    fn cancelled_response_instructs_ai_to_keep_waiting() {
        // 中文说明：取消/弹窗关闭场景必须返回「继续等待」语义，
        // 避免被 AI 当成对话结束信号、导致客户端把下一条消息计为新一轮次数。
        for raw in ["CANCELLED", "用户取消了操作"] {
            let text = extract_text(raw);
            assert!(
                text.contains("请再次调用") && text.contains("zhi"),
                "取消分支应当提示 AI 重新调用 zhi 等待用户回复，实际: {}",
                text
            );
            assert!(
                text.contains("禁止"),
                "取消分支应当明确禁止主动结束对话，实际: {}",
                text
            );
            assert!(
                !text.contains("用户取消了操作"),
                "新文案不应再回落到旧的『用户取消了操作』字面值，实际: {}",
                text
            );
        }
    }

    #[test]
    fn empty_structured_response_instructs_ai_to_keep_waiting() {
        // 中文说明：结构化响应里既无选项也无文本/图片时，同样必须发出「继续等待」指令。
        let response = serde_json::json!({
            "user_input": "",
            "selected_options": [],
            "images": [],
            "metadata": {
                "timestamp": "2026-05-17T00:00:00Z",
                "request_id": "test",
                "source": "popup"
            }
        });

        let text = extract_text(&response.to_string());
        assert!(text.contains("请再次调用") && text.contains("zhi"));
        assert!(text.contains("禁止"));
        assert!(!text.contains("用户未提供任何内容"));
    }

    #[test]
    fn normal_choice_keeps_existing_response_shape() {
        let response = serde_json::json!({
            "user_input": "补充说明",
            "selected_options": ["方案 A"],
            "images": [],
            "metadata": {
                "timestamp": "2026-05-17T00:00:00Z",
                "request_id": "test",
                "source": "popup"
            }
        });

        let text = extract_text(&response.to_string());

        assert!(text.contains("选择的选项: 方案 A"));
        assert!(text.contains("补充说明"));
        assert!(!text.contains("用户最终要求"));
    }
}
