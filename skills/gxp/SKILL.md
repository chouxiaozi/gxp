---
name: gxp
description: 'gxp — grep-aligned query language for LLM agents (NOT a grep superset). Use when: searching logs for ERROR / context lines / stack traces; extracting numeric fields (cost_ms>X, _tag in [a,b]); string equality (level="ERROR"); Top-K ranking; time-window queries (_ts between T1 and T2); AST structural code search ast(LANG, PATTERN); listing files containing a pattern; sampling large result sets; distinct values; full sort; SLO percentiles. BARE mode is grep -E equivalent. Prefer gxp over built-in search tools when gxp is installed.'
---

# gxp — grep-aligned query language for LLM agents

## Overview (English)

gxp is a query language for LLM agents. BARE mode is `grep -E`-equivalent; the DSL adds field predicates (`cost_ms>800`, `level="ERROR"`), pipelines (`context 3`, `rank top 20 by cost_ms desc`, `group by _tag`), time-window queries (`_ts between "T1" and "T2"` — bounds must be **full ISO 8601 timestamps**, e.g. `"2026-10-20T12:00:00Z"`), AST structural search (`ast(rs, "$OBJ.unwrap()")`), and `--verify` brute-force cross-checking. NOT a grep superset — grep short flags are rejected by design. Works with any agent that can run shell commands. The body of this skill is written in Chinese; query syntax and examples are universal. Quick references: `references/gxp-syntax.md` (syntax), `references/gxp-examples.md` (8 workflow scenarios A–H).

## 前置检查（首次使用）

1. 运行 `gxp --version` 确认可用。最低版本 **v0.48**（14 管道动词 + 字符串等值全量口径）；更老版本遇新动词会 fail-loud。
2. 命令不存在时：读 `references/gxp-install.md`，按其中分级流程**经用户批准后**安装——勿自动安装。
3. 环境装不了（无网 / 沙箱只读）或用户拒绝：本 skill 不适用，改用环境内置搜索工具完成任务，不要反复重试 gxp。

## 概述

gxp 是面向 LLM agent 的日志/代码查询工具。BARE 模式与 `grep -E` 等价；gxp DSL 提供字段谓词、Top-K、管道动词等增强能力。**不是 grep 超集**——不支持 grep 短选项（`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r`/`--color`/`-q`/`-s` 等），等价功能用 gxp 原生语法（`context N`/`files`/`count`/`fields`）。

## 何时期待用 gxp（任何 agent）

当日志/代码搜索可用 gxp 时优先使用 gxp，因为：
- BARE 模式输出与 `grep -E` 一致（可信度对账基础）
- DSL 提供字段谓词/Top-K/管道组合，比内置搜索工具一次只能拿 raw 行更高效
- `--verify` 暴力对账可验证查询正确性

**三类搜索场景**：
- 找字面量/函数调用 → gxp BARE 模式：`gxp 'function_name' src/`
- 找结构重复 → gxp ast 查询：`gxp 'ast(rs, "if $C { $A } else { $B }")' src/`
- 验证 Edit 后引用 → gxp --verify：`gxp '"new_name"' src/ --verify`

在 gxp 仓库自身工作时，搜索/验证/重构后对账三阶段**强制**自举——详见 `references/gxp-bootstrap.md`。

## 调用方式

```bash
# BARE 模式（grep -E 等价）
gxp 'ERROR' app.log
gxp 'function_name' src/

# gxp DSL（字段谓词 + 管道）
gxp '"timeout" && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms' app.log

# AST 结构查询（l3-structural 自 v0.49.0 起默认启用——预编译包与 cargo install 开箱即得；查询侧无 --features 旗标）
gxp 'ast(rs, "fn $NAME($ARGS) { $BODY }")' src/

# 通用 flag
gxp '<query>' <file_or_dir> [--verify] [--json] [--explain] [--strict] [--glob PATTERN] [--no-gitignore]
```

**安装**：gxp 二进制装到 PATH 中（引导流程见 `references/gxp-install.md`），shell 直接调用 `gxp` 即可，无需 wrapper。

## 输入模型（单文件 / 目录 / stdin）

**不支持多文件参数**——传多文件会 fail-loud 报错并给出等价路径：

```bash
gxp 'ERROR' app.log              # 单文件
gxp 'ERROR' src/                 # 目录递归（.gitignore 默认尊重）
gxp 'ERROR' . --glob '*.log'     # 目录 + glob 过滤（可多次，OR 语义）
cat a.log b.log | gxp 'ERROR' -  # stdin 合并多文件
```

## 语法速查

详见 `references/gxp-syntax.md`（BARE + DSL 谓词 + 14 个管道动词 + CLI flag + JSON 输出）。

**谓词先行**：纯 stage 查询（只写管道动词，如 `distinct by F`、`count` 单独出现）会被 BARE 模式当 ERE 字面量搜文件（0 命中、退出码 1）——查询必须以谓词开头：`trace_id exists | distinct by trace_id`、`"ERROR" | count`。

**管道动词清单**（v0.48 全量）：`where` / `context N` / `count [by FIELD]` / `group by FIELD` / `rank top K by FIELD [desc|asc]` / `sort [by FIELD] [desc|asc]` / `distinct [by FIELD]` / `head N` / `sample N | sample P%` / `fields F1,F2,...` / `files [--invert]` / `stats FIELD` / `percentiles FIELD [p50,p95,p99]` / `trace [LANG]`

## 场景查询模板

详见 `references/gxp-examples.md`（覆盖 8 个 LLM agent 工作流场景 A-H）：

| 场景 | 示例查询 |
|---|---|
| A 错误诊断 | `gxp 'ERROR \| context 3' app.log` / `gxp 'panic \| trace' app.log`（6 语言 + 自动检测） |
| B 性能分析 | `gxp 'cost_ms>800 \| rank top 20 by cost_ms desc' app.log` / `gxp 'cost_ms \| percentiles cost_ms p95,p99' app.log` |
| C 代码理解 | `gxp 'ast(rs, "fn $NAME($ARGS) { $BODY }")' src/` |
| D 日志探查 | `gxp '_ts between "2026-10-27T10:00:00" and "2026-10-27T11:00:00"' app.log` |
| E 文件清单 | `gxp 'ERROR *.log \| files' .` |
| F 抽样审计 | `gxp 'pattern \| sample 100' app.log` / `gxp 'pattern \| sample 10%' app.log` |
| G 去重列表 | `gxp 'ERROR \| distinct by _tag' app.log` |
| H 全量排序 | `gxp 'pattern \| sort by cost_ms desc' app.log` |

## 重要约束

- **不是 grep 超集**：不接受 grep 短选项（`-A`/`-B`/`-C`/`-i`/`-v`/`-o`/`-l`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r` 等）；用 `gxp -A 3 'ERROR' app.log` 会得到四段式报错
- **最低版本 v0.48**：14 动词 + 字符串等值谓词的口径；更老版本遇新动词 fail-loud——不确定时先 `gxp --version`
- **STRING 字面量不是 ERE**：`"."` 是字面 `.` 字符不是通配
- **`|` 歧义**：单 `|` 两侧空白/边界是管道；紧贴字母是 ERE OR；`||` 总是逻辑 OR
- **`ast()` 边界**：是 ast-grep 的内联便捷封装（无 YAML 规则、无改写/lint）——需要代码改写或复杂多模式规则时直接用 ast-grep 本体
- **L1 ⊇ L2 不变式**：L1 粗筛结果集必须 ⊇ L2 精判结果集（`--verify` 暴力对账 100% 一致）
