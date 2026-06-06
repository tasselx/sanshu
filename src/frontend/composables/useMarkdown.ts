import { katex as markdownItKatex } from '@mdit/plugin-katex'
import hljs from 'highlight.js'
import { renderToString as renderKatexToString } from 'katex'
import MarkdownIt from 'markdown-it'
import taskLists from 'markdown-it-task-lists'
import { ref } from 'vue'

// 可执行语言列表
const EXECUTABLE_LANGUAGES = new Set([
  'python',
  'py',
  'javascript',
  'js',
  'node',
  'go',
  'golang',
  'java',
  'html',
])

// 流程图语言别名：统一转为 Mermaid 渲染卡片，避免被当作普通代码块显示。
const MERMAID_LANGUAGES = new Set([
  'mermaid',
  'mmd',
  'flowchart',
])

// 数学公式代码块别名：支持 ```math / ```tex / ```latex 的显示模式公式。
const MATH_LANGUAGES = new Set([
  'math',
  'tex',
  'latex',
])

// 语言显示名称映射
const LANG_DISPLAY_NAMES: Record<string, string> = {
  js: 'JavaScript',
  javascript: 'JavaScript',
  ts: 'TypeScript',
  typescript: 'TypeScript',
  py: 'Python',
  python: 'Python',
  go: 'Go',
  golang: 'Go',
  java: 'Java',
  rs: 'Rust',
  rust: 'Rust',
  html: 'HTML',
  css: 'CSS',
  scss: 'SCSS',
  less: 'Less',
  json: 'JSON',
  yaml: 'YAML',
  yml: 'YAML',
  xml: 'XML',
  sql: 'SQL',
  bash: 'Bash',
  sh: 'Shell',
  shell: 'Shell',
  zsh: 'Zsh',
  powershell: 'PowerShell',
  ps1: 'PowerShell',
  c: 'C',
  cpp: 'C++',
  csharp: 'C#',
  cs: 'C#',
  swift: 'Swift',
  kotlin: 'Kotlin',
  kt: 'Kotlin',
  ruby: 'Ruby',
  rb: 'Ruby',
  php: 'PHP',
  lua: 'Lua',
  r: 'R',
  dart: 'Dart',
  scala: 'Scala',
  toml: 'TOML',
  ini: 'INI',
  dockerfile: 'Dockerfile',
  docker: 'Dockerfile',
  makefile: 'Makefile',
  graphql: 'GraphQL',
  vue: 'Vue',
  svelte: 'Svelte',
  jsx: 'JSX',
  tsx: 'TSX',
  md: 'Markdown',
  markdown: 'Markdown',
  mermaid: 'Mermaid',
  mmd: 'Mermaid',
  flowchart: 'Mermaid',
  math: 'Math',
  tex: 'TeX',
  latex: 'LaTeX',
  diff: 'Diff',
  text: 'Text',
  txt: 'Text',
  plaintext: 'Text',
  node: 'Node.js',
}

// hljs 主题映射（使用 Vite ?inline 导入，离线可用）
const THEME_LOADERS: Record<string, () => Promise<string>> = {
  'github': () => import('highlight.js/styles/github.css?inline').then(m => m.default),
  'github-dark': () => import('highlight.js/styles/github-dark.css?inline').then(m => m.default),
  'monokai': () => import('highlight.js/styles/monokai.css?inline').then(m => m.default),
  'atom-one-dark': () => import('highlight.js/styles/atom-one-dark.css?inline').then(m => m.default),
  'vs2015': () => import('highlight.js/styles/vs2015.css?inline').then(m => m.default),
}

// 可选主题列表（供 UI 展示）
export const AVAILABLE_HLJS_THEMES = [
  { value: 'auto', label: '跟随应用主题' },
  { value: 'github', label: 'GitHub Light' },
  { value: 'github-dark', label: 'GitHub Dark' },
  { value: 'monokai', label: 'Monokai' },
  { value: 'atom-one-dark', label: 'Atom One Dark' },
  { value: 'vs2015', label: 'VS 2015' },
]

// base64 编码（兼容中文和特殊字符）
function safeBase64Encode(str: string): string {
  return btoa(unescape(encodeURIComponent(str)))
}

// base64 解码
export function safeBase64Decode(str: string): string {
  try {
    return decodeURIComponent(escape(atob(str)))
  }
  catch {
    return atob(str)
  }
}

// 获取语言显示名称
function getDisplayLang(lang: string): string {
  if (!lang)
    return ''
  return LANG_DISPLAY_NAMES[lang.toLowerCase()] || lang.toUpperCase()
}

// 判断语言是否可执行
export function isExecutableLanguage(lang: string): boolean {
  return EXECUTABLE_LANGUAGES.has(lang.toLowerCase())
}

