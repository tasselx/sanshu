<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useDialog, useMessage } from 'naive-ui'
import { onMounted, ref } from 'vue'

interface AttachmentFile {
  path: string
  filename: string
  kind: string // 'image' | 'file'
  ext: string
  size: number
  modified_ms: number
}

const message = useMessage()
const dialog = useDialog()

// 当前生效的工作目录
const workspaceDir = ref('')
// 附件列表
const files = ref<AttachmentFile[]>([])
const isLoading = ref(false)

// 加载工作目录与文件列表
async function loadAll() {
  isLoading.value = true
  try {
    workspaceDir.value = await invoke<string>('get_attachment_workspace_dir')
    files.value = await invoke<AttachmentFile[]>('list_attachments')
  }
  catch (error) {
    console.error('加载附件工作目录失败:', error)
    message.error(`加载失败：${String(error)}`)
  }
  finally {
    isLoading.value = false
  }
}

// 选择自定义目录
async function chooseDir() {
  try {
    const picked = await invoke<string | null>('select_attachment_workspace_dir', {
      defaultPath: workspaceDir.value || null,
    })
    if (!picked)
      return
    workspaceDir.value = await invoke<string>('set_attachment_workspace_dir', { dir: picked })
    message.success('工作目录已更新')
    await loadAll()
  }
  catch (error) {
    console.error('设置工作目录失败:', error)
    message.error(`设置失败：${String(error)}`)
  }
}

// 恢复默认全局目录
async function resetDir() {
  try {
    workspaceDir.value = await invoke<string>('set_attachment_workspace_dir', { dir: null })
    message.success('已恢复默认工作目录')
    await loadAll()
  }
  catch (error) {
    console.error('恢复默认目录失败:', error)
    message.error(`操作失败：${String(error)}`)
  }
}

// 打开目录
async function openDir() {
  try {
    await invoke('open_attachment_workspace_dir')
  }
  catch (error) {
    console.error('打开目录失败:', error)
    message.error(`打开失败：${String(error)}`)
  }
}

// 删除单个文件
async function deleteFile(file: AttachmentFile) {
  try {
    await invoke('delete_attachment', { filename: file.filename })
    files.value = files.value.filter(f => f.path !== file.path)
    message.success('已删除')
  }
  catch (error) {
    console.error('删除文件失败:', error)
    message.error(`删除失败：${String(error)}`)
  }
}

// 清空全部
function clearAll() {
  if (files.value.length === 0) {
    message.info('工作目录已经是空的')
    return
  }
  dialog.warning({
    title: '确认清空',
    content: `将删除工作目录中的全部 ${files.value.length} 个文件，且不可恢复。是否继续？`,
    positiveText: '清空',
    negativeText: '取消',
    onPositiveClick: async () => {
      try {
        const count = await invoke<number>('clear_attachments')
        files.value = []
        message.success(`已清空 ${count} 个文件`)
      }
      catch (error) {
        console.error('清空失败:', error)
        message.error(`清空失败：${String(error)}`)
      }
    },
  })
}

function formatSize(bytes: number): string {
  if (bytes < 1024)
    return `${bytes} B`
  if (bytes < 1024 * 1024)
    return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatTime(ms: number): string {
  if (!ms)
    return '-'
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

function getFileIcon(file: AttachmentFile): string {
  if (file.kind === 'image')
    return 'i-carbon-image'
  const e = (file.ext || '').toLowerCase()
  if (/(zip|rar|7z|tar|gz)/.test(e))
    return 'i-carbon-document-zip'
  if (/(pdf)/.test(e))
    return 'i-carbon-document-pdf'
  if (/(doc|docx)/.test(e))
    return 'i-carbon-document'
  if (/(xls|xlsx|csv)/.test(e))
    return 'i-carbon-document-tasks'
  if (/(mp4|mov|avi|mkv|webm)/.test(e))
    return 'i-carbon-video'
  if (/(mp3|wav|flac|aac|ogg)/.test(e))
    return 'i-carbon-music'
  if (/(js|ts|tsx|jsx|json|rs|py|go|java|c|cpp|h|vue|html|css|md|sh)/.test(e))
    return 'i-carbon-code'
  return 'i-carbon-document-blank'
}

onMounted(loadAll)
</script>

<template>
  <n-space vertical size="large">
    <!-- 工作目录信息 -->
    <div class="flex items-start">
      <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0 mt-2" />
      <div class="flex-1 min-w-0">
        <div class="text-sm font-medium leading-relaxed mb-1">
          当前工作目录
        </div>
        <div class="text-xs opacity-60 leading-relaxed mb-2">
          粘贴的图片与拖入的文件都会保存到这里，并以本地路径形式提供给 AI 读取。
          超过 7 天的旧文件会在应用启动时自动清理。
        </div>
        <code class="bg-black-100 px-2 py-1 rounded text-xs break-all block">{{ workspaceDir || '加载中...' }}</code>
      </div>
    </div>

    <!-- 操作按钮 -->
    <div class="flex flex-wrap gap-2">
      <n-button size="small" type="primary" @click="chooseDir">
        <template #icon>
          <div class="i-carbon-folder w-4 h-4" />
        </template>
        选择目录
      </n-button>
      <n-button size="small" @click="resetDir">
        <template #icon>
          <div class="i-carbon-reset w-4 h-4" />
        </template>
        恢复默认
      </n-button>
      <n-button size="small" @click="openDir">
        <template #icon>
          <div class="i-carbon-launch w-4 h-4" />
        </template>
        打开目录
      </n-button>
      <n-button size="small" :loading="isLoading" @click="loadAll">
        <template #icon>
          <div class="i-carbon-renew w-4 h-4" />
        </template>
        刷新
      </n-button>
      <n-button size="small" type="error" ghost @click="clearAll">
        <template #icon>
          <div class="i-carbon-trash-can w-4 h-4" />
        </template>
        清空全部
      </n-button>
    </div>

    <!-- 文件列表 -->
    <div>
      <div class="text-sm font-medium leading-relaxed mb-2">
        已存文件 ({{ files.length }})
      </div>

      <div v-if="files.length === 0" class="text-xs opacity-50 py-4 text-center">
        工作目录为空
      </div>

      <div v-else class="space-y-2">
        <div
          v-for="file in files"
          :key="file.path"
          class="flex items-center gap-3 p-2 rounded-lg border border-gray-600 bg-container-secondary"
        >
          <div :class="getFileIcon(file)" class="w-5 h-5 text-primary-400 flex-shrink-0" />
          <div class="flex-1 min-w-0">
            <div class="text-sm text-on-surface truncate" :title="file.filename">
              {{ file.filename }}
            </div>
            <div class="text-xs opacity-50">
              {{ formatSize(file.size) }} · {{ formatTime(file.modified_ms) }}
            </div>
          </div>
          <n-button size="tiny" type="error" quaternary circle @click="deleteFile(file)">
            <template #icon>
              <div class="i-carbon-close w-3 h-3" />
            </template>
          </n-button>
        </div>
      </div>
    </div>
  </n-space>
</template>
