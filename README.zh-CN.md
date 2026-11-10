# gxp

[English](README.md) | [简体中文](README.zh-CN.md)

**grep-aligned query language for LLM agents** — 一门为 LLM agent 设计的日志/代码查询语言。Rust 实现，单二进制，零持久索引。

> 不是 grep 的严格超集：不支持 grep 短选项，等价功能用原生语法（`context N` ≈ `-C`、`files` ≈ `-l`、`count` ≈ `-c`）。

## 为什么

LLM agent 在日志诊断、性能分析、代码理解等工作流中，通常要在 rg + jq + ast-grep 之间往返：三套语法、多层管道、三种输出格式。gxp 把这些场景收进一门 DSL，输出自包含（JSON 含 `path`/`line_no`/`line`/captures，下游工具无需从对话历史推断）：

| 场景 | 传统工具链 | gxp |
|---|---|---|
| 错误诊断 | rg + 手工聚合 + 上下文拼接 | `gxp 'level in [ERROR] \| context 3' app.log` |
| 慢查询 Top20 | rg + jq + sort 多层管道 | `gxp '"checkout" && cost_ms>800 \| rank top 20 by cost_ms desc' *.log` |
| AST 结构匹配 | ast-grep + jq | `gxp 'ast(rs, "$OBJ.unwrap()")' src/` |
| 时间窗口追踪 | rg 子串近似（易错）| `gxp '_ts between "2026-10-20T12:00:00Z" and "2026-10-20T12:30:00Z"' app.log` |
| 异常聚合 | rg + jq group | `gxp 'ERROR \| group by _tag' app.log` |

[agent A/B 实验](docs/agent-ab-experiment.md)（16 任务双臂对照）：两臂均 16/16 全对；gxp 臂 **-9% token**（249k vs 274k）。调用轮次 32 vs 25——gxp 多出的轮次全部是新 DSL 发现成本（--help/语法试错/限流吞调用）加自发 `--verify`，rg+jq 臂对应开销是 rg×jq 双引擎交叉验证；有效查询轮次打平（≈13 vs ≈13）。最悬殊场景：随机抽样 gxp 3 轮（`| sample 20`），rg+jq 臂在 jq 里手写蓄水池采样 5 轮 47.8k token（全场单任务最高）。英文摘要：[agent-ab-experiment-en.md](docs/agent-ab-experiment-en.md)。

## 快速开始

三种安装方式任选：

```bash
# ① 一键安装（Linux / macOS / Windows Git Bash；下载对应平台预编译包 + sha256 校验）
curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh

# ② 一键安装 + Claude Code skill（语法速查 + 8 场景模板装到 ~/.claude/skills/gxp/）
curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh -s -- --with-skill

# ③ cargo 安装（crates.io；l3-structural 默认开启，AST 查询开箱即用）
cargo install --locked gxp
```

也可从 [Releases](https://github.com/cianlabs/gxp/releases) 手动下载预编译包（Linux / Windows / macOS arm64 / macOS x64，均含 sha256）。agent 用户推荐 ②——skill 附 `gxp-syntax.md` 速查 + `gxp-examples.md` 8 场景查询模板。

## 核心能力

- **BARE 模式**：`gxp 'ERROR' app.log` ≡ `grep -E 'ERROR' app.log`
- **布尔 + 字段谓词**：`"timeout" && _tag in [checkout,billing] && cost_ms>800`（logfmt / JSON line / `[tag]` / ISO 8601 四种格式自动探测）
- **14 个管道动词**：`where` / `context` / `count` / `group by` / `rank` / `sort` / `distinct` / `stats` / `percentiles` / `sample` / `head` / `fields` / `files` / `trace`，链式组合
- **AST 结构查询**：`ast(NAME, "PATTERN")`，9 语言（ts/tsx/js/py/rs/go/java/c/cpp），按扩展名过滤 + fail-loud
- **capture 提取**：`capture(NAME, /REGEX/)`，捕获值参与后续谓词与 stage
- **stack trace 聚合**：`trace [LANG]`，6 语言 + 自动检测（含 rust 新旧 panic 格式）
- **`--verify`**：每条查询可暴力对账优化路径（L1 ⊇ L2 下推不变式），结果 100% 一致才 PASS
- **目录扫描**：rayon 并行 + `--glob` 过滤 + .gitignore 尊重

## 性能

裸扫描吞吐不是 gxp 的卖点——900MB count 同机交错实测 ripgrep 575ms < GNU grep 1024ms < gxp 1454ms；gxp 的赢面在 agent 往返次数与输出自包含（见上方 A/B 实验）。44MB 全管道 ~107ms / L1 两遍链 ~33.5ms / L2 字段抽取 ~8.2ms（i7-10700，diag median）。完整数据与方法学详见 [docs/performance-baseline.md](docs/performance-baseline.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/gxp-design-full.md](docs/gxp-design-full.md) | 设计基线：语义 / EBNF / 执行模型 / 决策记录 |
| [docs/rust-implementation-plan.md](docs/rust-implementation-plan.md) | 实现方案：crate 选型 / 模块结构 / 路线图 |
| [docs/capability-matrix.md](docs/capability-matrix.md) | 能力矩阵：五层流水线全量状态 |
| [docs/changelog.md](docs/changelog.md) | 版本历史（v0.1 → v0.49） |
| [AGENTS.md](AGENTS.md) | 协作约束：硬约束 / 测试规范 / 自举流程 |

## 许可

MIT OR Apache-2.0，见 [LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE)。
