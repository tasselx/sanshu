# fast-context 面板适配修复报告

> 日期：2026-06-05
> 涉及文件：5 个（前端 4 + 后端 1）
> 改动规模：+101 / -14 行

## 问题描述

用户配置 `sou_default_backend: fast_context` 后，弹窗界面仍然显示：
- 顶部 Header：「代码索引 索引失败」（红色告警）
- 面板引导条：「配置 API 密钥以启用代码索引」

同时 MCP 日志中每次 `ji` 调用后都出现：
```
后台索引失败: project_root=..., error=未配置 base_url
```

## 根因分析

### Bug 1：前端 `checkAcemcpConfigured()` 只检查 ACE 配置

`useAcemcpSync.ts` 中的 `checkAcemcpConfigured()` 硬编码只检查 `base_url` 和 `token`（ACE 后端所需），完全忽略了 `sou_default_backend` 的实际值。当后端策略为 `fast_context` 时，ACE 配置为空是正常的，但函数返回 `false` 导致弹窗显示误导性引导。

### Bug 2：后端 `try_trigger_background_index` 只走 ACE

`memory/mcp.rs` 中，`ji` 每次调用后触发的后台索引逻辑 `try_trigger_background_index` 硬编码调用 ACE 索引接口。fast-context 不依赖本地索引机制，无需触发 ACE 索引。

### UX 问题：面板不识别 fast-context

`ZhiIndexPanel` 和 `PopupHeader` 完全按 ACE 的索引状态渲染，没有 fast-context 场景的展示逻辑。

## 修复方案

### 1. `useAcemcpSync.ts` — `checkAcemcpConfigured()` 适配后端策略

```typescript
// 修改前：只检查 ACE
return !!(config.base_url && config.token)

// 修改后：根据 sou_default_backend 判断
if (backend === 'fast_context') return !!config.fast_context_api_key
if (backend === 'ace') return !!(config.base_url && config.token)
return aceOk || fcOk  // auto / both
```

### 2. `memory/mcp.rs` — `try_trigger_background_index` 跳过 fast-context

```rust
// 新增：读取后端策略，fast_context 模式直接返回
let backend = app_config.mcp_config.sou_default_backend
    .as_deref().unwrap_or("auto");
if backend == "fast_context" {
    return Ok(());  // fast-context 不依赖本地索引
}
// auto/both 模式下 ACE 未配置也不报错
if acemcp_config.base_url.is_none() {
    return Ok(());
}
```

### 3. `ZhiIndexPanel.vue` — 新增 fast-context 就绪面板

- 新增 `backend` prop 和 `'fast-context'` 显示模式
- 当后端为 `fast_context` 时显示「⚡ fast-context 已就绪」的绿色状态面板
- 引导文案从「配置 API 密钥」改为「配置搜索后端」

### 4. `McpPopup.vue` — 传递后端策略

- 新增 `souBackend` 状态，在 `onMounted` 时读取 `sou_default_backend`
- 通过 `:backend` prop 传递给 `ZhiIndexPanel`

### 5. `AppContent.vue` — Header 状态适配

- Header 的 ACE 索引状态指示器在 `fast_context` 模式下隐藏
- 避免显示无意义的「索引失败」告警

## 验证方式

1. 编译后启动，弹窗不再显示「配置 API 密钥」引导
2. 弹窗面板显示「⚡ fast-context 已就绪」
3. Header 不再显示红色「索引失败」
4. `ji` 调用后日志不再出现「后台索引失败: error=未配置 base_url」

## fast-context 测试结果

在修复过程中，通过 MCP 直接调用 `sou` 工具验证了 fast-context 功能：

| 测试查询 | 命令数 | 有效率 | 返回文件数 |
|:---|:---|:---|:---|
| API key detection logic | 22 | 90.9% | 5 |
| ZhiIndexPanel displayMode | 18 | 88.9% | 6 |

fast-context 搜索功能完全正常，可作为 ACE 的独立替代方案。
