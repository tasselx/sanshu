<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'
import { useLogViewer } from '../../composables/useLogViewer'

interface AcemcpLogTargetInfo {
  target: string
  label: string
  exists: boolean
  size_bytes?: number
  modified_utc?: string
}

const message = useMessage()
const { open: openLogViewer } = useLogViewer()

const logFilePath = ref('')
const logDirectory = ref('')
const targets = ref<AcemcpLogTargetInfo[]>([])
const loading = ref(false)
const copying = ref(false)

const currentTarget = computed(() => {
  return targets.value.find(t => t.target === 'current')
})

function formatBytes(bytes?: number) {
  if (!bytes || bytes <= 0)
    return '0B'

  const KB = 1024
  const MB = 1024 * 1024
  const GB = 1024 * 1024 * 1024

  if (bytes >= GB)
    return `${(bytes / GB).toFixed(2)}GB`
  if (bytes >= MB)
    return `${(bytes / MB).toFixed(2)}MB`
  if (bytes >= KB)
    return `${(bytes / KB).toFixed(2)}KB`
  return `${bytes}B`
}

function formatModified(value?: string) {
  if (!value)
    return '暂无'

  const date = new Date(value)
  if (Number.isNaN(date.getTime()))
    return value

  return date.toLocaleString()
}

async function refreshLogStatus() {
  loading.value = true
  try {
    const [filePath, directory, targetList] = await Promise.all([
      invoke('get_acemcp_log_file_path') as Promise<string>,
      invoke('get_acemcp_log_directory') as Promise<string>,
      invoke('list_acemcp_log_targets') as Promise<AcemcpLogTargetInfo[]>,
    ])

    logFilePath.value = filePath || ''
    logDirectory.value = directory || ''
    targets.value = Array.isArray(targetList) ? targetList : []
  }
  catch (err) {
    message.error(`刷新日志状态失败: ${err}`)
  }
  finally {
    loading.value = false
  }
}

async function openLogDirectory() {
  try {
    const dir = logDirectory.value || await invoke('get_acemcp_log_directory') as string
    if (!dir) {
      message.error('无法获取日志目录')
      return
    }
    await invoke('open_external_url', { url: dir })
  }
  catch (err) {
    message.error(`打开日志目录失败: ${err}`)
  }
}

async function copyRecentLogs() {
  if (copying.value)
    return

  copying.value = true
  try {
    const lines = await invoke('read_acemcp_logs', { maxLines: 1000, target: 'combined' }) as string[]
    if (!lines.length) {
      message.info('日志为空')
      return
    }

    await navigator.clipboard.writeText(lines.join('\n'))
    message.success(`已复制最近 ${lines.length} 行日志`)
  }
  catch (err) {
    message.error(`复制日志失败: ${err}`)
  }
  finally {
    copying.value = false
  }
}

onMounted(refreshLogStatus)
</script>

