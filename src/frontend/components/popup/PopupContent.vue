<script setup lang="ts">
import type { McpRequest } from '../../types/popup'
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { onMounted, watch } from 'vue'
import { safeBase64Decode, useMarkdown } from '../../composables/useMarkdown'

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  currentTheme: 'dark',
})

const emit = defineEmits<Emits>()

const { renderMarkdown, loadHljsTheme } = useMarkdown()
const message = useMessage()

// 预处理引用内容，移除增强prompt格式标记
function preprocessQuoteContent(content: string): string {
  let processedContent = content

  const markersToRemove = [
    /### BEGIN RESPONSE ###\s*/gi,
    /Here is an enhanced version of the original instruction that is more specific and clear:\s*/gi,
    /<augment-enhanced-prompt>\s*/gi,
    /<\/augment-enhanced-prompt>\s*/gi,
    /### END RESPONSE ###\s*/gi,
  ]

  markersToRemove.forEach((marker) => {
    processedContent = processedContent.replace(marker, '')
  })

  processedContent = processedContent
    .replace(/\n\s*\n\s*\n/g, '\n\n')
    .trim()

  return processedContent
}

// 引用消息内容
function quoteMessage() {
  if (props.request?.message) {
    const processedContent = preprocessQuoteContent(props.request.message)
    emit('quoteMessage', processedContent)
  }
}

// 事件委托 — 处理 markdown 内容区域的点击
async function handleMarkdownClick(e: MouseEvent) {
  const target = e.target as HTMLElement

  // 代码块复制按钮
  const copyBtn = target.closest('.code-block-copy') as HTMLElement | null
  if (copyBtn) {
    e.stopPropagation()
    e.preventDefault()
    const encodedCode = copyBtn.getAttribute('data-code')
    if (!encodedCode) return
    try {
      const code = safeBase64Decode(encodedCode)
      await navigator.clipboard.writeText(code)
      // 切换图标为 checkmark
      const icon = copyBtn.querySelector('div')
      if (icon) {
        const oldClass = icon.className
        icon.className = 'i-carbon-checkmark'
        icon.style.cssText = 'width:14px;height:14px;display:block;color:#22c55e;'
        setTimeout(() => {
          icon.className = oldClass
          icon.style.cssText = 'width:14px;height:14px;display:block;'
        }, 2000)
      }
      message.success('代码已复制到剪贴板')
    }
    catch {
      message.error('复制失败')
    }
    return
  }

  // 代码块运行按钮
  const runBtn = target.closest('.code-block-run') as HTMLElement | null
  if (runBtn) {
    e.stopPropagation()
    e.preventDefault()
    const lang = runBtn.getAttribute('data-lang') || ''
    const encodedCode = runBtn.getAttribute('data-code')
    if (!encodedCode) return
    const code = safeBase64Decode(encodedCode)
    await handleCodeExecution(lang, code, runBtn)
    return
  }

  // 内联代码复制
  const inlineCode = target.closest('.markdown-content p code, .markdown-content li code') as HTMLElement | null
  if (inlineCode) {
    try {
      await navigator.clipboard.writeText(inlineCode.textContent || '')
      message.success('代码已复制到剪贴板')
    }
    catch {
      message.error('复制失败')
    }
  }
}

// 代码执行处理
async function handleCodeExecution(lang: string, code: string, triggerEl: HTMLElement) {
  const wrapper = triggerEl.closest('.code-block-wrapper')
  if (!wrapper) return

  // 检查是否已有输出面板
  let outputPanel = wrapper.querySelector('.code-execution-output') as HTMLElement | null
  if (outputPanel) {
    outputPanel.remove()
    return
  }

  // 创建输出面板
  outputPanel = document.createElement('div')
  outputPanel.className = 'code-execution-output'
  outputPanel.innerHTML = '<div class="output-header"><span>运行中...</span></div><pre style="margin:0;padding:0;border:none;background:none;font-size:inherit;color:inherit;">等待执行结果...</pre>'
  wrapper.appendChild(outputPanel)

  try {
    const normalizedLang = lang.toLowerCase()

    // HTML 预览 — 使用 iframe（确保 UTF-8 编码）
    if (normalizedLang === 'html') {
      // 如果代码中没有 charset 声明，自动添加 UTF-8 meta 标签
      let htmlCode = code
      if (!code.includes('charset') && !code.includes('CHARSET')) {
        htmlCode = `<meta charset="UTF-8">\n${code}`
      }
      const blob = new Blob([htmlCode], { type: 'text/html;charset=utf-8' })
      const blobUrl = URL.createObjectURL(blob)
      outputPanel.innerHTML = `<div class="output-header"><span>HTML 预览</span><button class="code-block-copy" style="border:none;background:none;cursor:pointer;padding:2px;" onclick="this.closest('.code-execution-output').remove()"><div class="i-carbon-close" style="width:14px;height:14px;display:block;color:#9ca3af;"></div></button></div>`
      const iframe = document.createElement('iframe')
      iframe.className = 'html-preview-frame'
      iframe.sandbox.add('allow-scripts')
      iframe.src = blobUrl
      iframe.style.cssText = 'width:100%;min-height:200px;max-height:500px;border:none;border-top:1px solid #374151;border-radius:0 0 0.5rem 0.5rem;background:#ffffff;'
      outputPanel.appendChild(iframe)
      // 清理 blob URL
      iframe.onload = () => URL.revokeObjectURL(blobUrl)
      return
    }

    // JS — 沙箱 iframe 执行
    if (normalizedLang === 'javascript' || normalizedLang === 'js') {
      const result = await executeJavaScriptInSandbox(code)
      renderExecutionResult(outputPanel, result)
      return
    }

    // 后端语言 — 通过 Tauri invoke 执行
    const result = await invoke<CodeExecutionResult>('execute_code_snippet', {
      request: { language: normalizedLang, code },
    })
    renderExecutionResult(outputPanel, result)
  }
  catch (error) {
    outputPanel.innerHTML = `<div class="output-header"><span class="output-error">执行错误</span></div><pre style="margin:0;padding:0;border:none;background:none;font-size:inherit;color:inherit;" class="output-error">${String(error)}</pre>`
  }
}

