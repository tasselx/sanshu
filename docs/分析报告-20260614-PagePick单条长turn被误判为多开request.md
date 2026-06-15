# 分析报告：6/14「为什么多开了一条 request」——其实没多开，是单条超长 turn 的用量被拆成两行

> 分析时间：2026-06-14 13:32–13:40（UTC+8）
> 触发：用量面板出现两行 `12:18 PM / 1500.2万` 与 `01:26 PM / 10万`（均 `claude-opus-4-8-thinking-xhigh`），用户怀疑「多开了一条 request」。
> 方法：以客户端权威计数源 `cursor.requestTraces.log` 为准，交叉比对 main.log（wakelock）、renderer.log（模型）、会话 transcript，全部结论有逐行证据。
> 结论先行：**根本没有多开 request。两行是同一条 request 的两次「用量上报批次」。**

## TL;DR

1. **只有 1 条 request**：`requestId = e04143c7-e7c6-43a2-ab7c-1488ff059711`，从 `12:18:38` 贯穿到 `~13:36`，整份 1.29MB 的 trace 里 `agent.request` 的 `span_started` 只出现 **1 次**、tagged 的 requestId 只有 **1 个**。
2. **它为什么这么贵（~15M token / ~78 分钟）**：一句话提示词触发了一次**完整的自主建站式开发**——约 **134 次模型迭代 + 226 次真实工具调用（其中 175 次文件写入 / 70 次读 / 20 次 shell / 20 次 MCP / 12 次 grep）**，每次迭代都重发不断增长的工程上下文，累计到约 1500 万 token。
3. **三个怀疑全部排除**：① 无 XML 文本泄漏（transcript 里 `<invoke` 字面 0 处，trace 里是 226 次**真实执行**的工具调用而非文本）；② 非 zhi 反复保活（该会话属 PagePick 项目，全程仅 20 次 MCP 调用，不是弹窗循环）；③ 就是**任务本身长**（175 次文件写 = 在搭一整个浏览器插件）。
4. **13:26 那一行的真相**：会话主流 `rpc.run` 跑了 **67.5 分钟后于 `13:26:07` 以 `error=true` 结束**（长连接超时/断流），这正是「01:26 PM / 10万」那行的时间戳——它是流出错点的一次用量批次，**仍属同一条 request**，不是新请求。
5. **与 6/11 真·断流相反**：6/11 是 7 条独立 `agent.request`（XML 泄漏 / TLS 断连，每次「继续」都新开一条）；本次自始至终 **1 条** `agent.request`，没有断成多条。

---

## 1. 数据源（证据清单）

| 来源 | 路径 |
|---|---|
| 客户端 request 权威计数 | `~/Library/Application Support/Cursor/logs/20260614T114202/window1_wb4/output_20260614T121728/cursor.requestTraces.log`（1.29MB / 5259 行） |
| wakelock 时间线 | `…/20260614T114202/main.log` |
| 模型归属 | `…/window1_wb4/renderer.log` |
| 触发会话 transcript | `~/.cursor/projects/Users-tassel-Downloads-PagePick/agent-transcripts/a4d63572-…/a4d63572-….jsonl`（**该会话属 PagePick 项目，非 sanshu**） |

> 关键澄清：12:18 这条会话 `composerId = a4d63572-f6fb-427a-9728-8665ebf62943`，工作区是 `~/Downloads/PagePick`。这就是它没出现在 sanshu `agent-transcripts` 里的原因。

## 2. 完整时间线（UTC+8 本地）

