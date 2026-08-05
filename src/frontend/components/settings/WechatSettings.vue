<script setup lang="ts">
import type { WechatConfig, WechatDiagnostics, WechatNotificationImageTheme, WechatNotificationMode, WechatStatus } from '../../types/wechat'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { renderWechatNotificationImages } from '../../utils/wechatNotificationImage'
import WechatHistoryPanel from './WechatHistoryPanel.vue'
import WechatLogPanel from './WechatLogPanel.vue'
import WechatPendingPanel from './WechatPendingPanel.vue'

const message = useMessage()
const config = ref<WechatConfig>({ enabled: false, notification_mode: 'always', notification_image_theme: 'auto', project_aliases: {} })
const status = ref<WechatStatus>({ enabled: false, bound: false, binding: false, target_user: null })
const diagnostics = ref<WechatDiagnostics>({
  base_url: null,
  account_id: null,
  bot_user_id: null,
  context_ready: false,
  cursor_ready: false,
  state_file: '',
  history_file: '',
  history_count: 0,
})
const showManager = ref(false)
const activeTab = ref('overview')
const showBinding = ref(false)
const qrUrl = ref('')
const bindingStatus = ref('等待开始绑定')
const verifyCodeRequired = ref(false)
const verifyCode = ref('')
const isBinding = ref(false)
const isTesting = ref(false)
const diagnosticsLoading = ref(false)
const pendingCount = ref(0)
const previewVisible = ref(false)
const previewLoading = ref(false)
const previewImages = ref<string[]>([])
const cleanupFunctions: Array<() => void> = []

const notificationModeLabel = computed(() => {
  const labels: Record<WechatNotificationMode, string> = {
    always: '每次立即通知',
    smart: '智能判断（15 秒）',
    manual: '仅手动通知',
  }
  return labels[config.value.notification_mode]
})

async function loadState() {
  try {
    const [loadedConfig, loadedStatus] = await Promise.all([
      invoke<WechatConfig>('get_wechat_config'),
      invoke<WechatStatus>('get_wechat_status'),
    ])
    config.value = {
      ...loadedConfig,
      notification_mode: loadedConfig.notification_mode || 'always',
      notification_image_theme: loadedConfig.notification_image_theme || 'auto',
      project_aliases: loadedConfig.project_aliases || {},
    }
    status.value = loadedStatus
  }
  catch (error) {
    console.error('加载微信通知状态失败:', error)
    message.error('加载微信通知状态失败')
  }
}

async function loadDiagnostics() {
  diagnosticsLoading.value = true
  try {
    diagnostics.value = await invoke<WechatDiagnostics>('get_wechat_diagnostics')
  }
  catch (error) {
    message.error(`加载微信诊断信息失败：${String(error)}`)
  }
  finally {
    diagnosticsLoading.value = false
  }
}

async function openManager() {
  showManager.value = true
  await Promise.all([loadState(), loadDiagnostics()])
}

function updateHistoryCount(count: number) {
  diagnostics.value.history_count = count
}

async function updateNotificationMode(value: string | number) {
  const mode = String(value) as WechatNotificationMode
  const previous = config.value.notification_mode
  config.value.notification_mode = mode
  try {
    await invoke('set_wechat_config', { wechatConfig: config.value })
    message.success('微信通知策略已更新')
  }
  catch (error) {
    config.value.notification_mode = previous
    message.error(`保存微信通知策略失败：${String(error)}`)
  }
}

async function updateNotificationImageTheme(value: string | number) {
  const theme = String(value) as WechatNotificationImageTheme
  const previous = config.value.notification_image_theme
  config.value.notification_image_theme = theme
  try {
    await invoke('set_wechat_config', { wechatConfig: config.value })
    message.success('通知图片主题已更新')
  }
  catch (error) {
    config.value.notification_image_theme = previous
    message.error(`保存通知图片主题失败：${String(error)}`)
  }
}

async function showNotificationPreview() {
  previewLoading.value = true
  previewVisible.value = true
  try {
    const pages = await renderWechatNotificationImages({
      id: 'preview123',
      message: '# 示例通知\n\n这是 Markdown、代码和流程图的图片预览。\n\n```ts\nconst answer = "zhi"\n```\n\n```mermaid\ngraph LR\n  A[请求] --> B[回复]\n```',
      predefined_options: ['按当前方案执行', '调整后再讨论'],
      is_markdown: true,
      project_alias: '当前项目',
      agent_label: '预览 AI',
      image_theme: config.value.notification_image_theme,
    })
    previewImages.value = pages.map(page => `data:image/png;base64,${page}`)
  }
  catch (error) {
    previewVisible.value = false
    message.error(`生成通知图片预览失败：${String(error)}`)
  }
  finally {
    previewLoading.value = false
  }
}

