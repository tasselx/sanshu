# 合并总结：upstream/main (0.6.2) → 本地 main

> 合并时间：2026-06-02  
> 合并提交：`8ddd3a0` (merge: 合并上游 upstream/main (0.6.2) 并保留本地定制)

---

## 上游变更（4 个 commit）

### 1. `d46ed71` refactor(rust): 重构代码结构和导入语句

**影响范围**：89 个文件，纯代码风格重构

- 调整所有模块的 `use` 语句顺序，统一代码风格
- 优化 `builder.rs` 命令注册结构，移除多余注释分隔符
- 重构 `cli.rs` 匹配表达式格式
- 整理 `commands.rs` 模块导出顺序
- 调整 `setup.rs` 和 `mcp_server.rs` 导入语句
- 优化 `config/settings.rs` 字段排列格式
- 改进 `storage.rs` 链式调用格式
- 调整 `constants` 模块常量定义格式
- 优化 `validation.rs` 条件判断格式
- 重新组织 `lib.rs` 模块导出结构

### 2. `2859daa` feat(sou/fast_context): 添加路径修复功能

**影响范围**：3 个文件（`fast_context.rs`、`sou/mod.rs`、测试）

- 新增 `PathFallback` 结构体管理路径回退信息
- 添加 `path_repaired` 统计字段追踪路径修复情况
- 请求路径不存在时自动回退到最近的父目录
- 增加路径缺失警告和提示功能，提供候选路径建议
- 重构输出分类逻辑，添加诊断行过滤
- 为 rg、readfile、tree、ls、glob 命令添加路径缺失处理

### 3. `840acac` release: Release 0.6.2

版本号统一升级到 `0.6.2`（Cargo.toml、package.json、tauri.conf.json、version.json）

### 4. `400ed18` docs: update README to version 0.6.2

更新 README 中的下载链接和版本信息至 v0.6.2

---

## 本地定制保留（13 个文件，+923 行）

### 核心功能：「防超时」体系

解决 Cursor MCP 客户端 ~30s 工具调用超时导致的「长等待被丢弃 → AI 误判失败 → 新开 request」问题。

| 文件 | 改动量 | 功能 |
|------|--------|------|
| `popup.rs` | +314 行 | poll/重连弹窗系统：`PendingPopup` + `POPUP_POLL_WINDOW=240s`，把「一次长调用」拆成「多次短调用」 |
| `interaction/mcp.rs` | +158 行 | progress 心跳机制（每 10s 发送），在 30s 超时前反复重置客户端计时器；弹窗失败改为 soft error + 重试指引 |
| `response.rs` | +77 行 | 空响应/取消场景改写为「继续等待」语义，防止 AI 提前结束对话 |
| `server.rs` | +83 行 | zhi 参数解析失败改为 soft error + 修正指引（而非 hard error 导致对话直接结束） |

### 配套文件

| 文件 | 说明 |
|------|------|
| `.cursor/rules/sanshu-强制交互.mdc` | 强制交互硬约束规则（防止提前结束对话） |
| `build.rs` | 编译期注入 git sha、构建时间等元数据（启动水印用） |
| `build.sh` | 构建脚本 |
| `install.sh` / `install-universal.sh` | 安装脚本微调 |
| `McpPopup.vue` / `PopupActions.vue` | 前端弹窗组件 |
| `README.md` | 自定义 README 内容 |
| `.gitignore` | 新增忽略规则 |

---

## 冲突解决记录

共 4 个文件存在合并冲突，全部以**保留本地修改**为原则解决：

| 文件 | 冲突位置 | 解决方式 |
|------|----------|----------|
| `popup.rs` | `use` 导入区 | 保留本地完整导入（poll/重连系统所需的 `Child`、`Stdio`、`HashMap`、`Mutex` 等） |
| `response.rs` | 纯文本回退分支 | 保留本地的空响应兜底检查 + 采用上游的 `log_debug!` 格式化风格 |
| `server.rs` | zhi 参数错误处理 | 保留本地的 soft error + 重试指引（上游是 hard error `Err(McpError::invalid_params)`) |
| `interaction/mcp.rs` | 导入区 + 弹窗失败处理 | 保留本地的心跳系统导入 + soft error 弹窗失败处理（上游是 `Err(popup_error)`) |

---

## 当前状态

- **分支**：`main`
- **已推送**：`origin/main` 已同步
- **工作区**：干净，无未提交文件
