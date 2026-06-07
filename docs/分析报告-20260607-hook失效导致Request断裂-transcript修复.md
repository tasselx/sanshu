# 分析报告：stop hook 失效导致 Request 断裂（已修复为 transcript 判定）

> 日期：2026-06-07
> 触发问题：用户反馈「最近一条记录怎么突然又断掉了，是不是 hook 没生效」
> 关联记录：Cursor usage 中一条 **256.8 万 token** 的 `claude-opus-4-8-thinking-xhigh` 记录（17:33）
> 关联前文：`docs/分析报告-20260607-Request提前结束未调用zhi.md`（上一次是 output 预算耗尽被截断，本次不同）

## 一、现象

截图那条 256.8 万 token 的记录，经日志定位 = **2026-06-07 17:33 那次会话的 `stop` 事件**
（工作区为 `/Users/tassel/Downloads/Qoder` 的逆向任务，但 hook 是用户级、对所有工作区生效）。

Cursor hook 日志原文（时间为 UTC，`09:33Z` = 北京 17:33）：

```
Command: hooks/check-zhi-on-stop.sh (32ms) exit code: 0
INPUT: { "status": "completed", "input_tokens": 2505441, "output_tokens": 62524, "hook_event_name": "stop" }
OUTPUT: {}
```

`250.5 万 + 6.25 万 ≈ 256.8 万`，与截图完全吻合。`stop` hook 触发了、`check-zhi-on-stop.sh`
也执行了，但返回 `{}`（放行）→ turn 正常结束 → 被算成一条独立 Request（即「断掉」）。

## 二、根因（与上次「被截断」是不同的断法）

### 1. 这次是「主动 completed」而非「被截断」

`stop` payload 中 `status = "completed"`：模型**自己认为任务完成、主动正常结束了 turn**，
不是 output budget 耗尽被动截断。

### 2. turn 内其实调用过 zhi，但结尾没有以 zhi 收尾

解析该会话 transcript（`e89a133a-…jsonl`，共 31 行、1 个 turn）逐行工具调用：

- 本轮调用过 **4 次 `zhi`、多次 `ji`**（均为 `CallMcpTool → user-sanshu`）；
- 但**最后一个工具调用是 `Read`**，其后输出纯文本即 `turn_ended: success`；
- 即：**本轮没有以 zhi 收尾确认**就结束了。

### 3. hook 为什么没拦住——整套防线第一环失效

旧版 `check-zhi-on-stop.sh` 依赖 `/tmp/sanshu-zhi-hook-state.json` 状态文件判断本轮是否调过 zhi，
而该文件由 `postToolUse` 的 `track-zhi.sh` 写入。实测：

- **Glass 模式下 agent 通过 `CallMcpTool` 调用 MCP，不触发 `postToolUse` 的 `MCP: user-sanshu` matcher**，
  `track-zhi.sh` **从未真正执行**；
- 证据：所有 Cursor hook 日志中 `postToolUse` 仅出现 `Hook step requested: postToolUse`，
  **从无 `Found ... to execute` + 运行 track-zhi** 的记录；`/tmp/sanshu*` 长期为空
  （事后调用 `ji` 也不生成）；
- 于是 `stop` 时永远命中 `if [ ! -f "$STATE_FILE" ] → echo '{}'` 的「无状态文件即放行」分支，
  **hook 完全失效**，本轮是否以 zhi 收尾都照样放行。

> 附带澄清：事件名不是问题。日志显示 Cursor 3.7.12 已通过「Claude user hooks」兼容层成功
> `Loaded 4 user hook(s) for steps: preToolUse, postToolUse, stop`。问题出在 Glass 模式的
> MCP 调用路径（CallMcpTool）绕过了 postToolUse 的 MCP matcher。

### 根因一句话

> **防线第一环（track-zhi 写状态文件）在 Glass 模式下压根没生效 → stop 兜底逻辑每次都放行
> → 本轮即使没以 zhi 收尾也正常结束 → 被算成独立 Request。**

## 三、修复方案（A+B 双保险）

### 方案 A（主修复）：`check-zhi-on-stop.sh` 改为解析 transcript

`stop` payload 自带 `transcript_path`，其中**真实记录**了 `CallMcpTool → user-sanshu → zhi`
（Glass / 普通模式都记录）。新逻辑：

1. 读 `transcript_path`，用 `jq` 取「本轮」= 最后一条真实 `user` 消息之后的所有 assistant `tool_use`；
2. 判定：
   - 本轮未调用任何 sanshu 工具 → **放行**（纯文本问答 / 非 sanshu 任务，不强制）；
   - 本轮调过 sanshu 工具且**最后一个工具是 zhi** → **放行**；
   - 本轮调过 sanshu 工具但**最后一个工具不是 zhi** → **注入 followup**，强制补调 zhi；
3. transcript 不可用时回退到 `/tmp` 状态文件（旧逻辑）。

sanshu 工具识别同时兼容两种记录格式：
- Glass：`name == "CallMcpTool" && input.server == "user-sanshu"`；
- 普通模式：`name` 匹配 `user-sanshu`（如 `mcp__user-sanshu__zhi`）。

