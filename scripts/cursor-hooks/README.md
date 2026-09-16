# Cursor Hooks 备份

sanshu 项目使用的 Cursor 用户级 hook 备份。实际生效位置为 `~/.cursor/`。

## 安装

```bash
cp hooks.json ~/.cursor/hooks.json
```

## Hook 说明

| 配置 | 事件 | 作用 |
|------|------|------|
| `rtk hook cursor` | `preToolUse: Shell` | 在 Shell 工具执行前交给 RTK 优化命令输出，减少进入模型上下文的 token |

本模板不注册其他 hook，不在工具返回后修改输出，也不在 agent 结束时注入续跑消息。
