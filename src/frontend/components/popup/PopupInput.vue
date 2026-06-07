<script setup lang="ts">
import type { AttachmentItem, CustomPrompt, McpRequest } from '../../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { useIntersectionObserver, useStorage } from '@vueuse/core'
import { useSortable } from '@vueuse/integrations/useSortable'
import { useMessage } from 'naive-ui'
import { computed, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue'
import { useKeyboard } from '../../composables/useKeyboard'
import { useMcpToolsReactive } from '../../composables/useMcpTools'
import { buildConditionalContext } from '../../utils/conditionalContext'

interface Props {
  request: McpRequest | null
  loading?: boolean
  submitting?: boolean
  enhanceEnabled?: boolean
}

interface Emits {
  update: [data: {
    userInput: string
    rawUserInput: string
    conditionalContext: string
    selectedOptions: string[]
    attachments: AttachmentItem[]
  }]
  enhance: []
  openMcpToolsTab: []
}

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  submitting: false,
  enhanceEnabled: false,
})

const emit = defineEmits<Emits>()

// 响应式数据
const userInput = ref('')
const selectedOptions = ref<string[]>([])
// 附件列表（图片/任意文件，均已落盘到工作目录，仅持有本地路径）
const attachments = ref<AttachmentItem[]>([])
// 拖拽悬停状态（用于显示拖入提示）
const isDragHovering = ref(false)
const textareaRef = ref<any | null>(null)

// 自定义prompt相关状态
const customPrompts = ref<CustomPrompt[]>([])
const customPromptEnabled = ref(true)
const showInsertDialog = ref(false)
const pendingPromptContent = ref('')

// 移除条件性prompt状态管理，直接使用prompt的current_state

// 分离普通prompt和条件性prompt
const normalPrompts = computed(() =>
  customPrompts.value.filter(prompt => prompt.type === 'normal' || !prompt.type),
)

const conditionalPrompts = computed(() =>
  customPrompts.value.filter(prompt => prompt.type === 'conditional'),
)

// MCP 工具状态管理
const { mcpTools, loadMcpTools } = useMcpToolsReactive()

// 检查关联的 MCP 工具是否启用
function isMcpToolEnabled(toolId?: string): boolean {
  if (!toolId)
    return true // 没有关联工具时默认可用
  const tool = mcpTools.value.find(t => t.id === toolId)
  return tool?.enabled ?? false
}

// 获取 MCP 工具名称（用于提示文案）
function getMcpToolName(toolId?: string): string {
  if (!toolId)
    return ''
  const tool = mcpTools.value.find(t => t.id === toolId)
  return tool?.name ?? toolId
}

