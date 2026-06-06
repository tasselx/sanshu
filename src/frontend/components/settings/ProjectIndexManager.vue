<script setup lang="ts">
import type { IndexStatus, ProjectIndexStatus } from '../../types/tauri'
import { invoke } from '@tauri-apps/api/core'
import { useDialog, useMessage } from 'naive-ui'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAcemcpSync } from '../../composables/useAcemcpSync'
import { ProjectCard, ProjectCardSkeleton } from '../index'
import McpIndexStatusDrawer from '../popup/McpIndexStatusDrawer.vue'

// 使用 Acemcp 同步状态管理
const { triggerIndexUpdate } = useAcemcpSync()

const message = useMessage()
const dialog = useDialog()

/**
 * 规范化 Windows 路径
 * 去除扩展长度路径前缀 (\\?\ 或 //?/) 并统一使用正斜杠
 */
function normalizePath(path: string): string {
  let p = path || ''
  // 去除 Windows 扩展长度路径前缀
  if (p.startsWith('\\\\?\\')) {
    p = p.slice(4)
  }
  else if (p.startsWith('//?/')) {
    p = p.slice(4)
  }
  // 统一使用正斜杠
  return p.replace(/\\/g, '/')
}

// 本地状态
const loading = ref(true)
const allProjects = ref<Record<string, ProjectIndexStatus>>({})
const watchingProjects = ref<string[]>([])
const selectedProject = ref<string>('')
const showDrawer = ref(false)
const resyncLoading = ref(false)
// 目录存在状态缓存（key 为规范化后的路径）
const directoryExistsCache = ref<Record<string, boolean>>({})

// 搜索和筛选状态
const searchQuery = ref('')
const statusFilter = ref<IndexStatus | 'all' | 'stale'>('all')
const sortBy = ref<'status' | 'time' | 'name'>('status')

// 状态筛选选项
const statusOptions = [
  { label: '全部状态', value: 'all' },
  { label: '索引中', value: 'indexing' },
  { label: '待重建', value: 'stale' },
  { label: '已完成', value: 'synced' },
  { label: '失败', value: 'failed' },
  { label: '未索引', value: 'idle' },
]

// 排序选项
const sortOptions = [
  { label: '按状态', value: 'status' },
  { label: '按时间', value: 'time' },
  { label: '按名称', value: 'name' },
]

// 轮询定时器
let pollingTimer: number | null = null

// 选中项目的状态信息（用于抽屉组件）
const selectedProjectStatus = computed<ProjectIndexStatus | null>(() => {
  if (!selectedProject.value)
    return null
  return allProjects.value[selectedProject.value] || null
})

// 选中项目的状态摘要文本
const selectedStatusSummary = computed(() => {
  const status = selectedProjectStatus.value
  if (!status)
    return '未索引'
  if (status.is_stale && status.status !== 'indexing')
    return '配置已变更'
  switch (status.status) {
    case 'idle':
      return '空闲'
    case 'indexing':
      return `索引中 ${status.progress}%`
    case 'synced':
      return '已同步'
    case 'failed':
      return '索引失败'
    default:
      return '未知状态'
  }
})

// 选中项目的状态图标
const selectedStatusIcon = computed(() => {
  const selected = selectedProjectStatus.value
  if (selected?.is_stale && selected.status !== 'indexing')
    return 'i-carbon-warning-alt text-amber-500'

  const status = selected?.status
  switch (status) {
    case 'idle':
      return 'i-carbon-circle-dash text-gray-400'
    case 'indexing':
      return 'i-carbon-in-progress text-blue-500 animate-spin'
    case 'synced':
      return 'i-carbon-checkmark-filled text-green-500'
    case 'failed':
      return 'i-carbon-warning-filled text-red-500'
    default:
      return 'i-carbon-help text-gray-400'
  }
})

// 选中项目是否正在索引
const selectedIsIndexing = computed(() => {
  return selectedProjectStatus.value?.status === 'indexing'
})

// 是否有正在索引的项目（用于控制轮询频率）
const hasIndexingProject = computed(() => {
  return Object.values(allProjects.value).some(p => p.status === 'indexing')
})

