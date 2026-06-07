//! 附件工作目录管理
//!
//! 负责把用户在弹窗中「粘贴」或「拖入」的文件落盘到本地工作目录，
//! 之后统一以「本地绝对路径」的形式交给 AI 读取，从而避免超长 base64 内联。
//!
//! 设计要点（KISS）：
//! - 工作目录全局共享：默认 `<config_dir>/sanshu/workspace`，可在设置中自定义；
//! - 命名确保不重复：在原文件名主体后追加毫秒时间戳，冲突时再递增计数；
//! - GUI 进程负责落盘，MCP 进程只需透传响应中的绝对路径，无需重复解析目录。

pub mod commands;

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 视为「图片」的扩展名集合（仅用于前端预览与 kind 标注）
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "tiff", "tif", "avif", "heic",
];

/// 附件自动清理阈值：删除「修改时间超过 7 天」的文件
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// 附件落盘后返回给前端的信息
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentInfo {
    /// 绝对路径
    pub path: String,
    /// 文件名（含后缀）
    pub filename: String,
    /// 类型："image" | "file"
    pub kind: String,
    /// 后缀（小写，不含点；无后缀为空字符串）
    pub ext: String,
    /// MIME 类型（仅常见图片可推断，其余为 None）
    pub media_type: Option<String>,
    /// 文件大小（字节）
    pub size: u64,
}

/// 工作目录中已有文件的信息（用于设置页列表展示）
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentFileInfo {
    pub path: String,
    pub filename: String,
    pub kind: String,
    pub ext: String,
    pub size: u64,
    /// 修改时间（Unix 毫秒时间戳）
    pub modified_ms: u64,
}

/// 解析附件工作目录：优先使用配置中的自定义目录，否则回退默认全局目录；并确保目录存在。
pub fn resolve_workspace_dir() -> Result<PathBuf> {
    let custom = crate::config::load_standalone_config()
        .ok()
        .and_then(|c| c.mcp_config.attachment_workspace_dir)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let dir = match custom {
        Some(p) => PathBuf::from(p),
        None => default_workspace_dir()?,
    };

    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("创建工作目录失败 {:?}: {}", dir, e))?;
    Ok(dir)
}

/// 默认全局工作目录：`<config_dir>/sanshu/workspace`
pub fn default_workspace_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("无法获取配置目录"))?;
    Ok(base.join("sanshu").join("workspace"))
}

/// 根据扩展名判定附件类型
pub fn kind_for_ext(ext: &str) -> &'static str {
    if IMAGE_EXTS.contains(&ext.to_lowercase().as_str()) {
        "image"
    } else {
        "file"
    }
}

/// 常见图片 MIME 推断（其余返回 None）
fn media_type_for_ext(ext: &str) -> Option<String> {
    let m = match ext.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        "avif" => "image/avif",
        "heic" => "image/heic",
        _ => return None,
    };
    Some(m.to_string())
}

/// 清理文件名主体：去除路径分隔符与非法字符，防止目录穿越
fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        // 控制主体长度，避免文件名过长
        trimmed.chars().take(80).collect()
    }
}

/// 生成不冲突的唯一路径：`<主体>_<毫秒时间戳>[.<后缀>]`，冲突时追加递增计数。
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let safe_stem = sanitize_stem(stem);
    let mut counter = 0u32;
    loop {
        let name = match (ext.is_empty(), counter) {
            (true, 0) => format!("{}_{}", safe_stem, ts),
            (true, _) => format!("{}_{}_{}", safe_stem, ts, counter),
            (false, 0) => format!("{}_{}.{}", safe_stem, ts, ext),
            (false, _) => format!("{}_{}_{}.{}", safe_stem, ts, counter, ext),
        };
        let candidate = dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// 从路径拆出（主体, 小写后缀）
fn split_stem_ext(filename: &str) -> (String, String) {
    let p = Path::new(filename);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    (stem, ext)
}

/// 由落盘后的文件路径构建 AttachmentInfo
fn info_from_saved(path: &Path) -> Result<AttachmentInfo> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(AttachmentInfo {
        path: path.to_string_lossy().to_string(),
        filename,
        kind: kind_for_ext(&ext).to_string(),
        media_type: media_type_for_ext(&ext),
        ext,
        size,
    })
}

