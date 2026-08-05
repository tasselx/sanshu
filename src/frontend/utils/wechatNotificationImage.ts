import domToImage from 'dom-to-image-more'
import { safeBase64Decode, useMarkdown } from '../composables/useMarkdown'

export type WechatNotificationImageTheme = 'auto' | 'paper' | 'midnight'

export interface WechatNotificationRequest {
  id?: string
  message?: string
  predefined_options?: string[]
  is_markdown?: boolean
  project_alias?: string
  agent_label?: string
  image_theme?: WechatNotificationImageTheme
}

const IMAGE_WIDTH = 1080
const IMAGE_HEIGHT = 1560
const PAGE_PADDING = 72
const HEADER_HEIGHT = 190
const FOOTER_HEIGHT = 76
const CONTENT_WIDTH = IMAGE_WIDTH - PAGE_PADDING * 2
const CONTENT_HEIGHT = IMAGE_HEIGHT - HEADER_HEIGHT - FOOTER_HEIGHT - PAGE_PADDING

const THEME_TOKENS: Record<Exclude<WechatNotificationImageTheme, 'auto'>, {
  background: string
  surface: string
  title: string
  body: string
  muted: string
  accent: string
  border: string
}> = {
  paper: {
    background: '#eef0f3',
    surface: '#ffffff',
    title: '#17191f',
    body: '#374151',
    muted: '#6b7280',
    accent: '#0f766e',
    border: '#d1d5db',
  },
  midnight: {
    background: '#111318',
    surface: '#191c23',
    title: '#f3f4f6',
    body: '#d1d5db',
    muted: '#9ca3af',
    accent: '#5eead4',
    border: '#374151',
  },
}

const IMAGE_PLACEHOLDER = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(`
<svg xmlns="http://www.w3.org/2000/svg" width="936" height="220" viewBox="0 0 936 220">
  <rect width="936" height="220" rx="12" fill="#374151"/>
  <text x="468" y="112" text-anchor="middle" dominant-baseline="middle" fill="#e5e7eb" font-family="Microsoft YaHei, sans-serif" font-size="28">图片加载失败</text>
</svg>`)} `

function requestShortCode(requestId: string): string {
  const code = requestId.replace(/[^a-z0-9]/gi, '').slice(0, 6).toUpperCase()
  return code || 'ZHI'
}

function resolveTheme(theme: WechatNotificationImageTheme = 'auto'): Exclude<WechatNotificationImageTheme, 'auto'> {
  if (theme === 'paper' || theme === 'midnight')
    return theme
  return document.documentElement.classList.contains('dark') ? 'midnight' : 'paper'
}

function waitForImages(root: HTMLElement): Promise<void> {
  const images = Array.from(root.querySelectorAll<HTMLImageElement>('img'))
  return Promise.all(images.map((image) => {
    if (image.complete)
      return Promise.resolve()
    return new Promise<void>((resolve) => {
      const done = () => {
        image.removeEventListener('load', done)
        image.removeEventListener('error', done)
        resolve()
      }
      image.addEventListener('load', done, { once: true })
      image.addEventListener('error', done, { once: true })
      window.setTimeout(done, 3000)
    })
  })).then(() => undefined)
}

async function renderMermaidBlocks(root: HTMLElement, requestId: string) {
  const wrappers = Array.from(root.querySelectorAll<HTMLElement>('.mermaid-block-wrapper'))
  if (!wrappers.length)
    return
  const mermaid = (await import('mermaid')).default
  mermaid.initialize({
    startOnLoad: false,
    theme: root.classList.contains('theme-light') ? 'default' : 'dark',
    securityLevel: 'strict',
    logLevel: 'error',
    flowchart: { htmlLabels: false, curve: 'basis' },
  })
  for (const [index, wrapper] of wrappers.entries()) {
    const target = wrapper.querySelector<HTMLElement>('.mermaid-render')
    const encodedCode = wrapper.getAttribute('data-diagram-code')
    if (!target || !encodedCode)
      continue
    try {
      const { svg } = await mermaid.render(
        `wechat-mermaid-${requestId || 'request'}-${index}`.replace(/[^\w-]/g, '-'),
        safeBase64Decode(encodedCode),
      )
      target.innerHTML = svg
      target.classList.remove('mermaid-error')
    }
    catch (error) {
      target.classList.add('mermaid-error')
      target.textContent = `流程图渲染失败：${String(error)}`
    }
  }
}

function appendReplyOptions(root: HTMLElement, options: string[], tokens: ReturnType<typeof getThemeTokens>) {
  if (!options.length)
    return
  const section = document.createElement('section')
  section.className = 'wechat-notification-options'
  section.style.cssText = `margin-top:24px;padding-top:18px;border-top:1px solid ${tokens.border};`
  options.forEach((option, index) => {
    const line = document.createElement('div')
    line.textContent = `${String.fromCharCode(65 + index)}. ${option}`
    line.style.cssText = `margin:6px 0;color:${tokens.accent};font-weight:600;`
    section.appendChild(line)
  })
  root.appendChild(section)
}

function getThemeTokens(theme: Exclude<WechatNotificationImageTheme, 'auto'>) {
  return THEME_TOKENS[theme]
}

function createPageRoot(
  request: WechatNotificationRequest,
  contentHtml: string,
  offset: number,
  totalPages: number,
  page: number,
  theme: Exclude<WechatNotificationImageTheme, 'auto'>,
) {
  const tokens = getThemeTokens(theme)
  const root = document.createElement('div')
  root.className = `wechat-notification-card wechat-notification-theme-${theme}`
  root.style.cssText = [
    `width:${IMAGE_WIDTH}px`,
    `height:${IMAGE_HEIGHT}px`,
    `box-sizing:border-box`,
    `padding:${PAGE_PADDING}px`,
    `background:${tokens.background}`,
    `font-family:system-ui,"Microsoft YaHei",sans-serif`,
    `color:${tokens.body}`,
    `overflow:hidden`,
    `position:fixed`,
    `left:-100000px`,
    `top:0`,
    `z-index:-1`,
  ].join(';')

  const surface = document.createElement('div')
  surface.style.cssText = [
    'width:100%',
    'height:100%',
    'box-sizing:border-box',
    'padding:0',
    `background:${tokens.surface}`,
    `border:1px solid ${tokens.border}`,
    'border-radius:24px',
    'overflow:hidden',
    'position:relative',
  ].join(';')

  const header = document.createElement('header')
  header.style.cssText = `height:${HEADER_HEIGHT}px;box-sizing:border-box;padding:32px 36px 18px;`
  const headerTitle = document.createElement('div')
  headerTitle.textContent = '三术 · zhi'
  headerTitle.style.cssText = `color:${tokens.accent};font-size:26px;font-weight:600;`
  const headerMeta = document.createElement('div')
  headerMeta.textContent = `项目：${request.project_alias || '未命名项目'}  ·  AI：${request.agent_label || `AI-${requestShortCode(request.id || '')}`}`
  headerMeta.style.cssText = `margin-top:12px;color:${tokens.muted};font-size:23px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;`
  const headerCode = document.createElement('div')
  headerCode.textContent = `#${requestShortCode(request.id || '')}  ·  ${page}/${totalPages}`
  headerCode.style.cssText = `margin-top:8px;color:${tokens.muted};font:24px ui-monospace,SFMono-Regular,Consolas,monospace;`
  header.append(headerTitle, headerMeta, headerCode)

  const viewport = document.createElement('div')
  viewport.style.cssText = `height:${CONTENT_HEIGHT}px;overflow:hidden;padding:0 36px;box-sizing:border-box;`
  const content = document.createElement('div')
  content.className = `markdown-content ${theme === 'paper' ? 'theme-light' : 'theme-dark'}`
  content.style.cssText = [
    `width:${CONTENT_WIDTH - 72}px`,
    `transform:translateY(-${offset}px)`,
    'transform-origin:top left',
    `color:${tokens.body}`,
    'font-size:24px',
    'line-height:1.55',
  ].join(';')
  content.innerHTML = contentHtml
  viewport.appendChild(content)

  const footer = document.createElement('footer')
  footer.textContent = '回复模板会在图片后单独发送，可直接复制修改。'
  footer.style.cssText = `height:${FOOTER_HEIGHT}px;box-sizing:border-box;padding:20px 36px 0;color:${tokens.muted};font-size:20px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;`

  surface.append(header, viewport, footer)
  root.appendChild(surface)
  return root
}

