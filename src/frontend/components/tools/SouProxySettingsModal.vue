<!-- eslint-disable vue/no-mutating-props -->
<!-- eslint-disable style/max-statements-per-line -->
<script setup lang="ts">
/**
 * 代理设置独立弹窗组件
 * 包含：代理配置、自动检测、测速、测速报告等功能
 */
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useDialog, useMessage } from 'naive-ui'
import { computed, onUnmounted, ref } from 'vue'

// Props
const props = defineProps<{
  show: boolean
  config: any // 传入的 config 对象（响应式）
}>()

const emit = defineEmits<{
  (e: 'update:show', v: boolean): void
}>()

const message = useMessage()
const dialog = useDialog()

// --- 类型定义 ---
interface DetectedProxy {
  host: string
  port: number
  proxy_type: string
  response_time_ms: number | null
}

// 搜索结果预览片段
interface SearchResultSnippet {
  file_path: string
  snippet: string
  line_number: number | null
}

// 搜索结果预览
interface SearchResultPreview {
  total_matches: number
  snippets: SearchResultSnippet[]
  response_length: number
}

interface SpeedTestMetric {
  name: string
  metric_type: string
  proxy_time_ms: number | null
  direct_time_ms: number | null
  success: boolean
  error: string | null
  search_result_preview?: SearchResultPreview | null
}

interface SpeedTestResult {
  mode: string
  proxy_info: DetectedProxy | null
  metrics: SpeedTestMetric[]
  timestamp: string
  recommendation: string
  success: boolean
}

interface SpeedTestQueryDetail {
  query: string
  proxy_time_ms: number | null
  direct_time_ms: number | null
  success: boolean
  error: string | null
}

// 测速阶段状态
type SpeedTestStageStatus = 'Pending' | 'Running' | 'Completed' | 'Failed'

// 测速进度事件数据
interface SpeedTestProgress {
  stage: number
  stage_name: string
  percentage: number
  status: SpeedTestStageStatus
  detail: string | null
  sub_step: string | null
}

type IndexStatus = 'idle' | 'indexing' | 'synced' | 'failed'

interface ProjectIndexStatusLite {
  project_root: string
  status: IndexStatus
  total_files: number
  last_success_time: string | null
}

// --- 状态变量 ---

const proxyDetecting = ref(false)
const detectedProxies = ref<DetectedProxy[]>([])
const proxyTesting = ref(false)
const speedTestResult = ref<SpeedTestResult | null>(null)
const speedTestProgress = ref('')
const speedTestProgressData = ref<SpeedTestProgress | null>(null)
let unlistenSpeedTestProgress: (() => void) | null = null
const speedTestMode = ref<'proxy' | 'direct' | 'compare'>('compare')
const speedTestQuery = ref('代码搜索测试')
const multiQuerySearchDetails = ref<SpeedTestQueryDetail[]>([])
const multiQueryDetailsExpanded = ref(false)

const extraDetectPortsText = ref('')
const proxyPickerVisible = ref(false)
const selectedProxyIndex = ref(0)

// 测速项目相关
const speedTestProjectRoot = ref('')
const projectPickerVisible = ref(false)
const projectPickerLoading = ref(false)
const projectPickerSelected = ref('')
const indexedProjects = ref<ProjectIndexStatusLite[]>([])
const projectUploadMode = ref<'sample' | 'full'>('sample')
const projectUploadMaxFiles = ref(200)

const addProjectVisible = ref(false)
const addProjectPath = ref('')
const addProjectIndexing = ref(false)

// 组件卸载时清理监听器
onUnmounted(() => {
  if (unlistenSpeedTestProgress) {
    unlistenSpeedTestProgress()
    unlistenSpeedTestProgress = null
  }
})

// --- 计算属性 ---

const showModal = computed({
  get: () => props.show,
  set: v => emit('update:show', v),
})

const speedTestQueries = computed(() => {
  return (speedTestQuery.value || '')
    .split(/\r?\n/g)
    .flatMap(line => line.split(';'))
    .map(s => s.trim())
    .filter(Boolean)
    .slice(0, 5)
})

const multiQuerySearchSummary = computed(() => {
  const list = multiQuerySearchDetails.value
  if (list.length <= 1) {
    return null
  }

  const proxyTimes = list.map(i => i.proxy_time_ms).filter((v): v is number => v !== null)
  const directTimes = list.map(i => i.direct_time_ms).filter((v): v is number => v !== null)

  const proxyAvg = proxyTimes.length > 0
    ? Math.round(proxyTimes.reduce((a, b) => a + b, 0) / proxyTimes.length)
    : null

  const directAvg = directTimes.length > 0
    ? Math.round(directTimes.reduce((a, b) => a + b, 0) / directTimes.length)
    : null

  return {
    total: list.length,
    proxy_avg_ms: proxyAvg,
    direct_avg_ms: directAvg,
    proxy_ok: proxyTimes.length,
    direct_ok: directTimes.length,
  }
})

const currentProjectInfo = computed(() => {
  if (!speedTestProjectRoot.value)
    return null
  return indexedProjects.value.find(p => p.project_root === speedTestProjectRoot.value)
})

const speedTestMetricsForDisplay = computed(() => {
  const r = speedTestResult.value
  if (!r) {
    return []
  }

  const metrics = r.metrics || []

  // 多查询时：逐条搜索指标会比较多，默认只展示“搜索平均 + 其他指标”
  if (multiQuerySearchSummary.value) {
    const out = metrics.filter(m => m.metric_type !== 'search')

    // 兜底：如果没有“搜索平均”，保留第一条搜索指标
    if (!out.some(m => m.metric_type === 'search_multi_avg')) {
      const firstSearch = metrics.find(m => m.metric_type === 'search')
      if (firstSearch) {
        out.push(firstSearch)
      }
    }

    return out
  }

  return metrics
})

// 测速按钮禁用逻辑
const speedTestDisabled = computed(() => {
  if (!props.config.base_url || !props.config.token) {
    return true
  }
  if (speedTestMode.value === 'direct') {
    return false
  }
  return !props.config.proxy_host || !props.config.proxy_port
})

// 测速按钮禁用原因
const speedTestDisabledReason = computed(() => {
  if (!props.config.base_url) {
    return '请先配置租户地址'
  }
  if (!props.config.token) {
    return '请先配置 ACE Token'
  }
  if (speedTestMode.value === 'direct') {
    return ''
  }
  if (!props.config.proxy_host) {
    return '请先填写代理地址（或使用自动检测）'
  }
  if (!props.config.proxy_port) {
    return '请先填写代理端口'
  }
  return ''
})

// --- 方法 ---

/** 自动检测本地代理 */
async function detectProxy() {
  proxyDetecting.value = true
  detectedProxies.value = []
  try {
    const extraPorts = parseExtraPorts(extraDetectPortsText.value)
    const proxies = await invoke('detect_acemcp_proxy', {
      extraPorts,
    }) as DetectedProxy[]
    detectedProxies.value = proxies

    if (proxies.length === 0) {
      message.warning('未检测到本地代理，请手动输入')
    }
    else if (proxies.length === 1) {
      applyProxy(proxies[0])
      message.success(`已检测到代理 ${proxies[0].host}:${proxies[0].port}，建议测速验证`)
    }
    else {
      selectedProxyIndex.value = 0
      proxyPickerVisible.value = true
      message.success(`检测到 ${proxies.length} 个代理，请选择一个`)
    }
  }
  catch (err) {
    message.error(`代理检测失败: ${err}`)
  }
  finally {
    proxyDetecting.value = false
  }
}

