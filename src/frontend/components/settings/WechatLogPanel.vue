<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'
import { useLogViewer } from '../../composables/useLogViewer'

interface ParsedLogLine {
  raw: string
  timestamp: string
  level: string
  message: string
}

const message = useMessage()
const { open: openFullLogViewer } = useLogViewer()
const lines = ref<ParsedLogLine[]>([])
const keyword = ref('')
const levels = ref<string[]>(['ERROR', 'WARN', 'INFO'])
const logFile = ref('')
const logDirectory = ref('')
const loading = ref(false)
const LOG_RE = /^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}) \[([A-Z]+)\] \[[^\]]+\] (.*)$/

const levelOptions = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'].map(value => ({ label: value, value }))
const filteredLines = computed(() => {
  const query = keyword.value.trim().toLowerCase()
  const selected = new Set(levels.value)
  return lines.value.filter(line => selected.has(line.level) && (!query || line.raw.toLowerCase().includes(query)))
})

function parseLine(raw: string): ParsedLogLine {
  const match = LOG_RE.exec(raw)
  return match
    ? { raw, timestamp: match[1], level: match[2], message: match[3] }
    : { raw, timestamp: '', level: 'INFO', message: raw }
}

function levelClass(level: string) {
  if (level === 'ERROR')
    return 'text-red-500 dark:text-red-300'
  if (level === 'WARN')
    return 'text-amber-600 dark:text-amber-300'
  if (level === 'DEBUG' || level === 'TRACE')
    return 'opacity-55'
  return 'opacity-80'
}

async function loadLogs() {
  loading.value = true
  try {
    const [rawLines, file, directory] = await Promise.all([
      invoke<string[]>('read_acemcp_logs', { maxLines: 5000, target: 'combined' }),
      invoke<string>('get_acemcp_log_file_path'),
      invoke<string>('get_acemcp_log_directory'),
    ])
    lines.value = rawLines.filter(line => line.includes('[wechat]')).map(parseLine).reverse()
    logFile.value = file
    logDirectory.value = directory
  }
  catch (error) {
    message.error(`加载微信诊断日志失败：${String(error)}`)
  }
  finally {
    loading.value = false
  }
}

async function copyVisible() {
  try {
    await navigator.clipboard.writeText(filteredLines.value.map(line => line.raw).join('\n'))
    message.success(`已复制 ${filteredLines.value.length} 行微信日志`)
  }
  catch (error) {
    message.error(`复制微信日志失败：${String(error)}`)
  }
}

async function openDirectory() {
  try {
    const directory = logDirectory.value || await invoke<string>('get_acemcp_log_directory')
    await invoke('open_external_url', { url: directory })
  }
  catch (error) {
    message.error(`打开日志目录失败：${String(error)}`)
  }
}

onMounted(loadLogs)
</script>

<template>
  <div class="flex h-[58vh] min-h-96 flex-col gap-3">
    <div class="rounded-lg border border-gray-200/70 bg-gray-50/60 px-3 py-2 dark:border-gray-700/70 dark:bg-black/10">
      <div class="text-xs opacity-60">
        当前日志文件
      </div>
      <n-ellipsis class="font-mono text-xs">
        {{ logFile || '正在读取...' }}
      </n-ellipsis>
    </div>

    <div class="flex flex-wrap items-center gap-2">
      <n-input v-model:value="keyword" clearable size="small" placeholder="搜索微信日志" class="min-w-48 flex-1">
        <template #prefix>
          <div class="i-carbon-search w-4 h-4 opacity-60" />
        </template>
      </n-input>
      <n-select v-model:value="levels" multiple size="small" :options="levelOptions" class="w-56" placeholder="日志级别" />
      <n-button size="small" secondary :loading="loading" @click="loadLogs">
        <template #icon>
          <div class="i-carbon-renew w-4 h-4" />
        </template>
        刷新
      </n-button>
      <n-button size="small" secondary :disabled="filteredLines.length === 0" @click="copyVisible">
        <template #icon>
          <div class="i-carbon-copy w-4 h-4" />
        </template>
        复制当前结果
      </n-button>
      <n-button size="small" secondary @click="openDirectory">
        <template #icon>
          <div class="i-carbon-folder-open w-4 h-4" />
        </template>
        打开目录
      </n-button>
      <n-button size="small" secondary @click="openFullLogViewer">
        <template #icon>
          <div class="i-carbon-launch w-4 h-4" />
        </template>
        完整日志
      </n-button>
    </div>

    <div class="text-xs opacity-60">
      显示 {{ filteredLines.length }} / {{ lines.length }} 行；内嵌视图读取最近 5000 行中的微信日志。
    </div>

    <div class="flex-1 overflow-y-auto rounded-lg border border-gray-200/70 bg-gray-50/70 p-3 font-mono text-xs leading-5 dark:border-gray-700/70 dark:bg-black/30">
      <n-spin :show="loading">
        <n-empty v-if="!loading && filteredLines.length === 0" description="暂无匹配的微信日志" class="py-16" />
        <div v-else class="space-y-1">
          <div v-for="(line, index) in filteredLines" :key="`${line.timestamp}-${index}`" :class="levelClass(line.level)" class="break-all">
            <span class="opacity-55">{{ line.timestamp }}</span>
            <span class="mx-2 font-semibold">{{ line.level }}</span>
            <span>{{ line.message }}</span>
          </div>
        </div>
      </n-spin>
    </div>
  </div>
</template>