async function toggleEnabled(enabled: boolean) {
  config.value.enabled = enabled
  try {
    await invoke('set_wechat_config', { wechatConfig: config.value })
    status.value.enabled = enabled
    message.success(enabled ? '微信通知已启用' : '微信通知已关闭')
  }
  catch (error) {
    config.value.enabled = !enabled
    message.error(`保存微信通知配置失败：${String(error)}`)
  }
}

async function startBinding() {
  isBinding.value = true
  showBinding.value = true
  qrUrl.value = ''
  verifyCode.value = ''
  verifyCodeRequired.value = false
  bindingStatus.value = '正在获取登录二维码...'
  try {
    await invoke('start_wechat_binding')
  }
  catch (error) {
    isBinding.value = false
    bindingStatus.value = `启动绑定失败：${String(error)}`
  }
}

async function submitVerifyCode() {
  if (!verifyCode.value.trim())
    return
  try {
    await invoke('submit_wechat_verify_code', { code: verifyCode.value.trim() })
    verifyCodeRequired.value = false
    bindingStatus.value = '配对码已提交，正在确认...'
  }
  catch (error) {
    message.error(`提交配对码失败：${String(error)}`)
  }
}

async function testConnection() {
  isTesting.value = true
  try {
    const result = await invoke<string>('test_wechat_connection')
    message.success(result)
  }
  catch (error) {
    message.error(`微信通知测试失败：${String(error)}`)
  }
  finally {
    isTesting.value = false
  }
}

async function clearBinding() {
  try {
    await invoke('clear_wechat_binding')
    await loadState()
    await loadDiagnostics()
    message.success('微信绑定已清除')
  }
  catch (error) {
    message.error(`清除微信绑定失败：${String(error)}`)
  }
}

onMounted(async () => {
  cleanupFunctions.push(await listen<string>('wechat-qrcode', (event) => {
    qrUrl.value = event.payload
    bindingStatus.value = '请使用接收通知的微信扫码并确认'
  }))
  cleanupFunctions.push(await listen<string>('wechat-binding-status', (event) => {
    const labels: Record<string, string> = {
      requesting_qr: '正在连接微信服务并生成二维码...（最长等待 15 秒）',
      qr_ready: '请使用接收通知的微信扫码并确认',
      scaned: '二维码已扫描，请在微信中确认',
      confirmed: '登录已确认，正在等待目标微信发送绑定消息',
      waiting_message: '请在当前微信会话向 Bot 发送任意消息完成绑定',
    }
    bindingStatus.value = labels[event.payload] || `当前状态：${event.payload}`
  }))
  cleanupFunctions.push(await listen('wechat-verify-code-required', () => {
    verifyCodeRequired.value = true
    bindingStatus.value = '请输入微信中显示的配对码'
  }))
  cleanupFunctions.push(await listen('wechat-binding-complete', async () => {
    isBinding.value = false
    bindingStatus.value = '绑定完成'
    await loadState()
    await loadDiagnostics()
    message.success('微信通知绑定成功')
  }))
  cleanupFunctions.push(await listen<string>('wechat-binding-error', (event) => {
    isBinding.value = false
    bindingStatus.value = `绑定失败：${event.payload}`
  }))
  await loadState()
})

onUnmounted(() => cleanupFunctions.forEach(cleanup => cleanup()))

watch(showManager, (show) => {
  if (!show)
    activeTab.value = 'overview'
})
</script>