// 计算项目列表（带搜索、筛选和排序）
const projectList = computed(() => {
  let list = Object.values(allProjects.value)

  // 搜索过滤
  if (searchQuery.value.trim()) {
    const query = searchQuery.value.toLowerCase()
    list = list.filter(p => p.project_root.toLowerCase().includes(query))
  }

  // 状态筛选
  if (statusFilter.value !== 'all') {
    list = list.filter((p) => {
      if (statusFilter.value === 'stale')
        return !!p.is_stale && p.status !== 'indexing'
      return p.status === statusFilter.value
    })
  }

  // 排序
  const statusOrder = { indexing: 0, stale: 1, synced: 2, failed: 3, idle: 4 }
  list.sort((a, b) => {
    switch (sortBy.value) {
      case 'status':
        return statusOrder[a.is_stale && a.status !== 'indexing' ? 'stale' : a.status]
          - statusOrder[b.is_stale && b.status !== 'indexing' ? 'stale' : b.status]
      case 'time': {
        const timeA = a.last_success_time ? new Date(a.last_success_time).getTime() : 0
        const timeB = b.last_success_time ? new Date(b.last_success_time).getTime() : 0
        return timeB - timeA // 最近的在前
      }
      case 'name': {
        const nameA = a.project_root.split(/[/\\]/).pop() || ''
        const nameB = b.project_root.split(/[/\\]/).pop() || ''
        return nameA.localeCompare(nameB)
      }
      default:
        return 0
    }
  })

  return list
})

// 统计信息
const stats = computed(() => {
  const projects = Object.values(allProjects.value)
  return {
    total: projects.length,
    indexing: projects.filter(p => p.status === 'indexing').length,
    stale: projects.filter(p => p.is_stale && p.status !== 'indexing').length,
    synced: projects.filter(p => p.status === 'synced' && !p.is_stale).length,
    failed: projects.filter(p => p.status === 'failed').length,
  }
})

// 初始化加载
onMounted(async () => {
  await loadAllData()
  // 加载完成后检测所有目录的存在状态
  await checkAllDirectoriesExist()
  startPolling()
})

// 组件卸载时清理
onUnmounted(() => {
  stopPolling()
})

// 开始轮询
function startPolling() {
  if (pollingTimer)
    return
  // 根据是否有索引中的项目调整轮询频率
  const interval = hasIndexingProject.value ? 3000 : 30000
  pollingTimer = window.setInterval(async () => {
    await refreshData()
    // 动态调整轮询频率
    if (pollingTimer) {
      const newInterval = hasIndexingProject.value ? 3000 : 30000
      if (newInterval !== interval) {
        stopPolling()
        startPolling()
      }
    }
  }, interval)
}

// 停止轮询
function stopPolling() {
  if (pollingTimer) {
    clearInterval(pollingTimer)
    pollingTimer = null
  }
}

// 刷新数据（不显示加载状态）
async function refreshData() {
  try {
    const [statusResult, watchingResult] = await Promise.all([
      invoke<{ projects: Record<string, ProjectIndexStatus> }>('get_all_acemcp_index_status'),
      invoke<string[]>('get_watching_projects'),
    ])
    allProjects.value = statusResult.projects
    watchingProjects.value = watchingResult
  }
  catch (err) {
    console.error('刷新项目索引数据失败:', err)
  }
}

// 加载所有数据（显示加载状态）
async function loadAllData() {
  loading.value = true
  try {
    const [statusResult, watchingResult] = await Promise.all([
      invoke<{ projects: Record<string, ProjectIndexStatus> }>('get_all_acemcp_index_status'),
      invoke<string[]>('get_watching_projects'),
    ])
    allProjects.value = statusResult.projects
    watchingProjects.value = watchingResult
  }
  catch (err) {
    console.error('加载项目索引数据失败:', err)
    message.error('加载项目索引数据失败')
  }
  finally {
    loading.value = false
  }
}

// 复制项目路径（直接使用子组件传递的规范化路径）
async function copyPath(path: string) {
  try {
    await navigator.clipboard.writeText(path)
    message.success('路径已复制到剪贴板')
  }
  catch (err) {
    message.error('复制失败')
  }
}