### 方案 B（冗余）：修正 `track-zhi.sh` + `hooks.json`

- `hooks.json`：去掉 `track-zhi.sh` 不生效的 `matcher: "MCP: user-sanshu"`，
  改为对所有 `postToolUse` 触发、脚本内部判断是否 sanshu 工具；
- `track-zhi.sh`：工具名提取健壮化（优先 `tool_name`，并识别 `server` / `mcp__user-sanshu__*`），
  仅在确为 sanshu 工具时写状态文件（避免被其它工具覆盖）。
- 该路径仅在**普通 agent 模式**下作为回退生效（Glass 模式仍以 transcript 为准）。

## 四、验证结果

用真实历史 transcript 端到端跑新 `check-zhi-on-stop.sh`：

| 会话 | 本轮最后一个工具 | 是否用过 sanshu | 期望 | 实际 |
|------|------------------|----------------|------|------|
| e89a133a (截图那次) | Read | 是(zhi/ji) | block | ✅ block |
| 0e231bde / d7169245 | zhi | 是 | allow | ✅ allow |
| cdb3b85f | ji | 是 | block | ✅ block |
| 6af299a4 | Write | 是(zhi) | block | ✅ block |
| 12bd2829 | (无工具) | 否 | allow | ✅ allow |
| 040ebb96 | Glob | 否 | allow | ✅ allow |

`track-zhi.sh`：喂 `mcp__user-sanshu__zhi` 正确写入 `{tool:zhi}`，随后喂 `Shell` 不覆盖；
两处 `hooks.json` 通过 `jq` 合法性校验。

## 五、补充：token 阈值闸门（覆盖全程未调 sanshu 的会话）

针对「四（六）」提到的边界——某次会话**全程没调任何 sanshu 工具**（如截图那次逆向任务），
原 transcript 判定会放行。新增 **token 闸门**：

- `check-zhi-on-stop.sh` 从 `stop` payload 读取 `input_tokens + output_tokens`；
- 当本轮属于「没调 sanshu 工具」一类、但 token 总量 **超过 `ZHI_TOKEN_THRESHOLD`（默认 150 万）**，
  仍注入 followup，要求结束前用 zhi 向用户确认收尾；
- 该闸门在主判定路径和 /tmp 回退路径都生效（token 来自 payload，不依赖 transcript）。

判定优先级最终为：
1. 调过 sanshu 且最后非 zhi → 拦截（漏收尾）；
2. 最后是 zhi → 放行（token 再大也放行）；
3. 没调 sanshu：token 超阈值 → 拦截；否则放行。

> 阈值说明：`input_tokens` 含 `cache_read`（长对话会偏大），默认 150 万是为抓住「截图那种 ~256 万的大额 turn」
> 而不误伤一般轮次。阈值是脚本顶部常量 `ZHI_TOKEN_THRESHOLD`，可按需调整。

## 六、Glass 模式 与 多模式兼容

- **Glass 模式**：当前这套 Cursor 调用 MCP 的方式——agent 通过读 `mcps/` 描述符 + `CallMcpTool` 调用 MCP。
  证据：所有 sanshu / Qoder 的 transcript 里 MCP 调用都记为 `name:"CallMcpTool"`，
  其 `input` 为 `{server, toolName, arguments}`。该路径**不触发** `postToolUse` 的 `MCP:` matcher。
- **普通 agent 模式**：MCP 工具作为原生工具暴露，工具名形如 `mcp__user-sanshu__zhi`，会正常触发 `postToolUse`。
- **本次修复对两者都兼容**：
  - `check-zhi-on-stop.sh` 的 sanshu/zhi 识别同时匹配 `CallMcpTool+server`（Glass）与 `name` 含 `user-sanshu`（普通）；
  - transcript 不可用时回退到 `/tmp` 状态文件（由 `track-zhi.sh` 在普通模式写入）；
  - `track-zhi.sh` 去掉了失效的 matcher、字段提取健壮化，普通模式下作冗余数据源。

## 七、改动文件清单

实际生效（`~/.cursor/`）与项目备份（`scripts/cursor-hooks/`）两处同步：

- `check-zhi-on-stop.sh`（重写：transcript 三态判定 + token 闸门 + /tmp 回退）
- `track-zhi.sh`（健壮化：字段提取 + 仅 sanshu 才写状态、去 matcher）
- `hooks.json`（去掉 track-zhi 的失效 matcher）
- `truncate-mcp-output.sh`（纳入项目备份，原仅存于 `~/.cursor/hooks/`）
- `scripts/cursor-hooks/README.md`（更新工作原理：v3 transcript + token 阈值 + 多模式兼容）

## 八、已知边界

1. `loop_limit=2`：最多注入 2 次 followup；若模型仍坚持不调 zhi，第 3 次放行。
2. transcript 解析依赖 `jq`（系统已安装 `/usr/bin/jq`）。
3. token 闸门用 `input_tokens+output_tokens`（含 cache_read）；长对话中即使小问答 input 也可能偏大，
   如发现误拦过多可调高 `ZHI_TOKEN_THRESHOLD`。
