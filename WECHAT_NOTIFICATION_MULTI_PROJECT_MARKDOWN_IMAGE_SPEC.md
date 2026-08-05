# 微信通知多项目、多 AI 与 Markdown 图片技术规范

## 1. 文档状态

- 日期：2026-08-06
- 状态：访谈完成，待用户批准，尚未实施
- 目标文件：`src/frontend/components/settings/WechatSettings.vue`
- 设计原则：KISS、YAGNI、SOLID；先修复可证明的并发路由缺口，再增加最小必要的识别与图片配置能力。

## 2. 已确认需求

1. 不同项目路径同时调用 `zhi` 时，微信通知必须能直接识别项目来源。
2. 同一路径由两个 AI 同时调用 `zhi` 时，微信通知必须能识别 AI 来源，并确保一条回复只提交给目标请求。
3. `zhi` 增加可选的 `agent_label`；调用方未传时使用 `AI-请求短码` 兜底。
4. 微信回复首行必须包含 `#请求短码`，短码是唯一回复路由键。
5. 请求默认 300 秒过期；过期后主动发送微信提示。
6. 回复模板必须自动带上请求短码、项目别名和 AI 别名，复制后只需修改选择或回复内容。
7. 项目别名默认取项目根目录末级，可在微信通知管理页修改；路径末级重名时展示父目录辅助区分。
8. 待处理请求在管理页集中展示；已回复、已取消或已过期记录保留 24 小时，完整消息继续使用现有聊天记录。
9. 微信通知图片使用完整 Markdown 渲染，支持标题、列表、引用、表格、代码、KaTeX、Mermaid 和内嵌图片。
10. 图片保持 1080×1560 自动分页；提供“跟随应用、纸白、深夜”三种卡片主题，代码高亮继续使用现有高亮主题配置。
11. 微信管理概览页提供图片主题切换和本地示例预览；远程图片或字体失败时使用中文占位，不能阻断整条通知。

## 3. 当前代码证据与问题

### 3.1 已有身份与请求数据

- `src/rust/mcp/types.rs:7-26` 的 `ZhiRequest` 已有 `workspace`，但没有 AI/客户端实例字段。
- `src/rust/mcp/tools/interaction/mcp.rs:53-70` 会把 `workspace` 映射到 `PopupRequest.project_root_path`。
- `src/frontend/types/popup.d.ts:3-11` 的 `McpRequest` 已有 `id` 与 `project_root_path`，因此项目路径无需重新推断或另建注册中心。
- `src/rust/mcp/server.rs:193-214` 手工声明 `zhi` 工具 schema，新增字段时必须同步更新，不能只修改 Rust 结构体。

### 3.2 并发误投缺口

- `src/rust/wechat/commands.rs:242-294` 每次通知都会启动独立的 `listen_for_reply`。
- `src/rust/wechat/commands.rs:449-496` 的每个监听器都轮询同一个绑定微信会话，但向各自 Tauri 进程发送 `wechat-event`。
- `src/rust/wechat/parser.rs:35-39` 仅在回复首行主动写了 `#短码` 时校验短码；省略短码后，多个监听器可能同时把“选择：A”或普通文本解析为自己的回复。
- 当前状态文件、历史文件和监听器均为本机全局资源。不同 AI 通常对应不同 MCP/GUI 进程，不能依赖进程内 `Mutex` 聚合待处理状态。

结论：项目/AI 标签只解决“看得懂”，强制短码才解决“投得准”。两者都必须实现。

### 3.3 当前图片并非 Markdown 渲染

- `src/frontend/utils/wechatNotificationImage.ts:12-48` 固定生成 1080×1560、每页 31 行的 Canvas 图片。
- `src/frontend/utils/wechatNotificationImage.ts:85-122` 只绘制纯文本，颜色仅跟随应用明暗模式。
- `src/frontend/composables/useMarkdown.ts:1-5,21-33,102-119` 已有 MarkdownIt、KaTeX、Mermaid 语言识别和六种代码高亮主题。
- `src/frontend/components/popup/PopupContent.vue:66-138,775-840` 已有 Mermaid 异步渲染、Markdown 图片预览和主题 class，但这些能力尚未被通知图片复用。

结论：不能继续在现有 Canvas 中逐字符模拟完整 Markdown；应复用浏览器 DOM 渲染结果再转 PNG。

