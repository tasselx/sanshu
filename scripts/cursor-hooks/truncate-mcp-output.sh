#!/bin/bash
# MCP 工具输出截断 hook（postToolUse）
# 当 MCP 工具返回超过 500 行的输出时，截断并加上提示。
# 减少对话历史膨胀，为单次 request 腾出更多 token 空间。

MAX_LINES=500
LOG_FILE="/tmp/sanshu-truncate-debug.log"

input=$(cat)

# 尝试从输入 JSON 中提取工具输出
output=$(echo "$input" | jq -r '.output // empty' 2>/dev/null)

# 如果 .output 为空，尝试其他可能的字段名
if [ -z "$output" ]; then
  output=$(echo "$input" | jq -r '.result // .toolOutput // .mcp_tool_output // empty' 2>/dev/null)
fi

# 无输出或输出为空，不处理
if [ -z "$output" ] || [ "$output" = "null" ]; then
  echo '{}'
  exit 0
fi

# 计算行数
line_count=$(echo "$output" | wc -l | tr -d ' ')

# 同时检查字符数（超过 50000 字符也截断，即使行数不多）
char_count=${#output}

# 调试日志
{
  echo "--- $(date '+%Y-%m-%d %H:%M:%S') ---"
  echo "lines=$line_count chars=$char_count"
  tool_type=$(echo "$input" | jq -r '.toolType // .tool // "unknown"' 2>/dev/null)
  echo "tool=$tool_type"
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