| 时间 | 事件 | 来源 |
|---|---|---|
| 12:17:28 | window1_wb4 trace 开始录制 | requestTraces |
| **12:18:38** | 用户提示词「帮我开发一个跟印象笔记一样的剪藏浏览器插件」；request `e04143c7` 启动；wakelock id=0 `agent-loop` 起 | transcript / requestTraces / main.log |
| 12:18→13:26 | **一条连续 agent 流**：~134 次模型迭代、224 次工具完成（含 175 写）；wakelock 在长工具等待期 `agent-loop-resumed` 反复续（id=0→1→2），始终同一 requestId | requestTraces / main.log |
| **13:26:07** | 主流 `rpc.run` 跑满 **67.5 分钟（durationMs=4049064）后 `error=true`** —— 对应面板「01:26 PM / 10万」 | requestTraces 第 4963 行 |
| 13:26→13:36 | 仅剩 **2 次**工具完成；内部重连（全程 4 次 handshake）；request 收尾，仍是同一 requestId | requestTraces |
| 13:32:16 | 用户在**新窗口**（wb5 / sanshu）开本分析会话，wakelock id=3 `activeCount=2`（PagePick 的 id=2 仍未释放） | main.log |

## 3. 实锤一：整份 trace 只有 1 条 `agent.request`

- `grep 'name="agent.request"'` → `span_started` **1 次**；`agent_request_tagged` 的 requestId 去重后 **只有 1 个**：`e04143c7-…`。
- 从首行到末行（`05:36:13Z = 13:36:13`），**每一条 span 都挂同一个 `requestId=e04143c7`**，无第二个 requestId。
- `renderer.log` 中 `buildRequestedModel` 仅 **1 次**（`catalogModelId=claude-opus-4-8`），即一个逻辑 turn。

> 即：客户端层面这就是**一条**请求。用量面板把它按时间戳拆成两行展示，最右侧每行的「1」是行内展示数字，**不等于「又起了一条 agent 请求」**。这与 `docs/分析报告-20260611-…` 第 9.2 节记录的「单 turn 多行用量批次」现象同构。

## 4. 实锤二：15M token 的构成 = 真实的大规模开发，不是空转

对 requestTraces 的 span 频次统计：

| span | 次数 | 含义 |
|---|---|---|
| `AgentResponseAdapter.thinkingCompleted` | 134 | ~134 次模型迭代（每次都重发增长中的上下文） |
| `AgentResponseAdapter.toolCallStarted/Completed` | 226 / 226 | ~226 次**真实**工具调用 |
| `LocalWriteExecutor.execute` | **175** | 175 次文件写入（搭整个插件代码） |
| `LocalReadExecutor.execute` | 70 | 70 次文件读 |
| `ShellCoreExecutor.execute` | 20 | 20 次 shell |
| `LocalMcpToolExecutor.execute` | 20 | 20 次 MCP |
| `LocalGrepExecutor.execute` | 12 | 12 次检索 |

估算：15M token / 134 次迭代 ≈ 每次 ~11 万 token 上下文，对长上下文编码代理完全正常。**226 次工具是真执行（有 `*.execute` / `processToolCallCompleted` 完成 span），不是被输出成文本的伪调用。**

## 5. 实锤三：三个怀疑逐一排除

| 怀疑 | 证据 | 结论 |
|---|---|---|
| Opus 工具调用 XML 文本泄漏 | transcript 里字面 `<invoke` **0 处**；trace 里 226 次工具均有真实 `execute`/`Completed` 完成 span | **排除** |
| zhi 反复保活 / 弹窗循环 | 会话属 PagePick，全程仅 20 次 MCP 调用，无 sanshu zhi 参与；无高频同类调用 | **排除** |
| 任务本身就长 | 175 次文件写 + 134 次迭代 = 从零搭一整个剪藏插件 | **成立（主因）** |
| 附带：13:26 的额外一行 | `rpc.run` 67.5 分钟后 `error=true`，落在同一 requestId 内 | 长连接断流的用量批次，**非新请求** |

## 6. renderer 里的「error」是噪音

`renderer.log` 中的 error 全是启动期噪音：`Cannot register two commands with the same id`、`punycode DeprecationWarning` 等扩展注册告警；**没有** provider error / TLS / abort / 截断。trace 里唯一的真实 `error=true` 只有 13:26 那条 `rpc.run`（长流出错）。

## 7. 结论与建议