// 切换项目监听状态
async function toggleWatching(projectRoot: string) {
  // 规范化路径，去除 Windows 扩展前缀
  const normalizedPath = normalizePath(projectRoot)
  const currentlyWatching = watchingProjects.value.some(p => normalizePath(p) === normalizedPath)
  try {
    if (currentlyWatching) {
      await invoke('stop_project_watching', { projectRootPath: normalizedPath })
      message.success('已停止监听项目')
    }
    else {
      // 修正：调用启动监听命令而非手动索引
      await invoke('start_project_watching', { projectRootPath: normalizedPath })
      message.success('已记录 MCP 持久监听，三术进程会按配置接管监听')
    }
    watchingProjects.value = await invoke<string[]>('get_watching_projects')
  }
  catch (err) {
    console.error('切换监听状态失败:', err)
    message.error(`操作失败: ${err}`)
  }
}

// 重新索引（带二次确认）
function handleReindex(projectRoot: string) {
  const normalizedPath = normalizePath(projectRoot)
  dialog.warning({
    title: '确认重新索引',
    content: `确定要重新索引项目吗？\n\n${normalizedPath}\n\n这将重新扫描所有文件并更新索引。`,
    positiveText: '确认',
    negativeText: '取消',
    onPositiveClick: async () => {
      try {
        await triggerIndexUpdate(normalizedPath)
        message.success('已触发重新索引')
        // 延迟刷新状态
        setTimeout(() => loadAllData(), 1000)
      }
      catch (err) {
        console.error('重新索引失败:', err)
        message.error('重新索引失败')
      }
    },
  })
}

// 查看项目结构树
function viewProjectTree(projectRoot: string) {
  selectedProject.value = normalizePath(projectRoot)
  showDrawer.value = true
}

// 抽屉中的重新同步处理
async function handleDrawerResync() {
  if (!selectedProject.value)
    return
  resyncLoading.value = true
  try {
    // selectedProject 已经是规范化的路径
    await triggerIndexUpdate(selectedProject.value)
    message.success('已触发重新索引')
    // 延迟刷新状态
    setTimeout(() => loadAllData(), 1000)
  }
  catch (err) {
    console.error('重新索引失败:', err)
    message.error(`重新索引失败: ${err}`)
  }
  finally {
    resyncLoading.value = false
  }
}

// 检测目录是否存在
async function checkDirectoryExists(projectRoot: string): Promise<boolean> {
  const normalizedPath = normalizePath(projectRoot)
  // 优先使用缓存
  if (normalizedPath in directoryExistsCache.value) {
    return directoryExistsCache.value[normalizedPath]
  }
  try {
    const exists = await invoke<boolean>('check_directory_exists', {
      directoryPath: normalizedPath,
    })
    directoryExistsCache.value[normalizedPath] = exists
    return exists
  }
  catch (err) {
    console.error('检测目录存在性失败:', err)
    return true // 默认存在，避免误删
  }
}

// 加载所有项目的目录存在状态
async function checkAllDirectoriesExist() {
  const projects = Object.keys(allProjects.value)
  const results = await Promise.all(
    projects.map(async (projectRoot) => {
      const exists = await checkDirectoryExists(projectRoot)
      return { projectRoot, exists }
    }),
  )
  results.forEach(({ projectRoot, exists }) => {
    const normalizedPath = normalizePath(projectRoot)
    directoryExistsCache.value[normalizedPath] = exists
  })
}

