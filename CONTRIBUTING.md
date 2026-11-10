# CONTRIBUTING — gxp

> 给贡献者（含 AI 编码助手）的快速上手指引。详细规范见 `AGENTS.md`，本文件不重复。
> 门禁配置按场景适配，不固化（见下方 §3）。

## 1. 首次 setup

新 clone 仓库后：

```powershell
. .\scripts\env.ps1                 # 激活 Rust GNU toolchain + scoop gcc
.\scripts\setup-hooks.ps1           # 安装 git pre-commit hook
```

`env.ps1` 每个 PowerShell 会话开头跑一次（PATH 是会话级，不持久）。
`setup-hooks.ps1` 每 clone 跑一次（hook 在 `.git/hooks/`，不入 git）。

## 2. 提交前

```powershell
.\scripts\check.ps1                 # fmt + clippy + test，全绿才提交
```

若 `check.ps1` 退出码非 0，修问题后重跑。`-Only fmt` / `-Only clippy` / `-Only test` 可单跑。

`git commit` 会自动触发 pre-commit hook（fmt + clippy，不含 test，commit 要快）。
完整 test 在 CI 兜底。逃生通道：`git commit --no-verify`（保留，但慎用）。

## 3. 门禁配置按场景适配

当前（v1 单人 Windows）门禁挂载项：

| 层 | 项 | 何时挂 / 撤 |
|---|---|---|
| `lib.rs` | `#![deny(missing_docs)]` | v1 挂；改公开 API 必须带 `///` |
| `rustfmt.toml` | 锁 edition 2021 | v1 挂；rustfmt 升级时重评估 |
| pre-commit | fmt + clippy（含 `--all-features`） | v1 挂；团队协作后可加 commit-msg |
| CI 阻断 | fmt + clippy + test（均 `--all-features`）+ doc + all-features + cargo-deny | v1 挂；matrix 后加 ubuntu/macos |
| CI 非阻断 info | MSRV (1.90) / DRY (jscpd) / 覆盖率 (llvm-cov) / 变异 (cargo-mutants，周定时) | v1 挂为 info-only；L1/L2 实现完后设阈值转阻断 |
| `xtask` 模式 | 统一本地/CI 命令 | v2 评估（当前 PowerShell 够用） |

**修改门禁时**：
- L0 语义改动 → 先改设计文档（AGENTS.md §5.1）
- L1 实现改动 → 先改 Rust 方案 + 决策表
- L2 代码改动 → 直接改 → commit
- 配置漂移 → 同步改 CONTRIBUTING.md

**写文档改动时**：先过 `docs/document-governance.md` §3 checklist；版本发布前跑 §4 体检。pre-commit hook 自动跑 governance 体检（详见 `.git/hooks/pre-commit` + `scripts/check_governance.sh`）。

## 4. Commit 规范

Conventional Commits（详见 `AGENTS.md` §5.4）。scope 参考：

| scope | 用途 |
|---|---|
| `design` / `rust-plan` | 文档改动 |
| `l1` / `l2` / `l3` / `dir` / `cli` / `dsl` / `pipeline` / `mode` | 模块改动 |
| `ci` | 门禁/脚本/CI 配置 |
| `test` / `bench` | 测试与基准 |

小步提交：一个逻辑改动一个 commit。测试与代码同 commit；文档与代码分开 commit（除非文档是改动的直接产物）。

## 5. 测试要求

- 新功能必须带测试（AGENTS.md §5.6）
- 测试命名：`test_<被测行为>_<条件/边界>`
- 穷举测试：`exhaustive_<维度>_<范围>`
- 不变式校验必须 100% 通过：
  - `L1(粗筛) ⊇ L2(精判)`
  - `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（BARE 模式）
  - fail-loud：错误立即报

## 6. 不变式（任何改动必须保持）

- `L1 ⊇ L2`：详见 design §3.2
- BARE 模式与 `grep -E` 行为一致
- fail-loud：错误立即报，不静默容错
- 诚实 > 承诺：声称的能力必须可验证

触及这些不变式的改动必须先改设计文档（AGENTS.md §5.1 L0 流程）。

## 7. 避坑清单（Windows 环境）

- scoop install 必须带 `-s`（hash 校验 bug）
- Windows Rust 用 GNU toolchain（非 MSVC），见 `scripts/env.ps1`
- 若网络环境需要代理：rustup 与 cargo 的代理配置是两条独立通道（环境变量 vs `.cargo/config.toml`），不能混用，按本机实际网络环境配置
- PowerShell 5.1 处理非 ASCII 文件有编码 bug，优先用工具或 `git checkout`，别用 `Set-Content -Encoding UTF8`
