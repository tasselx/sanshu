# 分析报告：Request 提前结束未调用 zhi

> 日期：2026-06-07  
> 涉及会话：composerId `a94930c3-301e-4336-afac-0e1a90367ccd`  
> 工作区：`/Users/tassel/Downloads/Hopper`（非 git 仓库，二进制逆向分析任务）

## 现象

| 请求 | 开始时间 | 结束时间 | 持续时间 | Token 总量 | 调用 ji | 调用 zhi | 结果 |
|------|----------|----------|----------|-----------|---------|---------|------|
| 第1次 | 12:06:29 | 12:07:58 | 89秒 | 32.8万 | ✅ | ❌ | 直接结束 |
| 第2次 | 12:08:40 | 12:09:15 | 35秒 | 31.5万 | ✅ | ❌ | 直接结束 |
| 第3次 | 12:09:44 | 12:37:22 | 27分钟 | - | ✅ | ✅(多次) | 正常完成 |

用户观察：前两次请求消耗大量 token 但没有调用 sanshu MCP 的 `zhi` 工具就结束了。

## 根因

### Output Token Budget 耗尽

**不是** MCP 连接失败，**不是** 权限被阻止。

### 证据

#### 1. MCP 连接正常

```
12:06:09.869 [MCPService] createClient completed for server: user-sanshu, connected=true, statusType=connected
```

#### 2. ji 调用成功（两次请求都调用了）

```
12:06:39 [agentTranslation] mcpToolCall bubble: serverName="sanshu", toolName="ji"
12:06:39 [permissions-service] shouldBlockMcp: ALLOWED (auto-run, no team block)
12:09:08 [agentTranslation] mcpToolCall bubble: serverName="sanshu", toolName="ji"
12:09:09 [permissions-service] shouldBlockMcp: ALLOWED (auto-run, no team block)
```

#### 3. 第二次请求从 ji 调用到结束仅 7 秒

```
12:09:08 ji 调用
12:09:15 generation-ended（wakelock released, heldForMs=35240）
```

模型调用 `ji` 后几乎立刻被截断，没有足够 token 输出后续内容。

#### 4. 第三次请求正常运行 27 分钟

```
12:09:57 zhi 首次调用
12:10:24 → 12:37:12 多次 ji/zhi/radare2 调用
12:37:22 generation-ended（heldForMs=1658147）
```

## 技术分析：Token 预算如何被耗尽

### Input Context 组成（估算）

| 组件 | 估算 Token |
|------|-----------|
| 15+ MCP server tool descriptors | 5-8万 |
| Workspace rules (强制交互 + karpathy) | 1-2万 |
| User rules (AURA-X-KYS + MCP Routing) | 2-3万 |
| Cursor system prompt + skills 列表 | 3-5万 |
| 对话历史（第2次请求包含第1次的输出） | 5-10万 |
| Workspace context files | 1-2万 |
| **Input 合计** | **~20-30万** |

### Output Budget 分配

- claude-4.6-opus-high-thinking 输出上限约 32K tokens
- high-thinking 模式下 thinking tokens 占用 50-80%
- 实际可用于 text + tool calls 的 tokens：约 6-16K
- 第一次请求：thinking 耗完预算，模型只输出了 ji 调用 + 部分文本
- 第二次请求：上下文更大（包含第一次输出），预算更紧

### 为什么第三次成功

第三次是 Cursor 自动续发的 continuation turn：
- 模型已知前两轮 ji 加载了记忆，无需重复
- 直接进入任务执行（radare2 分析）
- 每次 tool call 结果回来后模型继续生成（multi-turn agent loop），不是一次性全部输出

## 根因修正（二次分析）

初步分析认为"MCP descriptor 全量注入"是主因，经过进一步验证后修正：

**Cursor Glass 模式下 MCP tool schema 不会全量注入 system prompt**。实际注入的是：
1. 通用 `CallMcpTool` 函数定义
2. `<mcp_file_system_servers>` 服务器列表 + serverUseInstructions（~3-5K tokens）
3. 模型通过文件系统按需读取具体 schema

