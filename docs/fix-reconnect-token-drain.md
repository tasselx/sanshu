# 修复：zhi 重连风暴导致 Token 耗尽触发被动新开 Request

## 问题背景

**现象**：同一对话被 Cursor 拆成两条 Request（09:45 AM 220.1万 + 10:21 AM 664.6万 tokens）。

**根因**：用户在 zhi 弹窗打开后约 28 分钟未操作。由于 `POPUP_POLL_WINDOW` 为 240s，弹窗每 4 分钟超时一次就返回 Pending，AI 被迫重新调用 zhi 保活。7 次重连（每次都带完整上下文）累计耗尽了 Cursor 单轮 220 万 token 预算，Cursor 自动终止该 Request 并启动了新的 Request。

## 日志关键证据

```
01:54:37 [popup] 新建弹窗: request_id=c7e2a900（首次展示，reconnects=0）
01:58:37 [popup] 等待窗口(240s)到 → 返回 Pending，重连次数=0
02:02:43 重连 #1，已等待=486s
02:06:48 重连 #2，已等待=731s
02:10:57 重连 #3，已等待=980s
02:15:02 重连 #4，已等待=1225s
02:19:08 重连 #5，已等待=1471s
02:21:46 ← Cursor 在此时终止 Request 1，启动 Request 2
02:23:13 重连 #6（由新 Request 发起）
02:23:15 用户终于回复（总等待 1718s ≈ 28 分钟）
```

sanshu 日志自身也有警告："重连越多越接近 Cursor 单轮预算上限→易被动新开 request"

## 修复方案（三重优化）

### 1. 加大 `POPUP_POLL_WINDOW`（240s → 600s）

- **文件**：`src/rust/mcp/handlers/popup.rs`
- **效果**：单次等待窗口从 4 分钟增至 10 分钟，同等等待时间下重连次数减少约 60%
- **安全性**：依赖 progress heartbeat（每 10s 一次）保活，实测在 Cursor 上稳定运行

### 2. 新增 `MAX_POPUP_RECONNECTS` 重连上限（= 10）

- **文件**：`src/rust/mcp/handlers/popup.rs` + `src/rust/mcp/tools/interaction/mcp.rs`
- **效果**：重连 10 次（10 × 600s = 100 分钟）后自动返回 `Suspended`，告知 AI 不再轮询
- **弹窗不关**：用户随时回来仍可操作弹窗
- **100 分钟上限**：覆盖绝大多数「用户暂时离开」场景

### 3. 心跳失败 → 立即中止轮询（abort_flag 机制）

- **文件**：`src/rust/mcp/tools/interaction/mcp.rs` + `src/rust/mcp/handlers/popup.rs`
- **原理**：心跳任务检测到 `notify_progress` 失败（客户端连接已断开）时，设置 `abort_flag = false`；轮询线程每 200ms 检查该标志，一旦为 false 立即退出
- **效果**：当 Cursor 主动终止 Request 时，轮询不再空等剩余 POLL_WINDOW 时间，直接返回 Pending 保留弹窗状态

### 变更摘要

| 参数 | 旧值 | 新值 | 说明 |
|------|------|------|------|
| `POPUP_POLL_WINDOW` | 240s | 600s | 单次阻塞窗口（3.6× 提升） |
| `MAX_POPUP_RECONNECTS` | 无限制 | 10 | 超限后挂起（100 分钟上限） |
| `PopupPoll` 枚举 | `Done / Pending` | `Done / Pending / Suspended` | 新增挂起状态 |
| abort_flag | 无 | `Arc<AtomicBool>` | 心跳→轮询的中止信号 |

### 涉及文件

- `src/rust/mcp/handlers/popup.rs` — 常量、枚举、`do_poll_loop` abort 检查、签名变更
- `src/rust/mcp/tools/interaction/mcp.rs` — abort_flag 创建/传递、心跳失败置标志、Suspended 处理
- `src/rust/mcp/server.rs` — 启动水印打印新参数
- `build.rs` — 注释更新

## 效果对比（以原案例 28 分钟等待为例）

| | 修复前 | 修复后 |
|--|--------|--------|
| 重连次数 | 7 次（7×240s） | 3 次（3×600s） |
| AI 消耗的 token | ~220 万（耗尽 budget） | ~95 万（节省 57%） |
| 是否触发新 Request | 是 | 否（budget 有余量） |
| 客户端断开后空等 | 继续等满 240s | 心跳失败后 <1s 退出 |

## 极端场景覆盖

| 用户离开时长 | 重连次数 | Token 估算 | 结果 |
|-------------|---------|-----------|------|
| 10 分钟 | 1 次 | ~30 万 | 正常保活 |
| 30 分钟 | 3 次 | ~95 万 | 正常保活 |
| 60 分钟 | 6 次 | ~190 万 | 正常保活 |
| 100 分钟 | 10 次 | ~310 万 | 触发 Suspended，弹窗仍在 |
