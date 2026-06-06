# 全局日志与调试页改造总结

## 背景

本次改造解决以下问题：

- “实时日志”弹窗显示了行数但内容区为空。
- “查看日志”只复制到剪贴板，不符合“看到日志”的预期。
- 日志时间使用 UTC 但未标注时区，容易被误认为系统时间异常。
- 全局日志入口混放在 Sou 配置页。
- MCP 工具配置弹窗缺少统一高度边界，长内容会撑高窗口。

## 实现方案

采用“独立全局日志与调试页”方案：

- 在主界面新增“日志”Tab，集中展示日志状态、日志路径和全局日志操作。
- 复用现有 `AcemcpLogViewerDrawer` 作为统一日志查看器，避免重复实现大日志渲染逻辑。
- Sou 配置页只保留 Sou / ACE 专属调试能力。
- 修复 `@tanstack/vue-virtual` 的 `count` 配置，使日志行变化后虚拟列表能正确渲染。
- 将日志格式化时间改为本地时区。
- 在 MCP 工具配置弹窗外层增加统一滚动边界。

## 关键文件

- `src/frontend/components/tabs/LogsTab.vue`：新增全局日志与调试页。
- `src/frontend/components/layout/MainLayout.vue`：新增“日志”Tab。
- `src/frontend/components/tools/AcemcpLogViewerDrawer.vue`：修复虚拟列表响应式渲染。
- `src/frontend/components/tools/SouConfig.vue`：移除全局日志入口，保留 Sou 专属调试。
- `src/frontend/components/tabs/McpToolsTab.vue`：增加配置弹窗内部滚动边界。
- `src/rust/utils/logger.rs`：日志时间改为本地时区。

## 验证方式

- 运行 `pnpm build` 验证前端类型和构建。
- 运行 `cargo check` 验证 Rust 代码。

## 手工验收点

- 打开主界面“日志”Tab，能看到日志状态、路径和操作按钮。
- 点击“查看日志”或“查看实时日志”，日志抽屉应显示已有日志并继续追加新增日志。
- 日志时间应按本机时区显示。
- Sou 配置页不再出现全局日志按钮。
- MCP 工具配置弹窗内容过长时在弹窗内部滚动，不撑高整个窗口。
