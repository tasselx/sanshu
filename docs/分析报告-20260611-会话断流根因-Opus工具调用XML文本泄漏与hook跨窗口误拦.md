# 深层分析报告：6/11 晚同一会话被拆成 7 条 request 的根因

> 分析时间：2026-06-11 22:43–23:00（UTC+8）
> 分析对象：PPDuck3 逆向会话 `composerId = d482361a-60a9-409c-9654-…`（实际 ID `d482361a-60a9-409c-92fd-5499cde01a5f`），21:19–21:43
> 方法：**不参考旧报告结论**，直接交叉比对四方原始日志，全部结论有逐行证据。

## 数据源（证据文件清单）

| 来源 | 路径 |
|---|---|
| 会话 transcript（决定性证据） | `~/.cursor/projects/Users-tassel-Documents-GitHub-reverse-lab-PPDuck3/agent-transcripts/d482361a-…/d482361a-….jsonl` |
| Cursor 主进程日志（wakelock 时间线） | `~/Library/Application Support/Cursor/logs/20260611T211841/main.log` |
| Cursor 渲染端日志（每条 generation 的模型） | `…/logs/20260611T211841/window1/renderer.log` |
| stop hook 调试日志 | `/tmp/sanshu-stop-debug.log` |
| sanshu MCP 服务日志 | `~/Library/Application Support/sanshu/log/sanshu-mcp.log` |
| hook 脚本 | `~/.cursor/hooks/track-zhi.sh`、`~/.cursor/hooks/check-zhi-on-stop.sh` |

## TL;DR（三层根因）

1. **前 3 次 0-token 中止（21:19–21:21，未计费）**：会话最初跑在 `claude-fable-5` 上，连续遭遇上游 **Provider Error**（"trouble connecting to the model provider"），用户随后手动切到 `claude-opus-4-8`。
2. **主因（7 条计费 request 全部如此结束）**：`claude-opus-4-8`（thinking-max 与 thinking-high 均复现）把**工具调用以纯文本形式输出**——assistant 正文里出现字面的 `count\n<invoke name="…"><parameter …>` XML，而不是结构化 tool_use 块。Cursor 解析不到任何工具调用 → 认为模型说完了 → turn 以 `status:"success"` **正常结束**。用户每点一次「继续」= 新的 user message = **新开一条 request**。
3. **21:42 那条 11.5 万是 hook 自己造出来的**：stop hook 的状态文件是**全局共享**的 `/tmp/sanshu-zhi-hook-state.json`，没有会话隔离。window:3 里另一个会话（sanshu 项目诊断会话）在 21:41:43 调了 `ji`，污染了状态文件；window:1 的 PPDuck3 turn 在 21:42:09 结束时被误判为「调了 sanshu 没用 zhi 收尾」→ 注入 followup → **凭空多一条 request**。

另：**旧报告的「thinking tokens 吃光 output 预算被截断」结论不成立**，证据见第 5 节。

---

## 1. 完整时间线（四方日志对齐）

wakelock 区间来自 `main.log`（= 一条 generation/request 的活跃区间），token 数来自 `/tmp/sanshu-stop-debug.log`（stop hook 收到的本轮用量），模型来自 `renderer.log` 的 `buildRequestedModel`，结局来自 transcript。

