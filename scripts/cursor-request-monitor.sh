#!/bin/bash
# Cursor Request 监控器
# 实时监控 Cursor 的 renderer.log，检测同一会话内的多次 request。
# 当检测到同一 composerId 出现第 2+ 次 Acquired wakelock 时，发送 macOS 通知。
#
# 用法：
#   ./cursor-request-monitor.sh          # 监控最新的 Cursor 日志
#   ./cursor-request-monitor.sh --stats  # 显示当前统计

set -euo pipefail

LOG_DIR="$HOME/Library/Application Support/Cursor/logs"
STATE_FILE="/tmp/cursor-request-monitor-state.json"
PID_FILE="/tmp/cursor-request-monitor.pid"

# 颜色
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# 发送 macOS 通知
notify() {
  local title="$1"
  local message="$2"
  osascript -e "display notification \"$message\" with title \"$title\" sound name \"Basso\"" 2>/dev/null || true
}

# 找到最新的 renderer.log
find_latest_log() {
  local latest_dir
  latest_dir=$(ls -dt "$LOG_DIR"/*/ 2>/dev/null | head -1)
  if [ -z "$latest_dir" ]; then
    echo ""
    return
  fi

  # 查找所有 window*_wb*/renderer.log，取最新的
  local log_file
  log_file=$(find "$latest_dir" -name "renderer.log" -path "*/window*_wb*/*" 2>/dev/null | \
    while read -r f; do echo "$(stat -f '%m' "$f") $f"; done | \
    sort -rn | head -1 | cut -d' ' -f2-)

  echo "$log_file"
}

# 显示统计
show_stats() {
  if [ ! -f "$STATE_FILE" ]; then
    echo -e "${YELLOW}暂无监控数据${NC}"
    return
  fi
  echo -e "${CYAN}=== Cursor Request 统计 ===${NC}"
  jq -r 'to_entries[] | "\(.key): \(.value.count) 次 request（首次: \(.value.first_time)）"' "$STATE_FILE" 2>/dev/null
  echo ""
  local total
  total=$(jq '[.[].count] | add // 0' "$STATE_FILE" 2>/dev/null)
  local sessions
  sessions=$(jq 'length' "$STATE_FILE" 2>/dev/null)
  echo -e "${GREEN}总计: $sessions 个会话, $total 次 request${NC}"
}

# 处理日志行
process_line() {
  local line="$1"

  # 匹配 Acquired wakelock 事件
  if echo "$line" | grep -q "Acquired wakelock.*composerId="; then
    local composer_id
    composer_id=$(echo "$line" | sed -n 's/.*composerId=\([a-f0-9-]*\).*/\1/p')
    local timestamp
    timestamp=$(echo "$line" | sed -n 's/^\([0-9-]* [0-9:\.]*\).*/\1/p')
    local wakelock_id
    wakelock_id=$(echo "$line" | sed -n 's/.*id=\([0-9]*\).*/\1/p')

    if [ -z "$composer_id" ]; then
      return
    fi

    # 短 ID 用于显示
    local short_id="${composer_id:0:8}"

    # 读取或初始化状态
    if [ ! -f "$STATE_FILE" ]; then
      echo '{}' > "$STATE_FILE"
    fi

    # 更新计数
    local current_count
    current_count=$(jq -r --arg id "$composer_id" '.[$id].count // 0' "$STATE_FILE" 2>/dev/null)
    local new_count=$((current_count + 1))

    if [ "$new_count" -eq 1 ]; then
      # 首次 request
      jq --arg id "$composer_id" --arg ts "$timestamp" \
        '.[$id] = {"count": 1, "first_time": $ts, "last_time": $ts}' \
        "$STATE_FILE" > "${STATE_FILE}.tmp" && mv "${STATE_FILE}.tmp" "$STATE_FILE"
      echo -e "${GREEN}[Request #$new_count] composerId=$short_id wakelock=$wakelock_id ($timestamp)${NC}"
    else
      # 多次 request！
      jq --arg id "$composer_id" --arg ts "$timestamp" --argjson cnt "$new_count" \
        '.[$id].count = $cnt | .[$id].last_time = $ts' \
        "$STATE_FILE" > "${STATE_FILE}.tmp" && mv "${STATE_FILE}.tmp" "$STATE_FILE"
      echo -e "${RED}[⚠️ Request #$new_count] composerId=$short_id wakelock=$wakelock_id ($timestamp) — 同一会话第 $new_count 次 request！${NC}"
      notify "Cursor 多次 Request" "会话 $short_id 第 $new_count 次 request（$timestamp）"
    fi
  fi

  # 匹配 Released wakelock 事件（用于计算持续时间）
  if echo "$line" | grep -q "Released wakelock.*composerId="; then
    local composer_id
    composer_id=$(echo "$line" | sed -n 's/.*composerId=\([a-f0-9-]*\).*/\1/p')
    local timestamp
    timestamp=$(echo "$line" | sed -n 's/^\([0-9-]* [0-9:\.]*\).*/\1/p')
    local short_id="${composer_id:0:8}"
    echo -e "${CYAN}[Released] composerId=$short_id ($timestamp)${NC}"
  fi
}

# 主逻辑
main() {
  # 处理参数
  if [ "${1:-}" = "--stats" ]; then
    show_stats
    exit 0
  fi

  if [ "${1:-}" = "--stop" ]; then
    if [ -f "$PID_FILE" ]; then
      kill "$(cat "$PID_FILE")" 2>/dev/null || true
      rm -f "$PID_FILE"
      echo -e "${GREEN}监控已停止${NC}"
    else
      echo -e "${YELLOW}监控未在运行${NC}"
    fi
    exit 0
  fi

  # 检查是否已在运行
  if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo -e "${YELLOW}监控已在运行 (PID: $(cat "$PID_FILE"))${NC}"
    echo "使用 --stop 停止，或 --stats 查看统计"
    exit 1
  fi

  # 清空旧状态
  echo '{}' > "$STATE_FILE"

  # 找到日志文件
  local log_file
  log_file=$(find_latest_log)

  if [ -z "$log_file" ]; then
    echo -e "${RED}找不到 Cursor renderer.log${NC}"
    exit 1
  fi

  echo -e "${GREEN}=== Cursor Request 监控器 ===${NC}"
  echo -e "日志文件: ${CYAN}$log_file${NC}"
  echo -e "状态文件: ${CYAN}$STATE_FILE${NC}"
  echo -e "使用 Ctrl+C 停止，--stats 查看统计"
  echo ""

  # 记录 PID
  echo $$ > "$PID_FILE"

  # 清理函数
  cleanup() {
    rm -f "$PID_FILE"
    echo -e "\n${YELLOW}监控已停止${NC}"
    show_stats
  }
  trap cleanup EXIT INT TERM

  # 实时监控
  tail -F "$log_file" 2>/dev/null | while IFS= read -r line; do
    process_line "$line"
  done
}

main "$@"