4. 普通模式裸工具名（无 `user-sanshu` 前缀，如纯 `zhi`）依赖 `track-zhi.sh` 回退识别；
   主流 `mcp__user-sanshu__*` 命名已由 transcript 主判定覆盖。

## 九、v4 重大修正（状态文件主判定）—— 2026-06-07 18:3x

> 本节推翻了第三~六节「transcript 主判定」的核心假设，请以本节为准。

### 9.1 新发现：v3 transcript 判定有两个致命问题

实跑中 `stop` hook 真实拦截了本会话一次（debug log：`decision=no_sanshu, total_tok=9295236`），
但本轮**明明调过 4 次 zhi**。复盘 `/tmp/sanshu-hook-debug.log` 与 transcript 文件后定位到两个 bug：

1. **transcript 被 Cursor 压缩**：长会话时 `transcript_path` 指向的 `.jsonl` 会被压缩成「对话摘要」。
   本会话几十次工具调用在该文件里只剩 **6 行摘要**，所有 `CallMcpTool→zhi` 全部消失。
   于是「读 transcript 判本轮是否调 zhi」在最该拦截的大额会话里恰好**假阴性**，误判 `no_sanshu`。
2. **关键反转：`postToolUse` 其实对所有模式都触发**。`/tmp/sanshu-hook-debug.log` 铁证：
   本轮 4 次 zhi 全被 `track-zhi.sh` 捕获，工具名为 `MCP:zhi` 和 `mcp__user-sanshu__zhi`。
   v3 之前「Glass 不触发 postToolUse」的结论是错的——真正原因只是 **matcher 写法不对** + **工具名漏判**：
   `track-zhi.sh` 的 case 只认裸 `zhi` 和 `*sanshu*`，而 `MCP:zhi` 既不等于 `zhi` 也不含 `sanshu`，
   被漏判、没写状态文件。

### 9.2 修复

- **`track-zhi.sh` 工具名归一化**：去掉 `MCP:` 前缀、取 `__` 分隔的最后一段，
  使 `MCP:zhi` / `mcp__user-sanshu__zhi` / `zhi` 统一识别为 `zhi`，可靠写入状态文件。
- **`check-zhi-on-stop.sh` 判定优先级重排（v4）**：
  1. **状态文件为主**（postToolUse 实测可靠）：新鲜 `tool=zhi` 放行；新鲜 `tool!=zhi` 拦截；过期忽略。
  2. **transcript 仅作正信号**：本轮看到 zhi 才放行；**看不到不拦**（规避压缩造成的假阴性）。
  3. **token 闸门兜底**：超阈值拦截，否则放行。

### 9.3 各模式工具名形态（实测）

| 模式 | postToolUse 工具名 | transcript 中形态 |
|------|-------------------|------------------|
| 普通 agent | `MCP:zhi` | 可能被压缩丢失 |
| Claude 兼容层 | `mcp__user-sanshu__zhi` | 同上 |
| Glass | `CallMcpTool`(+`input.server`) | `name:CallMcpTool, input.server/toolName` |

### 9.4 收益

- 不再依赖会被压缩的 transcript 作主判定 → 杜绝「调过 zhi 却被误拦」。
- 状态文件由 `postToolUse` 实时写入，Glass / 普通 / Claude 三种模式通吃。
- 保留 token 闸门，覆盖「全程不调 sanshu 的大额会话」。
- 本次修复后，**本轮（040ebb96）以 zhi 收尾即应实跑判定为 `state tool=zhi → allow`**，可在
  `/tmp/sanshu-stop-debug.log` 末行核对。

## 十、v5：移除 token 闸门（followup 拦截本身会 +1 request）

### 10.1 新发现

用户在 Cursor 后台看到**同一会话被拆成两条 request**：946 万（被 `block_bigturn` 拦）+ 268 万
（被拦后注入 followup 续跑而产生的新 request）。由此确认一个关键机制：

> **任何 followup 拦截 = 让 agent 续跑 = Cursor 记一条新 generation / request。**

因此 hook 拦截的价值是「**保留上下文 + 强制 zhi 确认、避免静默断裂**」，**而非节省 request**——
request 数与「断裂后手动新开」基本相当（都 +1），区别在于拦截能留住上下文。

### 10.2 决策（用户拍板）

对「全程没调任何 sanshu 工具的大额会话」用 token 阈值强行拦截，**不能省 request，反而凭空多一条
续跑 request**。故**移除 token 闸门**，只保留「调了 sanshu 却没以 zhi 收尾」这一真正违反强制交互的拦截。

### 10.3 v5 判定（最终）

1. 状态文件新鲜 `tool=zhi` → 放行；`tool!=zhi` → 拦截；过期忽略。
2. transcript 正信号：本轮看到 zhi → 放行。
3. 兜底：放行（纯任务 / 没调 sanshu，不强制）。

### 10.4 trade-off

- 优点：不再因 token 凭空多 request；调过 zhi 的会话稳定放行。
- 代价：全程没调 sanshu 的大额纯任务会话（如最初截图的逆向任务）**不再被拦**，可能静默结束——
  这是用户在「省 request」与「防静默断裂」之间的明确取舍。
