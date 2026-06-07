# Cursor Request 发起机制逆向分析

> 日期：2026-06-07
> 工作区：`/Users/tassel/Documents/GitHub/sanshu`
> 方法：静态逆向 `workbench.desktop.main.js` + `cursor-agent-exec/dist/main.js`，并比对 Cursor 官方 hooks 文档（cursor.com/docs/hooks）
> 目标：查清「一条 request（计费单位）如何产生、何时结束、何时新开」，从而回答「如何在一个 request 里做更多事、避免无谓新开」

---

## 一、结论速览（TL;DR）

| 问题 | 结论 |
|------|------|
| 一条 request 是什么 | 一个 **generation** = 一条 `StreamUnifiedChatWithTools`（**BiDiStreaming 双向流**） |
| request 何时开始 | 一条新 user message 进入 agent loop → 分配新 `generationUUID` → 建流 → `Acquired wakelock` |
| request 何时结束 | agent loop 结束 → controller `dispose()` → `Released wakelock reason="generation-ended"` → 流关 |
| turn 内多次工具往返 | 都在**同一条流**里，token 累积（含 `zhi` 等待用户） |
| 何时新开 request | ① 主聊天框发新消息 ② `stop` hook 的 `followup_message`（作为下一条 user message） |
| 能否「不新开地续跑」 | hooks **做不到**；唯一途径是 agent 在同一 turn 内持续交互、**不让 loop 结束** |
| 改 Cursor 能否实现 | 理论可 patch，但所有路径都归结到「不让 loop 结束」，与 `zhi` 保活等价，且引入升级失效 + 流超时 + ToS 风险，**无净收益** |

---

## 二、请求通道：StreamUnifiedChatWithTools

AI 请求走 gRPC/Connect 的 `aiserver.v1.ChatService`，定义位于打包模块 `out-build/proto/aiserver/v1/chat_connectweb.js`：

```js
typeName: "aiserver.v1.ChatService",
methods: {
  streamUnifiedChat:                     { kind: ServerStreaming },
  streamUnifiedChatWithTools:            { kind: BiDiStreaming },   // ← agent 主通道
  streamUnifiedChatWithToolsSSE:         { kind: ServerStreaming },
  streamUnifiedChatWithToolsPoll:        { kind: ServerStreaming },
  streamUnifiedChatWithToolsIdempotent:  { kind: BiDiStreaming },
  streamUnifiedChatWithToolsIdempotentSSE:  { kind: ServerStreaming },
  streamUnifiedChatWithToolsIdempotentPoll: { kind: ServerStreaming },
  ...
}
```

要点：

- **`streamUnifiedChatWithTools` 是 BiDiStreaming（双向流）**。一条流内可双向多次收发——模型生成、工具调用、结果回传、模型继续，全在同一条流里完成。
- SSE / Poll 变体是 ServerStreaming，用于不支持双向流的网络环境（HTTP/2 受限时降级）。
- Idempotent 变体支持断线重连续传（幂等）。

**含义**：一个 turn（含多次工具往返）= 一条 `streamUnifiedChatWithTools` 流 = 一条计费 request。这解释了为何单条 request 的 token 能累积到数百万甚至 3700 万——只要流不关，token 一直累积。

---

## 三、Request 生命周期：ComposerWakelockManager

`workbench.desktop.main.js` 中的 `ComposerWakelockManager` 负责在一条 generation 活跃期间持有 wakelock（阻止系统后台节流），并在结束时释放。

### Acquire（开始）

```js
// [ComposerWakelockManager] Acquired wakelock id=${e} reason="${n}" composerId=${composerId}
this._wakelockId = e;
this._backgroundThrottlingDisableId = t;
e !== void 0 && this._logService.info(`[ComposerWakelockManager] Acquired wakelock id=${e} reason="${n}" composerId=${this._composerHandle.composerId}`)
```

### Release（结束）

```js
// [ComposerWakelockManager] Released wakelock id=${e} reason=...
await this._powerMainService.stopWakelock(e);
this._logService.info(`[ComposerWakelockManager] Released wakelock id=${e} reason=...`)
```

### 与 agent loop 绑定（reactive controller）

