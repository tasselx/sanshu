# Cursor Request 优化与 Hook 部署总结

> 日期：2026-06-07
> 工作区：`/Users/tassel/Documents/GitHub/sanshu`

## 问题背景

1. AI 模型没有调用 `zhi` 工具确认就自行结束 turn，违反强制交互规则
2. Cursor 将一次用户消息拆分成多个 request（多次计费）
3. 长对话中 output token budget 被 high-thinking 耗尽

## 代码分析结论

检查了 sanshu 最近 5 次提交的代码改动，**未发现导致提前结束的 bug**：

- `response.rs`：附件系统从 base64 内联改为本地路径传递（减少上下文膨胀）
- `popup.rs`：`POPUP_POLL_WINDOW` 600s → 900s，`MAX_POPUP_RECONNECTS` 10 → 5
- `interaction/mcp.rs`：新增 brief 长度告警和调用频率监控（仅日志）
- `sanshu-强制交互.mdc`：新增 Rules 9-11「预算感知」章节

## 已部署方案

### 1. Cursor Hooks（用户级，所有工作区生效）

| Hook | 事件 | 作用 |
|------|------|------|
| `rtk hook cursor` | preToolUse (Shell) | 压缩 Shell 命令输出，减少 token 浪费 |
| `track-zhi.sh` | postToolUse (全部, 无 matcher) | 归一化工具名后追踪 sanshu 调用，写状态文件 |
| `truncate-mcp-output.sh` | postToolUse (MCP: 全部) | 截断超过 500 行的 MCP 工具输出 |
| `check-zhi-on-stop.sh` | stop (loop_limit=2) | 状态文件主判定：仅「调了 sanshu 没 zhi 收尾」才拦截（v5 已移除 token 闸门）|

**文件位置**：
- 配置：`~/.cursor/hooks.json`
- 脚本：`~/.cursor/hooks/`
- 备份：`scripts/cursor-hooks/`

### 2. Request 监控脚本

**文件**：`scripts/cursor-request-monitor.sh`

实时监控 Cursor 的 `renderer.log`，按 composerId 统计 request 次数，同一会话出现第 2 次 request 时弹 macOS 通知。

**使用方式**：
```bash
# 手动启动
./scripts/cursor-request-monitor.sh &

# 查看统计
./scripts/cursor-request-monitor.sh --stats

# 停止
./scripts/cursor-request-monitor.sh --stop
```

### 3. LaunchAgent（可选自启动）

**文件**：`~/Library/LaunchAgents/com.sanshu.cursor-request-monitor.plist`

```bash
# 加载自启动
launchctl load ~/Library/LaunchAgents/com.sanshu.cursor-request-monitor.plist

# 卸载
launchctl unload ~/Library/LaunchAgents/com.sanshu.cursor-request-monitor.plist
```

## 已有优化（不需要额外操作）

| 优化项 | 状态 |
|-------|------|
| `cursor.maxIterations: 100` | 已设置 |
| 附件路径化（不内联 base64） | 已实现 |
| `POPUP_POLL_WINDOW: 900s` | 已实现 |
| `MAX_POPUP_RECONNECTS: 5` | 已实现 |
| 预算感知规则 (Rules 9-11) | 已添加 |

## 使用建议（行为调整）

| 优先级 | 建议 |
|-------|------|
| P2 | 长对话（3+ 轮工具调用）切换为 standard/medium thinking |
| P2 | 一次说清楚具体需求，减少模型「猜测」消耗的 thinking tokens |
| P3 | 不用的 MCP server 按需禁用，减少 system prompt 注入 |
| P4 | 对话历史过大时新开对话 |

## Cursor 日志分析

### Request 生命周期日志模式

```
Acquired wakelock composerId=xxx  → 新 request 开始
buildRequestedModel composerId=xxx → 模型被请求（计费 +1）
Released wakelock reason="generation-ended" → request 结束
```

### 架构关键文件

| 文件 | 位置 | 作用 |
|------|------|------|
| `workbench.desktop.main.js` | Cursor 安装目录 | 包含 `buildRequestedModel`，request 创建逻辑 |
| `cursor-agent-worker` | extensions/ | 工具执行 worker，不管 request 发送 |

## 调试文件