function getPageOffsets(contentRoot: HTMLElement): number[] {
  const height = contentRoot.scrollHeight
  if (height <= CONTENT_HEIGHT)
    return [0]
  const offsets = [0]
  let offset = 0
  while (offset + CONTENT_HEIGHT < height) {
    const candidates = Array.from(contentRoot.children)
      .map(element => (element as HTMLElement).offsetTop)
      .filter(top => top > offset + 12 && top <= offset + CONTENT_HEIGHT)
    const next = candidates.length ? Math.max(...candidates) : offset + CONTENT_HEIGHT
    offset = Math.max(next, offset + 1)
    offsets.push(offset)
  }
  return offsets
}

export async function renderWechatNotificationImages(request: WechatNotificationRequest): Promise<string[]> {
  if (typeof document === 'undefined')
    throw new Error('Markdown 图片生成需要浏览器环境')

  const theme = resolveTheme(request.image_theme)
  const tokens = getThemeTokens(theme)
  const markdown = useMarkdown()
  const contentRoot = document.createElement('div')
  contentRoot.className = `markdown-content ${theme === 'paper' ? 'theme-light' : 'theme-dark'}`
  contentRoot.style.cssText = [
    `width:${CONTENT_WIDTH - 72}px`,
    'position:fixed',
    'left:-100000px',
    'top:0',
    'visibility:hidden',
    `color:${tokens.body}`,
    'font-size:24px',
    'line-height:1.55',
  ].join(';')
  contentRoot.innerHTML = request.is_markdown === false
    ? `<p>${escapeHtml(request.message || '').replace(/\n/g, '<br>')}</p>`
    : markdown.renderMarkdown(request.message || '')
  appendReplyOptions(contentRoot, request.predefined_options || [], tokens)
  document.body.appendChild(contentRoot)

  try {
    await renderMermaidBlocks(contentRoot, request.id || '')
    await waitForImages(contentRoot)
    if (document.fonts?.ready)
      await document.fonts.ready
    await new Promise(resolve => window.requestAnimationFrame(() => resolve(undefined)))

    const offsets = getPageOffsets(contentRoot)
    const pages: string[] = []
    for (const [index, offset] of offsets.entries()) {
      const pageRoot = createPageRoot(
        request,
        contentRoot.innerHTML,
        offset,
        offsets.length,
        index + 1,
        theme,
      )
      document.body.appendChild(pageRoot)
      try {
        pages.push(await domToImage.toPng(pageRoot, {
          width: IMAGE_WIDTH,
          height: IMAGE_HEIGHT,
          pixelRatio: 1,
          imagePlaceholder: IMAGE_PLACEHOLDER.trim(),
          httpTimeout: 3000,
          ignoreCSSRuleErrors: true,
          onImageError: info => console.warn('微信通知图片资源加载失败:', info.url),
        }))
      }
      finally {
        pageRoot.remove()
      }
    }
    return pages.map(dataUrl => dataUrl.split(',')[1] || dataUrl)
  }
  finally {
    contentRoot.remove()
  }
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}