在 composer stream controller 中发现：

```js
// agent loop 活跃 / 恢复时获取
... && this._acquire("agent-loop-resumed")), { defer: !0 }

// controller 释放（generation 真正结束）时
dispose() {
  this._disposed || (
    this._disposed = !0,
    this._disposeReactive?.(),
    this._disposeReactive = void 0,
    this._release("generation-ended")   // ← 关键：generation 结束
  )
}
```

**结论**：`request 活跃区间 = agent loop 生命周期`。

- agent loop 活跃 / 恢复 → `_acquire("agent-loop-resumed")`
- agent loop 结束 → controller `dispose()` → `_release("generation-ended")` → 流关闭 → request 结束 → 触发 `stop` hook

这与日志中观察到的模式一致：

```
Acquired wakelock composerId=xxx          → 新 request 开始
Released wakelock reason="generation-ended" → request 结束
```

---

## 四、generation_id / generationUUID

- 在 `workbench.desktop.main.js` 中 `generationUUID` 出现 60 处、`generation_id` 16 处，绝大多数是**读取 / 透传**（如 `generation_id: e.generationId ?? ""`），分配点在更上游（提交流时生成）。
- 在 `cursor-agent-exec/dist/main.js`（agent 执行器，8.9MB）中：
  - `buildRequestedModel()` 仅**构造**要请求的模型描述对象（`modelId / maxMode / parameters`），不负责计费或分配 generation。
  - `generation_id` 同样是读取传入的 `e.generationId`。

- 官方文档对 `generation_id` 的定义：**「changes with every user message」（随每条 user message 变化）**。

**含义**：每条 user message 对应一个新 `generationUUID` → 一条新流 → 一条新 request。

---

## 五、新 Request 的两个触发源

### 1. 用户在主聊天框发新消息

上一条流在上一个 turn 结束时已 `dispose()`（`generation-ended`），新消息 → 新 `generationUUID` → 新建 `streamUnifiedChatWithTools` 流 → **新 request**。

### 2. stop hook 的 followup_message

- 校验器位于 `packages/hooks/src/validators/stopResponse.ts`：

```js
// followup_message must be a string if provided
n.followup_message !== void 0 && typeof n.followup_message != "string"
  && t.push("followup_message must be a string if provided")
```

- 还存在 `agent.v1.SubagentStopRequestResponse` 的 `followup_message` 字段（subagent 版本：`subagentStopResponse.ts`）。
- 官方文档：`stop` hook 的 `followup_message` 会「submit it as the **next user message**」。

**含义**：`followup_message` = 系统替用户发的「下一条 user message」→ 必然新 `generationUUID` → 新流 → **新 request**。这就是为何每次 hook 拦截续跑，后台都新增一条 request。

---

## 六、为什么「zhi 弹窗延续」不新开、而「主框发消息」新开

| 用户如何回复 | 底层行为 | request 走向 |
|---|---|---|
| 在 `zhi`（fallback MCP）弹窗里回复 | 这是 agent 在 loop 内调用的一个 MCP 工具，调用期间流**一直开着**等待返回；用户回复 = 流内工具结果返回 | **同一条 request 延续**，token 累积 |
| 在主聊天框发新消息 | 上一个 turn 早已结束、流已 `dispose()` | **新开一条 request** |

关键：`zhi` 是工具调用，工具调用期间 agent loop **不 dispose**，流不关。用户在弹窗里回复只是让这个工具返回结果，loop 继续——始终同一条 generation。

---

## 七、Cursor hooks 能力边界（官方文档查证）

来源：`https://cursor.com/docs/hooks`

1. **事件总数**：18 个 Agent hooks + 2 个 Tab hooks + 1 个 `workspaceOpen`。
2. **没有比 `stop` 更早的「将要结束」事件**（不存在 `beforeStop / willStop / preStop`）。
3. **`stop` 触发时机**：agent loop **结束之后**。输出字段只有 `followup_message`（无 `continue / block / decision` 这类 Claude 风格字段）。
4. **followup 语义**：以「下一条 user message」提交，结合 `generation_id`「随每条 user message 变化」→ 每次 followup = 新 generation / 新 request。
5. **自动续跑受限**：`stop` / `subagentStop` 的 followup 续跑受 `loop_limit`（默认 5）约束。
6. **payload 字段**：公共字段含 `transcript_path`；`stop` 专属字段为 `status`、`loop_count`；**不含** `input_tokens / output_tokens`（仅 `preCompact` 有 `context_tokens` 等上下文计量字段）。
7. **hooks 无法在同一条 request 内续跑**——所有自动续跑都换新 generation。

