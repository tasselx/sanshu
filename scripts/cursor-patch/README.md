# Cursor 客户端 patch：重试不铸新 requestId

## 背景

超长 agent turn 断流后，`nal_agent_retries` 会**自动重试**（这通常是想要的：checkpoint resume / 续流）。

官方在 `attempt>0` 时会：

1. `crypto.randomUUID()` 铸新 **attempt** requestId  
2. 出网 header：`x-request-id` = attempt 新 id，`x-original-request-id` = 原始 generation id  

客户端 `requestTraces` 里父级 `agent.request` / 展示用 `requestId` 往往仍绑 generation（跨 attempt 稳定）。  
补丁针对的是 **attempt 身份与出网 header**，不是父 span 条数。

## 能改什么 / 不能改什么

| 目标 | 客户端 patch |
|------|----------------|
| 重试不铸新 attempt id + 出网 `x-request-id`≡original | **能** → `reuse-attempt-id`（默认，双保险） |
| 断流后完全不要自动重试 | 能 → `disable-retries`（可选/激进） |
| 服务端用量面板永远合并成一行 | **不能保证**（计费键在服务端） |
| 少烧 token | **主要靠用法**：切短 turn、稳定网络，不是 patch |

## 默认模式：`reuse-attempt-id`（v2 双保险）

对 `workbench.desktop.main.js` / `workbench.glass.main.js` 同时做两层：

1. **mint**：去掉 `attempt>0?(VAR=crypto.randomUUID(),await …onRetryStarting` 里的铸 UUID  
2. **header**：`{"x-request-id":attempt,"x-original-request-id":original}` → 两侧都用 original  

保留 `nal_agent_retries` 自动续流。  
幂等：已打过的层不会重复改；旧版只打了 mint 的包，再 `apply` 会补齐 header。

### 可选：`disable-retries`

- 强制 `nal_agent_retries` gate / 默认为关  
- 断流后不自动第二次 attempt  
- 代价：长 turn 直接停；手动 Continue 可能变成新 request  

## 用法

```bash
# 升级 Cursor 后先检测（只读）
./scripts/cursor-patch/apply-cursor-patch.sh check

# 模拟，不写盘
./scripts/cursor-patch/apply-cursor-patch.sh dry-run

# 用 backups 里的官方包做离线自检（不碰 /Applications）
./scripts/cursor-patch/apply-cursor-patch.sh selftest

# 默认 = reuse 双保险（先 dry-run 门槛 → 备份 → 原子写 → 写后校验，失败回滚）
./scripts/cursor-patch/apply-cursor-patch.sh apply

# 可选：彻底关闭自动重试
./scripts/cursor-patch/apply-cursor-patch.sh apply --mode=disable-retries

# 查看是否生效
./scripts/cursor-patch/apply-cursor-patch.sh status

# 恢复备份（版本不一致默认拒绝，需 --force）
./scripts/cursor-patch/apply-cursor-patch.sh restore
```

应用后 **完全退出并重启 Cursor** 才会加载新 bundle。

## 如何验收（重要）

**不要**只看 `cursor.requestTraces.log` 里的 `requestId`——该字段多半继承 generation，官方重试时也可能不变。

更可靠：

1. `check` / `status`：mint 官方处 = 0，header 强制相同 > 0  
2. 断流重试时抓代理请求头：`x-request-id` 应等于 `x-original-request-id`  
3. structured log 里 retry 诊断的 `requestId` 与 `originalRequestId` 应一致  

## 目标文件

- `/Applications/Cursor.app/Contents/Resources/app/out/vs/workbench/workbench.desktop.main.js`
- `/Applications/Cursor.app/Contents/Resources/app/out/vs/workbench/workbench.glass.main.js`

备份：`scripts/cursor-patch/backups/<timestamp>/`

## 安全机制

1. apply 前强制 **dry-run**（reuse 模式）  
2. 写盘前 **内存变换 + 生效校验**；体积异常拒绝写盘  
3. **原子写**（同目录 tempfile + `os.replace`）  
4. 写后再次校验；失败则从本次备份 **回滚**  
5. restore 时版本不一致默认拒绝；restore 前再备份当前包  

## 风险

1. Cursor 升级会覆盖 bundle，需 `check` 后重打  
2. 修改 app 内容可能影响 macOS 代码签名  
3. 与 ToS / 完整性策略冲突的风险自负  
4. 不要把旧版本 backup restore 到新版本 Cursor 上（脚本默认会拦）  

## 推荐策略（比 patch 更有效）

1. 大任务拆成多段短 turn  
2. 代理/网络避免掐长连接  
3. 保留自动重试；不要轻易 `disable-retries`  