// 拖拽排序相关状态
const promptContainer = ref<HTMLElement | null>(null)
const sortablePrompts = shallowRef<CustomPrompt[]>([])
const { start, stop } = useSortable(promptContainer, sortablePrompts, {
  animation: 200,
  ghostClass: 'sortable-ghost',
  chosenClass: 'sortable-chosen',
  dragClass: 'sortable-drag',
  handle: '.drag-handle',
  forceFallback: true,
  fallbackTolerance: 3,
  onStart: (evt) => {
    console.log('PopupInput: 拖拽开始:', evt)
    console.log('PopupInput: 拖拽开始时的容器:', evt.from)
    console.log('PopupInput: 拖拽开始时的元素:', evt.item)
  },
  onEnd: (evt) => {
    console.log('PopupInput: 拖拽排序完成:', evt)
    console.log('PopupInput: 从索引', evt.oldIndex, '移动到索引', evt.newIndex)
    console.log('PopupInput: 拖拽后的sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))

    // 检查是否真的发生了位置变化
    if (evt.oldIndex !== evt.newIndex && evt.oldIndex !== undefined && evt.newIndex !== undefined) {
      // 手动重新排列数组
      const newList = [...sortablePrompts.value]
      const [movedItem] = newList.splice(evt.oldIndex, 1)
      newList.splice(evt.newIndex, 0, movedItem)

      // 更新sortablePrompts
      sortablePrompts.value = newList
      console.log('PopupInput: 手动更新后的sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))

      // 立即更新 customPrompts 的顺序，确保数据同步
      // 保留条件性prompt，只更新普通prompt的顺序
      const conditionalPromptsList = customPrompts.value.filter(prompt => prompt.type === 'conditional')
      customPrompts.value = [...sortablePrompts.value, ...conditionalPromptsList]
      console.log('PopupInput: 位置发生变化，保存新排序')

      // 立即保存排序
      savePromptOrder()
    }
    else {
      console.log('PopupInput: 位置未发生变化，无需保存')
    }
  },
  onMove: (evt) => {
    console.log('PopupInput: 拖拽移动中:', evt)
    return true // 允许移动
  },
  onChoose: (evt) => {
    console.log('PopupInput: 选择拖拽元素:', evt)
  },
  onUnchoose: (evt) => {
    console.log('PopupInput: 取消选择拖拽元素:', evt)
  },
})

// 使用键盘快捷键 composable
const { pasteShortcut } = useKeyboard()

const message = useMessage()

// 计算属性
const hasOptions = computed(() => (props.request?.predefined_options?.length ?? 0) > 0)
const canSubmit = computed(() => {
  const hasOptionsSelected = selectedOptions.value.length > 0
  const hasInputText = userInput.value.trim().length > 0
  const hasAttachments = attachments.value.length > 0

  if (hasOptions.value) {
    return hasOptionsSelected || hasInputText || hasAttachments
  }
  return hasInputText || hasAttachments
})
const canEnhance = computed(() => userInput.value.trim().length > 0)

// 工具栏状态文本
const statusText = computed(() => {
  // 检查是否有任何输入内容
  const hasInput = selectedOptions.value.length > 0
    || attachments.value.length > 0
    || userInput.value.trim().length > 0

  // 如果有任何输入内容，返回空字符串让 PopupActions 显示快捷键
  if (hasInput) {
    return ''
  }

  return '等待输入...'
})

// 上下文追加区域 UI 状态
const COLLAPSE_THRESHOLD = 6 // 条件性 prompt ≥ 此值时默认折叠
const isContextCollapsed = useStorage('popup-context-collapsed', false) // 折叠/展开状态
const showContextDescription = useStorage('popup-context-show-desc', true) // 是否显示描述

// 已开启的条件性 prompt 数量（用于折叠时的统计摘要）
const enabledConditionalCount = computed(() =>
  conditionalPrompts.value.filter(p => p.current_state && isMcpToolEnabled(p.linked_mcp_tool)).length,
)

// 根据条件性 prompt 数量自动判断初始折叠状态
// 只在首次加载时检查，用户手动操作后以 useStorage 为准
function autoCollapseIfNeeded() {
  if (conditionalPrompts.value.length >= COLLAPSE_THRESHOLD && !isContextCollapsed.value) {
    // 不自动覆盖用户选择 — useStorage 已有值则跳过
  }
}

// 根据条件性 prompt 的标题/功能描述匹配预设图标
function getConditionalIcon(prompt: CustomPrompt): string {
  const text = `${prompt.name || ''} ${prompt.condition_text || ''} ${prompt.description || ''}`.toLowerCase()

  if (/文档|markdown|md/.test(text))
    return 'i-carbon-document'
  if (/测试|test/.test(text))
    return 'i-carbon-test-tool'
  if (/编译|构建|build|compile/.test(text))
    return 'i-carbon-build'
  if (/运行|执行|run|exec/.test(text))
    return 'i-carbon-play'
  if (/搜索|sou|语义/.test(text))
    return 'i-carbon-search'
  if (/框架|context7|文档查询|library/.test(text))
    return 'i-carbon-book'
  if (/确认|zhi|三术|关键节点/.test(text))
    return 'i-carbon-checkmark-outline'
  if (/ui|ux|美化|设计|页面/.test(text))
    return 'i-carbon-color-palette'
  if (/tavily|ai.?搜索|实时搜索/.test(text))
    return 'i-carbon-search-locate'
  if (/记忆|memory|ji/.test(text))
    return 'i-carbon-data-base'
  return 'i-carbon-settings-adjust'
}

// 悬浮/固定相关状态
const isFloating = useStorage('popup-input-floating', false) // 开启/关闭悬浮模式
const sentinelRef = ref<HTMLElement | null>(null) // 哨兵元素
const isSticking = ref(false) // 当前是否处于吸附状态

// 监听哨兵可见性以判断是否吸附
// 逻辑：当我们在页面上方时，底部的哨兵(sentinel)在视口下方不可见 -> isIntersecting=false -> isSticking=true
// 当我们滚到底部时，哨兵进入视口 -> isIntersecting=true -> isSticking=false
useIntersectionObserver(
  sentinelRef,
  ([{ isIntersecting }]) => {
    isSticking.value = !isIntersecting
  },
  { threshold: 0.1 },
)

function toggleFloating() {
  isFloating.value = !isFloating.value
}

// 发送更新事件
function emitUpdate() {
  // 获取条件性prompt的追加内容
  const conditionalContent = generateConditionalContent()

  // 将条件性内容追加到用户输入
  const finalUserInput = userInput.value + conditionalContent

  emit('update', {
    userInput: finalUserInput,
    rawUserInput: userInput.value,
    conditionalContext: conditionalContent,
    selectedOptions: selectedOptions.value,
    attachments: attachments.value,
  })
}

// 处理选项变化
function handleOptionChange(option: string, checked: boolean) {
  if (checked) {
    selectedOptions.value.push(option)
  }
  else {
    const idx = selectedOptions.value.indexOf(option)
    if (idx > -1)
      selectedOptions.value.splice(idx, 1)
  }
  emitUpdate()
}

// 处理选项切换（整行点击）
function handleOptionToggle(option: string) {
  const idx = selectedOptions.value.indexOf(option)
  if (idx > -1) {
    selectedOptions.value.splice(idx, 1)
  }
  else {
    selectedOptions.value.push(option)
  }
  emitUpdate()
}

// 处理粘贴：图片走「落盘 -> 取本地路径」流程，避免内联超长 base64
function handleImagePaste(event: ClipboardEvent) {
  const items = event.clipboardData?.items
  let hasImage = false

  if (items) {
    for (const item of items) {
      if (item.type.includes('image')) {
        hasImage = true
        const file = item.getAsFile()
        if (file) {
          addPastedImage(file)
        }
      }
    }
  }

  if (hasImage) {
    event.preventDefault()
  }
}

// 处理增强入口点击
function handleEnhanceClick() {
  if (props.submitting)
    return
  if (props.enhanceEnabled) {
    emit('enhance')
  }
  else {
    emit('openMcpToolsTab')
  }
}

// 粘贴图片：先转 base64，调用后端落盘到工作目录，拿到本地绝对路径
async function addPastedImage(file: File): Promise<void> {
  try {
    const dataUrl = await fileToBase64(file)
    const base64 = dataUrl.includes(',') ? dataUrl.split(',')[1] : dataUrl

    const info = await invoke<AttachmentItem>('save_pasted_attachment', {
      dataBase64: base64,
      filename: file.name || null,
    })

    // 复用已有的 data URL 作为本地预览，避免再次读盘
    attachments.value.push({ ...info, previewUrl: dataUrl })
    message.success(`图片已添加：${info.filename}`)
    emitUpdate()
  }
  catch (error) {
    console.error('粘贴图片保存失败:', error)
    message.error(`图片保存失败：${String(error)}`)
  }
}

// 拖入文件：Tauri 拖拽事件提供真实磁盘路径，后端复制到工作目录
async function addDroppedPaths(paths: string[]): Promise<void> {
  try {
    const infos = await invoke<AttachmentItem[]>('save_dropped_attachments', { paths })
    if (!infos || infos.length === 0) {
      message.warning('未能添加拖入的文件')
      return
    }

    for (const info of infos) {
      const item: AttachmentItem = { ...info }
      // 图片读取一份 data URL 用于本地预览（非图片不预览）
      if (item.kind === 'image') {
        try {
          item.previewUrl = await invoke<string>('read_attachment_preview', { path: item.path })
        }
        catch (e) {
          console.warn('读取图片预览失败:', e)
        }
      }
      attachments.value.push(item)
    }

    message.success(`已添加 ${infos.length} 个文件`)
    emitUpdate()
  }
  catch (error) {
    console.error('拖入文件保存失败:', error)
    message.error(`文件保存失败：${String(error)}`)
  }
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = reject
    reader.readAsDataURL(file)
  })
}

