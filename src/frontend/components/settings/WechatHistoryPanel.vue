<script setup lang="ts">
import type { WechatHistoryEntry } from '../../types/wechat'
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'

const emit = defineEmits<{ countChange: [count: number] }>()
const message = useMessage()
const entries = ref<WechatHistoryEntry[]>([])
const keyword = ref('')
const direction = ref<string | null>(null)
const kind = ref<string | null>(null)
const loading = ref(false)

const directionOptions = [
  { label: '全部方向', value: '' },
  { label: '发出', value: 'outgoing' },
  { label: '收到', value: 'incoming' },
]

const kindOptions = [
  { label: '全部类型', value: '' },
  { label: 'zhi 通知', value: 'zhi' },
  { label: '用户回复', value: 'reply' },
  { label: '系统消息', value: 'system' },
  { label: '连接测试', value: 'test' },
]

const filteredEntries = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  return entries.value.filter((entry) => {
    if (direction.value && entry.direction !== direction.value)
      return false
    if (kind.value && entry.kind !== kind.value)
      return false
    if (!query)
      return true
    return entry.content.toLowerCase().includes(query)
      || entry.request_code?.toLowerCase().includes(query)
  })
})

function kindLabel(value: WechatHistoryEntry['kind']) {
  return kindOptions.find(option => option.value === value)?.label || value
}

function formatTime(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}

async function loadHistory() {
  loading.value = true
  try {
    entries.value = await invoke<WechatHistoryEntry[]>('get_wechat_history', { limit: 200 })
    emit('countChange', entries.value.length)
  }
  catch (error) {
    message.error(`加载微信聊天记录失败：${String(error)}`)
  }
  finally {
    loading.value = false
  }
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    message.success('记录已复制')
  }
  catch (error) {
    message.error(`复制记录失败：${String(error)}`)
  }
}

async function copyVisible() {
  const text = filteredEntries.value
    .map(entry => `[${formatTime(entry.timestamp)}] ${entry.direction === 'outgoing' ? '发出' : '收到'} · ${kindLabel(entry.kind)}\n${entry.content}`)
    .join('\n\n')
  await copyText(text)
}

async function clearHistory() {
  try {
    await invoke('clear_wechat_history')
    entries.value = []
    emit('countChange', 0)
    message.success('微信聊天记录已清空')
  }
  catch (error) {
    message.error(`清空微信聊天记录失败：${String(error)}`)
  }
}

onMounted(loadHistory)
</script>

<template>
  <div class="flex h-[58vh] min-h-96 flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <n-input v-model:value="keyword" clearable size="small" placeholder="搜索正文或请求编号" class="min-w-52 flex-1">
        <template #prefix>
          <div class="i-carbon-search w-4 h-4 opacity-60" />
        </template>
      </n-input>
      <n-select v-model:value="direction" clearable size="small" :options="directionOptions" placeholder="方向" class="w-32" />
      <n-select v-model:value="kind" clearable size="small" :options="kindOptions" placeholder="类型" class="w-36" />
      <n-button size="small" secondary :loading="loading" @click="loadHistory">
        <template #icon>
          <div class="i-carbon-renew w-4 h-4" />
        </template>
        刷新
      </n-button>
      <n-button size="small" secondary :disabled="filteredEntries.length === 0" @click="copyVisible">
        <template #icon>
          <div class="i-carbon-copy w-4 h-4" />
        </template>
        复制当前结果
      </n-button>
      <n-popconfirm @positive-click="clearHistory">
        <template #trigger>
          <n-button size="small" secondary type="error" :disabled="entries.length === 0">
            <template #icon>
              <div class="i-carbon-trash-can w-4 h-4" />
            </template>
            清空
          </n-button>
        </template>
        清空后将从新消息开始记录，此操作不影响微信绑定。
      </n-popconfirm>
    </div>

    <div class="text-xs opacity-60">
      显示 {{ filteredEntries.length }} / {{ entries.length }} 条，最多保留 200 条；长正文单条最多保存 4000 字符。
    </div>

    <div class="flex-1 overflow-y-auto rounded-lg border border-gray-200/70 bg-gray-50/50 p-3 dark:border-gray-700/70 dark:bg-black/10">
      <n-spin :show="loading">
        <n-empty v-if="!loading && filteredEntries.length === 0" description="暂无匹配的微信聊天记录" class="py-16" />
        <div v-else class="space-y-3">
          <article
            v-for="entry in filteredEntries"
            :key="entry.id"
            class="rounded-lg border border-gray-200/70 bg-white/70 p-3 dark:border-gray-700/70 dark:bg-black-100/60"
          >
            <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
              <div class="flex items-center gap-2">
                <n-tag :type="entry.direction === 'incoming' ? 'success' : 'info'" size="small" :bordered="false">
                  {{ entry.direction === 'incoming' ? '收到' : '发出' }}
                </n-tag>
                <n-tag size="small" :bordered="false">
                  {{ kindLabel(entry.kind) }}
                </n-tag>
                <span v-if="entry.request_code" class="font-mono text-xs opacity-60">#{{ entry.request_code }}</span>
              </div>
              <div class="flex items-center gap-2 text-xs opacity-60">
                <span>{{ formatTime(entry.timestamp) }}</span>
                <n-button quaternary circle size="tiny" title="复制本条记录" @click="copyText(entry.content)">
                  <template #icon>
                    <div class="i-carbon-copy w-3.5 h-3.5" />
                  </template>
                </n-button>
              </div>
            </div>
            <n-ellipsis :line-clamp="4" expand-trigger="click" class="whitespace-pre-wrap text-sm leading-6">
              {{ entry.content }}
            </n-ellipsis>
          </article>
        </div>
      </n-spin>
    </div>
  </div>
</template>
