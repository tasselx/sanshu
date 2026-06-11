# 分析报告：请求「跑着跑着突然断、停止按钮变发送」保活失效

> 日期：2026-06-11
> 涉及会话：composerId `d482361a-60a9-409c-92fd-5499cde01a5f`
> 实际工作区：`/Users/tassel/.cursor/projects/.../reverse-lab-PPDuck3`（二进制逆向任务，非 sanshu 仓库本身）
> 模型：`claude-opus-4-8-thinking-max`（其中一条为 `claude-fable-5-thinking-max`）

## 现象

用户在 Cursor 用量面板看到同一会话被拆成多条高价 request（含一条 **85.5万 token**），
每条都是「看着在运行，突然就断掉，停止按钮从『停止』恢复成『发送』」。

## 结论（一句话）

**不是崩溃、不是掉线、不是 MCP 断连，而是 agent turn 正常结束（`turnEnded`）。**
每个 turn 结束 = 停止按钮变发送 = 计一次 request。根因是**保活的两道防线都没生效**：
模型层因 thinking-max + 超大上下文被截断，没来得及调 `zhi`；
stop hook 层按 v5 设计对「没调 sanshu 的纯任务 turn」故意放行。

## 时间线证据（main.log，window:1 全程未崩未重载）

每对 `Started → Stopping wakelock reason="agent-loop"` = 一次 turn = 一次 request：

| 起 | 止 | 持续 | 对应 token（stop-debug） |
|----|----|------|--------------------------|
| 21:19:53 | 21:20:39 | 46s | 0 |
| 21:21:53 | **21:27:16** | **323s ≈ 5.4 分钟** | **855019** |
| 21:28:23 | 21:29:52 | 88s | 98039 |
| 21:31:33 | 21:34:11 | 157s | 457285 |

- `requestTraces.log`：每次都以正常 `AgentResponseAdapter.turnEnded` + `agent.request` span_completed 收尾，**无 error、无中途 abort**（唯一的 `abortChatAndWaitForFinish` 出现在每次请求**开始**处，是提交前清理旧流的标准动作）。
- `cursor-sentry-events.log`：无 crash / OOM / renderer-killed。
- sanshu MCP（window1）：21:18:50 连接成功并全程在线，`zhi` 可达。

## 双层根因

### 第一层：模型从头到尾没调用 `zhi`（被 output 预算截断）

`/tmp/sanshu-stop-debug.log` 实锤——每次 turn 结束 hook 读 transcript 都是 `saw_zhi=0`：

```
[21:19:35] saw_zhi=0 total_tok=0
[21:20:39] saw_zhi=0 total_tok=0
[21:21:27] saw_zhi=0 total_tok=0
[21:27:16] saw_zhi=0 total_tok=855019
[21:29:52] saw_zhi=0 total_tok=98039
[21:31:14] saw_zhi=0 total_tok=98771
[21:34:11] saw_zhi=0 total_tok=457285
```

`requestTraces` / structured logs 全程检索 **零 `zhi`/`sanshu` 工具调用**。

机理（与 `docs/分析报告-20260607-Request提前结束未调用zhi.md` 一致）：
- `thinking-max` = 最高 thinking 预算，thinking tokens 可占 output 预算 80%+。
- 叠加超大上下文（一次 855K token，逆向任务里塞满反编译/反汇编输出）。
- 模型做完一批工具调用后，**还没轮到 emit `zhi` 工具调用，output 预算就耗尽 / turn 自然结束**。
- 排除 `maxIterations`：设置里 `cursor.maxIterations: 100`，而最长那条只有 23 次工具调用，远未触顶。

### 第二层：stop hook 兜底「故意放行」，没注入 followup 续命

`~/.cursor/hooks.json` 里 `check-zhi-on-stop.sh`（v5）每次 turn 结束都触发了，但每次判定 **`fallback allow`**：

```
[21:27:16] fallback allow total_tok=855019
```

v5 设计（见 `docs/总结-20260607-Cursor-Request优化与Hook部署.md`）：
- 只在「调了 sanshu 但没用 `zhi` 收尾」时才拦截注入 followup；
- 「纯任务 / 没调 sanshu」→ 兜底放行，**且已移除 token 闸门**（避免凭空多开 request）。

这几个 turn 属于纯逆向任务工具调用（没调 sanshu），于是 hook 主动放行，没有续命。
即便烧了 855K token，v5 也不再因 token 高而拦截。

> 补充：Cursor hooks 没有比 `stop` 更早的「将要结束」事件，
> 且 `stop` 注入的 followup 本身**也会新开一条 request**，
> 所以 hook 本质只能兜底「不丢上下文」，**无法在同一条 request 内续跑**。
> 真正「单条不新开」只能靠 agent 在 turn 内持续 `zhi`。

## 为什么会「看着在跑、突然就停」

「停止按钮 → 发送」就是 turn 结束的 UI 表现。用户预期"靠 zhi 一直续着不停"，
但模型在这几条里压根没调 zhi（被预算截断），hook 又按设计放行，于是 turn 自然结束、各计一次 request。
85.5万那条尤其烧钱：单条逼近 Opus 上下文上限。

## 缓解建议（按优先级）

| 优先级 | 建议 | 说明 |
|--------|------|------|
| P1 | 长对话/逆向任务**避免 thinking-max**，切 standard/medium thinking | thinking-max 在大上下文里最容易把 output 预算吃光、来不及调 zhi |
| P1 | 上下文过大（含大量反编译输出）时**及时新开会话** | 855K/457K token 单条既烧钱又易截断 |
| P2 | 收紧 MCP 工具返回（truncate-mcp-output 已截 500 行，逆向类可更激进） | 减少历史膨胀 |
| P2 | 若希望纯任务也强续命，可调整 v5 hook 判定（权衡：会多开 request） | 当前 v5 故意对纯任务放行 |
| P3 | 监控脚本 `cursor-request-monitor.sh` 盯的是旧日志目录，需修成自动跟最新窗口 | 见下 |

## 附带发现（非主因）

- `cursor-request-monitor.sh` 当前 LaunchAgent 实例盯的是旧目录 `20260608T092500/window3_wb3/renderer.log`，
  没跟到当前窗口，状态文件 `/tmp/cursor-request-monitor-state.json` 为 `{}`，所以没弹「多次 request」通知。
- sanshu MCP server 在 21:12 / 21:18 / 21:32 多次重启，对应窗口重载 / 开新窗口，**不是**请求中断原因。

## 关键日志位置

- `~/Library/Application Support/Cursor/logs/20260611T211841/main.log`（wakelock 时间线）
- `.../window1/output_20260611T211844/cursor.requestTraces.log`（turnEnded / 工具调用计数）
- `/tmp/sanshu-stop-debug.log`（`saw_zhi=0` / `fallback allow` 实锤）
- `~/Library/Application Support/sanshu/log/sanshu-mcp.log`（MCP 启停水印）
- `~/.cursor/hooks.json` + `~/.cursor/hooks/check-zhi-on-stop.sh`（stop hook v5）
