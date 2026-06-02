use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // 中文注释：嵌入「启动水印」所需的构建信息，便于一眼判断 Cursor 实际运行的 MCP 二进制
    // 是否为最新源码（历史上出现过「源码已改 240s、但跑的还是旧 20s 二进制」导致频繁新开 request）。

    // 1) git 短 SHA；工作区有未提交改动时追加 -dirty，提示二进制可能与提交不一致。
    let git_sha = git_short_sha();
    let dirty = git_worktree_dirty();
    let sha_stamp = if dirty {
        format!("{}-dirty", git_sha)
    } else {
        git_sha
    };
    println!("cargo:rustc-env=SANSHU_GIT_SHA={}", sha_stamp);

    // 2) 构建时间（UNIX 秒，UTC）；运行时再格式化为可读时间，避免在 build 脚本引入额外依赖。
    let build_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=SANSHU_BUILD_EPOCH={}", build_epoch);

    // 提交变化时重跑本脚本以刷新 SHA（dirty 状态无法被精确追踪，属已知取舍）。
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    tauri_build::build()
}

/// 读取 git 短 SHA，失败时回退为 "unknown"（非 git 环境/打包场景）。
fn git_short_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 判断工作区是否有未提交改动（git status --porcelain 非空即为 dirty）。
fn git_worktree_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}
