interface WechatNotificationRequest {
  id?: string
  message?: string
  predefined_options?: string[]
}

interface RenderLine {
  text: string
  kind: 'body' | 'option' | 'muted'
}

const IMAGE_WIDTH = 1080
const IMAGE_HEIGHT = 1560
const PADDING = 72
const CONTENT_WIDTH = IMAGE_WIDTH - PADDING * 2
const LINE_HEIGHT = 40
const LINES_PER_PAGE = 31

export function renderWechatNotificationImages(request: WechatNotificationRequest): string[] {
  const probe = document.createElement('canvas')
  const probeContext = probe.getContext('2d')
  if (!probeContext)
    throw new Error('Canvas 初始化失败')

  probeContext.font = '28px system-ui, "Microsoft YaHei", sans-serif'
  const code = requestShortCode(request.id || '')
  const lines: RenderLine[] = []
  appendWrappedLines(probeContext, lines, request.message || '', 'body')

  if ((request.predefined_options?.length ?? 0) > 0) {
    lines.push({ text: '', kind: 'muted' })
    request.predefined_options!.forEach((option, index) => {
      appendWrappedLines(
        probeContext,
        lines,
        `${String.fromCharCode(65 + index)}. ${option}`,
        'option',
      )
    })
  }

  const pages: string[] = []
  const totalPages = Math.max(1, Math.ceil(lines.length / LINES_PER_PAGE))
  for (let pageIndex = 0; pageIndex < totalPages; pageIndex++) {
    const pageLines = lines.slice(pageIndex * LINES_PER_PAGE, (pageIndex + 1) * LINES_PER_PAGE)
    pages.push(drawPage(pageLines, code, pageIndex + 1, totalPages))
  }
  return pages
}

function appendWrappedLines(
  context: CanvasRenderingContext2D,
  target: RenderLine[],
  text: string,
  kind: RenderLine['kind'],
) {
  for (const paragraph of text.replace(/\r\n/g, '\n').split('\n')) {
    if (!paragraph) {
      target.push({ text: '', kind })
      continue
    }
    let current = ''
    for (const character of paragraph) {
      const candidate = current + character
      if (current && context.measureText(candidate).width > CONTENT_WIDTH) {
        target.push({ text: current, kind })
        current = character
      }
      else {
        current = candidate
      }
    }
    target.push({ text: current, kind })
  }
}

function drawPage(lines: RenderLine[], code: string, page: number, totalPages: number): string {
  const canvas = document.createElement('canvas')
  canvas.width = IMAGE_WIDTH
  canvas.height = IMAGE_HEIGHT
  const context = canvas.getContext('2d')
  if (!context)
    throw new Error('Canvas 初始化失败')

  const dark = document.documentElement.classList.contains('dark')
  const colors = dark
    ? { background: '#111318', surface: '#191c23', title: '#f3f4f6', body: '#d1d5db', muted: '#9ca3af', accent: '#9ca3d9' }
    : { background: '#f5f6f8', surface: '#ffffff', title: '#17191f', body: '#374151', muted: '#6b7280', accent: '#6269a8' }

  context.fillStyle = colors.background
  context.fillRect(0, 0, IMAGE_WIDTH, IMAGE_HEIGHT)
  context.fillStyle = colors.surface
  roundRect(context, 36, 36, IMAGE_WIDTH - 72, IMAGE_HEIGHT - 72, 24)
  context.fill()

  context.fillStyle = colors.accent
  context.font = '600 26px system-ui, "Microsoft YaHei", sans-serif'
  context.fillText('三术 · zhi', PADDING, 112)
  context.fillStyle = colors.muted
  context.font = '24px system-ui, "Microsoft YaHei", sans-serif'
  context.textAlign = 'right'
  context.fillText(`#${code}  ${page}/${totalPages}`, IMAGE_WIDTH - PADDING, 112)
  context.textAlign = 'left'

  context.fillStyle = colors.title
  context.font = '700 38px system-ui, "Microsoft YaHei", sans-serif'
  context.fillText('需要你的确认', PADDING, 178)

  let y = 250
  for (const line of lines) {
    context.fillStyle = line.kind === 'option' ? colors.accent : line.kind === 'muted' ? colors.muted : colors.body
    context.font = line.kind === 'option'
      ? '600 28px system-ui, "Microsoft YaHei", sans-serif'
      : '28px system-ui, "Microsoft YaHei", sans-serif'
    context.fillText(line.text, PADDING, y)
    y += LINE_HEIGHT
  }

  context.fillStyle = colors.muted
  context.font = '22px system-ui, "Microsoft YaHei", sans-serif'
  context.fillText('回复模板会在图片后单独发送，可直接复制修改。', PADDING, IMAGE_HEIGHT - 82)
  return canvas.toDataURL('image/png').split(',')[1]
}

function requestShortCode(requestId: string): string {
  const code = requestId.replace(/[^a-z0-9]/gi, '').slice(0, 6).toUpperCase()
  return code || 'ZHI'
}

function roundRect(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  context.beginPath()
  context.roundRect(x, y, width, height, radius)
}