**真正的 token 消耗分布：**
- 对话历史（恢复的已有对话，含大量 MCP 工具返回结果如反编译输出）：~180K tokens
- System prompt（rules + MCP server list + skills）：~15-20K tokens
- Output thinking tokens（high-thinking 模式）：~120-125K tokens
- 实际有效输出（文本 + 工具调用）：~3-8K tokens

**32.8万 ≈ 200K input（满载）+ 128K output（thinking 占满）= Claude Opus 4.6 的硬上限**

## 规则冲突分析

### 冲突 1：节流 vs 禁止结束（死锁）

| 规则 | 要求 | 后果 |
|------|------|------|
| Rule 2 | 严禁主动结束 turn | 模型不能停 |
| Rule 6 | 只在四类场景调 zhi，禁止当进度心跳 | 分析阶段不能调 zhi |
| **结合** | 模型继续生成文本 → output 耗尽 → 被动截断 | ❌ 违反 Rule 2 |

### 冲突 2：AUTONOMOUS 模式 vs 强制交互

- AURA-X-KYS：Level 1-2 + 置信度 > 90% 可进入 AUTONOMOUS 模式，"做完再用 zhi 汇报"
- sanshu-強制交互：严禁结束 turn
- **结果**：模型选择"先做完再调 zhi"，但 output budget 不够撑到"做完"

### 冲突 3：规则文件重复注入

`sanshu-強制交互.mdc` 在 system prompt 的 `always_applied_workspace_rules` 中被注入了**两次**，浪费 ~1500 tokens。（疑为 Cursor 内部 bug）

## 已执行的修复

### 修改 `sanshu-強制交互.mdc`

新增"一·补充 B：预算感知"章节，增加 3 条规则：

- **Rule 9**（早期 zhi 保险）：长对话/首次响应时，在 `ji` 之后、正式工作之前必须先调一次 zhi
- **Rule 10**（AUTONOMOUS 不免除）：长上下文中即使是 AUTONOMOUS 模式也必须执行早期 zhi
- **Rule 11**（判断标准）：定义"长上下文"的具体条件（3+ 轮工具调用 / 含大段代码输出 / input 可能超 100K）

优先级：预算感知（Rule 9-11）> 节流（Rule 6）

## 其他缓解建议

### 1. MCP 工具返回结果膨胀对话历史

虽然 MCP schema 不全量注入，但工具**返回结果**（如 radare2 的 decompile_function 输出数千行代码）
会作为对话历史保留在 context 中。建议：
- 长对话中定期"新建对话"（Fresh start），避免历史累积
- MCP 工具侧做输出截断（如 decompile 结果限制在 200 行以内）

### 2. 谨慎使用 high-thinking 模式

high-thinking 的 thinking tokens 可占用 output budget 的 80%+。在长上下文场景下：
- thinking 消耗了几乎所有输出空间
- 留给实际 text/tool calls 的 tokens 极少
- 建议在已有长历史的对话中切换为 standard thinking

### 3. 解决规则重复注入

检查 Cursor workspace 设置，确认 `.cursor/rules/sanshu-強制交互.mdc` 只被注册一次。
可能原因：`alwaysApply: true` + 被多个配置入口加载。

## 日志文件位置

- Cursor 主日志：`~/Library/Application Support/Cursor/logs/20260607T120601/main.log`
- MCP 权限日志：`~/Library/Application Support/Cursor/logs/20260607T120601/window1_wb0/workbench.mcp.allowlist.log`
- sanshu MCP 连接日志：`~/Library/Application Support/Cursor/logs/20260607T120601/window1_wb0/mcp-server-user-sanshu.workbench.log`
- 渲染进程日志：`~/Library/Application Support/Cursor/logs/20260607T120601/window1_wb2/renderer.log`