- **回答「为什么多开了一条 request」**：没有多开。它是**一条** request（`e04143c7`），用量面板按时间戳把同一条请求拆成「12:18 主体 15M」+「13:26 批次 10万」两行；最右侧的「1」是面板每行展示数，不是请求数。客户端 `requestTraces` 实打实只有 1 个 `agent.request`。
- **真正值得在意的是成本**：一句话提示词跑出一条 **78 分钟 / ~15M token** 的超长单 turn（在最贵的 opus-4-8 thinking-xhigh 上），且 13:26 长连接还 `error=true` 断了一次。建议：
  1. 这类「从零搭整个项目」的开放式大任务，提示里先给**分阶段计划**并要求**每阶段停下确认**，把单 turn 切短，既省钱也避免长流超时断在中途；
  2. 长上下文建站任务可考虑非 thinking 档或更便宜模型跑脚手架，关键决策再上 opus；
  3. 看到面板「同一时段两行用量」先按 `requestTraces` 的 `agent.request` 数核对，再判断是否真·多开。
- **待核对项（本地日志无法证明）**：Cursor 服务端账单是否真按 2 条计费。本地 trace 只能证明客户端是 1 条 `agent.request`；若服务端仍按两行各计 1，需在网页版 usage / 账单明细侧核对。

---

## 8. 反例：6/13 15:09 的两行（与本案相反——这次确是 2 条独立 request）

用户随后又问「6/13 03:09 PM 两行（`326.7万` + `32.7万`，均 opus-4-8-thinking-xhigh）又是什么情况」。结论与 6/14 **相反**：这次是**两条真·独立 request**，来自**同时开着的两个不同会话（两个项目窗口）**。

证据（`logs/20260613T150510` 的两个窗口 trace）：

| 窗口 | requestId | 启动(本地) | composerId / 项目 | 内部规模 | 对应面板行 |
|---|---|---|---|---|---|
| window1_wb2 | `d96b5325` | 15:09:37 | `5d02cda5` / **jable**（`~/Documents/GitHub/jable`） | think=350 / tool=550 / **write=108**，跑到 ~17:01 | **326.7万** |
| window1_wb1 | `ab1d19c5` | 15:09:08 | `7c04f28f` / **Luma**（`~/config/Luma`） | think=0 / tool=0（短轮/早退） | **32.7万** |

- 两条 requestId 不同、composerId 不同、项目不同——是**两个并行会话各自的一条 request**，只是都在 15:09 这一分钟有请求，被面板列成同一时间戳的两行。
- 巧合点：两个会话粘贴的是**同一段** `[Jable] 403 …cf-mitigated=challenge` 内容（用户在 jable 与 Luma 两个窗口同时调同一个 Cloudflare 403 问题）。
- 所以这里**确实消耗了 2 条 request**，但原因是「你同时跑了 2 个会话」，不是计费故障，也不是一条被拆两半。

## 9. 通用识别口径（面板多行 ≠ 一定多开）

看到用量面板「同一/相近时间多行」，**不要直接按行数当 request 数**，按下面三步核对（全部以客户端 `cursor.requestTraces.log` 为准）：

1. **数 `agent.request` 的 `span_started`**：几个就是几条 request（权威）。
2. **看 requestId**：同一 requestId 的多行 = 同一条 request 的多次用量批次（**6/14 情形**，1 条）；不同 requestId = 不同 request。
3. **看 composerId / 窗口**：不同 composerId = 不同会话；**同时段多窗口并行**会天然产生多条并行 request（**6/13 情形**，2 条）。

| 维度 | 6/14（PagePick） | 6/13（jable + Luma） |
|---|---|---|
| `agent.request` span | 1 | 2 |
| requestId | 同 1 个 | 2 个不同 |
| composerId / 项目 | 同 1 个 | 2 个不同（并行窗口） |
| 两行成因 | 一条长 turn 的两次用量批次（含 13:26 流 error 批次） | 两个会话各一条 request |
| 是否「多开」 | **否** | **是，但属正常并行，非故障** |
