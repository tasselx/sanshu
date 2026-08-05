export type WechatNotificationMode = 'always' | 'smart' | 'manual'
export type WechatNotificationImageTheme = 'auto' | 'paper' | 'midnight'

export interface WechatConfig {
  enabled: boolean
  notification_mode: WechatNotificationMode
  notification_image_theme: WechatNotificationImageTheme
  project_aliases: Record<string, string>
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

export type WechatPendingStatus = 'pending' | 'replied' | 'expired' | 'cancelled'

export interface WechatPendingRequest {
  request_id: string
  request_code: string
  project_root_path: string
  project_key: string
  project_alias: string
  agent_label: string
  prompt_preview: string
  created_at: string
  expires_at: string
  updated_at: string
  status: WechatPendingStatus
  completion_source: 'wechat' | 'desktop' | null
}
