<script setup lang="ts">
import type { WechatPendingRequest, WechatPendingStatus } from '../../types/wechat'
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref } from 'vue'

const emit = defineEmits<{
  countChange: [count: number]
}>()

const message = useMessage()
const requests = ref<WechatPendingRequest[]>([])
const loading = ref(false)
const aliasDrafts = ref<Record<string, string>>({})
let refreshTimer: ReturnType<typeof setInterval> | null = null

const statusLabels: Record<WechatPendingStatus, string> = {
  pending: '等待回复',
  replied: '已回复',
  expired: '已过期',
  cancelled: '已取消',
}

const statusTypes: Record<WechatPendingStatus, 'success' | 'warning' | 'error' | 'default'> = {
  pending: 'warning',
  replied: 'success',
  expired: 'error',
  cancelled: 'default',
}

const pendingRequests = computed(() => requests.value.filter(request => request.status === 'pending'))

function formatTime(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}

function remainingText(request: WechatPendingRequest) {
  if (request.status !== 'pending')
    return `完成于 ${formatTime(request.updated_at)}`
  const seconds = Math.max(0, Math.ceil((new Date(request.expires_at).getTime() - Date.now()) / 1000))
  return seconds > 0 ? `剩余 ${seconds} 秒` : '正在过期'
}

function replyTemplate(request: WechatPendingRequest) {
  return `#${request.request_code}\n项目：${request.project_alias}\nAI：${request.agent_label}\n选择：\n补充：`
}

async function copyTemplate(request: WechatPendingRequest) {
  try {
    await navigator.clipboard.writeText(replyTemplate(request))
    message.success('回复模板已复制')
  }
  catch (error) {
    message.error(`复制回复模板失败：${String(error)}`)
  }
}

async function saveAlias(request: WechatPendingRequest) {
  const alias = (aliasDrafts.value[request.project_root_path] || '').trim()
  if (!alias || alias === request.project_alias)
    return
  try {
    await invoke('set_wechat_project_alias', {
      projectRootPath: request.project_root_path,
      alias,
    })
    message.success('项目别名已保存')
    await loadRequests()
  }
  catch (error) {
    message.error(`保存项目别名失败：${String(error)}`)
  }
}

async function loadRequests() {
  loading.value = requests.value.length === 0
  try {
    requests.value = await invoke<WechatPendingRequest[]>('get_wechat_pending_requests')
    for (const request of requests.value) {
      if (aliasDrafts.value[request.project_root_path] === undefined)
        aliasDrafts.value[request.project_root_path] = request.project_alias
    }
    emit('countChange', pendingRequests.value.length)
  }
  catch (error) {
    message.error(`加载待处理请求失败：${String(error)}`)
  }
  finally {
    loading.value = false
  }
}

onMounted(() => {
  void loadRequests()
  refreshTimer = setInterval(() => void loadRequests(), 1000)
})

onUnmounted(() => {
  if (refreshTimer)
    clearInterval(refreshTimer)
})
</script>

<template>
  <n-card size="small" title="待处理请求">
    <n-spin :show="loading">
      <n-empty v-if="!requests.length" description="暂无等待回复的 zhi 请求" />
      <div v-else class="space-y-3">
        <div
          v-for="request in requests"
          :key="request.request_id"
          class="rounded border border-gray-200/70 p-3 dark:border-gray-700/70"
        >
          <div class="flex flex-wrap items-start justify-between gap-2">
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2 text-sm font-medium">
                <span class="font-mono">#{{ request.request_code }}</span>
                <n-tag :type="statusTypes[request.status]" size="small" :bordered="false">
                  {{ statusLabels[request.status] }}
                </n-tag>
              </div>
              <div class="mt-1 truncate text-xs opacity-65" :title="request.project_root_path">
                项目：{{ request.project_alias }} · AI：{{ request.agent_label }} · {{ remainingText(request) }}
              </div>
              <div class="mt-2 line-clamp-2 text-xs opacity-80">
                {{ request.prompt_preview }}
              </div>
            </div>
            <n-button size="tiny" secondary @click="copyTemplate(request)">
              <template #icon>
                <div class="i-carbon-copy w-3.5 h-3.5" />
              </template>
              复制模板
            </n-button>
          </div>
          <div class="mt-3 flex flex-wrap items-center gap-2 border-t border-gray-200/60 pt-3 dark:border-gray-700/60">
            <span class="text-xs opacity-60">项目别名</span>
            <n-input
              v-model:value="aliasDrafts[request.project_root_path]"
              size="small"
              maxlength="40"
              :disabled="request.status !== 'pending'"
              class="min-w-[180px] flex-1"
            />
            <n-button
              size="small"
              tertiary
              :disabled="request.status !== 'pending' || !aliasDrafts[request.project_root_path]?.trim()"
              @click="saveAlias(request)"
            >
              保存别名
            </n-button>
          </div>
        </div>
      </div>
    </n-spin>
  </n-card>
</template>
