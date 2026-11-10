# gxp skill 反馈

> 用于记录 skill 在使用中暴露的问题、改进建议、新场景需求。
> 注意：经 Claude Code plugin / skills.sh 安装的用户，本文件会随插件更新被覆盖——反馈请直接提 <https://github.com/cianlabs/gxp/issues>（个人 skill 目录安装的用户可直接在本文件追加）。

## 反馈模板

```
### [YYYY-MM-DD] 标题

**触发场景**：什么任务/查询想用 gxp？

**期望行为**：希望 gxp 怎么响应？

**实际行为**：gxp 实际输出 / 是否被 LLM 调用？

**复现命令**：完整 gxp 命令行（含输入文件描述）

**严重等级**：P0 critical（阻断使用）/ P1 major（影响体验）/ P2 minor（建议改进）
```

## 示例

### [2026-10-27] trace 续行漏 js/csharp（已修）

**触发场景**：用 `gxp 'panic | trace' app.log` 调试 Node.js panic。

**期望行为**：自动识别 JS stack trace 续行。

**实际行为**：仅 java/python/rust/go 被识别为续行，js/csharp 漏。

**复现命令**：`gxp 'panic | trace' node_app.log`

**严重等级**：P1

## 待处理反馈

（无）