// 删除项目索引记录（带二次确认）
function handleDeleteProject(projectRoot: string) {
  const normalizedPath = normalizePath(projectRoot)
  const projectName = normalizedPath.split('/').pop() || normalizedPath

  dialog.warning({
    title: '确认删除',
    content: `确定要删除项目索引记录吗？\n\n项目: ${projectName}\n路径: ${normalizedPath}\n\n此操作将从列表中移除该项目，不会删除实际文件。`,
    positiveText: '删除',
    negativeText: '取消',
    onPositiveClick: async () => {
      try {
        console.debug('[handleDeleteProject] 开始删除, normalizedPath=', normalizedPath)
        const result = await invoke<string>('remove_acemcp_project_index', {
          projectRootPath: normalizedPath,
        })
        console.debug('[handleDeleteProject] 删除命令执行结果:', result)
        message.success('已删除项目索引记录')
        // 从本地缓存中移除
        delete directoryExistsCache.value[normalizedPath]
        // 刷新列表
        console.debug('[handleDeleteProject] 开始刷新列表')
        await loadAllData()
        console.debug('[handleDeleteProject] 刷新完成, 项目数=', Object.keys(allProjects.value).length)
      }
      catch (err) {
        console.error('删除项目索引记录失败:', err)
        message.error(`删除失败: ${err}`)
      }
    },
  })
}

// 获取指定项目的目录存在状态
function getDirectoryExists(projectRoot: string): boolean {
  const normalizedPath = normalizePath(projectRoot)
  // 如果还没检测过，默认返回 true
  return directoryExistsCache.value[normalizedPath] ?? true
}
</script>

<template>
  <div class="project-index-manager">
    <!-- 顶部工具栏 -->
    <div class="toolbar-section">
      <!-- 统计信息 -->
      <div class="stats-bar">
        <div class="stat-chip">
          <div class="i-carbon-folder" />
          <span>{{ stats.total }} 个项目</span>
        </div>
        <div v-if="stats.indexing > 0" class="stat-chip is-indexing">
          <div class="i-carbon-in-progress animate-spin" />
          <span>{{ stats.indexing }} 索引中</span>
        </div>
        <div v-if="stats.stale > 0" class="stat-chip is-stale">
          <div class="i-carbon-warning-alt" />
          <span>{{ stats.stale }} 待重建</span>
        </div>
        <div v-if="stats.synced > 0" class="stat-chip is-synced">
          <div class="i-carbon-checkmark-filled" />
          <span>{{ stats.synced }} 已完成</span>
        </div>
        <div v-if="stats.failed > 0" class="stat-chip is-failed">
          <div class="i-carbon-warning-filled" />
          <span>{{ stats.failed }} 失败</span>
        </div>
      </div>

      <!-- 搜索和筛选 -->
      <div class="filter-bar">
        <n-input
          v-model:value="searchQuery"
          placeholder="搜索项目..."
          clearable
          size="small"
          class="search-input"
        >
          <template #prefix>
            <div class="i-carbon-search opacity-50" />
          </template>
        </n-input>

        <n-select
          v-model:value="statusFilter"
          :options="statusOptions"
          size="small"
          class="filter-select"
          placeholder="状态"
        />

        <n-select
          v-model:value="sortBy"
          :options="sortOptions"
          size="small"
          class="sort-select"
          placeholder="排序"
        />

        <n-button size="small" quaternary @click="loadAllData">
          <template #icon>
            <div class="i-carbon-renew" />
          </template>
        </n-button>
      </div>
    </div>

    <!-- 加载状态 - 骨架屏网格 -->
    <div v-if="loading" class="card-grid">
      <ProjectCardSkeleton v-for="i in 6" :key="i" />
    </div>

    <!-- 空状态 -->
    <div v-else-if="projectList.length === 0 && !searchQuery && statusFilter === 'all'" class="empty-state">
      <div class="empty-icon">
        <div class="i-carbon-folder-off text-5xl opacity-30" />
      </div>
      <div class="empty-title">
        暂无项目索引数据
      </div>
      <div class="empty-desc">
        使用代码搜索工具后，项目将自动显示在这里
      </div>
    </div>

    <!-- 搜索无结果 -->
    <div v-else-if="projectList.length === 0" class="empty-state">
      <div class="empty-icon">
        <div class="i-carbon-search text-4xl opacity-30" />
      </div>
      <div class="empty-title">
        未找到匹配的项目
      </div>
      <div class="empty-desc">
        尝试调整搜索条件或筛选器
      </div>
      <n-button size="small" @click="searchQuery = ''; statusFilter = 'all'">
        清除筛选
      </n-button>
    </div>

    <!-- 项目卡片网格 -->
    <div v-else class="card-grid">
      <ProjectCard
        v-for="project in projectList"
        :key="project.project_root"
        :project="project"
        :is-watching="watchingProjects.includes(project.project_root)"
        :directory-exists="getDirectoryExists(project.project_root)"
        @view-tree="viewProjectTree(project.project_root)"
        @reindex="handleReindex(project.project_root)"
        @toggle-watching="toggleWatching(project.project_root)"
        @copy-path="copyPath"
        @delete="handleDeleteProject(project.project_root)"
      />
    </div>

    <!-- 项目结构树抽屉 -->
    <McpIndexStatusDrawer
      v-model:show="showDrawer"
      :project-root="selectedProject"
      :status-summary="selectedStatusSummary"
      :status-icon="selectedStatusIcon"
      :project-status="selectedProjectStatus"
      :is-indexing="selectedIsIndexing"
      :resync-loading="resyncLoading"
      @resync="handleDrawerResync"
    />
  </div>