## 4. 方案对比

### 4.1 并发回复路由

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 始终强制 `#短码` | 规则单一；不依赖跨进程计数；同路径和不同路径行为一致 | 回复必须保留模板首行 | 采用 |
| 仅多请求时强制短码 | 单请求少写一行 | 必须维护跨进程活动计数，存在竞态，规则难解释 | 不采用 |
| 广播给所有 AI 再二次选择 | 看似容错 | 会产生重复提交和噪音，违背 KISS/YAGNI | 不采用 |

### 4.2 待处理状态存储

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 每请求一个独立 JSON 文件 | 不同进程写不同文件；天然避免总文件覆盖；扫描即可汇总 | 需要定期清理小文件 | 采用 |
| 单一 `pending.json` | 文件数量少 | 多进程读改写会丢更新，需要跨进程锁 | 不采用 |
| 常驻协调服务/数据库 | 一致性强 | 引入服务生命周期和迁移，超出本次范围 | 不采用 |

### 4.3 Markdown 转图片

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 隐藏 DOM + `dom-to-image-more` | 直接复用 Markdown/CSS/KaTeX/Mermaid；支持 TypeScript、`pixelRatio`、隐藏节点捕获 | 需新增一个前端依赖；外部资源需兜底 | 采用 |
| 继续手写 Canvas Markdown 排版 | 无新依赖 | 表格、公式、流程图和复杂布局实现量大且易失真 | 不采用 |
| 截取可见弹窗 | 所见即所得 | 依赖窗口尺寸与可见状态，分页、后台发送不稳定 | 不采用 |

实时资料核对显示 `dom-to-image-more` 在 2026-07-10 发布到 3.10.1，仓库包含 TypeScript 定义，并提供 `pixelRatio`、隐藏根节点捕获和外部资源过滤能力。实施时锁定实际可安装版本并记录许可证；如果安装或 WebView2 冒烟失败，不私自换库，回到 `zhi` 调整方案。

## 5. 对外协议

### 5.1 `zhi` 请求新增字段

```json
{
  "brief": "...",
  "choices": [],
  "render_markdown": true,
  "workspace": "E:/ProjectCode/RustCode/sanshu",
  "agent_label": "Codex-实现"
}
```

规则：

- `agent_label` 可选，去除首尾空白后最大 40 个 Unicode 字符。
- 空值、全空白或超长值不直接写入图片；空值回退为 `AI-<短码>`，超长值返回明确参数错误。
- CLI 同步增加 `--agent-label`，保持 MCP 与 `等一下 --cli` 能力一致。
- 不增加 `project_label` 请求字段。项目别名由规范化路径映射统一管理，避免不同 AI 为同一路径传出不一致名称。

涉及链路：

1. `ZhiRequest.agent_label`
2. `PopupRequest.agent_label`
3. 临时请求 JSON
4. 前端 `McpRequest.agent_label`
5. 微信通知 payload

### 5.2 项目路径规范化与别名

- Windows 去除 `\\?\` 或 `//?/` 前缀。
- 分隔符统一为 `/`。
- Windows 路径键转小写；显示值保留原始大小写。
- 去除尾部分隔符。
- 默认别名为末级目录；若活动/近 24 小时记录中出现同名不同路径，则显示 `父目录/末级目录`。
- 用户修改的别名写入 `WechatConfig.project_aliases: HashMap<String, String>`，键为规范化路径。
- 自定义别名去除首尾空白，长度 1～40；禁止控制字符。

### 5.3 回复模板

有选项：

```text
三术 zhi #ABC123
项目：sanshu
AI：Codex-实现
A. 方案一
B. 方案二

复制并修改：
#ABC123
项目：sanshu
AI：Codex-实现
选择：A
补充：
```

无选项：

```text
三术 zhi #ABC123
项目：sanshu
AI：Codex-实现

复制并修改：
#ABC123
项目：sanshu
AI：Codex-实现
回复：在这里填写回复
```

解析规则：

1. 第一条非空行必须严格为 `#<当前请求短码>`，忽略 ASCII 大小写。
2. 缺少短码或短码不匹配时返回 `None`，不得发出 `wechat-event`。
3. `项目：` 和 `AI：` 行只用于人工核对，解析器忽略其内容，不参与路由。
4. 有选项时仍解析 `选择：`、`补充：`；无选项时解析 `回复：`。
5. 不再接受无短码的“继续”、单字母选项或自由文本。

