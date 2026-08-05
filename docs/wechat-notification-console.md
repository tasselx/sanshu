# 微信通知与管理控制台实施总结

## 目标

本次实现将每个新的 `zhi` 请求发送到已绑定的微信会话，并提供独立的微信管理控制台，集中管理通知策略、聊天记录、待处理请求和诊断日志。

## 通知与回复流程

1. `zhi` 创建新请求后，前端将请求内容和选项交给微信同步命令。
2. 后端生成包含项目、AI 实例、请求正文、选项和短码的 Markdown PNG，并发送到已绑定微信会话。
3. 用户在微信中按标准文本模板回复；身份行用于阅读，只有首行 `#短码` 参与路由。
4. 后端校验请求短码，解析选项和额外需求，再提交给原 `zhi` 请求；请求超过 300 秒会标记为过期并提示重新发起。

推荐回复模板：

```text
#ABC123
项目：sanshu
AI：Codex-实现
选择：A
补充：这里填写自定义额外需求
```

无补充内容时可省略第三行：

```text
#ABC123
项目：sanshu
AI：Codex-实现
选择：A
```

短码用于区分同时存在的多个 `zhi` 请求；项目别名和 AI 实例名帮助人在多个项目、多个 AI 并行时快速定位。管理页可以复制模板或修改项目别名。

## 多项目与多 AI

- 项目身份由规范化项目根路径生成，默认显示最后一级目录；同一路径可在管理页保存自定义别名。
- AI 实例名由调用方提供 `agent_label`；未提供时回退为 `AI-短码`。
- 每条请求拥有独立的短码和 300 秒有效期，待处理列表会持续刷新；已回复、已过期和已取消记录保留 24 小时后清理。

## Markdown 图片主题

通知正文沿用前端 Markdown 渲染器，支持代码高亮、KaTeX、任务列表和 Mermaid 流程图，再通过 DOM 转 PNG 生成 1080 × 1560 图片。设置页提供“跟随界面、纸张浅色、午夜深色”三种主题和预览；外部图片加载失败时使用占位图，避免整条通知失败。

## 管理控制台

微信设置入口已升级为管理弹窗，包含三个页签：

- **概览与配置**：显示启用状态、绑定状态、通知策略、待处理请求和安全诊断信息，并提供通知图片主题、主题预览、项目别名、绑定、测试及清除绑定操作。
- **聊天记录**：展示微信通知、回复、测试和系统消息；支持关键字、方向、类型筛选，以及复制和清空。
- **诊断日志**：读取最近 5000 行应用日志并筛选 `[wechat]` 记录；支持级别过滤、复制、刷新、打开日志目录及跳转完整日志查看器。

管理弹窗使用现有 Naive UI、UnoCSS 与 Carbon 图标体系，并保持项目现有低饱和度主题。

## 数据与边界

- 微信历史独立保存在 `%APPDATA%\sanshu\wechat-history.json`。
- 最多保留 200 条记录，单条内容最多保留 4000 个字符。
- 历史只保存文本和状态，不重复保存 PNG 数据。
- 诊断接口不返回 Token；用户标识按现有逻辑脱敏。
- 清除绑定不会同步删除历史，便于后续排查。

## 关键实现文件

- `src/rust/wechat/commands.rs`：绑定、通知、回复监听、历史写入及诊断命令。
- `src/rust/wechat/pending.rs`：待处理请求、短码生命周期、项目路径别名与 24 小时清理。
- `src/rust/wechat/history.rs`：独立历史文件、条数上限与内容截断。
- `src/rust/app/builder.rs`：Tauri 命令注册。
- `src/frontend/composables/useMcpHandler.ts`：将 `zhi` 请求正文传入微信通知链路。
- `src/frontend/components/settings/WechatSettings.vue`：微信管理入口与三页签弹窗。
- `src/frontend/components/settings/WechatPendingPanel.vue`：待处理请求、复制回复模板和项目别名管理。
- `src/frontend/utils/wechatNotificationImage.ts`：Markdown DOM 分页与主题图片生成。
- `src/frontend/components/settings/WechatHistoryPanel.vue`：微信聊天记录查询。
- `src/frontend/components/settings/WechatLogPanel.vue`：微信诊断日志查询。
- `src/frontend/types/wechat.ts`：前端共享类型。

## 验证

自动验证脚本：

```powershell
pwsh -File scripts/test-wechat-notification.ps1
```

脚本依次执行定向 Rust 格式检查、`cargo check`、微信短码/待处理生命周期单测、微信相关前端 ESLint 检查及 `pnpm build`。也可使用 `-SkipCheck` 或 `-SkipFrontend` 跳过对应阶段。

手工验收重点：

1. 新建 `zhi` 请求后，微信收到包含短码、正文和选项的 PNG。
2. 按标准模板回复后，原请求收到正确的选项与补充内容。
3. 打开“微信设置”，三个页签均可正常切换，窗口内容在小尺寸下保持可滚动。
4. 通知和回复进入聊天记录，筛选、复制、清空均正常。
5. 诊断日志只展示微信相关行，且可打开完整日志查看器和日志目录。
