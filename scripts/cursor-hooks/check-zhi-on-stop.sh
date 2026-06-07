#!/bin/bash
# stop 事件 hook：判断本轮 turn 是否已用 zhi 收尾确认，未收尾则注入 followup 强制 agent 继续。
#
# 判定优先级（v5：移除 token 闸门，避免凭空多产生 request）：
#   1) 状态文件（track-zhi.sh 产物，postToolUse 实测在 Glass/普通模式均触发，最可靠）：
#        - 新鲜 + tool=zhi   → 放行（本轮调过 zhi 收尾）
#        - 新鲜 + tool!=zhi  → 拦截（调了 sanshu 但没以 zhi 收尾）
#        - 过期             → 视为上一轮残留，忽略
#   2) transcript 正信号补充（仅「看到 zhi」可信）：
#        - 本轮 transcript 里出现 zhi → 放行
#   3) 兜底：放行（本轮没调任何 sanshu 工具，属纯任务/纯问答，不强制 zhi）
#
# 【为什么移除 token 闸门】
# followup 拦截 = 让 agent 续跑 = Cursor 记一条新 request。对「全程没调 sanshu 的大额会话」
# 用 token 阈值强行拦截并不能省 request，反而凭空多一条续跑 request。故按用户决策移除，
# 只在「调了 sanshu 却没以 zhi 收尾」这种真正违反强制交互的场景才拦截。
# 代价：全程没调 sanshu 的大额纯任务会话不再被拦（可能静默结束），这是用户的明确权衡。
#
# 【为什么状态文件为主、transcript 仅作正信号】
# Cursor 长会话会把 transcript 压缩成摘要，本轮 zhi 调用会消失，导致 transcript 误判（假阴性）。
# postToolUse 实测对所有工具（含 MCP:zhi / mcp__user-sanshu__zhi / Glass CallMcpTool）都触发，
# track-zhi.sh 归一化工具名后可可靠记录，故状态文件为主；transcript 只信「看到 zhi」正信号。
#
# 配合 hooks.json 中 stop 的 loop_limit=2，防止无限循环。

STATE_FILE="/tmp/sanshu-zhi-hook-state.json"
DEBUG_LOG="/tmp/sanshu-stop-debug.log"
# 状态文件超过该秒数视为上一轮残留，忽略。
STATE_MAX_AGE=1800

input=$(cat)

# 限制调试日志体积（仅保留最近 200 行）
if [ -f "$DEBUG_LOG" ]; then
  tail -200 "$DEBUG_LOG" > "${DEBUG_LOG}.tmp" 2>/dev/null && mv "${DEBUG_LOG}.tmp" "$DEBUG_LOG" 2>/dev/null
fi
log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" >> "$DEBUG_LOG" 2>/dev/null; }

# 放行：清除状态文件，确保下一轮从干净状态开始
allow() { rm -f "$STATE_FILE"; echo '{}'; exit 0; }

# 拦截：调过 sanshu 但没以 zhi 收尾
block_sanshu() {
  rm -f "$STATE_FILE"
  jq -n '{followup_message: "⚠️ 强制交互违规检测：本轮 turn 调用了 sanshu MCP 工具，但没有以 zhi 收尾确认就要结束。这违反「sanshu 强制交互硬约束」Rule 1/2。请立即调用 zhi 工具向用户确认当前任务状态，不要直接结束本轮对话。"}'
  exit 0
}

# 本轮 token 量（仅用于调试日志，不参与判定）
in_tok=$(echo "$input" | jq -r '.input_tokens // 0' 2>/dev/null)
out_tok=$(echo "$input" | jq -r '.output_tokens // 0' 2>/dev/null)
case "$in_tok" in ''|*[!0-9]*) in_tok=0 ;; esac
case "$out_tok" in ''|*[!0-9]*) out_tok=0 ;; esac
total_tok=$((in_tok + out_tok))

now=$(date +%s)

# ---------- 主判定：状态文件（track-zhi.sh 产物，最可靠）----------
if command -v jq >/dev/null 2>&1 && [ -f "$STATE_FILE" ]; then
  last_tool=$(jq -r '.tool // "unknown"' "$STATE_FILE" 2>/dev/null)
  last_ts=$(jq -r '.timestamp // 0' "$STATE_FILE" 2>/dev/null)
  case "$last_ts" in ''|*[!0-9]*) last_ts=0 ;; esac
  age=$((now - last_ts))
  if [ "$age" -le "$STATE_MAX_AGE" ]; then
    log "state tool=$last_tool age=$age total_tok=$total_tok"
    if [ "$last_tool" = "zhi" ]; then allow; else block_sanshu; fi
  else
    log "state stale age=$age ignore"
  fi
fi

# ---------- 补充：transcript 正信号（仅「看到 zhi」才放行）----------
tp=$(echo "$input" | jq -r '.transcript_path // empty' 2>/dev/null)
if command -v jq >/dev/null 2>&1 && [ -n "$tp" ] && [ -f "$tp" ]; then
  saw_zhi=$(jq -s -r '
    def isZhi($t):
      (($t.name == "CallMcpTool") and (((($t.input.toolName) // "")) == "zhi"))
      or ((($t.name) // "") | test("zhi"));
    def isUserText($r):
      ($r.role == "user")
      and ((((($r.message.content) // []) | map(.type)) | index("text")) != null);
    . as $rows
    | ([ range(0; ($rows | length)) | select(isUserText($rows[.])) ] | last) as $lu
    | ($rows[ (($lu // -1) + 1) : ]) as $turn
    | ([ $turn[]
         | select(.role == "assistant")
         | ((.message.content) // [])[]
         | select(.type == "tool_use")
         | select(isZhi(.)) ] | length)
  ' "$tp" 2>/dev/null)
  case "$saw_zhi" in ''|*[!0-9]*) saw_zhi=0 ;; esac
  log "transcript=$tp saw_zhi=$saw_zhi total_tok=$total_tok"
  if [ "$saw_zhi" -gt 0 ]; then allow; fi
fi

# ---------- 兜底：放行（本轮没调 sanshu，纯任务不强制 zhi）----------
log "fallback allow total_tok=$total_tok"
allow