// 转义 HTML 特殊字符
function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function normalizeInlineColorValue(value: string): string | null {
  const color = value.trim()

  if (/^#(?:[\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/i.test(color)) {
    return color
  }

  // 中文说明：仅放行数字、百分比、逗号、空格和斜杠组成的颜色函数，避免把任意内容写入内联 style。
  if (/^(?:rgb|hsl)a?\([\d\s%,./+-]+\)$/i.test(color)) {
    return color
  }

  return null
}

function renderInlineCode(tokenContent: string): string {
  const colorValue = normalizeInlineColorValue(tokenContent)
  const escapedContent = escapeHtml(tokenContent)

  if (!colorValue) {
    return `<code>${escapedContent}</code>`
  }

  return `<code class="markdown-color-code" style="--markdown-color-swatch:${colorValue};">`
    + `<span class="markdown-color-swatch" aria-hidden="true"></span>`
    + `<span>${escapedContent}</span>`
    + `</code>`
}

function renderMathBlock(code: string): string {
  try {
    const rendered = renderKatexToString(code, {
      displayMode: true,
      throwOnError: false,
      output: 'htmlAndMathml',
      trust: false,
    })
    return `<div class="markdown-math-block">${rendered}</div>\n`
  }
  catch (error) {
    return `<div class="markdown-math-block markdown-math-error" title="${escapeHtml(String(error))}">${escapeHtml(code)}</div>\n`
  }
}

function renderMermaidBlock(lang: string, code: string, encodedCode: string): string {
  const displayLang = getDisplayLang(lang || 'mermaid')
  const escapedCode = escapeHtml(code)

  return `<div class="mermaid-block-wrapper" data-diagram-code="${encodedCode}" data-diagram-lang="${escapeHtml(lang || 'mermaid')}">`
    + `<div class="mermaid-block-header">`
    + `<span class="mermaid-block-lang">${escapeHtml(displayLang)}</span>`
    + `<div class="mermaid-block-actions">`
    + `<button class="mermaid-source-toggle" title="切换流程图源码"><div class="i-carbon-code" style="width:14px;height:14px;display:block;"></div></button>`
    + `<button class="mermaid-zoom-out" title="缩小流程图"><div class="i-carbon-zoom-out" style="width:14px;height:14px;display:block;"></div></button>`
    + `<button class="mermaid-zoom-in" title="放大流程图"><div class="i-carbon-zoom-in" style="width:14px;height:14px;display:block;"></div></button>`
    + `<button class="mermaid-zoom-reset" title="重置缩放"><div class="i-carbon-reset" style="width:14px;height:14px;display:block;"></div></button>`
    + `<button class="mermaid-copy-image" title="复制流程图图片"><div class="i-carbon-copy" style="width:14px;height:14px;display:block;"></div></button>`
    + `</div>`
    + `</div>`
    + `<div class="mermaid-stage"><div class="mermaid-render" aria-live="polite">流程图渲染中...</div></div>`
    + `<pre class="mermaid-source"><code>${escapedCode}</code></pre>`
    + `</div>\n`
}

// ========== 单例 ==========

let instance: ReturnType<typeof createMarkdownInstance> | null = null