function parseExtraPorts(input: string): number[] {
  const parts = (input || '')
    .split(/[,，\s]+/g)
    .map(s => s.trim())
    .filter(Boolean)

  const nums = parts
    .map(s => Number(s))
    .filter(n => Number.isInteger(n) && n >= 1 && n <= 65535)

  return Array.from(new Set(nums))
}

function applyProxy(p: DetectedProxy) {
  props.config.proxy_host = p.host
  props.config.proxy_port = p.port
  props.config.proxy_type = p.proxy_type as 'http' | 'https' | 'socks5'
}

function confirmProxySelection() {
  const p = detectedProxies.value[selectedProxyIndex.value]
  if (!p) {
    message.warning('请先选择一个代理')
    return
  }
  applyProxy(p)
  proxyPickerVisible.value = false
  message.success(`已选择代理 ${p.host}:${p.port}`)
}

async function loadIndexedProjectsForSpeedTest() {
  projectPickerLoading.value = true
  console.log('[SouProxy] 🔄 开始加载已索引项目列表...')

  try {
    const statusResult = await invoke<{ projects: Record<string, ProjectIndexStatusLite> }>('get_all_acemcp_index_status')

    // 详细日志：打印原始返回数据
    console.log('[SouProxy] 📦 后端返回原始数据:', statusResult)
    console.log('[SouProxy] 📊 项目总数（原始）:', Object.keys(statusResult.projects || {}).length)

    const allProjects = Object.values(statusResult.projects || {})
    console.log('[SouProxy] 📋 所有项目列表:', allProjects.map(p => ({
      root: p.project_root,
      status: p.status,
      total_files: p.total_files,
      last_success_time: p.last_success_time,
    })))

    // 过滤条件：保留已索引文件数 > 0 的项目
    // 注意：如果项目正在索引中（status: indexing），可能 total_files 还未更新
    const list = allProjects.filter((p) => {
      const hasFiles = (p.total_files || 0) > 0
      console.log(`[SouProxy] 📁 项目 ${getProjectName(p.project_root)}: total_files=${p.total_files}, status=${p.status}, 通过过滤=${hasFiles}`)
      return hasFiles
    })

    console.log('[SouProxy] ✅ 过滤后项目数:', list.length)
    console.log('[SouProxy] 📝 过滤后项目列表:', list.map(p => getProjectName(p.project_root)))

    indexedProjects.value = list
  }
  catch (e) {
    console.error('[SouProxy] ❌ 加载已索引项目失败:', e)
    message.error(`加载已索引项目失败: ${e}`)
    indexedProjects.value = []
  }
  finally {
    projectPickerLoading.value = false
  }
}

async function openProjectPicker() {
  await loadIndexedProjectsForSpeedTest()

  if (indexedProjects.value.length === 0) {
    dialog.warning({
      title: '需要索引项目',
      content: '测速功能需要至少一个已索引的项目。是否现在添加项目并开始索引？',
      positiveText: '是',
      negativeText: '否',
      onPositiveClick: () => {
        addProjectVisible.value = true
      },
    })
    return
  }

  projectPickerSelected.value = speedTestProjectRoot.value || indexedProjects.value[0].project_root

  // 强制确保有选中值，如果当前没有，则选中列表第一个
  if (!projectPickerSelected.value && indexedProjects.value.length > 0) {
    projectPickerSelected.value = indexedProjects.value[0].project_root
  }

  projectPickerVisible.value = true
}

async function confirmProjectSelectionAndRun() {
  if (!projectPickerSelected.value) {
    message.warning('请选择一个测试项目')
    return
  }

  speedTestProjectRoot.value = projectPickerSelected.value
  projectPickerVisible.value = false

  await runSpeedTest()
}

async function addProjectAndIndexAndRun() {
  const path = addProjectPath.value.trim()
  if (!path) {
    message.error('请输入项目根路径')
    return
  }

  addProjectIndexing.value = true
  try {
    const exists = await invoke<boolean>('check_directory_exists', {
      directoryPath: path,
    })

    if (!exists) {
      message.error('目录不存在或不可访问，请检查路径')
      return
    }

    await invoke<string>('trigger_acemcp_index_update', {
      projectRootPath: path,
    })

    message.success('索引完成')
    speedTestProjectRoot.value = path
    addProjectVisible.value = false
    addProjectPath.value = ''

    await runSpeedTest()
  }
  catch (e) {
    message.error(`索引失败: ${e}`)
  }
  finally {
    addProjectIndexing.value = false
  }
}

async function runSpeedTest() {
  // Config 校验
  if (!props.config.base_url) {
    message.error('请先配置租户地址')
    return
  }
  if (!props.config.token) {
    message.error('请先配置 ACE Token')
    return
  }
  if (!speedTestProjectRoot.value) {
    await openProjectPicker()
    return
  }

  proxyTesting.value = true
  speedTestResult.value = null
  speedTestProgress.value = '正在准备测速...'
  speedTestProgressData.value = null
  multiQuerySearchDetails.value = []
  multiQueryDetailsExpanded.value = false

  // 注册进度事件监听器
  unlistenSpeedTestProgress = await listen<SpeedTestProgress>('speed_test_progress', (event) => {
    const progress = event.payload
    speedTestProgressData.value = progress

    // 构建进度文本
    const statusIcon = progress.status === 'Running'
      ? '⏳'
      : progress.status === 'Completed'
        ? '✅'
        : progress.status === 'Failed'
          ? '❌'
          : '⏸️'

    const subStepText = progress.sub_step ? ` - ${progress.sub_step}` : ''
    const detailText = progress.detail ? ` (${progress.detail})` : ''

    speedTestProgress.value = `${statusIcon} ${progress.stage_name}${subStepText}${detailText} [${progress.percentage}%]`
  })

  try {
    const rawQueryCount = (speedTestQuery.value || '')
      .split(/\r?\n/g)
      .flatMap(line => line.split(';'))
      .map(s => s.trim())
      .filter(Boolean)
      .length

    if (rawQueryCount > 5) {
      message.info('测试查询过多，已按前 5 条执行')
    }

    const uploadMaxFiles = projectUploadMode.value === 'sample'
      ? Math.max(1, Number(projectUploadMaxFiles.value) || 200)
      : undefined

    const effectiveTestQuery = (speedTestQuery.value || '').trim()
      ? speedTestQuery.value
      : '代码搜索测试'

    const result = await invoke('test_acemcp_proxy_speed', {
      testMode: speedTestMode.value,
      proxyHost: props.config.proxy_host,
      proxyPort: props.config.proxy_port,
      proxyType: props.config.proxy_type,
      proxyUsername: props.config.proxy_username,
      proxyPassword: props.config.proxy_password,
      testQuery: effectiveTestQuery,
      projectRootPath: speedTestProjectRoot.value,
      projectUploadMode: projectUploadMode.value,
      projectUploadMaxFiles: uploadMaxFiles,
    }) as SpeedTestResult

    const effectiveQueries = speedTestQueries.value.length > 0
      ? speedTestQueries.value
      : ['代码搜索测试']

    const searchMetrics = (result.metrics || []).filter(m => m.metric_type === 'search')
    multiQuerySearchDetails.value = effectiveQueries.map((q, idx) => {
      const m = searchMetrics[idx]
      return {
        query: q,
        proxy_time_ms: m?.proxy_time_ms ?? null,
        direct_time_ms: m?.direct_time_ms ?? null,
        success: m?.success ?? false,
        error: m?.error ?? (m ? null : '未返回搜索指标'),
      }
    })

    const s = multiQuerySearchSummary.value
    if (s) {
      const avgMetric: SpeedTestMetric = {
        name: `🔎 语义搜索（${s.total} 条平均）`,
        metric_type: 'search_multi_avg',
        proxy_time_ms: s.proxy_avg_ms,
        direct_time_ms: s.direct_avg_ms,
        success: true,
        error: null,
      }

      if (speedTestMode.value !== 'direct' && s.proxy_ok === 0) {
        avgMetric.success = false
        avgMetric.error = '代理侧无有效搜索耗时（全部失败或未返回）'
      }
      if (speedTestMode.value !== 'proxy' && s.direct_ok === 0) {
        avgMetric.success = false
        avgMetric.error = [avgMetric.error, '直连侧无有效搜索耗时（全部失败或未返回）'].filter(Boolean).join('；')
      }

      result.metrics.push(avgMetric)
    }

    speedTestResult.value = result

    if (result.success) {
      message.success('测速完成')
    }
    else {
      message.warning('测速完成，部分测试失败')
    }
  }
  catch (err) {
    message.error(`测速失败: ${err}`)
  }
  finally {
    // 清理进度事件监听器
    if (unlistenSpeedTestProgress) {
      unlistenSpeedTestProgress()
      unlistenSpeedTestProgress = null
    }
    proxyTesting.value = false
    speedTestProgress.value = ''
    speedTestProgressData.value = null
  }
}