// 移除单个附件（仅从列表移除，工作目录文件保留，可在设置中清理）
function removeAttachment(index: number) {
  attachments.value.splice(index, 1)
  emitUpdate()
}

// 格式化文件大小用于展示
function formatSize(bytes: number): string {
  if (bytes < 1024)
    return `${bytes} B`
  if (bytes < 1024 * 1024)
    return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

// 根据后缀返回文件类型图标
function getFileIcon(ext: string): string {
  const e = (ext || '').toLowerCase()
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

// 加载自定义prompt配置
async function loadCustomPrompts() {
  try {
    console.log('PopupInput: 开始加载自定义prompt配置')
    const config = await invoke('get_custom_prompt_config')
    if (config) {
      const promptConfig = config as any

      // 按sort_order排序
      customPrompts.value = (promptConfig.prompts || []).sort((a: CustomPrompt, b: CustomPrompt) => a.sort_order - b.sort_order)
      customPromptEnabled.value = promptConfig.enabled ?? true
      console.log('PopupInput: 加载到的prompt数量:', customPrompts.value.length)
      console.log('PopupInput: 条件性prompt列表:', customPrompts.value.filter(p => p.type === 'conditional'))

      // 同步到拖拽列表（只包含普通prompt）
      sortablePrompts.value = [...normalPrompts.value]
      console.log('PopupInput: 同步到sortablePrompts:', sortablePrompts.value.length)

      // 延迟初始化拖拽功能，等待组件完全挂载
      if (customPrompts.value.length > 0) {
        console.log('PopupInput: 准备启动拖拽功能')
        initializeDragSort()
      }
      else {
        console.log('PopupInput: 没有prompt，跳过拖拽初始化')
      }
    }
  }
  catch (error) {
    console.error('PopupInput: 加载自定义prompt失败:', error)
  }
}

// 处理自定义prompt点击
function handlePromptClick(prompt: CustomPrompt) {
  // 如果prompt内容为空或只有空格，直接清空输入框
  if (!prompt.content || prompt.content.trim() === '') {
    userInput.value = ''
    emitUpdate()
    return
  }

  if (userInput.value.trim()) {
    // 如果输入框有内容，显示插入选择对话框
    pendingPromptContent.value = prompt.content
    showInsertDialog.value = true
  }
  else {
    // 如果输入框为空，直接插入
    insertPromptContent(prompt.content)
  }
}

// 处理引用消息内容
function handleQuoteMessage(messageContent: string) {
  if (userInput.value.trim()) {
    // 输入框有内容，显示插入选择对话框
    pendingPromptContent.value = messageContent
    showInsertDialog.value = true
  }
  else {
    // 输入框为空，直接插入
    insertPromptContent(messageContent)
    message.success('原文内容已引用到输入框')
  }
}

// 插入prompt内容
function insertPromptContent(content: string, mode: 'replace' | 'append' = 'replace') {
  if (mode === 'replace') {
    userInput.value = content
  }
  else {
    userInput.value = userInput.value.trim() + (userInput.value.trim() ? '\n\n' : '') + content
  }

  // 聚焦到输入框
  setTimeout(() => {
    if (textareaRef.value) {
      textareaRef.value.focus()
      // 尝试将光标移到末尾（对于Naive UI组件）
      try {
        const inputElement = textareaRef.value.$el?.querySelector('textarea') || textareaRef.value.inputElRef
        if (inputElement && typeof inputElement.setSelectionRange === 'function') {
          inputElement.setSelectionRange(inputElement.value.length, inputElement.value.length)
        }
      }
      catch (error) {
        console.log('设置光标位置失败:', error)
      }
    }
  }, 100)

  emitUpdate()
}

// 处理插入模式选择
function handleInsertMode(mode: 'replace' | 'append') {
  insertPromptContent(pendingPromptContent.value, mode)
  showInsertDialog.value = false
  pendingPromptContent.value = ''
}

// 处理条件性prompt开关变化
async function handleConditionalToggle(promptId: string, value: boolean) {
  // 先更新本地状态
  const prompt = customPrompts.value.find(p => p.id === promptId)
  if (prompt) {
    prompt.current_state = value
  }

  // 保存到后端
  try {
    await invoke('update_conditional_prompt_state', {
      promptId,
      newState: value,
    })
    message.success('上下文追加状态已保存')
  }
  catch (error) {
    console.error('保存条件性prompt状态失败:', error)
    message.error(`保存设置失败: ${(error as any)?.message || String(error)}`)

    // 回滚本地状态
    if (prompt) {
      prompt.current_state = !value
    }
  }
}

// 生成条件性prompt的追加内容
function generateConditionalContent(): string {
  // 复用统一的上下文拼接逻辑，保持增强与输入一致
  const conditionalText = buildConditionalContext(conditionalPrompts.value)
  return conditionalText ? `\n\n${conditionalText}` : ''
}

// 获取条件性prompt的自适应描述
function getConditionalDescription(prompt: CustomPrompt): string {
  const isEnabled = prompt.current_state ?? false
  const template = isEnabled ? prompt.template_true : prompt.template_false

  // 如果有对应状态的模板，显示模板内容，否则显示原始描述
  if (template && template.trim()) {
    return template.trim()
  }

  return prompt.description || ''
}

// 移除拖拽排序初始化函数

// 初始化拖拽排序功能
async function initializeDragSort() {
  console.log('PopupInput: initializeDragSort 被调用')

  // 等待多个tick确保DOM完全渲染
  await nextTick()
  await nextTick()

  // 使用更长的延迟
  setTimeout(async () => {
    console.log('PopupInput: 开始查找容器')

    // 尝试多种方式查找容器
    let targetContainer = promptContainer.value

    if (!targetContainer) {
      targetContainer = document.querySelector('[data-prompt-container]') as HTMLElement
      console.log('PopupInput: querySelector结果:', targetContainer)
    }

    if (!targetContainer) {
      // 尝试通过类名查找
      const containers = document.querySelectorAll('.flex.flex-wrap')
      console.log('PopupInput: 找到的flex容器数量:', containers.length)
      for (let i = 0; i < containers.length; i++) {
        const container = containers[i] as HTMLElement
        if (container.querySelector('.sortable-item')) {
          targetContainer = container
          console.log('PopupInput: 通过sortable-item找到容器')
          break
        }
      }
    }

    if (targetContainer) {
      console.log('PopupInput: 找到目标容器:', targetContainer)
      const dragHandles = targetContainer.querySelectorAll('.drag-handle')
      console.log('PopupInput: 找到拖拽手柄数量:', dragHandles.length)

      const sortableItems = targetContainer.querySelectorAll('.sortable-item')
      console.log('PopupInput: 找到可排序项数量:', sortableItems.length)

      // 更新容器引用
      promptContainer.value = targetContainer

      console.log('PopupInput: 调用start()函数')
      start()
      console.log('PopupInput: start()函数调用完成')
    }
    else {
      console.log('PopupInput: 无法找到容器，DOM可能还没有渲染')
      console.log('PopupInput: 当前页面所有带data-prompt-container的元素:', document.querySelectorAll('[data-prompt-container]'))
      console.log('PopupInput: 当前页面所有.sortable-item元素:', document.querySelectorAll('.sortable-item'))
    }
  }, 500) // 增加延迟时间
}

// 保存prompt排序
async function savePromptOrder() {
  try {
    console.log('savePromptOrder被调用')
    console.log('当前sortablePrompts:', sortablePrompts.value.map(p => ({ id: p.id, name: p.name })))
    const promptIds = sortablePrompts.value.map(p => p.id)
    console.log('开始保存排序，prompt IDs:', promptIds)

    const startTime = Date.now()
    await invoke('update_custom_prompt_order', { promptIds })
    const endTime = Date.now()

    console.log(`排序已保存，耗时: ${endTime - startTime}ms`)
    message.success('排序已保存')
  }
  catch (error) {
    console.error('保存排序失败:', error)
    message.error('保存排序失败')
    // 重新加载以恢复原始顺序
    loadCustomPrompts()
  }
}

// 监听用户输入变化
watch(userInput, () => {
  emitUpdate()
})

// 移除拖拽相关的监听器

// 事件监听器引用
let unlistenCustomPromptUpdate: (() => void) | null = null
let unlistenWindowMove: (() => void) | null = null
let unlistenDragDrop: (() => void) | null = null

// 设置文件拖拽监听（Tauri webview 拖拽事件可拿到真实磁盘路径）
async function setupDragDropListener() {
  try {
    const webview = getCurrentWebviewWindow()
    unlistenDragDrop = await webview.onDragDropEvent((event) => {
      const payload = event.payload as any
      if (payload?.type === 'drop') {
        isDragHovering.value = false
        const paths: string[] = Array.isArray(payload.paths) ? payload.paths : []
        if (paths.length > 0)
          addDroppedPaths(paths)
      }
      else if (payload?.type === 'over' || payload?.type === 'enter') {
        isDragHovering.value = true
      }
      else {
        // leave / cancel
        isDragHovering.value = false
      }
    })
    console.log('文件拖拽监听器已设置')
  }
  catch (error) {
    console.error('设置文件拖拽监听器失败:', error)
  }
}

// 修复输入法候选框位置的函数
function fixIMEPosition() {
  if (textareaRef.value) {
    try {
      // 获取实际的 textarea 元素（Naive UI 的 n-input）
      const inputElement = (textareaRef.value as any).$el?.querySelector('textarea') || (textareaRef.value as any).inputElRef
      if (inputElement && document.activeElement === inputElement) {
        // 先失焦再聚焦，让输入法重新计算位置
        inputElement.blur()
        setTimeout(() => {
          inputElement.focus()
        }, 10)
      }
    }
    catch (error) {
      console.debug('修复IME位置失败:', error)
    }
  }
}

// 设置窗口移动监听器
async function setupWindowMoveListener() {
  try {
    const webview = getCurrentWebviewWindow()
    // 监听窗口移动事件
    unlistenWindowMove = await webview.onMoved(() => {
      // 窗口移动后修复输入法位置
      fixIMEPosition()
    })
    console.log('窗口移动监听器已设置')
  }
  catch (error) {
    console.error('设置窗口移动监听器失败:', error)
  }
}

// 组件挂载时加载自定义prompt
onMounted(async () => {
  console.log('组件挂载，开始加载prompt')
  await loadCustomPrompts()

  // 加载 MCP 工具配置（用于检查关联工具状态）
  await loadMcpTools()

  // 监听自定义prompt更新事件
  unlistenCustomPromptUpdate = await listen('custom-prompt-updated', () => {
    console.log('收到自定义prompt更新事件，重新加载数据')
    loadCustomPrompts()
  })
  // 设置窗口移动监听器
  setupWindowMoveListener()
  // 设置文件拖拽监听器
  setupDragDropListener()
})

onUnmounted(() => {
  // 清理事件监听器
  if (unlistenCustomPromptUpdate) {
    unlistenCustomPromptUpdate()
  }
  // 清理窗口移动监听器
  if (unlistenWindowMove) {
    unlistenWindowMove()
  }
  // 清理文件拖拽监听器
  if (unlistenDragDrop) {
    unlistenDragDrop()
  }

  // 停止拖拽功能
  stop()
})

// 重置数据
function reset() {
  userInput.value = ''
  selectedOptions.value = []
  attachments.value = []
  emitUpdate()
}

// 更新数据（用于外部同步）
function updateData(data: { userInput?: string, selectedOptions?: string[], attachments?: AttachmentItem[] }) {
  if (data.userInput !== undefined) {
    userInput.value = data.userInput
  }
  if (data.selectedOptions !== undefined) {
    selectedOptions.value = data.selectedOptions
  }
  if (data.attachments !== undefined) {
    attachments.value = data.attachments
  }

  emitUpdate()
}

// 中文注释：暴露原始输入与附加上下文，供本地增强链路精确组装提示词
function getRawUserInput() {
  return userInput.value
}

function getConditionalContext() {
  return generateConditionalContent()
}

// 移除了文件选择和测试图片功能

// 暴露方法给父组件
defineExpose({
  reset,
  canSubmit,
  canEnhance,
  statusText,
  updateData,
  handleQuoteMessage,
  getRawUserInput,
  getConditionalContext,
})
</script>

<template>
  <div class="space-y-3">
    <!-- 预定义选项 -->
    <div v-if="!loading && hasOptions" class="space-y-3" data-guide="predefined-options">
      <h4 class="text-sm font-medium text-white">
        请选择选项
      </h4>
      <n-space vertical size="small">
        <div
          v-for="(option, index) in request!.predefined_options"
          :key="`option-${index}`"
          class="rounded-lg p-3 border border-gray-600 bg-gray-100 cursor-pointer hover:opacity-80 transition-opacity"
          @click="handleOptionToggle(option)"
        >
          <n-checkbox
            :value="option"
            :checked="selectedOptions.includes(option)"
            :disabled="submitting"
            size="medium"
            @update:checked="(checked: boolean) => handleOptionChange(option, checked)"
            @click.stop
          >
            {{ option }}
          </n-checkbox>
        </div>
      </n-space>
    </div>

    <!-- 附件预览区域（图片缩略图 + 任意文件卡片） -->
    <div v-if="!loading && attachments.length > 0" class="space-y-3">
      <h4 class="text-sm font-medium text-white">
        已添加的附件 ({{ attachments.length }})
      </h4>

      <n-image-group>
        <div class="flex flex-wrap gap-3">
          <div
            v-for="(att, index) in attachments"
            :key="`att-${index}`"
            class="relative"
          >
            <!-- 图片：缩略图预览（点击放大） -->
            <n-image
              v-if="att.kind === 'image' && att.previewUrl"
              :src="att.previewUrl"
              width="100"
              height="100"
              object-fit="cover"
              class="rounded-lg border-2 border-gray-300 hover:border-primary-400 transition-all duration-200 cursor-pointer"
            />

            <!-- 其他文件：图标 + 文件名 + 后缀/大小 -->
            <div
              v-else
              :title="att.filename"
              class="w-[100px] h-[100px] rounded-lg border-2 border-gray-600 bg-container-secondary flex flex-col items-center justify-center gap-1 px-2 text-center"
            >
              <div :class="getFileIcon(att.ext)" class="w-7 h-7 text-primary-400" />
              <div class="text-[11px] text-on-surface truncate w-full leading-tight">
                {{ att.filename }}
              </div>
              <div class="text-[10px] text-on-surface-secondary uppercase">
                {{ att.ext || 'file' }} · {{ formatSize(att.size) }}
              </div>
            </div>

            <!-- 删除按钮 -->
            <n-button
              class="absolute -top-2 -right-2 z-10"
              size="tiny"
              type="error"
              circle
              @click="removeAttachment(index)"
            >
              <template #icon>
                <div class="i-carbon-close w-3 h-3" />
              </template>
            </n-button>

            <!-- 序号 -->
            <div class="absolute bottom-1 left-1 w-5 h-5 bg-primary-500 text-white text-xs rounded-full flex items-center justify-center font-bold shadow-sm z-5">
              {{ index + 1 }}
            </div>
          </div>
        </div>
      </n-image-group>
    </div>

    <!-- 文本输入区域 -->
    <div v-if="!loading">
      <!-- 哨兵元素：用于检测是否到底部 -->
      <div ref="sentinelRef" class="w-full h-px opacity-0 pointer-events-none absolute -mt-1" />

      <div
        class="transition-all duration-300 ease-[cubic-bezier(0.25,0.8,0.25,1)]" :class="[
          isFloating ? 'sticky bottom-0 z-[50]' : 'relative',
          (isFloating && isSticking)
            ? 'bg-surface/85 backdrop-blur-xl shadow-[0_-8px_30px_rgba(0,0,0,0.15)] border-t border-white/10 pb-5 pt-4 px-3 -mx-3 mb-0'
            : 'space-y-3',
        ]"
      >
        <!-- 标题栏 & 切换按钮 -->
        <div class="flex items-center justify-between mb-2">
          <h4 class="text-sm font-medium text-white">
            {{ hasOptions ? '补充说明 (可选)' : '请输入您的回复' }}
          </h4>
          <n-button
            text
            size="tiny"
            class="opacity-70 hover:opacity-100 transition-opacity"
            :title="isFloating ? '取消悬浮 (跟随底部)' : '开启悬浮 (固定底部)'"
            @click="toggleFloating"
          >
            <template #icon>
              <div
                class="transition-transform duration-300" :class="[
                  isFloating ? 'i-carbon-pin-filled text-primary-500 rotate-0' : 'i-carbon-pin text-on-surface-secondary -rotate-45',
                ]"
              />
            </template>
          </n-button>
        </div>

        <!-- 自定义prompt按钮区域 -->
        <div v-if="customPromptEnabled && customPrompts.length > 0" class="space-y-2" data-guide="custom-prompts">
          <div class="text-xs text-on-surface-secondary flex items-center gap-2">
            <div class="i-carbon-bookmark w-3 h-3 text-primary-500" />
            <span>快捷模板 (拖拽调整顺序):</span>
          </div>
          <div
            ref="promptContainer"
            data-prompt-container
            class="flex flex-wrap gap-2"
          >
            <div
              v-for="prompt in sortablePrompts"
              :key="prompt.id"
              :title="prompt.description || (prompt.content.trim() ? prompt.content : '清空输入框')"
              class="inline-flex items-center gap-1 px-2 py-1 text-xs bg-container-secondary hover:bg-container-tertiary rounded transition-all duration-200 select-none border border-gray-600 text-on-surface sortable-item"
            >
              <!-- 拖拽手柄 -->
              <div class="drag-handle cursor-move p-0.5 rounded hover:bg-container-tertiary transition-colors">
                <div class="i-carbon-drag-horizontal w-3 h-3 text-on-surface-secondary" />
              </div>

              <!-- 按钮内容 -->
              <div
                class="inline-flex items-center cursor-pointer"
                @click="handlePromptClick(prompt)"
              >
                <span>{{ prompt.name }}</span>
              </div>
            </div>
          </div>
        </div>

        <!-- 上下文追加区域 -->
        <div v-if="customPromptEnabled && conditionalPrompts.length > 0" class="space-y-2" data-guide="context-append">
          <!-- 标题栏：含折叠/展开、仅标题模式切换 -->
          <div class="flex items-center justify-between">
            <div
              class="text-xs text-on-surface-secondary flex items-center gap-2 cursor-pointer select-none"
              @click="isContextCollapsed = !isContextCollapsed"
            >
              <div
                class="w-3 h-3 transition-transform duration-200 text-primary-500" :class="[
                  isContextCollapsed ? 'i-carbon-chevron-right' : 'i-carbon-chevron-down',
                ]"
              />
              <span>上下文追加</span>
              <span class="text-on-surface-secondary opacity-60">
                ({{ enabledConditionalCount }}/{{ conditionalPrompts.length }})
              </span>
            </div>
            <!-- 右侧控制按钮 -->
            <div v-if="!isContextCollapsed" class="flex items-center gap-1">
              <n-tooltip>
                <template #trigger>
                  <n-button
                    text
                    size="tiny"
                    class="opacity-60 hover:opacity-100"
                    @click="showContextDescription = !showContextDescription"
                  >
                    <template #icon>
                      <div :class="showContextDescription ? 'i-carbon-text-short-paragraph' : 'i-carbon-text-line-spacing'" />
                    </template>
                  </n-button>
                </template>
                {{ showContextDescription ? '仅显示标题' : '显示标题和描述' }}
              </n-tooltip>
            </div>
          </div>

          <!-- 展开时的内容 -->
          <div v-if="!isContextCollapsed" :class="showContextDescription ? 'grid grid-cols-2 gap-2' : 'flex flex-wrap gap-1.5'">
            <div
              v-for="prompt in conditionalPrompts"
              :key="prompt.id"
              :class="[
                showContextDescription
                  ? 'flex items-center justify-between p-2 bg-container-secondary rounded border border-gray-600 transition-colors text-xs'
                  : 'inline-flex items-center gap-1.5 px-2 py-1 bg-container-secondary rounded border border-gray-600 transition-colors text-xs',
                isMcpToolEnabled(prompt.linked_mcp_tool) ? 'hover:bg-container-tertiary' : 'opacity-50 cursor-not-allowed',
              ]"
            >
              <!-- 动态图标 -->
              <div class="w-3 h-3 shrink-0" :class="[getConditionalIcon(prompt), (prompt.current_state && isMcpToolEnabled(prompt.linked_mcp_tool)) ? 'text-primary-500' : 'text-on-surface-secondary opacity-50']" />

              <div class="flex-1 min-w-0" :class="showContextDescription ? 'mr-2' : 'mr-1'">
                <div class="text-xs text-on-surface truncate font-medium" :title="prompt.condition_text || prompt.name">
                  {{ prompt.condition_text || prompt.name }}
                </div>
                <div v-if="showContextDescription && getConditionalDescription(prompt)" class="text-xs text-primary-600 dark:text-primary-400 opacity-50 dark:opacity-60 mt-0.5 truncate leading-tight" :title="getConditionalDescription(prompt)">
                  {{ getConditionalDescription(prompt) }}
                </div>
              </div>
              <!-- 使用 n-tooltip 包裹开关，当 MCP 工具未启用时显示提示 -->
              <n-tooltip :disabled="isMcpToolEnabled(prompt.linked_mcp_tool) || !prompt.linked_mcp_tool">
                <template #trigger>
                  <n-switch
                    :value="prompt.current_state ?? false"
                    size="small"
                    :disabled="!isMcpToolEnabled(prompt.linked_mcp_tool)"
                    @update:value="(value: boolean) => handleConditionalToggle(prompt.id, value)"
                  />
                </template>
                请先在设置中开启「{{ getMcpToolName(prompt.linked_mcp_tool) }}」MCP 工具
              </n-tooltip>
            </div>
          </div>
        </div>

        <!-- 附件提示区域 -->
        <div v-if="attachments.length === 0" class="text-center">
          <div
            class="text-xs transition-colors" :class="[
              isDragHovering ? 'text-primary-400 font-medium' : 'text-on-surface-secondary',
            ]"
          >
            {{ isDragHovering ? '📎 松开即可添加文件' : `💡 提示：可粘贴图片 (${pasteShortcut})，或将任意文件拖入窗口` }}
          </div>
        </div>

        <!-- 提示词增强入口 -->
        <div class="flex items-center justify-between text-xs my-2">
          <div class="flex items-center gap-2 text-on-surface-secondary">
            <div class="i-carbon-magic-wand w-3 h-3 text-primary-500" />
            <span>{{ enhanceEnabled ? '将当前文本发送给本地 AI 做结构化增强' : '提示词增强未启用' }}</span>
          </div>
          <n-button
            size="tiny"
            :type="enhanceEnabled ? 'info' : 'warning'"
            secondary
            :disabled="submitting || (enhanceEnabled && !canEnhance)"
            @click="handleEnhanceClick"
          >
            <template #icon>
              <div :class="enhanceEnabled ? 'i-carbon-magic-wand' : 'i-carbon-launch'" />
            </template>
            {{ enhanceEnabled ? '本地增强' : '启用增强' }}
          </n-button>
        </div>

        <!-- 文本输入框 -->
        <n-input
          ref="textareaRef"
          v-model:value="userInput"
          type="textarea"
          size="small"
          :placeholder="hasOptions ? `您可以在这里添加补充说明... (支持粘贴图片 ${pasteShortcut})` : `请输入您的回复... (支持粘贴图片 ${pasteShortcut})`"
          :disabled="submitting"
          :autosize="{ minRows: 3, maxRows: 6 }"
          data-guide="popup-input"
          class="shadow-sm"
          @paste="handleImagePaste"
        />
      </div>
    </div>

    <!-- 插入模式选择对话框 -->
    <n-modal v-model:show="showInsertDialog" preset="dialog" title="插入模式选择">
      <template #header>
        <div class="flex items-center gap-2">
          <div class="i-carbon-text-creation w-4 h-4" />
          <span>插入Prompt</span>
        </div>
      </template>
      <div class="space-y-4">
        <p class="text-sm text-on-surface-secondary">
          输入框中已有内容，请选择插入模式：
        </p>
        <div class="bg-container-secondary p-3 rounded text-sm">
          {{ pendingPromptContent }}
        </div>
      </div>
      <template #action>
        <div class="flex gap-2">
          <n-button @click="showInsertDialog = false">
            取消
          </n-button>
          <n-button type="warning" @click="handleInsertMode('replace')">
            替换内容
          </n-button>
          <n-button type="primary" @click="handleInsertMode('append')">
            追加内容
          </n-button>
        </div>
      </template>
    </n-modal>
  </div>
</template>

<style scoped>
/* Sortable.js 拖拽样式 */
.sortable-ghost {
  opacity: 0.5;
  transform: scale(0.95);
}

.sortable-chosen {
  cursor: grabbing !important;
}

.sortable-drag {
  opacity: 0.8;
  transform: rotate(5deg);
}
</style>
