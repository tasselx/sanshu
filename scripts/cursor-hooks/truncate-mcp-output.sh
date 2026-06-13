#!/bin/bash
# MCP 工具输出截断 hook（postToolUse）
# 当 MCP 工具返回超过 500 行的输出时，截断并加上提示。
# 减少对话历史膨胀，为单次 request 腾出更多 token 空间。
#
# 【2026-06-13 修复】原 hooks.json 用 matcher "MCP: " 过滤，但 Glass 模式下工具名是
# CallMcpTool、Claude 兼容层是 mcp__server__tool，全都匹配不上 → 本 hook 从未生效过
# （实证：02:18 一段 355K 字符弹窗粘贴原样进入上下文，触发压缩多计一条 request）。
# 改为不设 matcher，由脚本内部归一化工具名后判断是否 MCP 工具（与 track-zhi.sh 同法）。

MAX_LINES=500
LOG_FILE="/tmp/sanshu-truncate-debug.log"

input=$(cat)

# 识别是否为 MCP 工具调用（兼容三种形态：MCP:xxx / CallMcpTool / mcp__server__tool）
tool_name=$(echo "$input" | jq -r '
  .tool_name //
  .toolName //
  .input.toolName //
  .mcp_tool_name //
  "unknown"
' 2>/dev/null)
server=$(echo "$input" | jq -r '.input.server // .server // .mcp_server // empty' 2>/dev/null)

is_mcp=false
case "$tool_name" in
  MCP:*|CallMcpTool|mcp__*) is_mcp=true ;;
esac
[ -n "$server" ] && is_mcp=true

if [ "$is_mcp" = "false" ]; then
  echo '{}'
  exit 0
fi

# 尝试从输入 JSON 中提取工具输出
# 【2026-06-13 修复】实测 Cursor postToolUse payload 的字段名是 tool_output（此前缺失，
# 导致即使 matcher 命中也取不到输出、从不截断），放在首位；其余字段保留兼容。
output=$(echo "$input" | jq -r '.tool_output // .output // empty' 2>/dev/null)

# 如果仍为空，尝试其他可能的字段名
if [ -z "$output" ]; then
  output=$(echo "$input" | jq -r '.result // .toolOutput // .mcp_tool_output // empty' 2>/dev/null)
fi

# 无输出或输出为空，不处理
if [ -z "$output" ] || [ "$output" = "null" ]; then
  echo '{}'
  exit 0
fi

# 【v1.1 修复（2026-06-13）】tool_output 是 MCP 结果包装 JSON
# {"content":[{"type":"text","text":"..."}]}，应对模型可见的文本内容做行数/字符判断与截断，
# 而不是对包装层 JSON 动刀（截断包装层会产生残缺 JSON）。解包失败则按原始输出处理。
inner=$(echo "$output" | jq -r '[.content[]? | select(.type == "text") | .text] | join("\n")' 2>/dev/null)
if [ -n "$inner" ] && [ "$inner" != "null" ]; then
  output="$inner"
fi

# 计算行数
line_count=$(echo "$output" | wc -l | tr -d ' ')

# 同时检查字符数（超过 50000 字符也截断，即使行数不多）
char_count=${#output}

# 调试日志
{
  echo "--- $(date '+%Y-%m-%d %H:%M:%S') ---"
  echo "lines=$line_count chars=$char_count"
  echo "tool=$tool_name server=$server"
} >> "$LOG_FILE" 2>/dev/null

# 限制日志大小
if [ -f "$LOG_FILE" ]; then
  tail -200 "$LOG_FILE" > "${LOG_FILE}.tmp" 2>/dev/null && mv "${LOG_FILE}.tmp" "$LOG_FILE" 2>/dev/null
fi

# 判断是否需要截断
need_truncate=false
if [ "$line_count" -gt "$MAX_LINES" ]; then
  need_truncate=true
fi
if [ "$char_count" -gt 50000 ]; then
  need_truncate=true
fi

if [ "$need_truncate" = "false" ]; then
  echo '{}'
  exit 0
fi

# 按行数截断
truncated=$(echo "$output" | head -"$MAX_LINES")

# 计算截断后的字符数
truncated_chars=${#truncated}

# 构建截断提示
notice="\n\n... [⚠️ 输出已截断：原文共 ${line_count} 行 / ${char_count} 字符，仅保留前 ${MAX_LINES} 行（${truncated_chars} 字符）。如需查看更多内容，请使用更精确的参数重新调用该工具。]"

# 拼接截断后的内容
final="${truncated}${notice}"

# 返回 updated_mcp_tool_output
echo "$final" | jq -Rs '{"updated_mcp_tool_output": .}'
exit 0
