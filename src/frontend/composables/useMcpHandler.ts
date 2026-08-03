import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ref } from 'vue'
import { renderWechatNotificationImages } from '../utils/wechatNotificationImage'

export type WechatNotificationPhase
  = | 'idle'
    | 'countdown'
    | 'manual'
    | 'sending'
    | 'sent'
    | 'activity_cancelled'
    | 'cancelled'
    | 'error'

export interface WechatNotificationState {
  phase: WechatNotificationPhase
  secondsRemaining: number
}

interface WechatConfig {
  enabled: boolean
  notification_mode?: 'always' | 'smart' | 'manual'
}

interface WechatNotificationPayload {
  requestId: string
  predefinedOptions: string[]
  imagePages: string[]
}

const SMART_NOTIFICATION_DELAY_SECONDS = 15
const REQUIRED_ACTIVITY_SAMPLES = 2

/**
 * MCP处理组合式函数
 */
export function useMcpHandler() {
  const mcpRequest = ref(null)
  const showMcpPopup = ref(false)
  const wechatNotificationState = ref<WechatNotificationState>({
    phase: 'idle',
    secondsRemaining: 0,
  })
  let wechatNotificationTimer: ReturnType<typeof setInterval> | null = null
  let currentWechatPayload: WechatNotificationPayload | null = null
  let notificationGeneration = 0

  // 图标搜索模式状态
  const isIconMode = ref(false)
  const iconParams = ref<{
    query: string
    style: string
    savePath: string
    projectRoot: string
  } | null>(null)

  function stopWechatNotificationTimer() {
    if (wechatNotificationTimer) {
      clearInterval(wechatNotificationTimer)
      wechatNotificationTimer = null
    }
  }

  function resetWechatNotification() {
    notificationGeneration += 1
    stopWechatNotificationTimer()
    currentWechatPayload = null
    wechatNotificationState.value = { phase: 'idle', secondsRemaining: 0 }
  }

  async function deliverWechatNotification(payload: WechatNotificationPayload, generation: number) {
    stopWechatNotificationTimer()
    wechatNotificationState.value = { phase: 'sending', secondsRemaining: 0 }
    try {
      await invoke('start_wechat_sync', payload)
      if (generation === notificationGeneration)
        wechatNotificationState.value = { phase: 'sent', secondsRemaining: 0 }
      console.log('✅ 微信同步启动成功')
    }
    catch (error) {
      if (generation === notificationGeneration)
        wechatNotificationState.value = { phase: 'error', secondsRemaining: 0 }
      console.error('启动微信同步失败:', error)
    }
  }

  function cancelWechatNotification(reason: 'manual' | 'activity' = 'manual') {
    if (!['countdown', 'manual'].includes(wechatNotificationState.value.phase))
      return
    notificationGeneration += 1
    stopWechatNotificationTimer()
    currentWechatPayload = null
    wechatNotificationState.value = {
      phase: reason === 'activity' ? 'activity_cancelled' : 'cancelled',
      secondsRemaining: 0,
    }
  }

  async function sendWechatNotificationNow() {
    if (!currentWechatPayload || !['countdown', 'manual'].includes(wechatNotificationState.value.phase))
      return
    const payload = currentWechatPayload
    const generation = notificationGeneration
    currentWechatPayload = null
    await deliverWechatNotification(payload, generation)
  }

  async function scheduleWechatNotification(request: any, config: WechatConfig) {
    resetWechatNotification()
    if (!config.enabled || !request?.message)
      return

    const generation = notificationGeneration
    currentWechatPayload = {
      requestId: request.id || '',
      predefinedOptions: request.predefined_options || [],
      imagePages: renderWechatNotificationImages(request),
    }

    const mode = config.notification_mode || 'always'
    if (mode === 'always') {
      const payload = currentWechatPayload
      currentWechatPayload = null
      if (payload)
        await deliverWechatNotification(payload, generation)
      return
    }
    if (mode === 'manual') {
      wechatNotificationState.value = { phase: 'manual', secondsRemaining: 0 }
      return
    }

    let lastInputTick: number
    try {
      lastInputTick = await invoke<number>('get_system_last_input_tick')
    }
    catch (error) {
      console.error('读取系统输入状态失败，将按倒计时发送微信通知:', error)
      lastInputTick = -1
    }

    const startedAt = Date.now()
    let activitySamples = 0
    let polling = false
    wechatNotificationState.value = {
      phase: 'countdown',
      secondsRemaining: SMART_NOTIFICATION_DELAY_SECONDS,
    }

    wechatNotificationTimer = setInterval(async () => {
      if (polling || generation !== notificationGeneration)
        return
      polling = true
      try {
        const elapsedSeconds = (Date.now() - startedAt) / 1000
        const secondsRemaining = Math.max(0, Math.ceil(SMART_NOTIFICATION_DELAY_SECONDS - elapsedSeconds))
        if (secondsRemaining === 0) {
          const payload = currentWechatPayload
          currentWechatPayload = null
          if (payload)
            await deliverWechatNotification(payload, generation)
          return
        }
        wechatNotificationState.value = { phase: 'countdown', secondsRemaining }

        const inputTick = await invoke<number>('get_system_last_input_tick')
        if (lastInputTick >= 0 && inputTick !== lastInputTick) {
          activitySamples += 1
          lastInputTick = inputTick
          if (activitySamples >= REQUIRED_ACTIVITY_SAMPLES)
            cancelWechatNotification('activity')
        }
      }
      catch (error) {
        console.error('轮询系统输入状态失败:', error)
      }
      finally {
        polling = false
      }
    }, 500)
  }

  /**
   * 统一的MCP响应处理
   */
  async function handleMcpResponse(response: any) {
    try {
      resetWechatNotification()
      // 通过Tauri命令发送响应并退出应用
      await invoke('send_mcp_response', { response })
      await invoke('exit_app')
    }
    catch (error) {
      console.error('MCP响应处理失败:', error)
    }
  }

  /**
   * 统一的MCP取消处理
   */
  async function handleMcpCancel() {
    try {
      resetWechatNotification()
      // 发送取消信息并退出应用
      await invoke('send_mcp_response', { response: 'CANCELLED' })
      await invoke('exit_app')
    }
    catch (error) {
      // 静默处理MCP取消错误
      console.error('MCP取消处理失败:', error)
    }
  }

  /**
   * 显示MCP弹窗
   */
  async function showMcpDialog(request: any) {
    // 获取通知配置；微信双向回复依赖当前弹窗进程监听，因此启用微信时保留前端弹窗。
    let shouldShowFrontendPopup = true
    let wechatConfig: WechatConfig = { enabled: false, notification_mode: 'always' }
    try {
      const [telegramConfig, rawWechatConfig] = await Promise.all([
        invoke('get_telegram_config'),
        invoke('get_wechat_config'),
      ])
      wechatConfig = {
        enabled: !!(rawWechatConfig as any)?.enabled,
        notification_mode: (rawWechatConfig as any)?.notification_mode || 'always',
      }
      // 如果Telegram启用且配置了隐藏前端弹窗，则不显示前端弹窗
      if (
        telegramConfig
        && (telegramConfig as any).enabled
        && (telegramConfig as any).hide_frontend_popup
        && !wechatConfig.enabled
      ) {
        shouldShowFrontendPopup = false
        console.log('🔕 根据Telegram配置，隐藏前端弹窗')
      }
    }
    catch (error) {
      console.error('获取通知配置失败:', error)
      // 配置获取失败时，保持默认行为（显示弹窗）
    }

    // 根据配置决定是否显示前端弹窗
    if (shouldShowFrontendPopup) {
      // 设置请求数据和显示状态
      mcpRequest.value = request
      showMcpPopup.value = true
    }
    else {
      console.log('🔕 跳过前端弹窗显示，仅使用Telegram交互')
    }

    // 播放音频通知（无论是否显示弹窗都播放）
    try {
      await invoke('play_notification_sound')
    }
    catch (error) {
      console.error('播放音频通知失败:', error)
    }

    // 启动Telegram同步（无论是否显示弹窗都启动）
    try {
      if (request?.message) {
        await invoke('start_telegram_sync', {
          message: request.message,
          predefinedOptions: request.predefined_options || [],
          isMarkdown: request.is_markdown || false,
        })
        console.log('✅ Telegram同步启动成功')
      }
    }
    catch (error) {
      console.error('启动Telegram同步失败:', error)
    }

    // 根据通知策略立即发送、智能等待或保留手动发送入口。
    await scheduleWechatNotification(request, wechatConfig)
  }

  /**
   * 检查MCP模式
   */
  async function checkMcpMode() {
    try {
      const args = await invoke('get_cli_args') as Record<string, any>

      // 检查是否为图标搜索模式
      if (args?.icon_mode) {
        console.log('📦 检测到图标搜索模式')
        return {
          isMcp: false,
          mcpContent: null,
          isIconMode: true,
          iconParams: {
            query: args.icon_query || '',
            style: args.icon_style || 'all',
            savePath: args.icon_save_path || 'assets/icons',
            projectRoot: args.icon_project_root || '',
          },
        }
      }

      // 检查是否为 MCP 请求模式
      if (args?.mcp_request) {
        // 读取MCP请求文件
        const content = await invoke('read_mcp_request', { filePath: args.mcp_request })

        if (content) {
          await showMcpDialog(content)
        }
        return { isMcp: true, mcpContent: content, isIconMode: false, iconParams: null }
      }

      // 检查是否为 CLI 交互模式
      if (args?.cli_request) {
        const content = args.cli_request
        if (content) {
          await showMcpDialog(content)
        }
        return { isMcp: true, mcpContent: content, isIconMode: false, iconParams: null }
      }
    }
    catch (error) {
      console.error('检查MCP模式失败:', error)
    }
    return { isMcp: false, mcpContent: null, isIconMode: false, iconParams: null }
  }

  /**
   * 设置MCP事件监听器
   */
  async function setupMcpEventListener() {
    try {
      await listen('mcp-request', (event) => {
        showMcpDialog(event.payload)
      })
    }
    catch (error) {
      console.error('设置MCP事件监听器失败:', error)
    }
  }
  /**
   * 设置图标模式状态
   */
  function setIconMode(mode: boolean, params: typeof iconParams.value = null) {
    isIconMode.value = mode
    iconParams.value = params
  }

  return {
    mcpRequest,
    showMcpPopup,
    wechatNotificationState,
    isIconMode,
    iconParams,
    handleMcpResponse,
    handleMcpCancel,
    showMcpDialog,
    checkMcpMode,
    setupMcpEventListener,
    setIconMode,
    cancelWechatNotification,
    sendWechatNotificationNow,
    resetWechatNotification,
  }
}
