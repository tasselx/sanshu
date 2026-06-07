# 变更报告 - sanshu 层降低 request 拆分与 token 消耗（2026-06-07）

## 一、起因

用户在 Cursor 计费页看到「一个会话产生两条记录」（08:26 5.06M / 08:49 8.13M token），且反映计费规律很迷：
**有时 3000 多万 token 不新开 request，有时几百就新开。**

## 二、排查结论（机制）

1. **「新开 request」不由 token 数量决定**，而是由 request 生命周期事件触发：
   - Cursor 给每条 request 一个**预算（迭代/token 上限，服务端不透明）**。长回合只要不超预算、心跳不断，几千万 token 可全塞进**同一条 request**。
   - 另起一轮（主输入框发消息 / 回合结束后继续 / 预算耗尽被动切换 / 弹窗竞态）**必然新开 request**，哪怕只有几百 token。
   - 所以看起来随机，本质是**回合边界在变**，token 只是被归到当时活跃的那条 request 上。

2. **心跳已生效**：日志 `client_progress_token=客户端下发` 证明 Cursor 认 progress 心跳，600s 窗口能撑住；全量 399 次 zhi 调用里只有 **16 次重连**——早期「重连风暴烧 token」已基本修掉。

3. **当前真正的 token 大头**：单回合太长 + zhi/工具往返太多 ×「每次都重发整段上下文」。
   - 当天 **272 次「新建弹窗」**；问题会话 `357ad096`（项目理解）一条用户消息内 **136 次工具调用 + 58 思考步**，跨 08:26→09:25（约 1 小时）。
   - 09:04→09:24 出现 zhi **每 1–2 分钟一次的密集调用**＝保活/汇报式空转烧 context。

## 三、本次变更（规则 + 改码，已全部落地）

> 用户约定：**只改代码，不编译、不运行**，编译运行由用户自行完成。

### 1. 规则：新增「zhi 调用节流」（最高杠杆）

- 文件：`.cursor/rules/sanshu-强制交互.mdc`（工作区）+ `~/.cursor/rules/sanshu-强制交互.mdc`（用户级），两处同步。
- 新增「一·补充」三条：
  - 第 6 条：zhi 只用于四类场景（多方案抉择/计划变更/任务收尾/不确定破坏性操作），**禁止当作"每做完一小步就汇报"的进度心跳**。
  - 第 7 条：同一轮多个待确认点**合并成一次 zhi**（brief 列多项或用 choices），不连环弹窗。
  - 第 8 条：明确区分「Pending 保活续等」（对**同一个未响应弹窗**续等，保留、开销低）与「反复新建弹窗」（要避免）。
- **保留**第 3 条 Pending 保活机制不变（防提前结束的核心，且现在很省）。

### 2. 代码：`POPUP_POLL_WINDOW` 600s → 900s

- 文件：`src/rust/mcp/handlers/popup.rs`
- 心跳已确认有效，拉长单次阻塞窗口，进一步减少 Pending→重连（当天 39 次 Pending，每次重连都重发整段上下文）。

### 3. 代码：`MAX_POPUP_RECONNECTS` 10 → 5

- 文件：`src/rust/mcp/handlers/popup.rs`
- 更早封顶「用户长时间离开」的 token 消耗：5 × 900s = **75 分钟**后自动 Suspended（弹窗不关、用户回来仍可操作）。
- 代价：离开容忍时间从 100 分钟降到 75 分钟。

### 4. 代码：brief 过长 **告警日志**（不截断）

- 文件：`src/rust/mcp/tools/interaction/mcp.rs`
- 新增 `BRIEF_LEN_WARN_THRESHOLD = 4000`，brief 超长时打 `warn` 提示「精简 brief / 合并 zhi」。
- **为何只告警不截断**：截断会破坏弹窗展示内容（UX），且无法减少模型侧已生成的 token；真正省 token 靠"少调/合并 zhi"。

### 5. 代码：新增「zhi 调用节流监控」日志

- 文件：`src/rust/mcp/tools/interaction/mcp.rs`
- 新增全局 `ZHI_CALL_CADENCE`（workspace → 累计次数 + 上次时刻），每次 zhi 调用打 info 日志：
  `[zhi] 节流监控: workspace=..., 第 N 次 zhi 调用, 距上次=Xs`。
- 目的：**直接在 sanshu 日志里观测"频繁新建弹窗/保活空转"**，无需再去翻 Cursor 的 state.vscdb。
  改了节流规则后，对照"间隔是否拉长、调用是否变疏"即可验证规则生效。

### 不需改动

- `src/rust/mcp/server.rs` 启动水印**动态读取常量**（`POPUP_POLL_WINDOW.as_secs()` / `MAX_POPUP_RECONNECTS`），改常量后自动反映，无需修改。

## 日志诊断能力评估

**现有日志足以做本次诊断**（都来自 `~/Library/Application Support/sanshu/log/sanshu-mcp.log`）：
- `[zhi] 心跳已启用 ... client_progress_token=客户端下发`：证明 Cursor 认心跳、600/900s 窗口有效；
- `[popup] 重连弹窗 #N` / `返回 Pending` / `Suspended` / `弹窗完成 ... 重连次数=`：量化重连与等待；
- `[popup] 新建弹窗` / `已回收遗弃弹窗（对话很可能已被新开 request 打断）`：弹窗生命周期。

**原有盲区**（已用本次新增日志补上其一）：
- 单看 sanshu 日志看不出 **zhi 调用密度/保活空转**（本次诊断的 272 次新建弹窗是去查 Cursor DB 才发现的）→ 已由 `节流监控` 日志补上；
- sanshu 的 `request_id` 与 Cursor 的 conversation/request **无法直接关联**（这是 Cursor 服务端信息，sanshu 拿不到）→ 仍需在排查"被拆 request"时交叉 `state.vscdb` / `ai-tracking.db`。

## 四、改动文件清单

| 文件 | 改动 |
| --- | --- |
| `.cursor/rules/sanshu-强制交互.mdc` | 新增「一·补充」zhi 节流三条 |
| `~/.cursor/rules/sanshu-强制交互.mdc` | 同上（用户级副本同步） |
| `src/rust/mcp/handlers/popup.rs` | `POPUP_POLL_WINDOW` 900s、`MAX_POPUP_RECONNECTS` 5，注释同步 |
| `src/rust/mcp/tools/interaction/mcp.rs` | brief 超长告警 + zhi 调用节流监控日志 + 注释里 600s→900s |

## 五、预期效果

- 规则第 6/7/8 条直接削减「频繁新建弹窗」——这是当前 token 大头。
- 900s 窗口 + 减少重连，进一步降低 Pending→重连导致的整段上下文重发。
- 5 次封顶在用户长时间离开时更快止损。

## 六、sanshu 管不到（Cursor 服务端）

- 单条 request 的预算阈值、何时被动切 request；
- 主输入框发消息必然新开 request；
- 每次模型调用 Cursor 打包多少上下文。
- → 计费"迷糊感"的根在 Cursor 的 per-request 预算（不透明），sanshu 只能降低单回合烧预算的速度。

## 七、后续

- 本次只改源码，**需用户自行 `cargo build --release` 并重装/重启 MCP** 才生效；可对照启动水印日志 `POPUP_POLL_WINDOW=900s MAX_RECONNECTS=5` 确认跑的是新二进制。
