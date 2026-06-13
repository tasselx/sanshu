# 总结-20260613：多计 request 根因排查与 hook 失效修复

## 一、问题

用量页显示同一会话产生两条 request（claude-fable-5-thinking-high）：

| 时间 | token | 说明 |
|---|---|---|
| Jun 13, 01:44 | 966.3 万 | 会话 `2708224c`（GitKraken 工作区，Glass 模式）首条用户消息「继续完善目录脚本」 |
| Jun 13, 02:24 | 162.7 万 | 同一会话内凭空多出的一条 request |

## 二、排查结论

### 2.1 排除项（带证据）

- **zhi 弹窗循环不会多计 request**：01:45–02:21 单个 turn 内连续 12 次 zhi 调用全部 `is_error=false`，都在 01:44 那条 request 里。代价是 token（每次弹窗返回后模型重读全量上下文），不是 request 数。
- **不是手动停止 / 断流**：Cursor `main.log` 显示 01:44:10 启动的 agent-loop wakelock 全程未释放；stop hook（`check-zhi-on-stop.sh`）的调试日志停在 01:43:39，该会话从未产生 stop 事件 → turn 从未结束、也未被掐断。

### 2.2 根因

1. **02:18:37** 用户向 zhi 弹窗粘贴 **355,153 字符**超长文本（sanshu 已打 WARN「用户回复超长…token 消耗巨大」），作为工具返回原样进入上下文；
2. 单次模型调用的上下文被顶破窗口上限；
3. 02:21:28 第 12 次 zhi 返回后出现 **155 秒静默期**（无任何 MCP 调用）——Cursor 在后台做历史压缩（summarization/compaction）；
4. 压缩后的续跑在计费上被切成新的一条 request（02:24），02:24:03 紧接着发起第 13 次 zhi。

> 关键区分：request 多计与 token 总量无关（此前单条 3-4 千万 token 也没事，因为单次上下文没超限），决定因素是**单次进入上下文的体量**。

### 2.3 连带发现的两个实锤 bug

| Bug | 现象 | 证据 |
|---|---|---|
| truncate hook 双重失效 | `hooks.json` matcher `"MCP: "`（带空格）永远匹配不上真实工具名 `MCP:zhi`（无空格）/ `CallMcpTool`（Glass）→ hook 部署以来一次都没运行过 | `/tmp/sanshu-truncate-debug.log` 从不存在 |
| payload 字段名错误 | 两个 postToolUse 脚本提取输出用 `.output/.result/.toolOutput/...`，而真实字段是 `tool_output` → track-zhi 的 v7「回复语义」一直提取失败，状态文件 reply 恒为空，stop 端退化为 v6 | track-zhi 调试日志中的 raw_input；状态文件 `reply: ""` |

## 三、改动清单

### 3.1 Hook 层（已直接应用到 `~/.cursor/`，并同步到 `scripts/cursor-hooks/`）

1. **`hooks.json`**：去掉 truncate hook 的 `matcher: "MCP: "`，改由脚本内部判断。
2. **`truncate-mcp-output.sh`**：
   - 新增工具名归一化判断，覆盖 `MCP:*` / `CallMcpTool` / `mcp__*` 三种形态，非 MCP 工具立即退出；
   - 输出提取补 `.tool_output` 优先。
   - 已用真实字段名模拟验证：60K 输出成功截断、Shell 跳过、短 MCP 输出放行。
3. **`track-zhi.sh`**：输出提取补 `.tool_output` 优先，v7「断流自动续跑/保活续命」语义自此真正生效。已验证 reply 提取成功（`确认完成 | 收到`）。
4. `check-zhi-on-stop.sh` 审计无问题，未改动；preToolUse 的 `rtk` 已安装，正常。

### 3.2 Rust 层（已写入工作区，**未编译**，待用户自行编译重启 MCP 生效）

1. **`src/rust/mcp/handlers/response.rs`**：新增 `spill_long_user_input` —— 用户输入超过 `RESPONSE_LEN_WARN_THRESHOLD`（50K）时自动落盘到 `~/.sanshu/overflow_replies/reply_时间戳.txt`，只回传 2000 字符预览 + 文件路径；落盘失败退回原样回传保证不丢内容。覆盖结构化 `user_input` 与纯文本回退两条路径。
2. **`src/rust/mcp/tools/interaction/mcp.rs`**：巨型回复提示文案同步（注明超长部分已落盘）。

### 3.3 清理

- 移除开机自启动诊断脚本 LaunchAgent `~/Library/LaunchAgents/com.sanshu.cursor-request-monitor.plist`（删前备份至 `/tmp/com.sanshu.cursor-request-monitor.plist.bak`）；仓库内 `scripts/cursor-request-monitor.sh` 脚本保留。

## 四、问题是否解决

- **根因已查清**：是「单次超大输入 → 上下文超限 → 压缩切新 request」，不是 hook followup、不是手动停止、不是 zhi 循环。
- **防复发措施已就位（hook 层立即生效；Rust 层待编译）**：
  - 第一道防线：popup 超长用户输入自动落盘（Rust，编译后生效）；
  - 第二道防线：truncate hook 50K 截断（修复后立即生效）；
  - 行为约定：大文本不粘弹窗，存文件发路径让 agent 用 Read 分段读。
- **附带收益**：track-zhi v7 修复后，「断流自动续跑/保活续命」机制从今天起才真正工作。

## 五、追记：v7.1 修复（03:05 实锤的二次问题）

3.1 的 `.tool_output` 修复上线后第一次任务收尾就触发了 **v7 误报续跑**：用户已在弹窗选了「确认完成，结束任务」，但 stop hook 注入 followup 强制续跑（凭空多一条 request）。原因：

- `tool_output` 实际是 MCP 结果**包装 JSON**（`{"content":[{"type":"text","text":"..."}]}`），`grep '^{'` 取到的是包装层（无 `selected_options`/`user_input`）；
- jq 对空串 `split("\n")` 得空数组、取 `[0]` 输出字面 `null`，摘要变成「 | null」→ stop 端按「新指令未处理完」误拦。

修复（两个脚本均已应用并同步仓库）：

1. `track-zhi.sh`：提取 `tool_output` 后先用 jq 解包 `content[].text` 再做保活匹配与首行 JSON 提取；`sel`/`ui` 均无有效内容时保持摘要为空（stop 端退化为 v6 放行，不再拿「 | null」拦截）。
2. `truncate-mcp-output.sh`：同样先解包，对**模型可见文本**做行数/字符判断与截断，避免对包装层 JSON 动刀产生残缺 JSON。

验证：包装格式下回复摘要正确提取（`确认完成，结束任务 | 请记住xxx`）、保活信号识别正常（`keepalive: true` 且摘要为空）、60K 包装内文本成功截断。

## 六、备份与回滚

| 内容 | 原始版本 | 修改后快照 |
|---|---|---|
| hook 脚本 / hooks.json | 仓库 git HEAD 的 `scripts/cursor-hooks/`（`git diff` 可对照） | `~/.cursor/hooks/*.bak-20260613`、`~/.cursor/hooks.json.bak-20260613` |
| Rust 改动 | git 工作区（`git checkout --` 可回滚） | — |
| LaunchAgent plist | `/tmp/com.sanshu.cursor-request-monitor.plist.bak` | — |
