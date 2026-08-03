export type WechatNotificationMode = 'always' | 'smart' | 'manual'

export interface WechatConfig {
  enabled: boolean
  notification_mode: WechatNotificationMode
}

export interface WechatStatus {
  enabled: boolean
  bound: boolean
  binding: boolean
  target_user: string | null
}

export interface WechatDiagnostics {
  base_url: string | null
  account_id: string | null
  bot_user_id: string | null
  context_ready: boolean
  cursor_ready: boolean
  state_file: string
  history_file: string
  history_count: number
}

export interface WechatHistoryEntry {
  id: string
  timestamp: string
  direction: 'outgoing' | 'incoming'
  kind: 'zhi' | 'reply' | 'system' | 'test'
  request_code: string | null
  content: string
  status: string
}
