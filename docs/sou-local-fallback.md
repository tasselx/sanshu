# Sou 本地兜底与 FastContext 协议修复

## 问题结论

`FastContext` 的报错不是“没有搜到代码”，而是模型输出与本地桥接协议不一致：

- 服务端只接受顶层 `restricted_exec` / `answer`，历史响应却直接调用过 `readfile`。
- Connect 流式响应可能把 `[TOOL_CALLS]` 与 JSON 拆到不同帧，旧实现命中首帧后立即停止收集。
- 历史响应出现过 `{"file":"..."}` 与 `{"rg":"...","path":"..."}` 扁平命令，旧规范化器只覆盖带 `type`、嵌套对象和 `readfile` shorthand。
- 原“兜底重试”只是延迟 700ms 后再次请求同一个 FastContext 服务，不是离线搜索。

本次修复把这些协议形态规范化，并把未解析响应日志收敛为 `tool/chars/json_complete` 等结构信息，不再回显原始参数。

## 最终路由

默认 `auto` 顺序为：

1. ACE
2. Fast Context
3. Local

ACE 或 Fast Context 返回错误时继续下一后端；成功但没有代码片段时也继续回退。Local 的零命中是最终成功结果，不再重复远端请求。旧配置只包含 ACE / Fast Context 时，运行时会自动把 Local 补到末尾。

每次成功响应会给出：

- `requested_backend`
- `actual_backend`
- `degraded`
- `hit_count`
- `duration_ms`
- Local 场景下的 `engine`、`index_state` 与 `fallback_reason`

## Local 实现

### 即时路径

索引缺失、构建中或查询异常时，Local 使用 `rg --json` 做结构化搜索；若系统没有 `rg`，使用 Rust `ignore::WalkBuilder` 扫描。两条路径共用文本文件、1 MiB 文件上限和排除目录约束。

查询会提取：

- 完整 ASCII 标识符
- camelCase / PascalCase 分段
- snake_case / kebab-case / 路径分段
- 中文整段、二元词和三元词

候选结果按关键词覆盖数、完整查询命中、路径命中和词法分值排序。

### 热索引路径

SQLite FTS5 数据库存放在系统配置目录的 `sanshu/sou-index/` 下，并以项目规范路径的 SHA-256 前缀隔离。源码按 80 行、20 行重叠分块；索引存预分词文本，原始片段只作为返回内容保存。

首次查询不等待全项目建库，立即返回 `rg` 结果并在后台同步。稳定状态的后续查询直接使用 FTS5。`notify` 监听只标记项目变更；索引待同步或同步中时，当次查询使用即时路径保证读取当前文件，同时异步扫描文件元数据，只更新新增、修改、删除或过滤规则变化的文件，不重建未变化分块。

## 为什么基线不引入语义模型

当前目标首先是稳定兜底和毫秒级响应。嵌入模型会增加模型下载、ONNX 运行时、向量存储、版本治理和中文意图到代码标识符的召回不确定性，不能替代确定性的标识符检索。现阶段保持 FTS5 + rg，待真实查询日志证明词法召回存在稳定缺口后，再单独评估混合召回。

可作为后续候选的项目：

- [Tantivy](https://github.com/quickwit-oss/tantivy)：Rust BM25 全文索引，适合需要更复杂索引能力时替换 FTS5。
- [Zoekt](https://github.com/sourcegraph/zoekt)：面向代码的 trigram 搜索，但需要额外 Go 二进制或服务。
- [fastembed-rs](https://docs.rs/fastembed/latest/fastembed/)：本地 ONNX embedding 框架。
- [CodeRankEmbed](https://huggingface.co/nomic-ai/CodeRankEmbed)：代码检索 reranker / embedding 候选。

## 验证

统一入口：

```powershell
.\scripts\test-sou-local-fallback.ps1
```

脚本检查 FastContext 协议回归、Local FTS5 建库与增量更新、2,500 文件 warm p95 基准、Rust 编译、SouConfig ESLint 和前端生产构建。可用 `-SkipBenchmark`、`-SkipCheck` 或 `-SkipFrontend` 缩小范围。

性能目标是 warm FTS5 查询 `p95 <= 50ms`；基准会输出实际 `p95_us` 并在超标时失败。

## 本次验证记录

- `sou` 单元与路由测试：通过。
- 2,500 文件、80 次 warm FTS5 查询：`p95 = 6,500us`。
- `cargo check --lib`：通过。
- `SouConfig.vue` ESLint：通过。
- Vite 生产构建：通过；保留仓库既有的 IconWorkshop 动静态混用告警。
- 新 MCP 进程初始化握手：通过。该进程在恢复用户既有 ACE 多项目 watcher 期间，手工 `tools/call` 冒烟没有进入 `[MCP] 调用开始` 就超时，因此不把这次启动期排队计入 Local 查询延迟；Local 完整路由由隔离的异步测试覆盖。