| # | 模型 | 开始 | 结束 | 时长 | tokens | 账单行 | 结局 |
|---|------|------|------|------|--------|--------|------|
| 0a | fable-5 | 21:19:33 | 21:19:35 | 2.4s | 0 | 无 | Provider Error |
| 0b | fable-5 | 21:19:53 | 21:20:39 | 46s | 0 | 无 | Provider Error（transcript 里这轮无任何输出） |
| 0c | fable-5 | 21:21:24 | 21:21:27 | 2.6s | 0 | 无 | 0-token 中止（用户随后换模型） |
| 1 | opus-4-8-**thinking-max** | 21:21:53 | 21:27:16 | 5m23s | 855,019 | 21:21 / 85.5万 | 11 条 assistant 消息正常干活后，**第一次 XML 泄漏** → 静默结束 |
| 2 | thinking-max | 21:28:23 | 21:29:52 | 88s | 98,039 | 21:28 / 9.8万 | 开场即泄漏（TodoWrite+radare2+Shell 全部变文本），零进展 |
| 3 | thinking-max | 21:31:02 | 21:31:14 | 12s | 98,771 | 21:31 / 9.9万 | 泄漏，零进展 |
| 4 | thinking-max | 21:31:33 | 21:34:11 | 2m38s | 457,285 | 21:31 / 45.7万 | 模型自我察觉格式错误，恢复 3 轮真实工具调用，**随后再次泄漏** |
| 5 | thinking-**high** | 21:41:56 | 21:42:09 | 12.7s | 113,998 | 21:41 / 11.4万 | 泄漏；turn 结束时被 stop hook **误拦** |
| 6 | thinking-high | 21:42:09.8 | 21:42:24 | 14.3s | 114,937 | 21:42 / 11.5万 | **hook followup 自动注入的续跑**（与上一条仅隔 80ms，非人为），又泄漏 |
| 7 | thinking-high | 21:43:02 | 21:43:16 | 14.7s | 115,606 | 21:43 / 11.6万 | 用户「怎么又给我断掉了」，又泄漏 |

与用量截图完全吻合：thinking-max 4 条（85.5/9.8/9.9/45.7万）+ thinking-high 3 条（11.4/11.5/11.6万），共 7 条计费 request；3 次 fable-5 失败轮 0 token 未计费。

---

## 2. 主因实锤：工具调用被输出成纯文本（XML 泄漏）

### 2.1 现场（transcript 原文摘录）

第一次泄漏发生在大轮（#1）末尾，模型刚收到 `use_decompiler` 返回「pdg 不可用」，打算改用 pdc 反编译：

```text
（transcript 第 18 行，assistant 消息的 text 字段内）
`CompressWebp` 位于 `0x046c6150`。pdg 不可用，用默认 pdc 反编译它。

count
<invoke name="CallMcpTool">
<parameter name="server">user-radare2</parameter>
<parameter name="toolName">decompile_function</parameter>
<parameter name="arguments">{"address": "0x046c6150"}</parameter>
</invoke>

 找到了 `CompressWebp` 函数的地址，但在尝试使用反编译器时遇到问题……
```

要点：

- 这段 `<invoke>…</invoke>` 是 **assistant 文本内容的一部分**（Claude 旧版 XML 工具调用语法），不是结构化 `tool_use` 块；前面还带一个损坏的控制标记残留 `count`（该残留在第 14 行还单独出现过一次：`count 我先标记调用链认知…`，那次未致命）。
- XML 之后**还跟着完整的 thinking 文本**，语句完整收尾——输出并没有被截断。
- 紧接着 transcript 第 19 行：`{"type":"turn_ended","status":"success"}`——Cursor 视为正常说完，turn 结束。

### 2.2 量化：每一次静默断流都精确对应一次泄漏

对 transcript 全文统计（`grep` 计数）：

- 字面 `count\n<invoke` 泄漏：**8 处**（第 18、21、23、26、32、35、38、41 行）
- 真实 `"type":"tool_use"` 块：34 个（第 2–17、29–31 行）
- **凡含泄漏的行均无任何真实 tool_use；7 条计费 request 的最后一条 assistant 消息全部是泄漏行**（#1→18，#2→21/23，#3→26，#4→32，#5→35，#6→38，#7→41）。

> 注：第 21/23 行内容逐字节相同（21:28 的 user 消息与回复在 transcript 中重复了一次），但 `main.log` 里 21:28–21:31 只有一个 wakelock 周期（id=4，88s）——是 transcript 落盘的重放伪影，**没有**多计费。

### 2.3 模型自己承认了这个故障

第 4 轮（45.7万那条）开头，模型的 thinking 原文（transcript 第 29 行）：