## 6. 待处理请求模型与状态机

### 6.1 数据结构

```rust
struct WechatPendingRequest {
    request_id: String,
    request_code: String,
    project_root_path: String,
    project_key: String,
    project_alias: String,
    agent_label: String,
    prompt_preview: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: WechatPendingStatus,
    completion_source: Option<String>,
}

enum WechatPendingStatus {
    Pending,
    Replied,
    Expired,
    Cancelled,
}
```

约束：

- `prompt_preview` 使用现有安全截断策略，只保存最小必要摘要，不保存图片 Base64。
- 每个请求写入 `%APPDATA%/sanshu/wechat-pending/<request_id>.json`。
- 写入使用“同目录临时文件 + 原子重命名”，避免半截 JSON。
- 文件名只使用服务端生成的 UUID；禁止把工作区或别名拼进路径。
- 读取时忽略并记录单个损坏文件，不能让整个管理页失败。

### 6.2 状态转换

```text
通知图片和模板发送成功
        |
        v
     pending ---------------------> replied
        |                     微信或桌面完成
        |
        +---- 300 秒到期 ----------> expired
        |
        +---- 用户取消/进程关闭 ----> cancelled
```

- `start_wechat_sync` 在图片和回复模板全部发送成功后登记 `pending`，再启动回复监听。
- `listen_for_reply` 最长等待 300 秒。
- 微信成功解析并 emit 后标记 `replied`，`completion_source = "wechat"`。
- 桌面提交前标记 `replied`，`completion_source = "desktop"`；桌面取消标记 `cancelled`。
- 到期时标记 `expired`，并主动发送：

```text
项目 sanshu · AI Codex-实现 · #ABC123 已过期，请回到对应 zhi 重新发起。
```

- 若拥有请求的 GUI/MCP 进程提前异常退出，管理页扫描时以 `expires_at` 懒计算为 `expired`；主动微信到期提示属于进程存活时的保证，异常退出场景在诊断日志明确记录为未发送。
- `replied`、`expired`、`cancelled` 文件在 `updated_at + 24h` 后由查询命令顺手清理；聊天记录仍按现有上限保存。

## 7. Markdown 通知图片

### 7.1 输入

图片渲染请求补齐：

```ts
interface WechatNotificationRequest {
  id: string
  message: string
  predefined_options: string[]
  is_markdown: boolean
  project_root_path: string
  project_alias: string
  agent_label: string
  image_theme: 'auto' | 'paper' | 'midnight'
}
```

- `is_markdown = true`：使用现有 MarkdownIt 规则渲染。
- `is_markdown = false`：使用 `textContent` 和 `white-space: pre-wrap`，禁止把纯文本当 HTML 注入。

### 7.2 渲染流程

1. 创建离屏但可计算布局的通知根节点；不能使用 `display: none`。
2. 应用固定 1080×1560 卡片尺寸和选定主题 token。
3. 渲染 Markdown HTML，并复用现有代码高亮、KaTeX 与安全配置。
4. 调用共享 Mermaid 渲染函数，保持 `securityLevel: "strict"`。
5. 等待 `document.fonts.ready`；本地/内联图片等待完成，远程资源最多等待 3 秒。
6. 加载失败的图片替换为“图片加载失败：<安全截断的 alt>”占位；失败字体回退到系统中文字体。
7. 测量内容高度，以页面内容区高度计算页数；优先在顶层块边界分页，无法避开时允许在自然行边界续页。
8. 每页保留固定头部与尾部；正文通过独立页面容器捕获，不能先生成无限长大图再整体切割。
9. 使用 DOM 转图工具输出 PNG Base64；生成后立即销毁离屏节点和对象 URL。
10. 任一页失败时整次图片生成返回明确错误，不发送缺页通知；远程资源占位不视为生成失败。

### 7.3 固定版式

- 画布：1080×1560。
- 页头：`三术 · zhi`、项目别名、AI 别名、`#短码`、页码。
- 正文：Markdown 内容。
- 选项：正文后作为独立选择区渲染，不把选项再次送入 Markdown 解析。
- 页尾：`回复模板会在图片后单独发送，可直接复制修改。`
- PNG 使用 1× 像素比，保持当前 1080×1560 实际输出，避免无需求依据地扩大微信上传体积。