function buildSpeedTestReportPayload() {
  if (!speedTestResult.value) {
    return null
  }

  const uploadMaxFiles = projectUploadMode.value === 'sample'
    ? Math.max(1, Number(projectUploadMaxFiles.value) || 200)
    : undefined

  return {
    tool: 'sou',
    timestamp: speedTestResult.value.timestamp,
    mode: speedTestResult.value.mode,
    query: speedTestQuery.value,
    project: {
      root: speedTestProjectRoot.value,
      name: getProjectName(speedTestProjectRoot.value),
      upload_mode: projectUploadMode.value,
      upload_max_files: uploadMaxFiles,
    },
    proxy: speedTestResult.value.mode === 'direct'
      ? { enabled: false }
      : {
          enabled: true,
          type: props.config.proxy_type,
          host: props.config.proxy_host,
          port: props.config.proxy_port,
          username: props.config.proxy_username || undefined,
          password_set: Boolean(props.config.proxy_password),
        },
    config: {
      base_url: props.config.base_url,
      token_set: Boolean(props.config.token),
    },
    result: speedTestResult.value,
  }
}

async function copySpeedTestReport() {
  const report = buildSpeedTestReportPayload()
  if (!report) {
    message.warning('暂无测速结果可复制')
    return
  }

  try {
    await navigator.clipboard.writeText(JSON.stringify(report, null, 2))
    message.success('已复制测速报告（JSON）')
  }
  catch (e) {
    message.error(`复制失败: ${e}`)
  }
}

async function copyQueryDetail(detail: SpeedTestQueryDetail, idx: number) {
  if (!speedTestResult.value) {
    message.warning('暂无测速结果可复制')
    return
  }
  // 构造简略 payload
  const payload = {
    query: detail.query,
    proxy_ms: detail.proxy_time_ms,
    direct_ms: detail.direct_time_ms,
    success: detail.success,
    error: detail.error,
  }
  try {
    await navigator.clipboard.writeText(JSON.stringify(payload, null, 2))
    message.success(`已复制 Q${idx + 1} 明细`)
  }
  catch (e) {
    message.error(`复制失败: ${e}`)
  }
}

async function copyMetricResult(metric: SpeedTestMetric) {
  try {
    await navigator.clipboard.writeText(JSON.stringify(metric, null, 2))
    message.success(`已复制指标 "${metric.name}" 结果`)
  }
  catch (e) {
    message.error(`复制失败: ${e}`)
  }
}

async function downloadSpeedTestReport() {
  const report = buildSpeedTestReportPayload()
  if (!report) {
    message.warning('暂无测速结果可导出')
    return
  }

  try {
    const ts = speedTestResult.value?.timestamp || new Date().toISOString()
    const safeTs = ts.replace(/[:.]/g, '-').replace('T', '_').replace('Z', '')
    const filename = `sou-speedtest-${safeTs}.json`

    const blob = new Blob([JSON.stringify(report, null, 2)], { type: 'application/json;charset=utf-8' })
    const url = URL.createObjectURL(blob)

    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()

    setTimeout(() => URL.revokeObjectURL(url), 0)
    message.success(`已导出测速报告: ${filename}`)
  }
  catch (e) {
    message.error(`导出失败: ${e}`)
  }
}

// 辅助函数
function getProjectName(projectRoot: string): string {
  const normalizedPath = normalizePathForDisplay(projectRoot)
  const parts = (normalizedPath || '').replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : normalizedPath
}

function formatIndexTime(ts: string | null): string {
  if (!ts)
    return '未完成'
  try { return new Date(ts).toLocaleString() }
  catch { return ts }
}

function formatSpeedTestTime(ts: string): string {
  if (!ts)
    return '-'
  try { return new Date(ts).toLocaleString() }
  catch { return ts }
}

function calcDiff(proxyMs: number | null, directMs: number | null): string {
  if (proxyMs === null || directMs === null)
    return '-'
  if (directMs === 0)
    return '-'
  const diff = ((directMs - proxyMs) / directMs * 100).toFixed(0)
  if (Number(diff) > 0)
    return `⬇️${diff}%`
  if (Number(diff) < 0)
    return `⬆️${Math.abs(Number(diff))}%`
  return '0%'
}

// 获取差异颜色类名（主题适配）
function getDiffColorClass(proxyMs: number | null, directMs: number | null): string {
  if (proxyMs === null || directMs === null)
    return 'bg-gray-100 dark:bg-gray-800 text-gray-500'
  if (proxyMs < directMs)
    return 'bg-green-100 dark:bg-green-900/30 text-green-600 dark:text-green-400'
  if (proxyMs > directMs)
    return 'bg-red-100 dark:bg-red-900/30 text-red-600 dark:text-red-400'
  return 'bg-gray-100 dark:bg-gray-800 text-gray-500'
}

// 获取进度步骤状态（用于步骤指示器）
function getStepStatus(stepName: string): 'pending' | 'current' | 'completed' {
  const currentStage = speedTestProgressData.value?.stage ?? -1
  const stageMap: Record<string, number> = {
    初始化: 0,
    Ping: 1,
    搜索: 2,
    单文件: 3,
    项目: 4,
    报告: 5,
  }
  const stepStage = stageMap[stepName] ?? -1

  if (stepStage < currentStage)
    return 'completed'
  if (stepStage === currentStage)
    return 'current'
  return 'pending'
}

// 格式化字节数为可读字符串
function formatBytes(bytes: number): string {
  if (bytes === 0)
    return '0B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  const size = bytes / k ** i
  return `${size.toFixed(i > 0 ? 1 : 0)}${sizes[i]}`
}