> " I realize my tool calls didn't execute properly—**the format might have been off or they weren't issued correctly after my thinking**. Let me restructure this and make sure I'm using the right syntax…"

随后它恢复输出真实 tool_use，干了 3 轮活（第 29–31 行），但在第 32 行**再次复发**。

### 2.4 为什么会连环复发：上下文自我污染

第一次泄漏后，**带有字面 `<invoke>` XML 的 assistant 消息进入了对话历史**。后续每一轮，模型看到「上一条 assistant 就是这么写的」，就以极高概率模仿这个坏格式——第 21、26、35 行的泄漏内容几乎是逐字重发同一批失败的调用（同一条 Shell 命令反复出现 5 次）。这解释了：

- 为什么从 thinking-max 切到 thinking-high（#5–#7）**仍然**复发——污染在上下文里，不在模型档位里；
- 为什么第 4 轮自我纠错成功后还会再犯——历史里的坏样本越积越多，把模型反复拉回坏格式；
- 为什么「继续」越点越糟——每次「继续」都让模型在被污染的上下文上再赌一次。

### 2.5 这是 Opus 4.8 的已知故障 family（外部佐证）

- GitHub `anthropics/claude-code` issue #49747：*"[BUG] Opus 4.7 mixes legacy XML tool-use format into …"*
- Cursor 官方论坛 *"Claude can't interact with any tools…"*：「回复里散落着 XML 块…模型表现得像在调用 MCP 工具，实际上没有」——与本次现象一字不差
- Reddit r/ClaudeCode：*"P.S. to all those having issues with Opus 4.8's tool calls"*

即：**模型侧（Opus 4.8 家族 + extended thinking）在长上下文/思考后偶发把工具调用写成旧版 XML 文本**，任何 harness（Cursor/Claude Code）都解析不到调用，只能当普通文本收场。

---

## 3. 21:42 的 11.5 万：stop hook 跨窗口误拦，凭空 +1 request

时序证据（三方日志交叉）：

1. `sanshu-mcp.log:23682-23685` —— 21:41:43，`ji`（action=记忆，`project_path=/Users/tassel/Documents/GitHub/sanshu`）被调用。调用方是 **window:3 的 sanshu 诊断会话**（`main.log` 显示其 agent-loop 21:34:09–21:50:38 持续活跃；该会话 21:40:33 还调过 zhi，workspace=sanshu）。
2. `track-zhi.sh` 的 postToolUse 在 window:3 触发，把 `{"tool":"ji"}` 写入**全局** `/tmp/sanshu-zhi-hook-state.json`（`track-zhi.sh:13` 写死单一路径，无会话/窗口隔离）。
3. 21:42:09，**window:1** 的 PPDuck3 turn（#5）结束，`check-zhi-on-stop.sh` 读到这份不属于自己的状态：`/tmp/sanshu-stop-debug.log` 记录 `state tool=ji age=26` → 走 `block_sanshu`（`check-zhi-on-stop.sh:66`）注入 followup。
4. followup 以 user message 形式出现在 transcript 第 37 行（「⚠️ 强制交互违规检测…」），`main.log` 显示新 generation 在 **80ms 后**自动启动（21:42:09.762）——这就是 21:42 那条 11.5 万的来源。
5. 讽刺点：PPDuck3 会话这一轮**根本没调过任何 sanshu 工具**；而注入的 followup 也没起效——模型在第 38 行又把「跑实测 + 读 zhi schema」的调用泄漏成了文本，14 秒后再次静默结束。

> 顺带说明其余 6 次为什么 stop hook 放行：PPDuck3 会话从头到尾没有成功调用过任何 sanshu 工具（`saw_zhi=0` 贯穿全程），按 v5 设计「没调 sanshu 的纯任务轮不强制 zhi」（`check-zhi-on-stop.sh:96-98`）——hook 按设计放行，不是失效。

---

## 4. 前 3 次 0-token 中止：fable-5 的 Provider Error