### 7.4 主题

| 配置值 | 含义 | 行为 |
| --- | --- | --- |
| `auto` | 跟随应用 | 根据当前应用 light/dark 选择纸白或深夜 token |
| `paper` | 纸白 | 浅色纸张、深色正文、低饱和强调色 |
| `midnight` | 深夜 | 深色表面、gray-300 以上正文对比、低饱和强调色 |

代码高亮主题不与卡片主题重复存储，继续读取现有 `ui_config.hljs_theme`。

## 8. 微信通知管理 UI

保持现有“概览与配置 / 聊天记录 / 诊断日志”三页签，不新增图片工坊页。

### 8.1 概览卡片

- 顶部状态卡新增“待处理”数量；原“聊天记录”数量保留。
- “通知配置”内新增“通知图片”小节：
  - 三主题单选。
  - 当前代码高亮主题只读说明。
  - “预览示例”按钮。
- 预览使用与真实通知完全相同的 renderer 和分页逻辑；示例包含标题、列表、引用、表格、代码、公式、Mermaid 和图片占位。

### 8.2 项目标识

- 新增“项目标识”区，来源为活动请求和近 24 小时请求。
- 每行展示：别名、脱敏路径、编辑按钮。
- 编辑保存后只影响后续通知；已发出的图片和模板不追溯修改。
- 路径信息使用文本而非仅颜色区分；深色正文不低于 `gray-300`。

### 8.3 待处理请求

- 建议拆为 `WechatPendingPanel.vue`，避免继续扩大已达 431 行的 `WechatSettings.vue`。
- 每项展示：项目、AI、短码、剩余秒数、状态、创建时间、提示摘要。
- `pending` 显示剩余秒数；`replied/expired/cancelled` 显示中文状态原因。
- 提供“复制回复模板”，不提供在管理页替 AI 直接提交的旁路操作。
- 状态不能只靠颜色表达；图标与文字并用，沿用 Naive UI 和现有低饱和视觉语言。

## 9. 命令与文件改动范围

### 9.1 Rust

- `src/rust/mcp/types.rs`
  - 增加 `agent_label` 并贯穿 `ZhiRequest`、`PopupRequest`。
- `src/rust/mcp/server.rs`
  - 更新手工维护的 `zhi` JSON schema。
- `src/rust/mcp/tools/interaction/mcp.rs`
  - 校验、规范化并透传 `agent_label`。
- `src/rust/app/cli.rs`
  - 增加 `--agent-label`。
- `src/rust/config/settings.rs`
  - `WechatConfig` 增加 `notification_image_theme`、`project_aliases` 及 serde 默认值。
- `src/rust/wechat/pending.rs`（新增）
  - 单请求文件、原子写入、扫描、状态更新、24 小时清理、路径规范化和默认别名。
- `src/rust/wechat/parser.rs`
  - 强制短码；忽略项目/AI 标识行；补齐单元测试。
- `src/rust/wechat/commands.rs`
  - payload 元数据、300 秒超时、状态转换、到期通知、查询/别名命令。
- `src/rust/wechat/mod.rs`、`src/rust/app/builder.rs`
  - 注册模块与 Tauri 命令。

### 9.2 前端

- `src/frontend/types/popup.d.ts`
  - 增加 `agent_label`。
- `src/frontend/types/wechat.ts`
  - 增加主题、别名和待处理类型。
- `src/frontend/composables/useMcpHandler.ts`
  - 把项目路径、AI 标签、Markdown 标记和主题传入图片与同步命令；桌面完成时更新状态。
- `src/frontend/utils/wechatNotificationImage.ts`
  - 从手写纯文本 Canvas 改为异步 DOM Markdown 分页渲染。
- `src/frontend/utils/markdownRenderRoot.ts`（建议新增）
  - 提取可复用的 Mermaid/图片等待逻辑，避免复制 `PopupContent.vue` 的渲染实现。
- `src/frontend/components/settings/WechatSettings.vue`
  - 接入主题、预览、项目别名和待处理概览。
- `src/frontend/components/settings/WechatPendingPanel.vue`（新增）
  - 聚合请求状态和回复模板复制。
- `package.json`、`pnpm-lock.yaml`
  - 增加经验证的 DOM 转图依赖。

### 9.3 文档与验证脚本