</template>

<style scoped>
/* 项目索引管理容器 */
.project-index-manager {
  max-width: 100%;
  margin: 0 auto;
}

/* 顶部工具栏区域 */
.toolbar-section {
  margin-bottom: 16px;
  space-y: 12px;
}

/* 统计信息栏 - 莫兰迪胶囊标签 */
.stats-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}

.stat-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 12px;
  border-radius: 20px;
  font-size: 11px;
  font-weight: 500;
  border: 1px solid var(--color-border, rgba(128, 128, 128, 0.15));
  background: var(--color-container, rgba(255, 255, 255, 0.4));
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  color: var(--color-on-surface-secondary, #6b7280);
}

:root.dark .stat-chip {
  background: rgba(255, 255, 255, 0.03);
  color: #9ca3af;
}

.stat-chip:hover {
  transform: translateY(-1px);
  border-color: rgba(128, 128, 128, 0.3);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.04);
}

.stat-chip .i-carbon-folder {
  color: rgb(20, 184, 166);
}

.stat-chip.is-indexing {
  color: rgb(59, 130, 246);
  border-color: rgba(59, 130, 246, 0.25);
  background: rgba(59, 130, 246, 0.06);
}

.stat-chip.is-stale {
  color: rgb(245, 158, 11);
  border-color: rgba(245, 158, 11, 0.25);
  background: rgba(245, 158, 11, 0.06);
}

.stat-chip.is-synced {
  color: rgb(34, 197, 94);
  border-color: rgba(34, 197, 94, 0.25);
  background: rgba(34, 197, 94, 0.06);
}

.stat-chip.is-failed {
  color: rgb(239, 68, 68);
  border-color: rgba(239, 68, 68, 0.25);
  background: rgba(239, 68, 68, 0.06);
}

/* 筛选栏 - 扁平带衬底 */
.filter-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
  background: var(--color-container, rgba(255, 255, 255, 0.2));
  padding: 6px 8px;
  border-radius: 8px;
  border: 1px dashed var(--color-border, rgba(128, 128, 128, 0.12));
}

:root.dark .filter-bar {
  background: rgba(255, 255, 255, 0.01);
}

.search-input {
  flex: 1;
  min-width: 150px;
  max-width: 250px;
}

.filter-select {
  width: 105px;
}

.sort-select {
  width: 95px;
}

/* 卡片网格布局 */
.card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 16px;
}

/* 空状态样式 */
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 24px;
  text-align: center;
}

.empty-icon {
  margin-bottom: 16px;
}

.empty-title {
  font-size: 16px;
  font-weight: 500;
  margin-bottom: 8px;
  opacity: 0.8;
}

.empty-desc {
  font-size: 13px;
  opacity: 0.5;
  margin-bottom: 16px;
}

/* 响应式调整 */
@media (max-width: 768px) {
  .card-grid {
    grid-template-columns: 1fr;
  }

  .filter-bar {
    flex-direction: column;
    align-items: stretch;
  }

  .search-input {
    max-width: none;
  }

  .filter-select,
  .sort-select {
    width: 100%;
  }
}
</style>