- transcript 第 4、6 行：`{"type":"turn_ended","status":"error","error":"Provider Error We're having trouble connecting to the model provider…"}`
- `renderer.log`：21:19:33 / 21:19:53 / 21:21:24 三次 generation 的 `catalogModelId=claude-fable-5`；21:21:53 起变为 `claude-opus-4-8`（用户换了模型）。
- 三次 stop 时 `total_tok=0`（stop-debug log），用量页也没有对应账单行 → **未计费**。

这是上游临时故障，与后面 7 条的断流机理无关，但它解释了会话开头连续两次「继续」的由来。

---

## 5. 推翻旧结论：「thinking 吃光 output 预算被截断」不成立

旧报告（21:46 诊断会话）认为 turn 是被 output 预算截断、「没轮到 emit zhi」。transcript 出来后，该理论与事实矛盾：

| 截断论的预言 | 实际证据 |
|---|---|
| 输出应在中途断掉，句子不完整 | 8 处泄漏消息**全部完整收尾**，XML 后还有成段 thinking 文本（第 18/32/35 行等） |
| turn_ended 应带截断/限额状态 | 7 次全部 `status:"success"` |
| 大输出轮才会触发 | #3、#5–#7 输出极小（turn 仅 12–15 秒），照样断 |
| 换低 thinking 档应缓解 | 切到 thinking-high 后 3 连断（#5–#7） |

旧报告当时只有 stop-debug log 和 main.log（只能看到「没调 zhi 就结束了」），没有 transcript 内容，故只能猜测截断。**真实机理是：调用以文本形式发出 → harness 无调用可执行 → 模型停笔 → turn 正常结束。**

（也要公平地说：thinking 模式不是无辜的——泄漏总是发生在 thinking 块前后衔接处，模型自己也说 "weren't issued correctly **after my thinking**"。extended thinking 是诱因，截断不是机理。）

---

## 6. 损失量化

- 7 条计费 request 中，#1（85.5万）有 11 条消息的真实产出，#4（45.7万）有 3 轮产出；**#2、#3、#5、#6、#7 共约 54.2 万 token 几乎零产出**（每条都是重发 10–45 万上下文 + 一次泄漏失败）。
- 其中 #6（11.5万）是 hook 误拦直接制造的。

---

## 7. 建议（按层）

### 模型层（治本靠上游，用户可规避）
1. 长上下文逆向任务**暂避 Opus 4.8 thinking 系**（max/high 均已实测复发），改用其它模型家族或非 thinking 档；该故障已有社区 issue，可附本报告 transcript 证据向 Cursor/Anthropic 反馈。
2. **识别污染信号**：一旦看到 assistant 正文里出现 `<invoke name=…>` / 散落 XML / 停止按钮变发送但啥也没执行——**不要再点「继续」**（每次都是 10万+ token 的新 request，且大概率复发）。正确做法：新开会话，或在新消息里明确写「上一条的工具调用变成了纯文本，请重新以正常方式调用工具」。

### hook 层（本仓库可改，待确认后实施）
3. **状态文件按会话隔离**：`track-zhi.sh` / `check-zhi-on-stop.sh` 的 `STATE_FILE` 至少按 workspace 维度拆分（如 `/tmp/sanshu-zhi-hook-state-<workspace哈希>.json`；postToolUse/stop 的 payload 若含 `transcript_path`/`workspace_roots` 可直接取，需实测字段）。本次误拦就是 sanshu 会话污染了 PPDuck3 会话的判定。
4. 可选：stop hook 的 followup 文案里加一句「若上一条回复中工具调用被输出为 XML 文本，请改用正常工具调用格式重发」，让兜底续跑至少有机会自愈格式。

### 规则层
5. 「补充 C」规则把这次事故归因于 thinking-max 截断（规则 12–15 的背景描述），与本报告结论不符，建议把背景描述修正为「Opus 4.8 thinking 系工具调用 XML 文本泄漏」，结论性条款（先 zhi 再干活、避免赌满单 turn）仍然有效——早 zhi 锚点能把损失从「整轮白跑」降为「至少有一次交互确认」。

