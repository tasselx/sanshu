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

STATE_FILE="/tmp/sanshu-zhi-hook-state.json"
LOG_FILE="/tmp/sanshu-hook-debug.log"

input=$(cat)

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
  echo "tool_name=$tool_name norm=$norm_tool server=$server"
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
  echo "{\"tool\": \"$rec\", \"timestamp\": $(date +%s)}" > "$STATE_FILE"
fi

# 返回空 JSON，不修改工具输出
echo '{}'
exit 0