---

## 八、假设要 Patch：纯理论定位与风险

> 仅理论分析，**不实施**。

request 边界 = `streamUnifiedChatWithTools` 流的开关 = agent loop 生命周期。要「单条无限大不新开」，本质是「不让 loop 结束 / 流不关」。

| 候选 patch 点 | 思路 | 风险 / 可行性 |
|---|---|---|
| ① agent loop 完成判定 / controller `dispose` 处 | 将 dispose 时注入「自动续一轮」=内置一个自动 `zhi` | 最接近根因；但核心混淆代码、升级即失效；**且 server 端 BiDi 流有空闲超时**，纯挂起会被 server 关 → 仍需周期性活动 |
| ② `generationUUID` 分配处 | 让连续消息复用同一 UUID | 基本无效：计费按流而非 UUID，复用未必合并，还可能状态错乱 |
| ③ `stopResponse` followup 注入处 | 把 followup 改成「流内续传」 | 前置依赖 ①（流不关才有流可续），否则流已 dispose |

**核心结论**：所有路径最终都归结到「不让 agent loop 结束 / 流不关」，这与 `zhi` 保活**完全等价**。`zhi` 保活用 agent 行为达成同样效果，零 patch、零升级风险、不踩 server 流超时（因为有真实交互活动）。**patch 没有净收益**。

---

## 九、操作铁律（如何在一个 request 里做更多事）

> 目标**不是**「一条会话永久一条 request」，而是「让每个 request 尽量在 `zhi` 内延续、多做事；彻底结束后下次发消息再正常新开」。

1. 一次主框消息 = 一个 request 的起点（正常，接受新开）。
2. 该 request 内 agent 用 `zhi` 持续交互，把更多的事在这一条里做完，**不中途无谓结束 turn**。
3. 彻底做完 / 用户说结束 → turn 真正结束 → request 结束。
4. 下次主框发消息 → 新 request → 再在 `zhi` 内延续。循环往复。

---

## 十、证据附录

### 文件与体积

| 文件 | 体积 | 角色 |
|------|------|------|
| `…/app/out/vs/workbench/workbench.desktop.main.js` | 61,079,640 bytes / 59,444 行 | renderer 主进程：generation 生命周期、wakelock、proto 定义、hooks 校验 |
| `…/app/extensions/cursor-agent-exec/dist/main.js` | 8,967,996 bytes / 8 行 | agent 执行器：跑 loop、构造 `buildRequestedModel`、读取 generationId |

### 关键符号命中（workbench.desktop.main.js）

| 符号 | 命中数 | 说明 |
|------|-------|------|
| `streamUnifiedChatWithTools` | 2 | 请求通道（proto 定义 + 响应映射） |
| `generationUUID` | 60 | generation_id 透传/读取 |
| `generation_id` | 16 | 同上 |
| `Acquired wakelock` / `Released wakelock` / `generation-ended` | 各 1 | request 生命周期边界 |
| `followup_message` | 10 | stop / subagentStop 响应字段与校验器 |
| `buildRequestedModel` | 1 | 构造模型请求对象 |

### 官方文档来源

- `https://cursor.com/docs/hooks`（Agent hooks 事件、stop payload、followup 语义、loop_limit）

---

## 十一、与现有产出的关系

- 本文聚焦 **request 机制本身 + 代码证据**（面向「为什么 / 原理」）。
- `docs/总结-20260607-Cursor-Request优化与Hook部署.md` 聚焦 **部署 + 策略 + 操作铁律**（面向「怎么用」）。
- `docs/分析报告-20260607-hook失效导致Request断裂-transcript修复.md` 聚焦 **hook 脚本演进（v3→v5）的排查过程**。