<template>
  <div class="max-w-4xl mx-auto tab-content p-4">
    <n-space vertical size="large">
      <div class="logs-header">
        <div class="logs-heading">
          <div class="logs-icon">
            <div class="i-carbon-document-view" />
          </div>
          <div>
            <div class="logs-title">
              全局日志与调试
            </div>
            <div class="logs-subtitle">
              查看应用与 MCP 共享日志，定位启动、网络、更新和工具运行状态。
            </div>
          </div>
        </div>

        <n-button size="small" secondary :loading="loading" @click="refreshLogStatus">
          <template #icon>
            <div class="i-carbon-renew" />
          </template>
          刷新状态
        </n-button>
      </div>

      <n-alert type="info" :bordered="false" class="logs-alert">
        <template #icon>
          <div class="i-carbon-information" />
        </template>
        日志时间按本机时区显示；日志文件由 GUI 与 MCP 模式共用。
      </n-alert>

      <div class="logs-actions">
        <n-button type="primary" ghost @click="openLogViewer">
          <template #icon>
            <div class="i-carbon-view" />
          </template>
          查看实时日志
        </n-button>
        <n-button secondary @click="openLogViewer">
          <template #icon>
            <div class="i-carbon-document" />
          </template>
          查看日志
        </n-button>
        <n-button secondary @click="openLogDirectory">
          <template #icon>
            <div class="i-carbon-folder-open" />
          </template>
          打开目录
        </n-button>
        <n-button secondary :loading="copying" :disabled="copying" @click="copyRecentLogs">
          <template #icon>
            <div class="i-carbon-copy" />
          </template>
          复制最近日志
        </n-button>
      </div>

      <div class="logs-status-grid">
        <div class="logs-status-item">
          <div class="status-label">
            当前日志
          </div>
          <div class="status-value">
            {{ currentTarget?.exists ? '可读取' : '未生成' }}
          </div>
          <div class="status-detail">
            {{ currentTarget?.label || 'sanshu-mcp.log' }}
          </div>
        </div>
        <div class="logs-status-item">
          <div class="status-label">
            文件大小
          </div>
          <div class="status-value">
            {{ formatBytes(currentTarget?.size_bytes) }}
          </div>
          <div class="status-detail">
            当前文件，备份可在日志抽屉中切换
          </div>
        </div>
        <div class="logs-status-item">
          <div class="status-label">
            更新时间
          </div>
          <div class="status-value">
            {{ formatModified(currentTarget?.modified_utc) }}
          </div>
          <div class="status-detail">
            文件系统记录的最后修改时间
          </div>
        </div>
      </div>

      <div class="logs-path-panel">
        <div class="path-row">
          <span class="path-label">日志文件</span>
          <code class="path-code">{{ logFilePath || '默认配置目录 / sanshu / log / sanshu-mcp.log' }}</code>
        </div>
        <div class="path-row">
          <span class="path-label">日志目录</span>
          <code class="path-code">{{ logDirectory || '等待刷新' }}</code>
        </div>
      </div>
    </n-space>
  </div>
</template>

<style scoped>
.logs-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.logs-heading {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: 0;
}

.logs-icon {
  width: 40px;
  height: 40px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: #5eead4;
  background: rgba(20, 184, 166, 0.14);
  border: 1px solid rgba(94, 234, 212, 0.18);
}

.logs-title {
  font-size: 18px;
  font-weight: 600;
  color: rgba(248, 250, 252, 0.96);
}

.logs-subtitle {
  margin-top: 4px;
  font-size: 13px;
  color: rgba(203, 213, 225, 0.78);
}

.logs-alert {
  border-radius: 8px;
}

.logs-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}

.logs-status-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
}

.logs-status-item,
.logs-path-panel {
  border: 1px solid rgba(148, 163, 184, 0.16);
  border-radius: 8px;
  background: rgba(15, 23, 42, 0.34);
}

.logs-status-item {
  padding: 14px;
}

.status-label {
  font-size: 12px;
  color: rgba(203, 213, 225, 0.72);
}

.status-value {
  margin-top: 6px;
  font-size: 16px;
  font-weight: 600;
  color: rgba(248, 250, 252, 0.96);
}

.status-detail {
  margin-top: 6px;
  font-size: 12px;
  color: rgba(203, 213, 225, 0.68);
  word-break: break-all;
}

.logs-path-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px;
}

.path-row {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  gap: 10px;
  align-items: baseline;
}

.path-label {
  font-size: 12px;
  color: rgba(203, 213, 225, 0.72);
}

.path-code {
  min-width: 0;
  padding: 4px 6px;
  border-radius: 4px;
  font-size: 12px;
  line-height: 1.5;
  color: rgba(226, 232, 240, 0.94);
  background: rgba(2, 6, 23, 0.5);
  word-break: break-all;
}

@media (max-width: 720px) {
  .logs-header {
    flex-direction: column;
  }

  .logs-status-grid {
    grid-template-columns: 1fr;
  }

  .path-row {
    grid-template-columns: 1fr;
  }
}
</style>
