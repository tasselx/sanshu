# Cursor Hooks 备份

sanshu 项目使用的 Cursor 用户级 hook 备份。实际生效位置为 `~/.cursor/`。

## 安装

```bash
cp hooks.json ~/.cursor/hooks.json
mkdir -p ~/.cursor/hooks
cp track-zhi.sh check-zhi-on-stop.sh truncate-mcp-output.sh ~/.cursor/hooks/
chmod +x ~/.cursor/hooks/track-zhi.sh ~/.cursor/hooks/check-zhi-on-stop.sh ~/.cursor/hooks/truncate-mcp-output.sh
```

## Hook 说明

| 文件 | 事件 | 作用 |
|------|------|------|
| `hooks.json` | - | 配置文件，包含 rtk + sanshu hook |
| `track-zhi.sh` | `postToolUse` | 【主数据源】追踪 sanshu 工具调用（归一化工具名），写状态文件供 stop 判定；zhi 额外记录「用户回复摘要 + 保活标记」 |
| `truncate-mcp-output.sh` | `postToolUse` | MCP 工具输出超长时截断，减少对话历史膨胀 |
| `check-zhi-on-stop.sh` | `stop` | agent 结束前检查本轮是否需要 zhi 收尾：未收尾→拦截；已收尾但用户最后回复是新指令/保活中→自动续跑 |

## 工作原理（v7：会话隔离状态文件 + zhi 回复语义判定 + transcript 正信号）

`check-zhi-on-stop.sh` 在 `stop` 时执行，按以下优先级判断「本轮是否已用 zhi 收尾」：

1. **主判定 —— 状态文件**（`/tmp/sanshu-zhi-hook-state-<conversation_id>.json`，由 `track-zhi.sh`
   在每次 `postToolUse` 写入，**最可靠**；conversation_id 即 composerId，取不到时退化为 `global`，
   stop 端还可从 `transcript_path` 文件名推导，两端键规则一致）：
   - 新鲜（≤30 分钟）且 `tool=zhi` → **v7 进一步看最后一次 zhi 的回复性质**：
     - `keepalive=true`（弹窗仍开着、用户未回复）→ 拦截，让 agent 重调 zhi 续等；
     - 回复摘要含「完成/结束/done/stop/算了/取消」类字样 → 放行（用户明确收尾）；
     - 回复摘要是其它内容（= 新指令尚未处理完）→ 拦截，注入回复片段自动续跑；
     - 摘要为空（提取失败/旧格式）→ 放行（退化为 v6 行为）。
   - 新鲜且 `tool!=zhi` → 注入 followup，强制补调 zhi；
   - 过期则视为上一轮残留，忽略，进入下一步。

   v7 摘要的取法（`track-zhi.sh`）：从 postToolUse payload 的 `.output` 解析 zhi 返回首行 JSON，
   仅取 `selected_options` + `user_input` 第一行（用户回复后段常拼有「请记住…」偏好长文，
   含"完成确认"等字样，全文匹配会假阳性放行）。
2. **补充 —— transcript 正信号**：解析 `transcript_path`，本轮（= 最后一条真实 user 文本之后）
   若**出现 zhi** → 放行。注意：**只信「看到 zhi」这个正信号**；「没看到」不作为拦截依据（见下）。
3. **兜底 —— 放行**：本轮没调任何 sanshu 工具（纯任务/纯问答）→ 直接放行，不强制 zhi。

**工具识别归一化**（`track-zhi.sh`）：postToolUse 实测对所有工具都触发，工具名有三种形态，
统一去前缀后识别：
- 普通 agent 模式：`MCP:zhi`
- Claude 兼容层：`mcp__user-sanshu__zhi`
- Glass 模式：`CallMcpTool` + `input.server=user-sanshu`

归一化规则：去掉 `MCP:` 前缀、取 `__` 分隔的最后一段 → 裸工具名 `zhi`。

`loop_limit=2` 防止无限循环（最多给 AI 两次补调 zhi 的机会）。

## 演进历史与踩坑

- **v1（状态文件）**：`track-zhi.sh` 写 `/tmp`，`check` 读取。曾因 `postToolUse` 设了不生效的
  `MCP:` matcher 而从未触发，错误推断「Glass 不触发 postToolUse」。
- **v3（transcript 主判定）**：改读 `transcript_path` 三态判定。但**长会话时 Cursor 会把 transcript
  压缩成摘要**，本轮的 zhi 调用从 transcript 消失，导致误判 `no_sanshu` → token 超阈值 →
  **误拦**（明明调过 zhi 却被当成没调）。
- **v4（本版，状态文件主判定）**：实测**去掉 matcher 后 `postToolUse` 对所有模式都触发**
  （含 Glass 的 `CallMcpTool`、普通模式的 `MCP:zhi`、Claude 层的 `mcp__user-sanshu__zhi`），
  只需在 `track-zhi.sh` 里**归一化工具名**即可可靠记录。故回归状态文件为主判定；
  transcript 降级为「正信号补充」（看到 zhi 才信，规避压缩造成的假阴性）；token 闸门兜底。
- **v5（移除 token 闸门）**：实测发现 followup 拦截本身 = 让 agent 续跑 = Cursor 记一条新
  request。对「全程没调 sanshu 的大额会话」用 token 阈值强拦并不能省 request，反而凭空多一条续跑
  request。故移除 token 闸门，只在「调了 sanshu 却没以 zhi 收尾」时拦截。代价：纯大额任务会话不再
  被拦（可能静默结束），这是「省 request」与「防静默断裂」之间的明确取舍。
- **v6（状态文件按会话隔离）**：2026-06-11 实锤误拦——状态文件是全局单文件时，window:3
  会话调 `ji` 写入状态，window:1 的另一会话 30 秒后 stop，误读到 `tool=ji` 被拦，followup 凭空
  多产生一条 11.5 万 token 的 request（详见 `docs/分析报告-20260611-会话断流根因-…md`）。
  修复：`track-zhi.sh` 从 postToolUse payload 取 `conversation_id`（实测必有），状态文件改为
  `/tmp/sanshu-zhi-hook-state-<conversation_id>.json`；`check-zhi-on-stop.sh` 用同样的键读取
  （payload 无 `conversation_id` 时从 `transcript_path` 文件名推导）。附带每次写入时清理
  2 小时以上的残留状态文件。
- **v7（本版，zhi 回复语义判定·断流自动续跑）**：2026-06-11 23:44 实锤——turn 在 zhi 带回
  用户新指令 15 秒后因网络 TLS 断连中止（transcript `turn_ended status=error`，sentry 两连
  ConnectError），v6 看到 `tool=zhi` 即放行，用户被迫手动打「继续」（新 request）。v7 让
  track 端额外记录「zhi 回复摘要 + 保活标记」，stop 端据此区分「完成类回复（放行）/新指令
  未处理完（注入片段自动续跑）/保活中（强制重调 zhi 续等）」。成本中性：followup 续跑与
  手动「继续」同价（都是一条新 request），但免人工值守；网络未恢复时 followup 也会失败，
  `loop_limit=2` 封顶。代价：用户想结束但回复不含完成类字样时，会多一轮收尾确认弹窗。
