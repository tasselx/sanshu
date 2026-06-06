# 代码合并总结：upstream/main (v0.6.3) → main → dev

> 生成时间：2026-06-06

本次共完成两段合并：先把 `upstream/main` 合入 `main`，再把 `main` 合入 `dev`。

---

## 一、第一段：`upstream/main (v0.6.3)` → `main`

| 项目 | 内容 |
| --- | --- |
| 合并双方 | 本地 `main`(`5339e2e`) ⊕ `upstream/main`(`c0cf6e9 docs: update README to version 0.6.3`) |
| 合并提交 | `9d1d4e1`（双父：`5339e2e` + `c0cf6e9`） |
| 冲突文件 | 1 个：`README.md`（2 处，均在「编译/打包」文档段） |

### 冲突解决
upstream 文档引用了 3 个 Windows 脚本，但合并后仓库实际只存在 1 个：

- ✅ 实际存在：`scripts/build-windows.ps1`（支持 `-Mode Debug/Release/All`）
- ❌ 不存在：`scripts/build-release-windows.ps1`、`scripts/build-debug-windows.ps1`

**采纳方案**：保留两侧内容并合并，把脚本引用统一修正为真实存在的 `scripts/build-windows.ps1 -Mode Debug/Release/All`，保留 HEAD 的 `custom-protocol` 提示。修正后文档与脚本实际行为（`-Mode` 取值、`target/build-info/windows-build-info-{debug,release}.{json,md}` 输出路径）完全一致。

### 引入的主要变更（相对老 main）
- **新增**：`docs/log-viewer-global-debug.md`、`scripts/build-windows.ps1`、`src/frontend/components/tabs/LogsTab.vue`
- **删除**：8 个测试脚本/样例（`test_*.ps1` / `test_*.sh` / `test_*.json`）
- **修改**：47 个文件（前端组件、Rust 后端、`Cargo.toml`、`package.json` 等）

---

## 二、第二段：`main` → `dev`

| 项目 | 内容 |
| --- | --- |
| 合并双方 | `dev`(`e94524a`) ⊕ `main`(`9d1d4e1`) |
| 合并提交 | `9a440e3 Merge branch 'main' into dev` |
| 变更规模 | 58 个文件，+3231 / −2363 |
| 冲突文件 | 2 个：`.gitignore`、`src/frontend/composables/useAcemcpSync.ts` |

> ⚠️ 过程中曾发现 `dev` 上残留一个「索引与 HEAD 完全一致」的空合并态（`MERGE_HEAD=9d1d4e1` 但不带任何 main 内容）。若直接提交会丢弃 main 全部 80 个文件的变更，故先 `git merge --abort` 安全清除（工作树干净、零损失），再重新发起正常合并。

### 冲突解决

| 文件 | dev(HEAD) | main | 最终采纳 |
| --- | --- | --- | --- |
| `useAcemcpSync.ts` | 多后端检测逻辑（`fast_context`/`ace`/`auto`/`both`） | 旧版仅 `base_url`+`token` | **保留 dev**（main 为旧版，采用会功能回退） |
| `.gitignore` | 该处为空 | 新增 `/docs/` 忽略规则 | **保留 dev**（不引入 docs 忽略，保持 `docs/` 可入版，便于存放总结文档） |

---

## 三、最终状态

- ✅ 两段合并均已提交，无残留冲突标记。
- ✅ `main` 已完全并入 `dev`（`git rev-list --count main ^dev` = 0）。
- ✅ `dev` 工作树干净；当前所在分支为 `dev`。
- ⏳ `main` 与 `dev` 均尚未推送到远端（如需同步请显式确认后再 push）。

## 四、后续建议

1. 如需同步远端：分别在 `main`、`dev` 上 `git push`（请确认后操作）。
2. 仓库出现过外部并发改动迹象（分支被切换、产生空合并态、未跟踪文件被清除），建议确认是否有其他工具/自动化在并发操作本仓库，避免再次产生异常合并态。
