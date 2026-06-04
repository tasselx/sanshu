# BugFix: zhi 并发轮询导致重复弹窗

> 修复日期: 2026-06-03 | 影响文件: `src/rust/mcp/handlers/popup.rs`

## 问题描述

用户在结束长时间 Cursor 会话时，需要点击**两次确认结束**才能完成，因为 sanshu 的 zhi 工具创建了两个独立的弹窗。Cursor 计费面板显示在 11:13 AM 和 11:14 AM 分别产生了额外的 29.5万 和 15.3万 token 的 request。

## 复现条件

1. 一个长时间运行的 Cursor request（本次为 09:56 AM 开始，消耗 3552.3万 token）
2. AI 在接近结束时调用 zhi 弹窗请求用户确认
3. 用户未在 240s（POPUP_POLL_WINDOW）内响应
4. zhi 返回 Pending → AI 按保活规则重连
5. **此时 Cursor 因原 request 预算耗尽，自动新开了一个 request**
6. 新 request 的 AI 也调用 zhi → 创建了第二个弹窗

## 根因分析

### 日志时间线（UTC，+8h = 本地时间）

```
03:07:07  zhi(e9aecf30) 新建弹窗，等待用户确认
03:11:08  240s 超时 → Pending（弹窗放回 PENDING_POPUPS 注册表）
03:11:15  AI 重连 → zhi(4a61b7d8) 从 PENDING_POPUPS 取出弹窗，开始轮询
          ⚠️ 此时 PENDING_POPUPS 中该 key 为空
03:13:41  Cursor 新 request → zhi(03ba3fa8) 发现注册表空 → 新建第二个弹窗！
03:14:24  用户在 03ba3fa8 弹窗上点确认
03:15:15  4a61b7d8 的 240s 超时 → 又一轮 Pending
03:17:25  用户在旧弹窗上再次确认
```

### Bug 的精确位置

```rust
// src/rust/mcp/handlers/popup.rs (修复前)
pub fn poll_or_start_popup(...) -> Result<PopupPoll> {
    let pending = {
        let mut map = PENDING_POPUPS.lock()?;
        match map.remove(&key) {
            Some(p) => p,                    // 重连
            None => start_popup(request)?,   // ← BUG: 直接新建，未检查是否有并发轮询
        }
    };
    // 轮询期间弹窗不在 PENDING_POPUPS 中（已被 remove 取走）
    // 此窗口期内任何新的 zhi 调用都会走到 start_popup 分支
    loop { /* 轮询 */ }
}
```

**关键竞态**：`map.remove(&key)` 取走弹窗后到 `map.insert(key, pending)`（超时放回）之间，
注册表对该 key 为空。若另一个 Cursor request 的 zhi 调用在此期间进入，
会误判为"无活跃弹窗"而创建重复实例。

## 修复方案

### 新增 `POLLING_IN_FLIGHT` 全局跟踪器

```rust
/// 记录「哪些 key 当前正在被轮询」，填补注册表的信息空窗
static POLLING_IN_FLIGHT: Lazy<Mutex<HashSet<String>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));
```

### 重构 `poll_or_start_popup` 为三阶段

```
poll_or_start_popup (入口)
  ├─ acquire_popup (获取弹窗)
  │    ├─ PENDING_POPUPS 有 → 重连（原行为）
  │    ├─ 无 + 无并发轮询 → 新建（原行为）
  │    └─ 无 + 有并发轮询 → 等待释放后重连（新逻辑）
  │         ├─ 轮询方超时放回 → 从注册表取出重连
  │         ├─ 轮询方 Done（用户已响应）→ 返回 None
  │         └─ 等待超时 → 返回 error 触发 zhi 重试
  └─ do_poll_loop (轮询，原行为不变)
       ├─ 进入前: POLLING_IN_FLIGHT.insert(key)
       └─ 退出后: POLLING_IN_FLIGHT.remove(key)
```

### 并发保护时序

```
Thread A (旧 request 重连)        Thread B (新 request)
─────────────────────────         ─────────────────────
remove(&key) → Some(popup)
POLLING_IN_FLIGHT.insert(key)
                                  remove(&key) → None
                                  POLLING_IN_FLIGHT.contains(key) → true
                                  进入等待循环...
loop { poll... }
  timeout → insert(key, popup)
  POLLING_IN_FLIGHT.remove(key)
                                  检查 PENDING_POPUPS → found!
                                  重连成功 ✅（不再创建重复弹窗）
```

## 影响范围

- 仅修改 `popup.rs`，不涉及 zhi MCP handler、前端 GUI、响应解析等
- 对正常流程（首次弹窗、单次重连）**零影响**——新逻辑仅在 `PENDING_POPUPS` 为空 + 有并发轮询时触发
- `parse_mcp_response` 正常处理合成的 Done 消息（走纯文本分支）

## 验证要点

1. 正常弹窗创建和响应流程不受影响
2. 重连机制正常工作
3. **关键**：长会话末尾用户延迟响应时，不再创建重复弹窗
4. 日志中应出现 `同 key 弹窗正在被另一个 zhi 调用轮询中` 而非 `新建弹窗`