| 文件 | 用途 |
|------|------|
| `/tmp/sanshu-zhi-hook-state.json` | zhi 调用状态（track-zhi 写入） |
| `/tmp/sanshu-hook-debug.log` | track-zhi 调试日志 |
| `/tmp/sanshu-truncate-debug.log` | truncate-mcp-output 调试日志 |
| `/tmp/cursor-request-monitor-state.json` | request 监控统计 |
| `/tmp/sanshu-stop-debug.log` | check-zhi-on-stop 判定日志（state/transcript/decision） |

## 保活机制真相与策略（关键认知，2026-06-07 补充）

> 经实跑验证 + Cursor 官方 hooks 文档查证，澄清「单条 request 无限大、绝不新开」的真正实现方式。

### 核心诉求

单条 request 尽量大、**绝不新开 request**、靠 `zhi` 持续交互累积更多 token（曾达单条 **3700 万**）。

### request 计费机制（实测 + 官方文档）

| 行为 | 是否新开 request | 说明 |
|------|----------------|------|
| `zhi` 等待用户输入 | **否** | 同一 turn 内的工具调用，只让该条 request 的 token 累积 |
| `stop` hook 注入 `followup_message` | **是** | followup 作为「下一条 user message」提交，`generation_id` 随之变化 → 新 generation/request |

证据：同一会话 05:54(946 万) → 06:28(521 万) → 06:57(18.8 万)，每条分界点都是一次 followup 拦截。

### Cursor hooks 能力边界（官方文档 cursor.com/docs/hooks）

- 共 18 个 Agent hook 事件，**没有**比 `stop` 更早的「将要结束」事件（无 beforeStop/willStop/preStop）。
- `stop` 在 turn **已结束**时触发；其 followup 必然新开 request；受 `loop_limit`（默认 5）限制。
- **hooks 无法在同一条 request 内续跑**——这不是 hook 的能力范围。

### 结论与策略

1. **「单条无限大不新开」唯一途径 = agent 在同一个 turn 内持续 `zhi` 交互、永不让 turn 结束。**
   这是 agent 行为，不是 hook 功能。
2. `stop` hook 只作**兜底**：万一 agent 真停了，followup 新开一条把上下文续上（次优但不丢上下文）；
   正常保活时根本不触发。
3. agent 实践：到决策点就 `zhi` 等用户，用户回复后继续，**不主动结束 turn**，从而单条 request 持续累积 token。

### 操作铁律（如何在一个 request 里做更多事）

> 目标**不是**「一条会话永久一条 request」，而是「**让每个 request 尽量在 zhi 内延续、多做事**；彻底结束后下次发消息再正常新开」。

| 用户如何回复 | request 走向 |
|---|---|
| 在 zhi（fallback MCP）弹窗里回复 | 流内工具返回 → **同一条 request 延续**，token 累积 |
| 在主聊天框发新消息 | 新 user message → 上一条流已关 → **新开一条 request** |

- 一次主框消息 = 一个 request 的起点（正常，接受新开）。
- 该 request 内 agent 用 `zhi` 持续交互，把「更多的事」在这一条里做完，**不中途无谓结束 turn**。
- 彻底做完 / 用户说结束 → turn 真正结束 → request 结束。
- 下次主框发消息 → 新 request → 再在 `zhi` 内延续。循环往复。

**代码层原理**（逆向 `workbench.desktop.main.js`）：

- AI 请求走 `aiserver.v1.ChatService.StreamUnifiedChatWithTools`（`kind: BiDiStreaming` 双向流）。
- 一条 request = 一个 generation = 一条该双向流；turn 内的工具往返（含 `zhi` 等待）都在同一条流里。
- `ComposerWakelockManager`：agent loop 活跃/恢复 → `_acquire("agent-loop-resumed")`；controller `dispose()` → `_release("generation-ended")`。即 **request 活跃区间 = agent loop 生命周期**。
- `zhi` 弹窗回复 = 流内工具返回（流不关）；主框发消息 = 上一条流已 dispose（必新建流）。

证据：本会话从 06:57(18.8 万) 起，全程 `zhi` 弹窗交互、未再新开 request。

### 当前 hook 判定（v5）

1. 状态文件 `tool=zhi` → 放行；`tool!=zhi`（调了 sanshu 没 zhi 收尾）→ 拦截；过期忽略。
2. transcript 正信号：本轮看到 zhi → 放行。
3. 兜底：放行（纯任务 / 没调 sanshu，不强制；**已移除 token 闸门**，避免凭空多 request）。