/// 保存「粘贴」进来的 base64 数据为文件。
///
/// - `data_base64`：可带或不带 `data:*;base64,` 前缀；
/// - `suggested_filename`：建议文件名（可含后缀），为空时按 png 处理（粘贴图片场景）。
pub fn save_base64(data_base64: &str, suggested_filename: Option<&str>) -> Result<AttachmentInfo> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    // 去掉可能的 data URL 前缀
    let raw = match data_base64.find(";base64,") {
        Some(idx) => &data_base64[idx + ";base64,".len()..],
        None => data_base64,
    };
    let bytes = STANDARD
        .decode(raw.trim())
        .map_err(|e| anyhow!("base64 解码失败: {}", e))?;

    let (stem, ext) = match suggested_filename {
        Some(name) if !name.trim().is_empty() => {
            let (s, e) = split_stem_ext(name.trim());
            // 粘贴图片通常无后缀，默认 png
            (s, if e.is_empty() { "png".to_string() } else { e })
        }
        _ => ("pasted".to_string(), "png".to_string()),
    };

    let dir = resolve_workspace_dir()?;
    let target = unique_path(&dir, &stem, &ext);
    std::fs::write(&target, &bytes).map_err(|e| anyhow!("写入附件失败 {:?}: {}", target, e))?;
    info_from_saved(&target)
}

/// 复制「拖入」的文件到工作目录（保留原文件，命名确保唯一）。
pub fn save_dropped(src: &str) -> Result<AttachmentInfo> {
    let src_path = Path::new(src);
    if !src_path.is_file() {
        return Err(anyhow!("拖入的不是有效文件: {}", src));
    }
    let filename = src_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let (stem, ext) = split_stem_ext(filename);

    let dir = resolve_workspace_dir()?;
    let target = unique_path(&dir, &stem, &ext);
    std::fs::copy(src_path, &target)
        .map_err(|e| anyhow!("复制附件失败 {} -> {:?}: {}", src, target, e))?;
    info_from_saved(&target)
}

/// 列出工作目录中的全部文件（按修改时间倒序）
pub fn list() -> Result<Vec<AttachmentFileInfo>> {
    let dir = resolve_workspace_dir()?;
    let mut items = Vec::new();

    for entry in std::fs::read_dir(&dir).map_err(|e| anyhow!("读取工作目录失败: {}", e))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        items.push(AttachmentFileInfo {
            path: path.to_string_lossy().to_string(),
            filename,
            kind: kind_for_ext(&ext).to_string(),
            ext,
            size: meta.len(),
            modified_ms,
        });
    }

    // 修改时间倒序，最新的在前
    items.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(items)
}

/// 删除工作目录中的单个文件（仅允许纯文件名，禁止路径穿越）
pub fn delete(filename: &str) -> Result<()> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(anyhow!("非法文件名: {}", filename));
    }
    let dir = resolve_workspace_dir()?;
    let target = dir.join(filename);
    // 二次校验：目标必须仍位于工作目录内
    if target.parent() != Some(dir.as_path()) {
        return Err(anyhow!("文件不在工作目录内: {}", filename));
    }
    if target.is_file() {
        std::fs::remove_file(&target).map_err(|e| anyhow!("删除附件失败: {}", e))?;
    }
    Ok(())
}

/// 自动清理：删除工作目录中「修改时间超过保留期（默认 7 天）」的文件，返回删除数量。
/// 在应用启动时调用，做一次性 housekeeping。
pub fn cleanup_expired() -> Result<u32> {
    let dir = resolve_workspace_dir()?;
    let now = SystemTime::now();
    let mut count = 0u32;

    for entry in std::fs::read_dir(&dir).map_err(|e| anyhow!("读取工作目录失败: {}", e))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // now - modified > 阈值 即视为过期
        if let Ok(age) = now.duration_since(modified) {
            if age.as_secs() > MAX_AGE_SECS && std::fs::remove_file(&path).is_ok() {
                count += 1;
            }
        }
    }

    Ok(count)
}

/// 清空工作目录中的全部文件，返回删除数量
pub fn clear() -> Result<u32> {
    let dir = resolve_workspace_dir()?;
    let mut count = 0u32;
    for entry in std::fs::read_dir(&dir).map_err(|e| anyhow!("读取工作目录失败: {}", e))? {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.is_file() && std::fs::remove_file(&path).is_ok() {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 读取图片文件并返回 data URL（仅供弹窗本地预览，不会发送给 AI）
pub fn read_image_data_url(path: &str) -> Result<String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    let p = Path::new(path);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let mime = media_type_for_ext(&ext).unwrap_or_else(|| "image/png".to_string());
    let bytes = std::fs::read(p).map_err(|e| anyhow!("读取图片失败: {}", e))?;
    let encoded = STANDARD.encode(bytes);
    Ok(format!("data:{};base64,{}", mime, encoded))
}