- 更新 `docs/wechat-notification-console.md`，写明强制短码、300 秒、项目/AI 标签和 Markdown 主题。
- 扩展 `scripts/test-wechat-notification.ps1`，不新建重复入口。

## 10. 测试与验证

### 10.1 Rust 自动测试

1. 有选项/无选项均拒绝缺少短码的回复。
2. 拒绝错误短码，接受大小写不同的正确短码。
3. 项目/AI 标识行不进入用户回复。
4. 项目路径规范化覆盖 Windows 扩展前缀、斜杠、大小写和尾分隔符。
5. 默认项目别名、重名展示、自定义别名校验。
6. 单请求文件原子写入、损坏文件隔离、24 小时清理。
7. `pending -> replied/expired/cancelled` 合法转换；禁止终态回到 pending。
8. 300 秒超时使用暂停时间的 Tokio 测试，不真实等待 300 秒。
9. 新旧 `config.json` 反序列化兼容，缺少新字段时使用默认值。

### 10.2 前端自动测试

1. 主题值与配置序列化。
2. agent 缺省回退、项目别名和回复模板纯函数。
3. 分页计算和固定头尾。
4. 远程图片失败替换占位，不使整次渲染失败。
5. Markdown=false 时不执行 HTML。
6. 待处理列表的倒计时与中文状态显示。

DOM 转 PNG、KaTeX 和 Mermaid 最终像素结果必须使用 Vitest Browser/Playwright 或实际 WebView2 验证；JSDOM 不能替代 Canvas、字体和 SVG `foreignObject` 验收。

### 10.3 脚本与构建

扩展 `scripts/test-wechat-notification.ps1`，按顺序执行：

1. 涉及 Rust 文件的 `rustfmt --check`。
2. 微信 parser/pending/config 的定向 `cargo test`。
3. 前端定向 Vitest。
4. `pnpm lint` 的范围检查或项目现有等价命令。
5. `cargo check`。
6. 前端生产构建。

用户本轮明确要求实现后生成测试脚本、编译并运行；获得方案批准并实施后执行上述验证，再启动应用进行一次实际预览与并发请求冒烟。任何既有失败与本次回归分开报告。

### 10.4 手工验收场景

1. 两个不同路径项目同时发起 zhi：图片、模板、管理列表均显示不同项目别名。
2. 同一路径两个 AI 同时发起：显示不同 `agent_label` 或不同兜底标签。
3. 复制请求 A 的模板回复，只请求 A 完成，请求 B 继续等待。
4. 删除 `#短码` 后回复，两个请求均不完成，日志记录拒绝原因。
5. 等待 300 秒，状态变过期并收到包含项目、AI、短码的微信提示。
6. 桌面先完成后，对应待处理状态不再倒计时。
7. 三种主题预览均正确；长 Markdown 生成多页且页码连续。
8. 表格、代码、KaTeX、Mermaid、内嵌图片可见；失败远程图片显示中文占位。
9. 重启后读取旧配置不报错，项目别名仍可用，24 小时外状态被清理。

## 11. 非目标

- 不引入云端项目注册中心。
- 不广播一条微信回复给多个 AI。
- 不新增 AI 图像生成服务或 Markdown 生图指令。
- 不新增独立“图片工坊”或 Markdown 编辑器。
- 不改变 Telegram 行为。
- 不永久保存待处理状态，不复制一套与聊天记录重复的审计库。
- 不把完整工作区路径发送到微信；默认只发送别名，完整路径仅在本机管理页脱敏展示。

## 12. 实施顺序与 Go/No-Go

1. 协议与类型：`agent_label`、配置默认值、前后端类型。
2. 回复安全：强制短码、模板标识、parser 测试。
3. 待处理状态：独立文件、300 秒、24 小时清理、管理命令。
4. Markdown 图片：共享渲染、三主题、分页、资源占位。
5. 管理 UI：主题预览、项目别名、待处理面板。
6. 文档、测试脚本、编译、运行和实际并发验收。

Go 条件：用户通过 `zhi` 明确批准本规范并授权实施。

No-Go 条件：DOM 转图依赖无法安装、WebView2 不支持关键渲染、现有微信接口无法在 300 秒到期发送提示，或实施中发现必须引入常驻协调服务。出现任一条件时停止修改并通过 `zhi` 重新确认，不自行扩大范围。