// 规范化路径显示，移除Windows长路径前缀
function normalizePathForDisplay(path: string): string {
  if (!path)
    return path
  // 移除 Windows 长路径前缀 \\?\ 或 //?/
  return path.replace(/^\\\\\?\\|^\/\?\//, '')
}

function formatRelativeTime(timeStr: string | null): string {
  if (!timeStr)
    return '从未'
  try {
    const date = new Date(timeStr)
    const now = new Date()
    const diffMs = now.getTime() - date.getTime()
    const diffSec = Math.floor(diffMs / 1000)
    const diffMin = Math.floor(diffSec / 60)
    const diffHour = Math.floor(diffMin / 60)
    const diffDay = Math.floor(diffHour / 24)

    if (diffSec < 60)
      return '刚刚'
    if (diffMin < 60)
      return `${diffMin} 分钟前`
    if (diffHour < 24)
      return `${diffHour} 小时前`
    if (diffDay < 30)
      return `${diffDay} 天前`
    return date.toLocaleDateString()
  }
  catch {
    return '未知'
  }
}
</script>

<template>
  <n-modal
    v-model:show="showModal"
    class="custom-modal"
    preset="card"
    title="代理设置与网络诊断"
    :style="{ width: '900px', maxWidth: '95vw' }"
    :bordered="false"
    size="medium"
    role="dialog"
    aria-modal="true"
  >
    <div class="modal-content-wrapper">
      <!-- 顶部状态栏 -->
      <div class="mb-5 p-4 rounded-xl bg-gradient-to-r from-slate-50 to-slate-100 dark:from-slate-800 dark:to-slate-900/50 border border-slate-200 dark:border-slate-700 flex items-center justify-between">
        <div class="flex items-center gap-4">
          <div class="p-2 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400">
            <div class="i-fa6-solid-network-wired text-2xl" />
          </div>
          <div>
            <div class="font-medium text-base mb-1">
              启用代理服务
            </div>
            <div class="text-xs text-gray-500 text-gray-400">
              启用后，所有 ACE API 请求将通过此代理。
              <div v-if="!config.proxy_enabled" class="inline-block mt-1 px-1.5 py-0.5 rounded bg-orange-50 dark:bg-orange-900/30 text-orange-600 dark:text-orange-400 text-[10px]">
                当前直接连接
              </div>
              <div v-else class="inline-block mt-1 px-1.5 py-0.5 rounded bg-green-50 dark:bg-green-900/30 text-green-600 dark:text-green-400 text-[10px]">
                代理已启用 ({{ config.proxy_type.toUpperCase() }}://{{ config.proxy_host }}:{{ config.proxy_port }})
              </div>
            </div>
          </div>
        </div>
        <n-switch v-model:value="config.proxy_enabled" size="large">
          <template #checked>
            开启
          </template>
          <template #unchecked>
            关闭
          </template>
        </n-switch>
      </div>

      <n-tabs type="segment" animated>
        <!-- Tab 1: 代理配置 -->
        <n-tab-pane name="config" tab="配置参数">
          <n-space vertical size="large" class="pt-2">
            <!-- 代理表单 -->
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
              <!-- 基础信息 -->
              <div class="md:col-span-2">
                <div class="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
                  基础连接
                </div>
                <div class="grid grid-cols-12 gap-3">
                  <div class="col-span-12 md:col-span-5">
                    <n-form-item label="Host (地址)" size="small">
                      <n-input v-model:value="config.proxy_host" placeholder="127.0.0.1" clearable />
                    </n-form-item>
                  </div>
                  <div class="col-span-12 md:col-span-3">
                    <n-form-item label="Port (端口)" size="small">
                      <n-input-number v-model:value="config.proxy_port" :min="1" :max="65535" class="w-full" :show-button="false" />
                    </n-form-item>
                  </div>
                  <div class="col-span-12 md:col-span-4">
                    <n-form-item label="Type (类型)" size="small">
                      <n-select v-model:value="config.proxy_type" :options="[{ label: 'HTTP', value: 'http' }, { label: 'HTTPS', value: 'https' }, { label: 'SOCKS5', value: 'socks5' }]" />
                    </n-form-item>
                  </div>
                </div>
              </div>

              <!-- 认证信息 -->
              <div class="md:col-span-2">
                <div class="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2 mt-2">
                  身份认证 (可选)
                </div>
                <div class="grid grid-cols-12 gap-3">
                  <div class="col-span-12 md:col-span-6">
                    <n-form-item label="用户名" size="small">
                      <n-input v-model:value="config.proxy_username" placeholder="无" clearable />
                    </n-form-item>
                  </div>
                  <div class="col-span-12 md:col-span-6">
                    <n-form-item label="密码" size="small">
                      <n-input v-model:value="config.proxy_password" type="password" show-password-on="click" placeholder="无" clearable />
                    </n-form-item>
                  </div>
                </div>
              </div>
            </div>

            <!-- 检测区域 -->
            <div class="p-4 rounded-lg border border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-800/30">
              <div class="flex items-center justify-between mb-3">
                <div class="flex flex-col">
                  <div class="text-sm font-medium">
                    自动检测本地代理
                  </div>
                  <div class="text-xs text-gray-500">
                    自动扫描常用端口和进程
                  </div>
                </div>
                <n-button secondary size="small" :loading="proxyDetecting" @click="detectProxy">
                  <template #icon>
                    <div class="i-fa6-solid-satellite-dish" />
                  </template>
                  开始检测
                </n-button>
              </div>

              <n-form-item label="额外扫描端口 (可选)" label-placement="left" size="small" :show-feedback="false">
                <n-input v-model:value="extraDetectPortsText" placeholder="8888, 8081" class="max-w-[300px]" />
              </n-form-item>

              <!-- 检测结果展示 -->
              <n-collapse-transition :show="detectedProxies.length > 0">
                <div class="mt-3 flex flex-wrap gap-2">
                  <div
                    v-for="(p, idx) in detectedProxies" :key="idx"
                    class="px-3 py-1.5 rounded-full text-xs font-mono cursor-pointer border transition-colors flex items-center gap-2"
                    :class="config.proxy_port === p.port ? 'bg-blue-100 text-blue-700 border-blue-200 dark:bg-blue-900/40 dark:text-blue-300 dark:border-blue-700' : 'bg-slate-50 text-gray-600 border-slate-200 hover:bg-slate-100 dark:bg-slate-800 text-gray-300 dark:border-slate-700'"
                    @click="applyProxy(p)"
                  >
                    <span>{{ p.host }}:{{ p.port }}</span>
                    <span class="opacity-70">{{ p.proxy_type.toUpperCase() }}</span>
                    <span v-if="p.response_time_ms" class="px-1 rounded bg-black/10 dark:bg-white/20">{{ p.response_time_ms }}ms</span>
                  </div>
                </div>
              </n-collapse-transition>
            </div>
          </n-space>
        </n-tab-pane>

        <!-- Tab 2: 测速与诊断 - 左右分栏布局 -->
        <n-tab-pane name="speedtest" tab="网络测速与诊断">
          <div class="grid grid-cols-12 gap-5 pt-2 min-h-[400px]">
            <!-- 左侧：测试控制区 (40%) -->
            <div class="col-span-12 lg:col-span-5 space-y-5">
              <!-- 测试模式选择 -->
              <div class="space-y-2">
                <div class="text-xs font-semibold text-gray-500 dropdown-label flex items-center gap-1">
                  <div class="i-fa6-solid-gauge-high" />
                  测试模式
                </div>
                <n-select
                  v-model:value="speedTestMode"
                  :options="[
                    { label: '🔥 对比测试 (代理 vs 直连)', value: 'compare' },
                    { label: '🛡️ 仅代理模式', value: 'proxy' },
                    { label: '🌐 仅直连模式', value: 'direct' },
                  ]"
                />
              </div>

              <!-- 测试项目选择 (卡片式) -->
              <div class="space-y-2">
                <div class="text-xs font-semibold text-gray-500 dropdown-label flex items-center justify-between">
                  <div class="flex items-center gap-1">
                    <div class="i-fa6-solid-folder-tree" />
                    测试目标项目
                  </div>
                  <n-button v-if="currentProjectInfo" text size="tiny" type="primary" @click="openProjectPicker">
                    切换
                  </n-button>
                </div>

                <!-- 已选择状态 -->
                <div
                  v-if="currentProjectInfo"
                  class="group relative overflow-hidden rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 p-4 cursor-pointer transition-all hover:border-primary-400 hover:shadow-md"
                  @click="openProjectPicker"
                >
                  <div class="absolute top-0 right-0 p-2 opacity-10 group-hover:opacity-20 transition-opacity">
                    <div class="i-fa6-solid-folder-open text-6xl text-primary-500" />
                  </div>

                  <div class="relative z-10 flex items-start gap-3">
                    <div class="w-10 h-10 rounded-lg bg-primary-100 dark:bg-primary-900/40 flex items-center justify-center flex-shrink-0 text-primary-600 dark:text-primary-400">
                      <div class="i-fa6-solid-code" />
                    </div>
                    <div class="flex-1 min-w-0">
                      <div class="font-medium text-base text-gray-800 text-gray-100 truncate">
                        {{ getProjectName(currentProjectInfo.project_root) }}
                      </div>
                      <div class="text-xs text-gray-500 truncate font-mono mt-0.5" :title="currentProjectInfo.project_root">
                        {{ normalizePathForDisplay(currentProjectInfo.project_root) }}
                      </div>
                      <div class="flex items-center gap-3 mt-2 text-xs text-gray-400">
                        <span class="flex items-center gap-1 bg-slate-100 dark:bg-slate-700/50 px-1.5 py-0.5 rounded">
                          <div class="i-fa6-solid-file-lines text-[10px]" />
                          {{ currentProjectInfo.total_files }} 文件
                        </span>
                        <span v-if="currentProjectInfo.last_success_time" class="flex items-center gap-1">
                          <div class="i-fa6-regular-clock text-[10px]" />
                          {{ formatRelativeTime(currentProjectInfo.last_success_time) }}
                        </span>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 未选择状态 -->
                <div
                  v-else
                  class="border-2 border-dashed border-slate-300 dark:border-slate-600 hover:border-primary-400 dark:hover:border-primary-500 hover:bg-slate-50 dark:hover:bg-slate-800/50 rounded-xl p-6 flex flex-col items-center justify-center cursor-pointer transition-all text-gray-400 hover:text-primary-500 group"
                  @click="openProjectPicker"
                >
                  <div class="i-fa6-solid-folder-plus text-3xl mb-2 group-hover:scale-110 transition-transform" />
                  <div class="text-sm font-medium">
                    点击选择测试项目
                  </div>
                </div>
              </div>

              <!-- 查询语句 -->
              <div class="space-y-2">
                <div class="flex items-center justify-between text-xs font-semibold text-gray-500 dropdown-label">
                  <div class="flex items-center gap-1">
                    <div class="i-fa6-solid-magnifying-glass" />
                    测试查询语句
                  </div>
                  <span class="font-normal opacity-70">最多5条</span>
                </div>
                <n-input
                  v-model:value="speedTestQuery"
                  type="textarea"
                  :rows="3"
                  placeholder="输入语义查询，如：'查找数据库连接配置'..."
                  class="text-sm"
                />
              </div>

              <!-- 开始测速按钮 -->
              <n-tooltip :disabled="!speedTestDisabled">
                <template #trigger>
                  <n-button
                    type="primary"
                    block
                    size="large"
                    :loading="proxyTesting"
                    :disabled="speedTestDisabled"
                    class="h-12 text-base font-medium shadow-lg shadow-primary-500/20"
                    @click="runSpeedTest"
                  >
                    <template #icon>
                      <div class="i-fa6-solid-jet-fighter" />
                    </template>
                    {{ proxyTesting ? '全速诊断中...' : '开始网络诊断' }}
                  </n-button>
                </template>
                {{ speedTestDisabledReason }}
              </n-tooltip>

              <div v-if="proxyTesting" class="space-y-3">
                <!-- 进度头部 -->
                <div class="flex justify-between items-center text-xs">
                  <span class="text-gray-500 font-medium">诊断进度</span>
                  <span class="font-mono text-primary-600 text-primary-400">
                    {{ speedTestProgressData?.percentage ?? 0 }}%
                  </span>
                </div>

                <!-- 进度条 -->
                <n-progress
                  type="line"
                  :percentage="speedTestProgressData?.percentage ?? 5"
                  :show-indicator="false"
                  :processing="speedTestProgressData?.status === 'Running'"
                  :status="speedTestProgressData?.status === 'Failed' ? 'error' : 'success'"
                  class="h-2"
                />

                <!-- 当前阶段信息 -->
                <div class="bg-slate-50 dark:bg-slate-800/50 rounded-lg p-3 space-y-1">
                  <div class="flex items-center gap-2 text-sm">
                    <span
                      v-if="speedTestProgressData?.status === 'Running'"
                      class="i-fa6-solid-spinner animate-spin text-primary-500"
                    />
                    <span
                      v-else-if="speedTestProgressData?.status === 'Completed'"
                      class="i-fa6-solid-circle-check text-green-500"
                    />
                    <span
                      v-else-if="speedTestProgressData?.status === 'Failed'"
                      class="i-fa6-solid-circle-xmark text-red-500"
                    />
                    <span v-else class="i-fa6-regular-clock text-gray-400 dark:text-gray-500" />

                    <span class="font-medium text-gray-700 text-gray-200">
                      {{ speedTestProgressData?.stage_name ?? '初始化' }}
                    </span>
                    <span v-if="speedTestProgressData?.sub_step" class="text-gray-400">
                      - {{ speedTestProgressData.sub_step }}
                    </span>
                  </div>

                  <div v-if="speedTestProgressData?.detail" class="text-xs text-gray-500 dark:text-gray-400 pl-6">
                    {{ speedTestProgressData.detail }}
                  </div>
                </div>

                <!-- 进度步骤指示器 -->
                <div class="flex justify-between px-1">
                  <div v-for="step in ['初始化', 'Ping', '搜索', '单文件', '项目', '报告']" :key="step" class="flex flex-col items-center">
                    <div
                      class="w-2 h-2 rounded-full transition-all" :class="[
                        getStepStatus(step) === 'completed' ? 'bg-green-500 scale-125'
                        : getStepStatus(step) === 'current' ? 'bg-primary-500 animate-pulse scale-110'
                          : 'bg-gray-300 dark:bg-gray-600',
                      ]"
                    />
                    <span class="text-[10px] text-gray-400 mt-1">{{ step }}</span>
                  </div>
                </div>
              </div>
            </div>

            <!-- 右侧：测试结果区 (60%) -->
            <div class="col-span-12 lg:col-span-7 h-full flex flex-col">
              <!-- 无结果时的占位状态 -->
              <div
                v-if="!speedTestResult && !proxyTesting"
                class="flex-1 flex flex-col items-center justify-center p-8 rounded-2xl border border-slate-200 dark:border-slate-700 bg-slate-50/50 dark:bg-slate-800/20"
              >
                <div class="relative mb-6">
                  <div class="absolute inset-0 bg-blue-500/20 blur-xl rounded-full" />
                  <div class="relative i-fa6-solid-chart-simple text-6xl text-slate-300 dark:text-slate-600" />
                </div>
                <div class="text-center max-w-xs">
                  <div class="text-base font-medium text-slate-500 dark:text-slate-400 mb-2">
                    准备就绪
                  </div>
                  <div class="text-xs text-slate-400">
                    请在左侧配置测试参数，点击「开始网络诊断」获取详细的延迟与连通性分析报告。
                  </div>
                </div>
              </div>

              <!-- 加载骨架屏 -->
              <div v-else-if="proxyTesting && !speedTestResult" class="space-y-4 p-4">
                <div class="flex items-center gap-4 mb-6">
                  <n-skeleton circle width="48px" height="48px" />
                  <div class="flex-1 space-y-2">
                    <n-skeleton height="20px" width="60%" />
                    <n-skeleton height="14px" width="40%" />
                  </div>
                </div>
                <div class="grid grid-cols-2 gap-4">
                  <n-skeleton height="120px" :sharp="false" class="rounded-xl" />
                  <n-skeleton height="120px" :sharp="false" class="rounded-xl" />
                </div>
                <n-skeleton height="200px" :sharp="false" class="rounded-xl mt-4" />
              </div>

              <!-- 测试结果展示 -->
              <div v-if="speedTestResult" class="flex-1 flex flex-col bg-white dark:bg-slate-800 border border-slate-200 dark:border-slate-700 rounded-2xl overflow-hidden shadow-sm">
                <!-- 结果头部 Banner -->
                <div class="relative overflow-hidden p-5 flex items-center justify-between border-b border-slate-100 dark:border-slate-700/50">
                  <!-- 背景装饰 -->
                  <div
                    class="absolute inset-0 opacity-10 pointer-events-none"
                    :class="speedTestResult.success ? 'bg-green-500' : 'bg-amber-500'"
                  />

                  <div class="relative flex items-center gap-4">
                    <div
                      class="w-12 h-12 rounded-full flex items-center justify-center shadow-sm text-2xl"
                      :class="speedTestResult.success
                        ? 'bg-green-100 dark:bg-green-500/20 text-green-600 dark:text-green-400'
                        : 'bg-amber-100 dark:bg-amber-500/20 text-amber-600 dark:text-amber-400'"
                    >
                      <div :class="speedTestResult.success ? 'i-fa6-solid-check' : 'i-fa6-solid-triangle-exclamation'" />
                    </div>
                    <div>
                      <div class="font-bold text-lg leading-none mb-1">
                        {{ speedTestResult.success ? '测试通过' : '发现问题' }}
                      </div>
                      <div class="text-xs text-gray-500 font-mono">
                        TIME: {{ formatSpeedTestTime(speedTestResult.timestamp) }}
                      </div>
                    </div>
                  </div>

                  <div class="relative flex gap-2">
                    <n-button size="small" secondary @click="copySpeedTestReport">
                      复制报告
                    </n-button>
                    <n-button size="small" secondary @click="downloadSpeedTestReport">
                      <template #icon>
                        <div class="i-fa6-solid-download" />
                      </template>
                    </n-button>
                  </div>
                </div>

                <!-- Tabs 内容区 - 优化背景确保文字可读 -->
                <n-tabs type="line" animated class="flex-1 flex flex-col results-tabs" pane-class="flex-1 p-4 overflow-y-auto max-h-[500px] bg-slate-50 dark:bg-slate-900/50">
                  <!-- Tab 1: 核心指标 -->
                  <n-tab-pane name="overview" tab="📊 核心指标">
                    <div class="space-y-4">
                      <!-- 建议 Box -->
                      <div v-if="speedTestResult.recommendation" class="flex gap-3 p-4 rounded-xl bg-slate-50 dark:bg-slate-700/30 border border-slate-100 dark:border-slate-700">
                        <div class="i-fa6-solid-wand-magic-sparkles text-purple-500 mt-1" />
                        <div class="text-sm text-gray-700 text-gray-200">
                          <span class="font-bold block mb-1">智能诊断建议</span>
                          {{ speedTestResult.recommendation }}
                        </div>
                      </div>

                      <!-- 指标卡片网格 -->
                      <div class="grid grid-cols-2 gap-4">
                        <div
                          v-for="(metric, idx) in speedTestMetricsForDisplay"
                          :key="idx"
                          class="group relative p-4 rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 hover:border-blue-400 transition-all duration-200 shadow-sm"
                        >
                          <!-- 标题 - 使用高对比度颜色确保文字清晰 -->
                          <div class="flex justify-between items-start mb-4">
                            <span class="font-semibold text-sm text-gray-800 text-gray-100">{{ metric.name }}</span>
                            <div v-if="metric.success" class="i-fa6-solid-circle-check text-green-500" />
                            <div v-else class="i-fa6-solid-circle-xmark text-red-500" />
                          </div>

                          <!-- 数据 -->
                          <div class="flex items-end justify-between font-mono text-sm">
                            <div v-if="speedTestResult.mode !== 'direct'" class="flex-1">
                              <div class="text-xs text-gray-400 mb-1">
                                Proxy
                              </div>
                              <div class="text-xl font-bold" :class="metric.proxy_time_ms ? 'text-blue-600 dark:text-blue-400' : 'text-gray-300'">
                                {{ metric.proxy_time_ms ?? '-' }}<span class="text-xs font-normal text-gray-400">ms</span>
                              </div>
                            </div>

                            <div v-if="speedTestResult.mode === 'compare'" class="px-2 pb-1">
                              <div class="text-xs font-bold px-2 py-0.5 rounded-full" :class="getDiffColorClass(metric.proxy_time_ms, metric.direct_time_ms)">
                                {{ calcDiff(metric.proxy_time_ms, metric.direct_time_ms) }}
                              </div>
                            </div>

                            <div v-if="speedTestResult.mode !== 'proxy'" class="flex-1 text-right">
                              <div class="text-xs text-gray-400 mb-1">
                                Direct
                              </div>
                              <div class="text-xl font-bold" :class="metric.direct_time_ms ? 'text-purple-600 dark:text-purple-400' : 'text-gray-300'">
                                {{ metric.direct_time_ms ?? '-' }}<span class="text-xs font-normal text-gray-400">ms</span>
                              </div>
                            </div>
                          </div>

                          <!-- 错误提示 -->
                          <div v-if="metric.error" class="mt-3 text-xs text-red-500 bg-red-50 dark:bg-red-900/10 p-2 rounded">
                            {{ metric.error }}
                          </div>
                        </div>
                      </div>

                      <!-- 搜索详情列表 -->
                      <div v-if="multiQuerySearchSummary" class="mt-4">
                        <div class="text-xs font-semibold text-gray-500 mb-2 uppercase tracking-wider">
                          Search Queries
                        </div>
                        <div class="space-y-2">
                          <div v-for="(d, i) in multiQuerySearchDetails" :key="i" class="flex items-center justify-between p-3 rounded-lg bg-slate-50 dark:bg-slate-700/30 border border-slate-100 dark:border-slate-700">
                            <div class="flex items-center gap-2 truncate flex-1">
                              <div class="i-fa6-solid-terminal text-gray-400 text-xs" />
                              <span class="text-xs font-mono truncate" :title="d.query">{{ d.query }}</span>
                            </div>
                            <div class="flex gap-3 text-xs font-mono ml-4">
                              <span v-if="d.proxy_time_ms" class="text-blue-600">{{ d.proxy_time_ms }}ms</span>
                              <span v-if="d.direct_time_ms" class="text-purple-600">{{ d.direct_time_ms }}ms</span>
                            </div>
                          </div>
                        </div>
                      </div>
                    </div>
                  </n-tab-pane>

                  <!-- Tab 2: 完整诊断数据 -->
                  <n-tab-pane name="raw" tab="🛠️ 诊断数据">
                    <div class="space-y-4">
                      <n-alert title="数据说明" type="info" :bordered="false" class="mb-2">
                        以下展示测试过程中的完整配置上下文与后端返回的原始指标数据结构。
                      </n-alert>

                      <div>
                        <div class="flex items-center justify-between mb-2">
                          <span class="text-xs font-bold text-gray-500">REQUEST CONTEXT</span>
                          <n-tag size="tiny">
                            JSON
                          </n-tag>
                        </div>
                        <div class="bg-slate-50 dark:bg-slate-900 rounded-lg border border-slate-200 dark:border-slate-700 p-1">
                          <n-code
                            :code="JSON.stringify({
                              mode: speedTestMode,
                              query: speedTestQuery,
                              project: currentProjectInfo ? { root: currentProjectInfo.project_root, files: currentProjectInfo.total_files } : null,
                              timestamp: new Date().toISOString(),
                            }, null, 2)"
                            language="json"
                            class="text-xs font-mono"
                            style="max-height: 200px; overflow: auto;"
                          />
                        </div>
                      </div>

                      <div>
                        <div class="flex items-center justify-between mb-2">
                          <span class="text-xs font-bold text-gray-500">RESPONSE METRICS (RAW)</span>
                          <n-button size="tiny" text type="primary" @click="copySpeedTestReport">
                            复制完整JSON
                          </n-button>
                        </div>
                        <div class="bg-slate-50 dark:bg-slate-900 rounded-lg border border-slate-200 dark:border-slate-700 p-1">
                          <n-code
                            :code="JSON.stringify(speedTestResult, null, 2)"
                            language="json"
                            class="text-xs font-mono"
                          />
                        </div>
                      </div>
                    </div>
                  </n-tab-pane>

                  <!-- Tab 3: 搜索数据 -->
                  <n-tab-pane name="search-data" tab="🔍 搜索数据">
                    <div class="space-y-4">
                      <!-- 搜索结果预览卡片 -->
                      <template v-for="(metric, idx) in speedTestResult.metrics.filter(m => m.metric_type === 'search' && m.search_result_preview)" :key="idx">
                        <div class="rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 overflow-hidden">
                          <!-- 搜索指标头部 -->
                          <div class="px-4 py-3 border-b border-slate-100 dark:border-slate-700/50 bg-slate-50 dark:bg-slate-800/50 flex items-center justify-between">
                            <div class="flex items-center gap-2">
                              <div class="i-fa6-solid-magnifying-glass text-blue-500" />
                              <span class="font-medium text-sm text-gray-800 text-gray-100">{{ metric.name }}</span>
                            </div>
                            <div class="flex items-center gap-3 text-xs font-mono">
                              <span class="text-gray-400">匹配: {{ metric.search_result_preview?.total_matches || 0 }}</span>
                              <span class="text-gray-400">响应: {{ formatBytes(metric.search_result_preview?.response_length || 0) }}</span>
                            </div>
                          </div>

                          <!-- 搜索结果片段列表 -->
                          <div class="divide-y divide-slate-100 dark:divide-slate-700/50">
                            <template v-if="metric.search_result_preview?.snippets?.length">
                              <div
                                v-for="(snippet, i) in metric.search_result_preview.snippets"
                                :key="i"
                                class="p-4 hover:bg-slate-50 dark:hover:bg-slate-700/30 transition-colors"
                              >
                                <!-- 文件路径 -->
                                <div class="flex items-center gap-2 mb-2">
                                  <div class="i-fa6-solid-file-code text-xs text-gray-400" />
                                  <span class="text-xs font-mono text-blue-600 dark:text-blue-400 truncate" :title="snippet.file_path">
                                    {{ snippet.file_path }}
                                  </span>
                                  <span v-if="snippet.line_number" class="text-[10px] text-gray-400 bg-slate-100 dark:bg-slate-700 px-1.5 py-0.5 rounded">
                                    L{{ snippet.line_number }}
                                  </span>
                                </div>
                                <!-- 代码片段 -->
                                <div class="bg-slate-900 dark:bg-slate-950 rounded-lg p-3 overflow-x-auto">
                                  <pre class="text-xs font-mono text-slate-300 whitespace-pre-wrap break-all">{{ snippet.snippet }}</pre>
                                </div>
                              </div>
                            </template>
                            <div v-else class="p-8 text-center text-gray-400">
                              <div class="i-fa6-solid-inbox text-3xl mb-2 opacity-50" />
                              <div class="text-sm">
                                未获取到搜索结果预览
                              </div>
                            </div>
                          </div>
                        </div>
                      </template>

                      <!-- 无搜索指标时的占位 -->
                      <div v-if="!speedTestResult.metrics.some(m => m.metric_type === 'search' && m.search_result_preview)" class="py-12 text-center text-gray-400">
                        <div class="i-fa6-solid-search text-4xl mb-3 opacity-30" />
                        <div class="text-sm font-medium mb-1">
                          暂无搜索数据
                        </div>
                        <div class="text-xs opacity-70">
                          运行包含语义搜索的测试后，将在此处展示搜索结果预览
                        </div>
                      </div>
                    </div>
                  </n-tab-pane>
                </n-tabs>
              </div>
            </div>
          </div>
        </n-tab-pane>
      </n-tabs>
    </div>

    <!-- 子弹窗：多代理选择 -->
    <n-modal v-model:show="proxyPickerVisible" preset="card" style="width: 480px" size="small" :bordered="false">
      <template #header>
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-xl bg-primary-100 dark:bg-primary-900/30 flex items-center justify-center">
            <div class="i-fa6-solid-network-wired text-primary-600 dark:text-primary-400 text-lg" />
          </div>
          <div>
            <div class="font-semibold text-base">
              选择代理服务器
            </div>
            <div class="text-xs text-gray-500">
              已检测到 {{ detectedProxies.length }} 个可用代理
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-2 max-h-[300px] overflow-y-auto pr-1">
        <div
          v-for="(p, idx) in detectedProxies"
          :key="idx"
          class="group p-4 rounded-xl border-2 cursor-pointer transition-all duration-200"
          :class="selectedProxyIndex === idx
            ? 'border-primary-500 bg-primary-50 dark:bg-primary-900/20'
            : 'border-slate-200 dark:border-slate-700 bg-slate-50 dark:bg-slate-800/50 hover:border-primary-300 dark:hover:border-primary-600'"
          @click="selectedProxyIndex = idx"
        >
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-3">
              <!-- 选中指示器 -->
              <div
                class="w-5 h-5 rounded-full border-2 flex items-center justify-center transition-colors"
                :class="selectedProxyIndex === idx
                  ? 'border-primary-500 bg-primary-500'
                  : 'border-slate-300 dark:border-slate-600'"
              >
                <div v-if="selectedProxyIndex === idx" class="i-fa6-solid-check text-white text-xs" />
              </div>
              <div>
                <div class="font-mono font-medium text-sm text-gray-800 text-gray-200">
                  {{ p.host }}:{{ p.port }}
                </div>
                <div class="text-xs text-gray-500 mt-0.5">
                  {{ p.proxy_type.toUpperCase() }} 代理
                </div>
              </div>
            </div>
            <!-- 响应时间徽章 -->
            <div
              class="px-2.5 py-1 rounded-full text-xs font-medium"
              :class="p.response_time_ms && p.response_time_ms < 100
                ? 'bg-green-100 dark:bg-green-900/30 text-green-600 dark:text-green-400'
                : p.response_time_ms && p.response_time_ms < 300
                  ? 'bg-amber-100 dark:bg-amber-900/30 text-amber-600 dark:text-amber-400'
                  : 'bg-slate-100 dark:bg-slate-700 text-slate-600 dark:text-slate-300'"
            >
              <div class="i-fa6-solid-bolt inline-block mr-1" />
              {{ p.response_time_ms ?? '-' }}ms
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <div class="flex justify-end gap-3">
          <n-button secondary @click="proxyPickerVisible = false">
            取消
          </n-button>
          <n-button type="primary" @click="confirmProxySelection">
            <template #icon>
              <div class="i-fa6-solid-check" />
            </template>
            确认选择
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- 子弹窗：项目选择器 -->
    <n-modal v-model:show="projectPickerVisible" preset="card" style="width: 700px" size="medium" :bordered="false" class="custom-picker-modal">
      <template #header>
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-xl bg-gradient-to-br from-blue-500 to-indigo-600 flex items-center justify-center shadow-lg shadow-blue-500/30">
            <div class="i-fa6-solid-folder-tree text-white text-lg" />
          </div>
          <div>
            <div class="font-bold text-lg leading-tight">
              选择测试项目
            </div>
            <div class="text-xs text-gray-500 mt-1">
              请选择一个已索引的代码库进行网络延迟测试
            </div>
          </div>
        </div>
      </template>

      <!-- 加载状态 -->
      <div v-if="projectPickerLoading" class="grid grid-cols-1 md:grid-cols-2 gap-4 py-4">
        <n-skeleton height="100px" :sharp="false" class="rounded-xl" />
        <n-skeleton height="100px" :sharp="false" class="rounded-xl" />
        <n-skeleton height="100px" :sharp="false" class="rounded-xl" />
        <n-skeleton height="100px" :sharp="false" class="rounded-xl" />
      </div>

      <!-- 项目列表 Grid -->
      <div v-else class="grid grid-cols-1 md:grid-cols-2 gap-4 max-h-[450px] overflow-y-auto p-1">
        <div
          v-for="p in indexedProjects"
          :key="p.project_root"
          class="group relative overflow-hidden rounded-xl border-2 transition-all duration-300 cursor-pointer p-4 flex flex-col gap-2"
          :class="projectPickerSelected === p.project_root
            ? 'border-primary-500 bg-primary-50 dark:bg-slate-800 ring-2 ring-primary-200 dark:ring-primary-900'
            : 'border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 hover:border-primary-300 dark:hover:border-primary-600 hover:shadow-md'"
          @click="projectPickerSelected = p.project_root"
        >
          <!-- 选中时的扫描线动画 -->
          <div v-if="projectPickerSelected === p.project_root" class="absolute inset-0 bg-gradient-to-r from-transparent via-primary-500/10 to-transparent skew-x-12 translate-x-[-150%] animate-[shimmer_2s_infinite]" />

          <div class="flex justify-between items-start z-10">
            <div class="flex items-center gap-2 mr-2 min-w-0">
              <!-- 图标：使用高亮颜色增强视觉效果 -->
              <div class="i-fa6-solid-code-branch text-primary-500 dark:text-primary-400 group-hover:text-primary-600 dark:group-hover:text-primary-300 transition-colors" />
              <!-- 标题文字：使用高对比度颜色确保清晰可读 -->
              <div class="font-bold text-sm truncate text-gray-800 text-gray-100" :title="getProjectName(p.project_root)">
                {{ getProjectName(p.project_root) }}
              </div>
            </div>
            <!-- Checkbox 样式的选择指示器 -->
            <div
              class="w-5 h-5 rounded-full border-2 flex items-center justify-center transition-all"
              :class="projectPickerSelected === p.project_root ? 'bg-primary-500 border-primary-500 scale-110' : 'border-gray-300 dark:border-gray-600'"
            >
              <div v-if="projectPickerSelected === p.project_root" class="i-fa6-solid-check text-white text-[10px]" />
            </div>
          </div>

          <div class="text-xs text-gray-400 font-mono truncate z-10" :title="p.project_root">
            {{ normalizePathForDisplay(p.project_root) }}
          </div>

          <div class="mt-auto pt-3 flex items-center justify-between text-xs z-10">
            <span class="flex items-center gap-1.5 px-2 py-1 rounded bg-slate-100 dark:bg-slate-700/50 text-slate-600 dark:text-slate-300">
              <div class="i-fa6-solid-file" />
              {{ p.total_files }}
            </span>
            <span class="text-gray-400 flex items-center gap-1">
              <div class="i-fa6-regular-clock" />
              {{ formatRelativeTime(p.last_success_time) }}
            </span>
          </div>
        </div>

        <!-- 空状态 -->
        <div v-if="indexedProjects.length === 0" class="col-span-full py-12 text-center flex flex-col items-center justify-center opacity-60">
          <div class="i-fa6-solid-folder-open text-5xl text-slate-300 mb-4" />
          <div class="text-base font-medium">
            暂无可用项目
          </div>
          <div class="text-xs mt-2">
            请先添加项目并建立索引
          </div>
        </div>
      </div>

      <template #action>
        <div class="flex justify-between items-center w-full">
          <n-button secondary @click="addProjectVisible = true">
            <template #icon>
              <div class="i-fa6-solid-plus" />
            </template>
            添加新项目
          </n-button>
          <div class="flex gap-3">
            <n-button secondary @click="projectPickerVisible = false">
              取消
            </n-button>
            <n-button type="primary" :disabled="!projectPickerSelected" @click="confirmProjectSelectionAndRun">
              <template #icon>
                <div class="i-fa6-solid-play" />
              </template>
              开始测试
            </n-button>
          </div>
        </div>
      </template>
    </n-modal>

    <!-- 子弹窗：添加项目 -->
    <n-modal v-model:show="addProjectVisible" preset="card" style="width: 480px" size="small" :bordered="false">
      <template #header>
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-xl bg-green-100 dark:bg-green-900/30 flex items-center justify-center">
            <div class="i-fa6-solid-folder-plus text-green-600 dark:text-green-400 text-lg" />
          </div>
          <div>
            <div class="font-semibold text-base">
              添加新项目
            </div>
            <div class="text-xs text-gray-500">
              输入项目根目录路径进行索引
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <n-form-item label="项目路径" :show-feedback="false">
          <n-input
            v-model:value="addProjectPath"
            placeholder="例如：C:\Projects\my-app 或 /home/user/projects/my-app"
            clearable
          >
            <template #prefix>
              <div class="i-fa6-solid-folder text-gray-400" />
            </template>
          </n-input>
        </n-form-item>

        <div class="p-3 rounded-lg bg-blue-50 dark:bg-blue-900/10 border border-blue-100 dark:border-blue-800/30 text-xs text-blue-700 dark:text-blue-300">
          <div class="flex items-start gap-2">
            <div class="i-fa6-solid-circle-info mt-0.5 flex-shrink-0" />
            <div>
              添加后将自动创建索引，完成后可用于测速。请确保路径为项目根目录且包含代码文件。
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <div class="flex justify-end gap-3">
          <n-button secondary @click="addProjectVisible = false">
            取消
          </n-button>
          <n-button type="primary" :loading="addProjectIndexing" :disabled="!addProjectPath.trim()" @click="addProjectAndIndexAndRun">
            <template #icon>
              <div class="i-fa6-solid-database" />
            </template>
            {{ addProjectIndexing ? '索引中...' : '创建索引并测试' }}
          </n-button>
        </div>
      </template>
    </n-modal>
  </n-modal>
</template>

<style scoped>
/* 深度选择器覆盖 Naive UI 样式以匹配 UI 要求 */
:deep(.n-tabs-nav) {
  padding-left: 4px;
}
:deep(.n-card__content) {
  padding: 0 !important;
}

/* 优化结果区 Tabs 内容区样式 - 确保文字清晰可读 */
.results-tabs :deep(.n-tab-pane) {
  /* 添加内边框增强层次感 */
  border-top: 1px solid rgba(148, 163, 184, 0.1);
}

/* 暗色模式下优化指标卡片文字对比度 */
:deep(.dark) .results-tabs,
.dark .results-tabs {
  --text-primary: #f1f5f9;
  --text-secondary: #cbd5e1;
}

/* 优化代码块在暗色模式下的可读性 */
:deep(.n-code) {
  max-height: 300px;
  overflow: auto;
}
</style>