---

## 8. 后续整改（同日完成）

### 8.1 hook v6：状态文件按会话隔离（根除第 3 节误拦）

- `scripts/cursor-hooks/track-zhi.sh`：状态文件改为 `/tmp/sanshu-zhi-hook-state-<conversation_id>.json`（postToolUse payload 实测必含 `conversation_id`，即 composerId），附带清理 2 小时以上残留；
- `scripts/cursor-hooks/check-zhi-on-stop.sh`：用同样的键读取，payload 缺 `conversation_id` 时从 `transcript_path` 文件名推导；
- 已同步部署到 `~/.cursor/hooks/` 并实时验证（调试日志出现 `conv=<本会话ID>`）。

### 8.2 zhi 源码审查（应用户要求）与 3 项 P0 修复

| # | 问题（均有实证） | 修复 |
|---|---|---|
| P0-1 | `zhi_history.rs` 不限字段长：单条 user_reply 实测 10.1MB（6/7 用户在弹窗粘贴整份 spindump），单文件 9.8MB，每次 add 整文件重写、enhance 摘要也会加载 | 写入前按 4000 字符截断 prompt/user_reply 并打标记（`truncate_field`） |
| P0-2 | `popup.rs` 注册表仅按 workspace 键控：同 workspace 两个会话会互相「重连」对方弹窗，造成答案错配（与 hook 误拦同构） | `popup_key` 并入 brief 内容指纹（`workspace#hash`），同问题保活重连仍复用，不同问题各开弹窗 |
| P0-3 | 挂起/断流后用户才提交的弹窗回复被 `reap_abandoned_popups` 静默丢弃（UI 显示提交成功，实际进黑洞） | reap 时把有效回复持久化到 `~/.sanshu/orphan_replies/<request_id>.json`；下次同 workspace 的 zhi 完成时附带一次性提示（提示后文件改名 `.seen.json` 防重复） |

| P1 | 巨型粘贴回复原样回传模型（10MB≈百万级 token，是 6/7 单条 3700 万 token 的底层推手之一） | `RESPONSE_LEN_WARN_THRESHOLD=50K`（与客户端 truncate hook 对齐）：超过则 server 打 warn 日志 + zhi 返回附「只引用关键片段、勿复述全文」提示块；不截断用户内容 |

P1 备忘（未改，留待后续）：`ZHI_CALL_CADENCE` map 进程级微增长（可忽略）；「Pending 后 N 分钟无重连」诊断日志可辅助发现断流。

### 8.3 全仓巡检（zhi 之外模块）与 2 项 P1 修复

巡检范围：`mcp/server.rs`、`handlers/icon_popup.rs`、`tools/{memory,enhance,sou,icon,acemcp}`、`utils/logger.rs`、`telegram/mcp_handler.rs`、`acemcp/watcher.rs`。

| # | 问题 | 修复 |
|---|---|---|
| P1-a | `tu` 图标工具：async 处理器里直接同步调用 `create_icon_popup`（`cmd.output()` 等用户关弹窗，可达数分钟），占死一个 tokio worker；且无 progress 心跳/Pending 重连，用户挑图标超时即断（与 zhi 修复前同类） | `IconTool::tu` 改为 `spawn_blocking` 包裹（心跳留待后续，tu 使用频率低） |
| P1-b | enhance 对话历史 `ChatHistoryManager::add_entry`：`ai_response` 截 500 字符但 `user_input` 全量落盘；弹窗回复（source=popup）也写入此处，10MB 粘贴同样落盘 ×20 条上限，且 enhance 会把历史作为上下文送 API | `user_input` 写入前 `safe_truncate(4000)`（与 zhi_history MAX_FIELD_CHARS 同源） |

P2 备忘（经确认暂不修）：`ji` 记忆 `add_memory` 无单条长度上限、`get_project_info` 全量拼接概览（记忆越多每次会话开始越贵）；`watcher.rs` 13 处 `.lock().unwrap()` 锁中毒会 panic 常驻 MCP 进程；`is_tool_enabled` 每次 list/call 重读配置文件（list_tools 一次读 8 遍）。

