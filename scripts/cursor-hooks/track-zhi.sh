#!/bin/bash
# postToolUse hook：追踪 sanshu MCP 工具调用，写入状态文件，供 check-zhi-on-stop.sh 判定本轮是否以 zhi 收尾。
#
# 【实测结论】postToolUse 对所有工具都触发（含 Glass 的 CallMcpTool 与普通模式的 MCP:zhi），
# 因此本 hook 不设 matcher（对所有 postToolUse 触发），由脚本内部归一化工具名后判断是否为 sanshu 工具。
#
# 工具名在不同模式下形态不同，必须归一化：
#   - 普通 agent 模式：  MCP:zhi
#   - Claude 兼容层：    mcp__user-sanshu__zhi
#   - 内置工具：         Shell / Read / Write / Glob / Grep ...
# 归一化：去掉 "MCP:" 前缀，再取 "__" 分隔的最后一段，得到裸工具名（zhi / ji / ...）。
#
# 【v6 会话隔离】状态文件按 conversation_id 拆分，根除跨窗口/跨会话污染。
# 背景（2026-06-11 实锤）：旧版单一全局状态文件，window:3 的会话调 ji 后，
# window:1 的另一会话 stop 时误读该状态被拦截，凭空多产生一条 11.5 万 token 的 request。
# postToolUse payload 实测含 conversation_id（即 composerId，与 transcript 文件名同源），
# 取不到时退化为 global（与 stop 端相同的退化规则，保证两端键一致）。
#
# 【v7 回复语义】zhi 工具额外从 payload 的 .output 提取「用户回复摘要 + 保活标记」写入状态。
# 背景（2026-06-11 23:44 实锤）：turn 在 zhi 返回用户新指令后因网络 TLS 断连而中止，
# stop 端只知道「调过 zhi」便放行——用户被迫手动打「继续」（新 request）。
# v7 让 stop 端能区分「完成类回复（可结束）」与「新指令/保活中（应自动续跑）」。
# 提取失败时摘要为空，stop 端退化为 v6 行为（只看 tool=zhi 即放行），保证向后安全。

STATE_FILE_PREFIX="/tmp/sanshu-zhi-hook-state"
LOG_FILE="/tmp/sanshu-hook-debug.log"

input=$(cat)

# 会话隔离键：conversation_id 仅含 UUID 字符，过滤后拼接文件名防注入
conv=$(echo "$input" | jq -r '.conversation_id // empty' 2>/dev/null | tr -cd 'A-Za-z0-9-')
STATE_FILE="${STATE_FILE_PREFIX}-${conv:-global}.json"

# 机会式清理：删除 2 小时以上的残留状态文件，避免 /tmp 内按会话累积
find /tmp -maxdepth 1 -name 'sanshu-zhi-hook-state-*.json' -mmin +120 -delete 2>/dev/null

# 广泛尝试多种字段提取工具名（兼容 Cursor 普通模式 / Claude 兼容层的不同 payload）
tool_name=$(echo "$input" | jq -r '
  .tool_name //
  .toolName //
  .input.toolName //
  .input.arguments.toolName //
  .mcp_tool_name //
  "unknown"
' 2>/dev/null)

# 提取 server（若有），用于识别 sanshu 工具
server=$(echo "$input" | jq -r '
  .input.server //
  .server //
  .mcp_server //
  empty
' 2>/dev/null)

# 归一化工具名：MCP:zhi -> zhi ; mcp__user-sanshu__zhi -> zhi ; zhi -> zhi
norm_tool="$tool_name"
norm_tool="${norm_tool#MCP:}"     # 去掉 "MCP:" 前缀
norm_tool="${norm_tool##*__}"     # 取最后一个 "__" 之后（去掉 mcp__server__ 前缀）

# 调试日志（仅保留最近 100 行）
{
  echo "--- $(date '+%Y-%m-%d %H:%M:%S') ---"
  echo "tool_name=$tool_name norm=$norm_tool server=$server conv=${conv:-global}"
  echo "raw_input=$(echo "$input" | head -c 300)"
} >> "$LOG_FILE" 2>/dev/null
if [ -f "$LOG_FILE" ]; then
  tail -100 "$LOG_FILE" > "${LOG_FILE}.tmp" 2>/dev/null && mv "${LOG_FILE}.tmp" "$LOG_FILE" 2>/dev/null
fi

# 判断是否 sanshu 工具
is_sanshu=false
[ "$server" = "user-sanshu" ] && is_sanshu=true
case "$norm_tool" in
  zhi|ji|sou|uiux|enhance|tavily|context7|deepwiki|tu) is_sanshu=true ;;
esac
case "$tool_name" in *user-sanshu*|*sanshu*) is_sanshu=true ;; esac

# 仅当判定为 sanshu 工具时才更新状态（避免被其它非 sanshu 工具覆盖）
if [ "$is_sanshu" = "true" ]; then
  rec="$norm_tool"
  case "$norm_tool" in *zhi*) rec="zhi" ;; esac

  # 【v7】zhi 工具：提取用户回复摘要与保活标记，供 stop 端判断「能否结束本轮」
  reply_snippet=""
  keepalive=false
  if [ "$rec" = "zhi" ]; then
    # 字段名兼容顺序与 truncate-mcp-output.sh 保持一致
    output=$(echo "$input" | jq -r '.output // .result // .toolOutput // .mcp_tool_output // empty' 2>/dev/null)
    if [ -n "$output" ] && [ "$output" != "null" ]; then
      # 保活信号：弹窗仍开着、用户尚未回复（zhi 返回的固定话术）
      case "$output" in
        *用户仍在思考中*|*请再次调用*|*弹窗仍开着*|*等待已达上限*|*暂未给出回应*) keepalive=true ;;
      esac
      # zhi 的 Done 返回首行是 JSON：{"selected_options":[...],"user_input":"..."}
      # 只取 selected_options + user_input 第一行作摘要——用户回复后段常拼有
      # 「请记住…」偏好长文，含"完成确认"等字样，全文匹配会造成假阳性放行。
      first_json=$(echo "$output" | grep -m1 '^{' 2>/dev/null)
      if [ -n "$first_json" ]; then
        sel=$(echo "$first_json" | jq -r '(.selected_options // []) | join(" ")' 2>/dev/null)
        ui=$(echo "$first_json" | jq -r '(.user_input // "") | split("\n")[0]' 2>/dev/null)
        reply_snippet=$(printf '%s | %s' "$sel" "$ui" | head -c 300)
      fi
    fi
  fi

  jq -n \
    --arg tool "$rec" \
    --argjson ts "$(date +%s)" \
    --arg reply "$reply_snippet" \
    --argjson ka "$keepalive" \
    '{tool: $tool, timestamp: $ts, reply: $reply, keepalive: $ka}' > "$STATE_FILE"
fi

# 返回空 JSON，不修改工具输出
echo '{}'
exit 0