function createMarkdownInstance() {
  const hljsTheme = ref('github-dark')

  // 完整版 markdown-it（PopupContent 使用）
  const md = new MarkdownIt({
    html: true,
    xhtmlOut: false,
    breaks: true,
    langPrefix: 'language-',
    linkify: true,
    typographer: true,
    quotes: '""\'\'',
    highlight(str: string, lang: string) {
      if (lang && hljs.getLanguage(lang)) {
        try {
          return hljs.highlight(str, { language: lang }).value
        }
        catch { /* 忽略高亮错误 */ }
      }
      return escapeHtml(str)
    },
  })

  // 任务列表插件（只读模式）
  md.use(taskLists, { enabled: false, label: true })

  // 数学公式插件：同时支持 $...$、$$...$$、\(...\)、\[...\]，并保持非信任模式。
  md.use(markdownItKatex, {
    delimiters: 'all',
    mathFence: false,
    throwOnError: false,
    trust: false,
  })

  // 行内颜色值预览：让 `#1E2937` / `rgb(...)` / `hsl(...)` 等证据颜色直接带色块显示。
  md.renderer.rules.code_inline = (tokens, idx) => {
    return renderInlineCode(tokens[idx].content)
  }

  // 自定义 fence 渲染器 — 输出结构化代码块 HTML
  md.renderer.rules.fence = (tokens, idx, options, _env, _renderer) => {
    const token = tokens[idx]
    const lang = (token.info.trim().split(/\s+/)[0] || '').toLowerCase()
    const code = token.content
    const encodedCode = safeBase64Encode(code)
    const displayLang = getDisplayLang(lang)
    const executable = isExecutableLanguage(lang)

    if (MERMAID_LANGUAGES.has(lang)) {
      return renderMermaidBlock(lang, code, encodedCode)
    }

    if (MATH_LANGUAGES.has(lang)) {
      return renderMathBlock(code)
    }

    // 高亮处理
    let highlighted: string
    if (lang && hljs.getLanguage(lang)) {
      try {
        highlighted = hljs.highlight(code, { language: lang }).value
      }
      catch {
        highlighted = escapeHtml(code)
      }
    }
    else {
      highlighted = escapeHtml(code)
    }

    // 构建运行按钮（仅可执行语言）
    const runButton = executable
      ? `<button class="code-block-run" data-lang="${escapeHtml(lang)}" data-code="${encodedCode}" title="运行代码"><div class="i-carbon-play" style="width:14px;height:14px;display:block;"></div></button>`
      : ''

    return `<div class="code-block-wrapper" data-lang="${escapeHtml(lang)}">`
      + `<div class="code-block-header">`
      + `<span class="code-block-lang">${escapeHtml(displayLang)}</span>`
      + `<div class="code-block-actions">${
        runButton
      }<button class="code-block-copy" data-code="${encodedCode}" title="复制代码"><div class="i-carbon-copy" style="width:14px;height:14px;display:block;"></div></button>`
      + `</div>`
      + `</div>`
      + `<pre><code class="${lang ? `language-${escapeHtml(lang)} ` : ''}hljs">${highlighted}</code></pre>`
      + `</div>\n`
  }

  // 禁用外部链接跳转
  md.renderer.rules.link_open = (tokens, idx, options, _env, renderer) => {
    const token = tokens[idx]
    const href = token.attrGet('href')
    if (href && (href.startsWith('http://') || href.startsWith('https://'))) {
      token.attrSet('href', '#')
      token.attrSet('onclick', 'return false;')
      token.attrSet('style', 'cursor: default; text-decoration: none;')
      token.attrSet('title', `外部链接已禁用: ${href}`)
    }
    return renderer.renderToken(tokens, idx, options)
  }

  // 自动链接也禁用
  md.renderer.rules.autolink_open = (tokens, idx, options, _env, renderer) => {
    const token = tokens[idx]
    const href = token.attrGet('href')
    if (href && (href.startsWith('http://') || href.startsWith('https://'))) {
      token.attrSet('href', '#')
      token.attrSet('onclick', 'return false;')
      token.attrSet('style', 'cursor: default; text-decoration: none;')
      token.attrSet('title', `外部链接已禁用: ${href}`)
    }
    return renderer.renderToken(tokens, idx, options)
  }

  // 简化版 markdown-it（UpdateModal 使用，更保守）
  const mdSimple = new MarkdownIt({
    html: false,
    xhtmlOut: false,
    breaks: true,
    langPrefix: 'language-',
    linkify: true,
    typographer: true,
    highlight(str: string, lang: string) {
      if (lang && hljs.getLanguage(lang)) {
        try {
          return hljs.highlight(str, { language: lang }).value
        }
        catch { /* 忽略 */ }
      }
      return escapeHtml(str)
    },
  })

  // 渲染函数
  function renderMarkdown(content: string): string {
    try {
      return md.render(content)
    }
    catch (error) {
      console.error('Markdown 渲染失败:', error)
      return content
    }
  }

  function renderMarkdownSimple(content: string): string {
    try {
      return mdSimple.render(content)
    }
    catch (error) {
      console.error('Markdown 渲染失败:', error)
      return content
    }
  }

  // 加载 hljs 主题（本地文件，离线可用）
  async function loadHljsTheme(themeName: string, appTheme?: string) {
    // "auto" 模式根据应用主题选择
    const resolvedTheme = themeName === 'auto'
      ? (appTheme === 'light' ? 'github' : 'github-dark')
      : themeName

    const loader = THEME_LOADERS[resolvedTheme]
    if (!loader)
      return

    try {
      const css = await loader()

      // 移除旧样式
      const old = document.querySelector('style[data-hljs-theme]')
      if (old)
        old.remove()

      // 插入新样式
      const style = document.createElement('style')
      style.setAttribute('data-hljs-theme', resolvedTheme)
      style.textContent = css
      document.head.appendChild(style)

      hljsTheme.value = resolvedTheme
    }
    catch (error) {
      console.error('加载代码高亮主题失败:', error)
    }
  }

  return {
    hljsTheme,
    renderMarkdown,
    renderMarkdownSimple,
    loadHljsTheme,
    AVAILABLE_HLJS_THEMES,
  }
}

export function useMarkdown() {
  if (!instance) {
    instance = createMarkdownInstance()
  }
  return instance
}