巡检无问题项：logger 轮转/保留完善；zhi 轮询已正确 `spawn_blocking`；sou fast_context 有大小上限（tree 250KB / 行 250 字符）+ 限流 + 抖动重试 + JWT 缓存；memory 有相似度去重。

## 9. 当晚 23:44 二次断流：网络 TLS 断连（非模型/非 hook）与 hook v7

### 9.1 取证时间线（fable-5-thinking-max 会话，conv=77651bb4）

| 时间 | 事件 | 来源 |
|---|---|---|
| 22:42:59 | turn 开始（wakelock 启动） | main.log |
| 22:40→23:40 | api2.cursor.sh 超时 ×6（整晚网络不稳） | network-shared.log |
| 23:11/23:15 | TLS ConnectError ×3（撞在工具等待期，重试扛过） | sentry-events |
| 23:44:19 | 第 12 次 zhi 返回用户新提问（generation_id 无断点贯穿 23:38→23:44） | sanshu 日志 + hook 日志 |
| **23:44:29/31** | **TLS 连接失败 ×2**（下一次 API 调用无法建连） | sentry-events |
| 23:44:34 | turn 以 error 终止：`[aborted] Client network socket disconnected before secure TLS connection was established`；wakelock 释放（持有 61.6 分钟） | transcript + main.log |
| 23:44:34 | stop hook 运行：`tool=zhi age=15` → 按 v6 设计放行（无法分辨「回复是新指令」） | stop 调试日志 |
| 23:45:22 | 用户手动「继续」→ 新 request | main.log |

### 9.2 账单行解读（usage 面板）—— 已用 requestTraces 定性

`cursor.requestTraces.log`（`logs/<session>/window1_wb2/output_…/`）记录每个 `agent.request` span 的起止，
是「request 计数」的客户端权威源。当晚全部 agent.request 只有 3 个：

| requestId | 起止（本地） | 时长 | 对应 |
|---|---|---|---|
| 067f1aaf | 22:42:59 → 23:44:35 | 61.6 min | turn A（TLS 断连中止） |
| 36cbb416 | 23:45:22 → 00:01:03 | 15.7 min | 「继续」turn（**又一次 TLS 断连中止**，transcript 第 62 行 turn_ended error 同文案） |
| 91922c30 | 00:02:11 → 进行中 | - | 「完成 V7」turn（本轮） |

- **10:43 PM / 1727.3万**：turn A 主体（数十次 API 调用 × 每次重发 ~34万 token 上下文）。
- **11:42 PM / 34.2万**：**23:42 没有任何 agent.request 边界**（turn A 单 span 贯穿）——该行只是
  turn A 末段单次 API 调用（≈34万）的用量上报批次，按调用时间戳单独成行，**不是额外的 request**。
  面板最右侧的「请求数」列可直接核对（34.2万这行应为 0 或并入 turn A 计数）。
- **11:45 PM / 374.6万 / 请求数 1**：「继续」turn——几十次工具调用+多次 zhi 弹窗压在 1 条 request 里，符合设计。

**当晚共 3 条 request 的根因只有一个：网络 TLS 断连 ×2（23:44:29、00:01:01），每次都把跑着的
turn 杀掉，迫使用户手动续命。** renderer.log 显示 23:44:29→23:44:42 连 auth 刷新都失败（完全断网
~15 秒）；00:01 那次同样 auth 刷新失败（00:01:01、00:01:18 两连），且断点都落在「上一个工具刚返回、
agent loop 正要发起下一次 API 调用」的建连瞬间——工具等待期的断网（23:11/23:15 ×3）反而都被
重试扛过。网络层（代理/出口节点）不稳是当晚唯一推手，与模型行为无关。
全晚 TLS 断连分布：23:11 ×2、23:15、23:44 风暴（29s→42s 共 8+ 次）、00:01 ×2——约每 15~30 分钟
一次短时全断，疑似代理节点轮换/抖动，建议为 cursor.sh/cursor.com 域固定稳定出口节点。

