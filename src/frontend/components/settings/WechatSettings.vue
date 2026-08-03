<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useMessage } from 'naive-ui'
import { onMounted, onUnmounted, ref } from 'vue'

interface WechatConfig {
  enabled: boolean
}

interface WechatStatus {
  enabled: boolean
  bound: boolean
  binding: boolean
  target_user: string | null
}

const message = useMessage()
const config = ref<WechatConfig>({ enabled: false })
const status = ref<WechatStatus>({ enabled: false, bound: false, binding: false, target_user: null })
const showBinding = ref(false)
const qrUrl = ref('')
const bindingStatus = ref('等待开始绑定')
const verifyCodeRequired = ref(false)
const verifyCode = ref('')
const isBinding = ref(false)
const isTesting = ref(false)
const cleanupFunctions: Array<() => void> = []

async function loadState() {
  try {
    const [loadedConfig, loadedStatus] = await Promise.all([
      invoke<WechatConfig>('get_wechat_config'),
      invoke<WechatStatus>('get_wechat_status'),
    ])
    config.value = loadedConfig
    status.value = loadedStatus
  }
  catch (error) {
    console.error('加载微信通知状态失败:', error)
    message.error('加载微信通知状态失败')
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
    message.success('微信通知绑定成功')
  }))
  cleanupFunctions.push(await listen<string>('wechat-binding-error', (event) => {
    isBinding.value = false
    bindingStatus.value = `绑定失败：${event.payload}`
  }))
  await loadState()
})

onUnmounted(() => cleanupFunctions.forEach(cleanup => cleanup()))
</script>

<template>
  <n-space vertical size="large">
    <div class="flex items-center justify-between">
      <div class="flex items-center">
        <div class="w-1.5 h-1.5 bg-success rounded-full mr-3 flex-shrink-0" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            每次 zhi 请求发送微信通知
          </div>
          <div class="text-xs opacity-60">
            正文与选项生成 PNG，并支持通过标准模板回复
          </div>
        </div>
      </div>
      <n-switch :value="config.enabled" size="small" @update:value="toggleEnabled" />
    </div>

    <div class="pt-4 border-t border-gray-200 dark:border-gray-700">
      <div class="flex items-center justify-between gap-4">
        <div>
          <div class="text-sm font-medium mb-1">
            绑定状态
          </div>
          <div class="text-xs opacity-60">
            {{ status.bound ? `已绑定：${status.target_user}` : '尚未绑定接收通知的微信' }}
          </div>
        </div>
        <n-space>
          <n-button
            size="small"
            type="primary"
            :loading="isBinding || status.binding"
            :disabled="isBinding || status.binding"
            @click="startBinding"
          >
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
            清除后需要重新扫码并发送绑定消息。
          </n-popconfirm>
        </n-space>
      </div>
    </div>
  </n-space>

  <n-modal v-model:show="showBinding" preset="card" title="微信通知绑定" style="width: 520px; max-width: calc(100vw - 32px);">
    <n-space vertical size="large">
      <n-alert type="info" :show-icon="false">
        {{ bindingStatus }}
      </n-alert>
      <div v-if="qrUrl" class="flex justify-center rounded-lg bg-white p-4">
        <img :src="qrUrl" alt="微信登录二维码" class="w-64 h-64 object-contain">
      </div>
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
