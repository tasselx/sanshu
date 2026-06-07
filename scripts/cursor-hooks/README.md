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
| `track-zhi.sh` | `postToolUse` | 【主数据源】追踪 sanshu 工具调用（归一化工具名），写状态文件供 stop 判定 |
| `truncate-mcp-output.sh` | `postToolUse` | MCP 工具输出超长时截断，减少对话历史膨胀 |
| `check-zhi-on-stop.sh` | `stop` | agent 结束前检查本轮是否需要 zhi 收尾，未收尾则注入 followup |

## 工作原理（v5：状态文件主判定 + transcript 正信号，移除 token 闸门）

`check-zhi-on-stop.sh` 在 `stop` 时执行，按以下优先级判断「本轮是否已用 zhi 收尾」：

1. **主判定 —— 状态文件**（`/tmp/sanshu-zhi-hook-state.json`，由 `track-zhi.sh` 在每次
   `postToolUse` 写入，**最可靠**）：
   - 新鲜（≤30 分钟）且 `tool=zhi` → 放行（本轮调过 zhi）；
   - 新鲜且 `tool!=zhi` → 注入 followup，强制补调 zhi；
   - 过期则视为上一轮残留，忽略，进入下一步。
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
- **v5（本版，移除 token 闸门）**：实测发现 followup 拦截本身 = 让 agent 续跑 = Cursor 记一条新
  request。对「全程没调 sanshu 的大额会话」用 token 阈值强拦并不能省 request，反而凭空多一条续跑
  request。故移除 token 闸门，只在「调了 sanshu 却没以 zhi 收尾」时拦截。代价：纯大额任务会话不再
  被拦（可能静默结束），这是「省 request」与「防静默断裂」之间的明确取舍。