### 9.3 21:19→22:40 时段补查（上一个 Cursor 会话 20260611T211841）

当晚 21:19 起的全部 agent.request（跨两个项目、三个窗口）：

| 本地时间 | 会话（项目） | 请求数/时长 | 结局 |
|---|---|---|---|
| 21:19:33→21:43:02 | 「项目压缩 webp 图片逻辑」d482361a（PPDuck3，window1） | 10 条，2.6s~5.4min | 全部正常完成（每条 = 一次用户消息，正常计费） |
| 21:34:09→21:50:38 | 「请求意外中断」b8d69e16（sanshu，window3） | 1 条，16.5min | 正常完成（期间还扛过一次 21:44 workbench 重载） |
| 21:44:17→22:26:51 | 「项目开发工具」d40694b0（PPDuck3，window3） | 1 条，42.5min | **TLS 断连杀掉**（transcript turn_ended error 同文案；wakelock heldForMs=2553890 吻合） |
| 22:42:59→… | 本会话 77651bb4（sanshu，新 Cursor 会话 window1） | 见 9.2 表 | 两次被 TLS 杀掉 |

关键补充证据：
- 上一会话期间 renderer 的 ConnectError 全部集中在 **22:26→22:34**（10 次）——第一波网络故障
  正好在 22:26:51 杀掉跑了 42.5 分钟的 PPDuck3 turn；21:19→22:25 网络安静、零错误。
- 22:40:17 各扩展进程 exit code 0 干净退出 = 用户主动重启 Cursor（非崩溃），进入当前会话 20260611T224022。
- 至此当晚 TLS 断连共 **6 波**（22:26~22:34、23:11、23:15、23:44、00:01），跨两个项目共杀掉
  **3 个长 turn**（42.5min / 61.6min / 15.7min），全部命中「长 turn 的下一次建连」；
  短 turn（<6min 的 10 条）全部幸存——turn 越长，撞上断网窗口的概率越大。

### 9.4 hook v7：断流自动续跑（已实现并部署）

v6 盲区：stop 端只知道「本轮调过 zhi」，无法分辨最后一次 zhi 带回的是「完成指令」还是「新指令/保活中」。
v7 修复（成本中性：followup 续跑与手动「继续」同价，但免人工值守）：

- `track-zhi.sh`：zhi 调用时从 payload `.output` 解析返回首行 JSON，状态文件额外记录
  `reply`（selected_options + user_input 首行，截 300 字符——只取首行是为了避开用户回复
  后段「请记住…完成确认…」偏好长文造成的假阳性）与 `keepalive`（命中保活话术）。
- `check-zhi-on-stop.sh`：`tool=zhi` 时分流——保活中→拦截重调 zhi 续等；回复含
  完成/结束/done/stop/算了/取消→放行；其它（新指令未处理完）→注入回复片段自动续跑；
  摘要为空→退化 v6 放行。
- 5 个分支已用模拟 payload 实测：新指令→续跑拦截 ✓、完成→放行 ✓、保活→续等拦截 ✓、
  提取失败→v6 放行 ✓、非 zhi sanshu 工具→违规拦截 ✓。

## 附：本次分析的取证路径（可复现）

```bash
# 1. turn 结束事件与 token
cat /tmp/sanshu-stop-debug.log
# 2. 每条 generation 的起止与归属窗口
grep wakelock "~/Library/Application Support/Cursor/logs/20260611T211841/main.log"
# 3. 每条 generation 用的模型
grep buildRequestedModel "…/logs/20260611T211841/window1/renderer.log"
# 4. 泄漏与真实调用分布
grep -c 'count\\n<invoke' <transcript>; grep -o '"type":"tool_use"' <transcript> | wc -l
# 5. 跨窗口 ji 调用
grep -n "21:41:43" "~/Library/Application Support/sanshu/log/sanshu-mcp.log"
```
