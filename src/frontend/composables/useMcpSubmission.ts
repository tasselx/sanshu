import { invoke } from '@tauri-apps/api/core'

/**
 * MCP 响应提交状态（进程级单例）。
 *
 * 中文说明（2026-09-14）：弹窗进程一次只服务一个 zhi 请求，但提交入口有多处
 * （发送 / 继续 / 增强 / 取消 / 关窗触发的取消）。之前各处直接调 send_mcp_response，
 * 「已提交」标志只在父级设置，导致正常提交关窗时标志尚未生效、仍会补发一次 CANCELLED。
 * 这里把提交收口为唯一入口，并区分三个阶段：
 *   - idle      尚未提交，可发送任何响应（含 CANCELLED）
 *   - sending   正在发送，忽略并发的再次提交
 *   - submitted 已成功送达后端，之后只允许退出、不再发送任何响应
 * 发送失败回到 idle，使随后的取消仍能正常送出 CANCELLED。
 */
export type McpSubmissionPhase = 'idle' | 'sending' | 'submitted'

let phase: McpSubmissionPhase = 'idle'

export function getMcpSubmissionPhase(): McpSubmissionPhase {
  return phase
}

/** 新请求到达时重置（同进程复用弹窗的场景）。 */
export function resetMcpSubmission() {
  phase = 'idle'
}

export interface SubmitMcpResponseOptions {
  /** 发送成功后、退出前执行（如记录历史）。失败只记日志，不影响退出。 */
  afterSend?: () => Promise<void> | void
}

/**
 * 唯一提交入口：发送响应并退出应用。
 *
 * 返回 true 表示本次调用真正完成了发送；false 表示发送中被忽略，或已提交、本次只重试了退出。
 * 发送阶段与退出阶段抛出的错误会原样上抛，调用方负责提示用户；已提交后再次调用会重试退出。
 */
export async function submitMcpResponse(
  response: unknown,
  options: SubmitMcpResponseOptions = {},
): Promise<boolean> {
  if (phase === 'submitted') {
    // 中文说明（2026-09-15）：响应已成功送达后端，但上一次 exit_app 失败（否则进程已不在）。
    // 再次点击时不重发响应，只重试退出，避免窗口卡住无法关闭。
    console.warn('响应已提交，重试退出应用')
    await invoke('exit_app')
    return false
  }
  if (phase === 'sending') {
    console.warn('提交进行中，忽略重复提交')
    return false
  }

  phase = 'sending'
  try {
    await invoke('send_mcp_response', { response })
    phase = 'submitted'
  }
  catch (error) {
    phase = 'idle'
    throw error
  }

  try {
    await options.afterSend?.()
  }
  catch (error) {
    console.warn('提交后处理失败（不影响退出）:', error)
  }

  await invoke('exit_app')
  return true
}

/**
 * 取消入口：未提交则送出 CANCELLED 并退出；已提交则只退出；发送中则不动作，
 * 由正在进行的提交负责退出。
 */
export async function cancelMcpResponse(): Promise<void> {
  if (phase === 'submitted') {
    await invoke('exit_app')
    return
  }
  if (phase === 'sending')
    return
  await submitMcpResponse('CANCELLED')
}