// 在沙箱 iframe 中执行 JavaScript
function executeJavaScriptInSandbox(code: string): Promise<CodeExecutionResult> {
  return new Promise((resolve) => {
    const iframe = document.createElement('iframe')
    iframe.style.display = 'none'
    iframe.sandbox.add('allow-scripts')
    document.body.appendChild(iframe)

    const timer = setTimeout(() => {
      window.removeEventListener('message', handler)
      document.body.removeChild(iframe)
      resolve({ stdout: '', stderr: '执行超时（10秒）', exit_code: null, timed_out: true, error: null })
    }, 10000)

    function handler(e: MessageEvent) {
      if (e.source === iframe.contentWindow) {
        clearTimeout(timer)
        window.removeEventListener('message', handler)
        document.body.removeChild(iframe)
        resolve({
          stdout: e.data?.result || '',
          stderr: e.data?.error || '',
          exit_code: e.data?.error ? 1 : 0,
          timed_out: false,
          error: null,
        })
      }
    }
    window.addEventListener('message', handler)

    // 将代码注入 iframe
    const wrappedCode = JSON.stringify(code)
    iframe.srcdoc = `<script>
try {
  const __logs = [];
  const __origLog = console.log;
  console.log = (...args) => __logs.push(args.map(String).join(' '));
  console.warn = (...args) => __logs.push('[WARN] ' + args.map(String).join(' '));
  console.error = (...args) => __logs.push('[ERROR] ' + args.map(String).join(' '));
  const __result = eval(${wrappedCode});
  const __output = __logs.length > 0 ? __logs.join('\\n') : String(__result);
  parent.postMessage({ result: __output }, '*');
} catch(e) {
  parent.postMessage({ error: e.message || String(e) }, '*');
}
<\/script>`
  })
}

// 渲染执行结果
function renderExecutionResult(panel: HTMLElement, result: CodeExecutionResult) {
  const statusClass = result.timed_out ? 'output-timeout' : (result.exit_code === 0 ? 'output-success' : 'output-error')
  const statusText = result.timed_out ? '超时' : (result.exit_code === 0 ? '完成' : `退出码: ${result.exit_code}`)

  let content = ''
  if (result.stdout) content += result.stdout
  if (result.stderr) content += (content ? '\n' : '') + result.stderr
  if (result.error) content += (content ? '\n' : '') + result.error
  if (!content) content = '（无输出）'

  // 转义 HTML
  content = content.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

  panel.innerHTML = `<div class="output-header"><span class="${statusClass}">${statusText}</span><button style="border:none;background:none;cursor:pointer;padding:2px;color:#9ca3af;" onclick="this.closest('.code-execution-output').remove()"><div class="i-carbon-close" style="width:14px;height:14px;display:block;"></div></button></div><pre style="margin:0;padding:0;border:none;background:none;font-size:inherit;color:inherit;">${content}</pre>`
}

interface CodeExecutionResult {
  stdout: string
  stderr: string
  exit_code: number | null
  timed_out: boolean
  error: string | null
}

interface Props {
  request: McpRequest | null
  loading?: boolean
  currentTheme?: string
}

interface Emits {
  quoteMessage: [message: string]
}

// 初始化 hljs 主题
onMounted(() => {
  loadHljsTheme('auto', props.currentTheme)
})

// 主题变化时重新加载 hljs 样式
watch(() => props.currentTheme, (newTheme) => {
  loadHljsTheme('auto', newTheme)
})
</script>

<template>
  <div class="text-white">
    <!-- 加载状态 -->
    <div v-if="loading" class="flex flex-col items-center justify-center py-8">
      <n-spin size="medium" />
      <p class="text-sm mt-3 text-white opacity-60">
        加载中...
      </p>
    </div>

    <!-- 消息显示区域 -->
    <div v-else-if="request?.message" class="relative">
      <!-- Markdown 内容 -->
      <div
        v-if="request.is_markdown"
        class="markdown-content"
        :class="currentTheme === 'light' ? 'theme-light' : 'theme-dark'"
        @click="handleMarkdownClick"
        v-html="renderMarkdown(request.message)"
      />
      <div v-else class="whitespace-pre-wrap leading-relaxed text-white">
        {{ request.message }}
      </div>

      <!-- 引用原文按钮 -->
      <div class="flex justify-end mt-4 pt-3 border-t border-gray-600/30" data-guide="quote-message">
        <div
          title="点击将AI的消息内容引用到输入框中"
          class="inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium bg-blue-500/20 hover:bg-blue-500/30 text-white rounded-md transition-all duration-200 cursor-pointer border border-blue-500/50 hover:border-blue-500/70 shadow-sm hover:shadow-md"
          @click="quoteMessage"
        >
          <div class="i-carbon-quotes w-3.5 h-3.5" />
          <span>引用原文</span>
        </div>
      </div>
    </div>

    <!-- 错误状态 -->
    <n-alert v-else type="error" title="数据加载错误">
      <div class="text-white">
        Request对象: {{ JSON.stringify(request) }}
      </div>
    </n-alert>
  </div>
</template>