<template>
  <!-- 设置页只保留状态摘要，复杂配置统一进入管理弹窗。 -->
  <div class="flex flex-wrap items-center justify-between gap-4">
    <div class="flex min-w-0 items-center gap-3">
      <div class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-green-500/10">
        <div class="i-carbon-logo-wechat h-5 w-5 text-green-600 dark:text-green-300" />
      </div>
      <div class="min-w-0">
        <div class="flex flex-wrap items-center gap-2 text-sm font-medium">
          <span>微信通知</span>
          <n-tag :type="status.bound ? 'success' : 'default'" size="small" :bordered="false">
            {{ status.bound ? '已绑定' : '待绑定' }}
          </n-tag>
        </div>
        <div class="mt-1 truncate text-xs opacity-60">
          {{ config.enabled ? notificationModeLabel : '通知已关闭' }} · {{ status.bound ? status.target_user : '尚未选择接收会话' }}
        </div>
      </div>
    </div>
    <div class="flex items-center gap-3">
      <n-switch :value="config.enabled" size="small" @update:value="toggleEnabled" />
      <n-button size="small" secondary @click="openManager">
        <template #icon>
          <div class="i-carbon-settings-adjust w-4 h-4" />
        </template>
        管理微信通知
      </n-button>
    </div>
  </div>

  <n-modal
    v-model:show="showManager"
    preset="card"
    title="微信通知管理"
    style="width: 960px; max-width: calc(100vw - 32px);"
    content-style="padding-top: 8px;"
  >
    <template #header-extra>
      <n-tag :type="status.bound ? 'success' : 'default'" size="small" :bordered="false">
        {{ status.bound ? '服务已绑定' : '等待绑定' }}
      </n-tag>
    </template>

    <n-tabs v-model:value="activeTab" type="line" animated>
      <n-tab-pane name="overview" tab="概览与配置">
        <div class="max-h-[66vh] space-y-4 overflow-y-auto pr-1">
          <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5">
            <n-card size="small" :bordered="true">
              <div class="text-xs opacity-60">
                通知状态
              </div>
              <div class="mt-1 text-sm font-medium">
                {{ config.enabled ? '已启用' : '已关闭' }}
              </div>
            </n-card>
            <n-card size="small" :bordered="true">
              <div class="text-xs opacity-60">
                待处理请求
              </div>
              <div class="mt-1 text-sm font-medium">
                {{ pendingCount }} 条
              </div>
            </n-card>
            <n-card size="small" :bordered="true">
              <div class="text-xs opacity-60">
                绑定状态
              </div>
              <div class="mt-1 truncate text-sm font-medium">
                {{ status.bound ? '已绑定会话' : '等待绑定' }}
              </div>
            </n-card>
            <n-card size="small" :bordered="true">
              <div class="text-xs opacity-60">
                当前策略
              </div>
              <div class="mt-1 truncate text-sm font-medium">
                {{ notificationModeLabel }}
              </div>
            </n-card>
            <n-card size="small" :bordered="true">
              <div class="text-xs opacity-60">
                聊天记录
              </div>
              <div class="mt-1 text-sm font-medium">
                {{ diagnostics.history_count }} 条
              </div>
            </n-card>
          </div>

          <n-card size="small" title="通知配置">
            <div class="space-y-4">
              <div class="flex items-center justify-between gap-4">
                <div>
                  <div class="text-sm font-medium">
                    启用微信通知
                  </div>
                  <div class="mt-1 text-xs opacity-60">
                    发送 zhi PNG，并允许当前会话直接回复。
                  </div>
                </div>
                <n-switch :value="config.enabled" @update:value="toggleEnabled" />
              </div>
              <div class="border-t border-gray-200/70 pt-4 dark:border-gray-700/70">
                <div class="mb-2 text-sm font-medium">
                  通知策略
                </div>
                <n-radio-group
                  :value="config.notification_mode"
                  :disabled="!config.enabled"
                  size="small"
                  @update:value="updateNotificationMode"
                >
                  <n-radio-button value="always">
                    每次立即通知
                  </n-radio-button>
                  <n-radio-button value="smart">
                    智能判断
                  </n-radio-button>
                  <n-radio-button value="manual">
                    仅手动通知
                  </n-radio-button>
                </n-radio-group>
                <div class="mt-2 text-xs opacity-60">
                  智能判断等待 15 秒；检测到持续键盘或鼠标操作后，本次不发送。
                </div>
              </div>
              <div class="border-t border-gray-200/70 pt-4 dark:border-gray-700/70">
                <div class="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <div class="text-sm font-medium">
                      通知图片主题
                    </div>
                    <div class="mt-1 text-xs opacity-60">
                      Markdown、代码、公式和流程图会按所选主题生成连续图片。
                    </div>
                  </div>
                  <n-button size="small" secondary :loading="previewLoading" @click="showNotificationPreview">
                    <template #icon>
                      <div class="i-carbon-view w-4 h-4" />
                    </template>
                    预览图片
                  </n-button>
                </div>
                <n-radio-group
                  class="mt-3"
                  :value="config.notification_image_theme"
                  size="small"
                  @update:value="updateNotificationImageTheme"
                >
                  <n-radio-button value="auto">
                    跟随界面
                  </n-radio-button>
                  <n-radio-button value="paper">
                    纸张浅色
                  </n-radio-button>
                  <n-radio-button value="midnight">
                    午夜深色
                  </n-radio-button>
                </n-radio-group>
              </div>
            </div>
          </n-card>

          <WechatPendingPanel @count-change="pendingCount = $event" />

          <n-card size="small" title="绑定与连接">
            <div class="flex flex-wrap items-center justify-between gap-4">
              <div class="min-w-0">
                <div class="text-sm font-medium">
                  {{ status.bound ? `已绑定：${status.target_user}` : '尚未绑定接收通知的微信' }}
                </div>
                <div class="mt-1 text-xs opacity-60">
                  扫码后需在对应会话发送任意消息，建立主动通知上下文。
                </div>
              </div>
              <n-space>
                <n-button type="primary" size="small" :loading="isBinding || status.binding" :disabled="isBinding || status.binding" @click="startBinding">
                  {{ status.bound ? '重新绑定' : '开始绑定' }}
                </n-button>
                <n-button v-if="status.bound" size="small" :loading="isTesting" @click="testConnection">
                  测试通知
                </n-button>
                <n-popconfirm v-if="status.bound" @positive-click="clearBinding">
                  <template #trigger>
                    <n-button size="small" tertiary>
                      清除绑定
                    </n-button>
                  </template>
                  清除后需要重新扫码并发送绑定消息，聊天历史将保留。
                </n-popconfirm>
              </n-space>
            </div>
          </n-card>

          <n-card size="small" title="安全诊断信息">
            <n-spin :show="diagnosticsLoading">
              <div class="grid grid-cols-1 gap-x-6 gap-y-3 text-xs md:grid-cols-2">
                <div>
                  <span class="opacity-55">API Base URL</span><n-ellipsis class="mt-1 font-mono">
                    {{ diagnostics.base_url || '--' }}
                  </n-ellipsis>
                </div>
                <div>
                  <span class="opacity-55">Bot User ID</span><div class="mt-1 font-mono">
                    {{ diagnostics.bot_user_id || '--' }}
                  </div>
                </div>
                <div>
                  <span class="opacity-55">Account ID</span><div class="mt-1 font-mono">
                    {{ diagnostics.account_id || '--' }}
                  </div>
                </div>
                <div>
                  <span class="opacity-55">上下文 / 游标</span><div class="mt-1">
                    {{ diagnostics.context_ready ? '就绪' : '待建立' }} / {{ diagnostics.cursor_ready ? '就绪' : '待建立' }}
                  </div>
                </div>
                <div>
                  <span class="opacity-55">状态文件</span><n-ellipsis class="mt-1 font-mono">
                    {{ diagnostics.state_file || '--' }}
                  </n-ellipsis>
                </div>
                <div>
                  <span class="opacity-55">历史文件</span><n-ellipsis class="mt-1 font-mono">
                    {{ diagnostics.history_file || '--' }}
                  </n-ellipsis>
                </div>
              </div>
              <div class="mt-3 text-xs opacity-55">
                界面只展示掩码标识与就绪状态，不展示 token 和 context_token。
              </div>
            </n-spin>
          </n-card>
        </div>
      </n-tab-pane>

      <n-tab-pane name="history" tab="聊天记录">
        <WechatHistoryPanel @count-change="updateHistoryCount" />
      </n-tab-pane>

      <n-tab-pane name="logs" tab="诊断日志">
        <WechatLogPanel />
      </n-tab-pane>
    </n-tabs>
  </n-modal>

  <n-modal v-model:show="previewVisible" preset="card" title="通知图片预览" style="width: 620px; max-width: calc(100vw - 32px);">
    <n-spin :show="previewLoading">
      <div v-if="previewImages.length" class="space-y-3 overflow-y-auto" style="max-height: 70vh;">
        <img v-for="(image, index) in previewImages" :key="index" :src="image" :alt="`通知图片预览 ${index + 1}`" class="w-full rounded border border-gray-200/70 dark:border-gray-700/70">
      </div>
      <n-empty v-else description="暂无预览图片" />
    </n-spin>
  </n-modal>

  <n-modal v-model:show="showBinding" preset="card" title="微信通知绑定" style="width: 520px; max-width: calc(100vw - 32px);">
    <n-space vertical size="large">
      <n-alert type="info" :show-icon="false">
        {{ bindingStatus }}
      </n-alert>
      <div v-if="qrUrl" class="flex justify-center rounded-lg bg-white p-4">
        <img :src="qrUrl" alt="微信登录二维码" class="w-64 h-64 object-contain">
      </div>
      <n-button v-if="!isBinding && !qrUrl" type="primary" secondary @click="startBinding">
        重新获取二维码
      </n-button>
      <n-space v-if="verifyCodeRequired" vertical>
        <n-input v-model:value="verifyCode" placeholder="输入微信中显示的配对码" @keyup.enter="submitVerifyCode" />
        <n-button type="primary" :disabled="!verifyCode.trim()" @click="submitVerifyCode">
          提交配对码
        </n-button>
      </n-space>
      <div class="text-xs opacity-60 leading-relaxed">
        扫码确认后，请在对应会话向 Bot 发送任意消息。程序会保存该会话的关联信息，用于后续主动通知。
      </div>
    </n-space>
  </n-modal>
</template>
