# AGENTS.md — gxp 项目约束文件

> 本文件是 gxp 项目的协作基线。所有 agent（含 AI 编码助手）在本目录工作时必须先读本文件。
> 与其他规范（设计文档、Rust 方案）冲突时，**本文件优先**，但可即时澄清并修改。

---

## 0. 项目定位（一页内）

**gxp** = grep-aligned query language for LLM agents（**不是 grep 的严格超集**）。

- **不是 grep 超集**：不支持 grep 短选项（`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r`/`--color`/`-q`/`-s` 等）。等价功能用 gxp 原生语法（`context N`/`files`/`count`/`fields`）。
- **BARE 模式 ERE 兼容**：`gxp 'pattern' file` 与 `grep -E 'pattern' file` 行为一致。
- **L1→L2 下推不变式**：详见 design §3.2（L1 不淘汰任何 L2 候选）。
- **L1+L2 对所有文本文件通用**：不区分语言。L3（`ast(NAME, PATTERN)` AST 结构查询）按语言分档，设计文档 §8.2 有完整支持矩阵。
- **方向 B**：L1 只下推字段名前缀（`cost_ms=`）粗筛，L2 精确数值比较。digit-DP 已废弃（设计文档 §3.2）。
- **零持久索引**：不落数据库；v1 只做单次查询内缓存（T0/T1），跨查询增量（T2/T3）永久拒绝（详见 Rust 方案 §11.3.2）。
- **实现语言**：Rust（不可更换，见设计文档 §0 / Rust 方案 §0 三条硬约束）。

### 0.1 LLM 友好原则（特性取舍根本判据）

gxp 面向 LLM agent。特性取舍判据：场景驱动 + 输出自包含 + 不增理解负担。

1. **场景驱动**：确有 LLM 工作流需求才实现（拒绝"功能完备性"驱动）。**场景清单见 §0.2**——新特性必须对应清单内场景；清单外场景先补 §0.2 再评估特性。
2. **输出自包含**：下游工具所需字段全在 JSON，不从对话历史推断（如 `path`/`line_no`/`line`）；语法跨模式一致。
3. **不增理解负担**：新动词与现有动词语义正交可组合；优先复用组合（如 `tail N` = `sort|head|sort`）；文档先于代码（EBNF + 示例 + changelog 同步）。

**方案取舍判据顺序**（高 → 低）：
1. 场景驱动力（§0.2 锚定）
2. 应用友好度（输出形态对 LLM 是否自包含、语法是否正交可组合）
3. 不变式风险（D1-D11 + `L1 ⊇ L2` + Send+Sync）
4. 等价组合替代（§0.1 第 3 条，能用组合 80%+ 覆盖则不新增动词）
5. 实现成本（最后考虑，仅在 1-4 持平时作决定性因素）

> ⚠️ **不要把"实现成本低"作为首要优势**——gxp 面向 LLM agent，应用友好度优先于代码难度。

> **选型对比应 ≤2 候选，主轴判据先行**：列方案前先按 §0.1 判据顺序（场景驱动力 → 应用友好度 → 不变式风险 → 等价组合替代 → 实现成本）锁定主轴判据，再用主轴筛掉 ≥2 个候选中明显劣势的，最终只对 2 个候选做逐项对比。理由：v0.39 trace 评审阶段列 4 候选逐项对比消耗 50 轮 token，而主轴判据（场景 A 错误诊断 + 终止 stage 一等公民）本可在第 1 步筛掉 3 个。

### 0.2 LLM agent 工作流场景清单（特性取舍判据依据）

> gxp 服务的具体工作流。新增特性必须锚定清单内场景；场景清单更新（新增/细化）属 L3 文档改动。

| # | 场景类别 | LLM agent 工作流 | 当前一等公民支持 |
|---|---|---|---|
| A | 错误诊断 | 找 ERROR 行 + 上下文 + 异常聚合 + stack trace | `ERROR \| context N`；`group by _tag`；`trace [LANG]`（6 语言 + 自动检测，v0.39） |
| B | 性能分析 | 慢查询定位 + SLO 长尾指标 | `cost_ms>X \| rank top K by FIELD desc`；`percentiles FIELD` |
| C | 代码理解 | 找某 API 调用 + AST 结构模式 | `ast(LANG, "PATTERN")` |
| D | 日志探查 | 时间窗口 + trace_id 追踪 | `_ts between "T1" and "T2"`；`capture(NAME, /REGEX/)` |
| E | 文件清单 | 哪些文件含某模式 | `pattern *.log \| files` |
| F | 抽样审计 | 大结果集代表性采样（context window 物理限制） | `pattern \| sample N`；`pattern \| sample P%` |
| G | 去重列表 | 唯一错误/唯一用户/唯一 trace | `pattern \| distinct`；`pattern \| distinct by FIELD` |
| H | 全量排序 | 全量有序输出（rank 之外） | `pattern \| sort by FIELD [desc\|asc]` |
| I | 调用可靠性 | shell 引号不可靠环境（Windows PowerShell 5.1/pwsh 7+ 正确习惯互斥）下单参数查询安全传递 | `--query-file PATH`（v0.50；引号丢失后与合法字面量逐字节相同、产品侧不可检测，实测矩阵见 changelog v0.50） |

**判据用法**：新特性提案须指出对应场景（A-I），不在清单内 → 先补清单再评估。等价组合替代优先（§0.1 第 3 条）；新动词仅在组合无法覆盖或语法负担过重时落地。场景清单不是动词清单——动词清单见 `docs/capability-matrix.md` §1。

### 当前仓库状态

| 维度 | 状态 |
|---|---|
| 实现阶段 | 详见 `docs/capability-matrix.md` §4（测试基线 + 代码规模）+ `docs/changelog.md`（版本演进） |
| 仓库结构 | 独立 git 仓库；`main` 唯一长期分支；Conventional Commits（见 §5） |
| CI | workflow 已就绪（`.github/workflows/ci.yml`），三平台 matrix（详见 capability-matrix §1） |

---

## 1. 目录结构

**根目录约束**：根文件 = `AGENTS.md`/`.gitignore`/`Cargo.toml`/`Cargo.lock`/`CONTRIBUTING.md`/`README.md`/`LICENSE-MIT`/`LICENSE-APACHE`/`install.sh`；顶层目录 = `docs/`/`src/`/`tests/`/`benches/`/`scripts/`/`skills/`/`.github/`/`.claude-plugin/`（`fixtures/` 为本地产物不入库）。

```
gxp/
├── AGENTS.md / .gitignore / Cargo.toml / CONTRIBUTING.md / README.md / LICENSE-* / install.sh
├── .claude-plugin/  # marketplace.json + plugin.json（repo 根即 plugin：/plugin install gxp --marketplace cianlabs/gxp）
├── docs/     # design-full + rust-implementation-plan + changelog + capability-matrix + performance-baseline
├── src/      # main/lib/cli/error/plan + dsl/ + mode/ + l1/ + l2/ + l3/ + dir/ + io/ + pipeline/
├── skills/   # gxp skill 单一权威源（SKILL.md + references/；plugin / skills.sh / release zip 三出口同源）
├── tests/    # cli_{bare,pipeline,directory,ast,stdin,glob,readme_examples}.rs + dir_*.rs + dsl_parser + l1_scan + l2_extract + numeric_eval + common
└── benches/  # l1_scan / l2_extract / full_pipeline / scale_dir + diag_*
```

> skill 分发资产门禁：`sh scripts/check_plugin.sh`（claude plugin validate + SKILL.md 结构 + skills.sh 发现；CI plugin-lint job 同口径）。

> 实现语言：Rust（v0.1 起唯一实现，决策记录见 changelog v0.1）。

---

## 2. 设计决策硬约束（不可绕过）

以下决策已通过对抗评审，**任何修改必须先澄清并更新设计文档**：

| ID | 决策 | 文档锚点 |
|---|---|---|
| D1 | **不是 grep 超集**：不接受 grep 短选项 | 设计文档 §0、§1.4 |
| D2 | **方案 E 单一 gxp 模式**：取消模式判定 Halting Problem | 设计文档 §1.5 |
| D3 | **方案 D' 语法**：邻接=字面空格（非隐式 AND），`&&`/`||`/`!` shell 符号操作符，STRING 强制双引号 | 设计文档 §2.1-2.2 |
| D4 | **方向 B**：L1 字段名前缀粗筛 + L2 精确数值比较，digit-DP 已废弃 | 设计文档 §3.2 |
| D5 | **11 维数值字段格式契约**：支持整数/浮点/负数/前导零/正号/科学计数法；拒绝千分位/单位/空白 trim/布尔/NaN | 设计文档 §3.3 |
| D6 | **`!` 词法规则方向 C**：前后边界判定 + pred 强制括号 `!(cost>800)` | 设计文档 §3.4 |
| D7 | **不 fork ripgrep**：从头写 + 依赖 crates | Rust 方案 §0、§9.1 |
| D8 | **不 fork GNU grep**：Rust 头写 | Rust 方案 §9.1 |
| D9 | **T0/T1 only（当前实现）**：T2 sidecar / T3 daemon 永久拒绝（除非 D9 改判） | Rust 方案 §0、§11.3.2 |
| D10 | **统一 read_to_end**：所有文件走 `read_to_end`，不使用 mmap（gxp 单次查询 + 顺序扫描场景下 mmap 无优势，fs::read 预热 page cache 反而快 10ms，且规避 SIGBUS） | Rust 方案 §6.1 |
| D11 | **实现语言 Rust**：crate 生态、ast-grep 原生、ignore crate 三条硬约束 | Rust 方案 §0 |

### 语法与示例锚点

| 语法点 | 设计文档位置 | 示例 |
|---|---|---|
| BARE 模式（ERE 兼容） | §2.4 示例 1 | `gxp 'ERROR' app.log` |
| 布尔 + 字段作用域 + Top-K | §2.4 示例 2 | `"timeout" && _tag in [checkout,billing] && cost_ms>800 \| rank top 20 by cost_ms desc \| fields _ts,_tag,cost_ms` |
| `context` 管道动词（等价 -A/-B/-C） | §2.4 示例 8 | `ERROR \| context 3` |
| `files` 管道动词（等价 -l/-L） | §2.4 示例 9 | `ERROR *.log \| files` |
| `count` 管道动词（等价 -c） | §2.4 示例 9 | `ERROR app.log \| count` |
| 字面 `>` 的表达 | §2.4 示例 10 | `"cost>800"` / `/cost>800/` / `cost\>800` |
| grep 短选项的四段式报错 | §2.4 示例 11 | `gxp -A 3 'ERROR' app.log` → 四段式错误 |
| `--query-file` 免引号通道 | §2.4 示例 12 | `gxp --query-file query.gxp app.log`（场景 §0.2-I） |

---

## 3. 文档规范

### 3.1 文档分类与撰写规则

| 文档 | 路径 | 角色 | 变更门槛 |
|---|---|---|---|
| `gxp-design-full.md` | `docs/` | **设计基线**（语义、语法、EBNF、执行模型） | 高：改动须澄清对抗评审理由 |
| `rust-implementation-plan.md` | `docs/` | **实现方案**（crate 选型、代码骨架、路线图） | 中：与设计冲突时先改设计文档 |
| `AGENTS.md`（本文件） | 根 | **协作约束** | 中：agent 工作前必读 |
| `docs/changelog.md` | `docs/` | **版本历史** | 低：新功能落地时追加 |
| `docs/document-governance.md` | `docs/` | **文档治理规则**（R1-R5 + 体检 checklist） | 中：写文档改动前必读 §3 checklist |
| `docs/performance-baseline.md` | `docs/` | **性能数据单一参考源** | 低：跑完 bench 后更新 |
| `docs/capability-matrix.md` | `docs/` | **横向能力快照** | 低：测试基线变化时更新 |

**撰写规则**：诚实 > 承诺 → fail-loud → 不变式优先（先证明 `L1 ⊇ L2`）→ 决策记录 → 示例可执行 → 日期标注（相对日期落地为绝对）。Markdown 格式：GitHub-flavored，代码块标注语言，行号引用 `file_path:line_number`。

### 3.2 文档增长约束

- **AGENTS.md ≤ 345 行**：版本历史移至 `docs/changelog.md`（不在本文件保留版本行）。行数上限 2026-11-01 从 320 放到 330、2026-11-09 从 330 放到 340（理由：§6.5.2-6.5.3 自举组合+同构镜像检查项随 bug 修复积累，是防回归机制核心约束不可下沉）、2026-11-11 从 340 放到 345（理由：§0.2 场景行 I 调用可靠性场景随 `--query-file` 落地，是特性取舍判据链必需，不宜压缩既有场景描述）
- **Rust 方案 ≤ 1500 行**：版本历史引用 `docs/changelog.md`，不重复

## 4. 代码规范

- **edition 2021**，stable 工具链，host triple `x86_64-pc-windows-gnu`（GNU toolchain 绕开 link.exe 缺失，配 `scoop install gcc`）
- **格式化**：`cargo fmt`（提交前必跑）；**lint**：`cargo clippy --all-targets -- -D warnings`（CI 阻断）
- **依赖**：只允许 Rust 方案 §13 Cargo.toml 列出的 crate；新增必须先更新方案文档
- **unsafe**：生产代码无 `unsafe` 块；mmap 仅保留 `benches/diag_mmap.rs` 对照基准
- **错误处理**：`thiserror` 派生；用户可见错误走四段式模板（设计文档 §1.5）
- **测试**：每公开函数 ≥3 测试（正常/边界/拒绝）；数值比较走穷举（0..60000 × 6 区间）
- **命名**：模块/函数 snake_case，类型 UpperCamelCase，常量 UPPER_SNAKE，测试 `test_<行为>_<条件>`，EBNF lower_snake

---

## 5. 增量变更规范

### 5.1 改动分类与流程

| 等级 | 描述 | 流程 |
|---|---|---|
| **L0 语义改动** | 改 EBNF、改谓词语义、改不变式、改实现语言 | 设计文档 → Rust 方案 → 代码 → 重新对抗评审 |
| **L1 实现改动** | 改 crate 选型、改模块结构、改执行顺序 | Rust 方案 → 代码 → 更新 §15 决策表 |
| **L2 代码改动** | 函数实现、测试用例、bug 修复 | 代码 → 测试通过 → commit |
| **L3 文档改动** | 修正笔误、补充示例、调整格式 | 直接改 → 跑 governance §4 体检 → commit |

**改动前**：定位等级 → 检查 §2 硬约束 → 检查 `L1 ⊇ L2`/fail-loud/诚实 > 承诺。
**改动后**：更新决策表 → 跑穷举校验 → 更新版本号 → 跑 governance §4 体检 → commit。

### 5.2 Commit 约定

[Conventional Commits](https://www.conventionalcommits.org/)：`<type>(<scope>): <subject>`。type：`feat`/`fix`/`docs`/`refactor`/`test`/`chore`/`revert`。scope：`design`/`rust-plan`/`l1`/`l2`/`l3`/`dir`/`cli`/`dsl`/`pipeline`/`mode`/`test`/`bench`。

- **main**：唯一长期分支，commit 直接推到 main（单人开发阶段）
- **小步提交**：每个逻辑独立改动一个 commit；测试与代码同 commit；文档与代码分开 commit（除非文档是改动的直接产物）

### 5.3 任务中断处理

执行任务时发现 bug 的处理优先级：

1. **原任务可安全暂停**（代码能编译）→ 记录 bug 到任务列表，完成当前 commit 后再修
2. **原任务无法暂停**（代码不能编译）→ 先修到能编译，记录 bug，完成原任务后回头修
3. **bug 是不变式违反**（如 `L1 ⊇ L2` 被破坏）→ 立即修复，与原任务同 commit（不变式违反不能留过夜）

## 6. 测试与验证规范

### 6.1 测试金字塔

| 层级 | 目标 | 工具 |
|---|---|---|
| 单元 | ≥80% 行覆盖 | `cargo test` + `proptest` |
| 穷举 | 数值谓词 0..60000 × 6 区间 0 错 | explicit test |
| 集成 | 31 项目录层行为 | `tests/dir_*.rs` |
| 基准 | gxp vs GNU grep vs ripgrep | `criterion` + `benches/scale_dir.rs` |
| CI | Windows + Linux + macOS | GitHub Actions（三平台 matrix 已落地，V6 2026-10-23） |

### 6.2 不变性校验（必须 100% 通过）

- `L1(粗筛) ⊇ L2(精判)`：每个 L1 改动必须对账验证
- `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（BARE 模式）：每个 ERE 改动必须对账
- `tests/dir_*.rs` 31 项全绿：每次目录层改动后跑 `cargo test --test dir_*`
- `--verify` 暴力对账：每个 L1+L2 查询跑一遍暴力全扫，与优化路径结果 diff，100% 一致

### 6.3 agent 测试质量

> 门禁抓废弃代码和语法层伪断言；**语义层伪断言**需 agent 主动遵守。

| 规则 | 禁 | 应 |
|---|---|---|
| 恒真断言 | `assert_eq!(1, 1)` | `assert_eq!(result, expected)`（expected 独立计算） |
| 只调不断言 | `parse(x).unwrap();` | `let q = parse(x).unwrap(); assert_eq!(q.stages.len(), 2);` |
| 存在性断言 | `assert!(result.is_ok())` | `let Err(e) = result else { panic!() }; assert!(format!("{e}").contains("top"));` |
| 占位测试 | `#[test] fn test_todo() {}` | 等功能落地再写 |

自检：每个测试有 assert → 期望值独立计算 → 正常/边界/拒绝三场景 → fail-loud 路径测错误消息 → 无 TODO 占位。

### 6.4 并行路径交叉覆盖

1. **三路径必测**：单文件 / 目录 / stdin 都有 e2e 测试。
2. **组合条件矩阵**：多条件查询列 `needs_l2 × needs_l3 × 输入源` 矩阵，每个非空交集至少一个测试（正向+负向）。
3. **数据来源 × stage 矩阵（v0.43 新增，根因：5 个 bug 全在跨数据来源契约）**：字段值可能来自 3 类来源——行内 `field=`/`field:`、`capture(NAME, /RE/)` 提取、伪字段 `_match`/`_line`/`_file`/`_ts`/`_tag`。每个 stage 动词（where/rank/sort/distinct/stats/percentiles 等）对每类数据来源的处理逻辑不同，必须用 e2e 覆盖每个非空交集。**反例**：单元层用 `MatchedLine::new` 直接构造的行永远缺真实 capture/伪字段流路径，必走 e2e（v0.43 修复 percentiles 漏 `pseudo_field_value` 后的强制约束）。
4. **组合链顺序差异**：`A | sample` vs `sample | A`、`sort | head` vs `head | sort` 必测顺序语义差异（reorder_stages 保序执行）。
5. **暴力路径语义对齐**：`--verify` 必须与优化路径走相同语义链（L1→L2→L3），不跳层。检查清单详见 Rust 方案 §8.5.3。

### 6.5 自举强制约束（必须使用，违反等同不变式违反）

> gxp 是本项目的产物，也是工作工具。改代码前搜索、改代码后验证、重构后对账三个阶段都必须自举。
> **强制约束**：任何 Edit/Write 后立即跑对应自举查询，违反等同不变式违反（按 §5.3 立即修复，不留过夜）。LLM 默认用 Grep 工具而非 gxp 自举——这是已重犯 2 次的偏移（memory `feedback_self_bootstrap_search_phase_violation_repeated.md`），规则文档无法根治偏移，但强制约束让违反显式化、可追溯。

**搜索阶段（写代码前）**：分析源码结构、查找重复逻辑时优先用 gxp 而非 Grep/Explore agent。找结构重复用 `ast(rs, "if $C { $A } else { $B }")' src/`（Grep 找不到结构）；找字面量/函数调用用 BARE 模式（自举验证自身正确性）。**Windows 上含内层双引号的查询一律走 Bash 工具**（pwsh 5.1 吃内层引号、`\"` 转义反斜杠泄漏，实测矩阵见 changelog v0.50）；无 Bash 环境用 `--query-file`。

**搜索结果可信度判断**：BARE 模式结果必须与 `grep -E` 一致（不一致即 bug）；ast() 查询结果必须 `--verify` 0 FAIL 才可信。

**验证阶段（写代码后）**：`cargo build --features l3-structural --release` → `gxp '<query>' <dir> --verify` → 0 FAIL。推荐对自身源码跑 `gxp 'ast(rs, "$OBJ.unwrap()")' src/ --verify`。

**重构后**：拆分/搬迁后用 `gxp 'fn test_' tests/` 对比前后完整性，`gxp 'helper_name' src/ --verify` 验证调用点。

> 详见 Rust 方案 §8.5 盲区记录。

#### 6.5.1 Edit 后自举引用检查清单

> 每次 Edit（新增 Stage 变体 / 新增 MatchedLine 字段 / 新增公开 `fn` / 改签名）后**立即**跑对应自举查询，确认引用点齐全。理由：v0.39 trace 落地期间自举违规花 120 轮（占评审 25%），根因是 LLM 默认用 Grep 工具而非 gxp 自举（memory `feedback_self_bootstrap_search_phase_violation_repeated.md` 在册）。

| Edit 类型 | 自举命令（在 `src/` 上跑） | 期望覆盖点 |
|---|---|---|
| 新增 `Stage` 变体 | `gxp '"Stage::Foo"' src/ --verify` | ≥6 处：dsl/ast.rs 定义 + dsl/pipeline.rs parser + plan.rs StageView + pipeline/mod.rs 分支 + 测试断言 + capability-matrix |
| 新增 `MatchedLine` 字段 | `gxp '"字段名"' src/ --verify` | ≥3 处：struct 定义 + `with_captures`/构造点 + `emit_remaining`/JSON 输出 |
| 新增公开 `fn` | `gxp 'fn name' src/ --verify` | ≥1 调用点（不在 `#[cfg(test)]` 块内） |
| 改函数签名 | `gxp 'fn name' src/ --verify` + 跑 `cargo build` | 所有调用点 0 编译错误 |

**触发时机**：每次 Edit 后、commit 前必跑。**判通过**：`--verify` 0 FAIL 且引用点数 ≥ 期望；不达 → 立即补引用点或修复查询。

#### 6.5.2 新功能/bug 修复后自举组合覆盖检查（v0.43 新增）

> 触发：新增 `Stage` 变体 / 修 `apply_*` 或 stage 内 `pseudo_field_value`/`captures` 路径 / 改 `parse_one_stage` 后立即跑。
> 理由：v0.43-v0.44 共 11 bug 全在跨数据来源契约，单元层 `MatchedLine::new` 构造绕过真实 pipeline 流（详见 §6.6 强制 e2e 约束）。

| 检查目标 | 自举命令（在 `tests/` 上跑） | 判通过 |
|---|---|---|
| capture × stage N | `gxp '"capture" && "<verb>"' tests/` | ≥1 e2e 命中 |
| 伪字段 × stage N | `gxp '"_match" && "<verb>" \|\| "_line" && "<verb>" \|\| "_file" && "<verb>"' tests/` | ≥1 e2e 命中 |
| 谓词组合 × where | `gxp '"where" && ("between" \|\| "in" \|\| "exists")' tests/` | ≥1 e2e 命中每类 |
| capture × 同字段谓词（v0.44 新增，B8 暴露） | `gxp '"capture" && ">" \|\| "capture" && " in " \|\| "capture" && " between " \|\| "capture" && " exists"' tests/` | ≥1 e2e 命中每类 |
| capture × Not(同字段谓词)（v0.44 新增，B9 暴露） | `gxp '"capture" && "!"' tests/` | ≥1 e2e 命中 |
| JSON 格式 × 字段值类型 × 首尾空白（v0.44 新增，B10/B11） | `gxp '"exists" && "json"' tests/`（含尾随空格/CRLF/行首空格变体） | ≥1 e2e 命中 |
| 字符串等值 × capture/伪字段/Not（v0.48 新增，B15 暴露） | `gxp '/="ERROR/ && ("capture" \|\| "_tag" \|\| "!")' tests/` | ≥1 e2e 命中每类（capture/_tag/!） |

不达 → 立即补 e2e（参照 changelog v0.43 防回归机制），视为不变式违反（§5.3）。

#### 6.5.3 bug 修复后同构镜像扫描（v0.44 新增）

> 触发：修了某 `match` 分支 / 某调用点 / 某函数签名后立即跑。理由：B6→B7 / B8→B9 同构遗漏复发 2 次。

| 镜像对 | 自举命令（在 `src/` 上跑） | 检查要点 |
|---|---|---|
| `eval_prepared_with_captures` vs `eval_prepared_with_ctx_and_captures`（l3 镜像） | `gxp '"eval_prepared_with_captures"' src/ --verify` | Not/Capture/Compare 分支一致 |
| `execute_stages` vs `_streaming` vs `_directory`（三路径镜像） | `gxp '"execute_stages"' src/ --verify` | m.path 填充 / captures 传递一致 |
| `run_gxp_mode` vs `_streaming` vs `_with_content` | `gxp '"run_gxp_mode"' src/ --verify` | m.path 填充 / json vs 文本分支一致 |

不达 → 视为遗漏（§5.3 立即修复）。

### 6.6 内联测试 vs 集成测试边界

| 类型 | 放哪 | 特征 | 示例 |
|---|---|---|---|
| 单元测试 | `src/` 内联 `#[cfg(test)]` | 测私有 `fn`，可用 `tempfile` 做夹具 | `test_parse_integer` / `test_compile_single_string` |
| 集成测试 | `tests/*.rs` | 调公开入口（`run_with_writer`/`detect_and_dispatch`）+ tempfile | `test_run_gxp_mode_compare_predicate_executes` |

**门禁**：`scripts/check_no_e2e_inline.sh` 扫描 `src/**/*.rs` 的 `#[cfg(test)]` 块，出现 `run_with_writer`/`detect_and_dispatch`/`run_gxp_mode`/`run_bare_mode` 调用 → CI fail。

**原则**：`tempfile`/`std::fs` 本身不违规（单元测试常用夹具）；违规的是测公开入口而非私有逻辑。

**强制 e2e 约束（v0.44 新增，B6-B9 根因）**：新增 `Stage` 变体 / 修 `apply_*` / `pseudo_field_value` / `captures` 路径的 bug 修复，单元测试不能替代 e2e，必须走 e2e（参照 §6.4 第 3 条数据来源 × stage 矩阵）。理由：单元层用 `MatchedLine::new` 直接构造行绕过真实 pipeline 流，跨数据来源契约 bug（capture/伪字段/行内三类字段值来源在 pipeline 流过时的处理）永远碰不到。

## 7. 性能与资源约束

### 7.1 性能基线锚点（回归性门禁，v0.49 重锚定）

| 指标 | 基线锚点（本机 i7-10700） |
|---|---|
| L1 两遍链 | 33.51ms（v0.49 diag median） |
| L2 字段抽取（31931 行） | 8.21ms（v0.47 criterion，v0.49 未复测） |
| 全查询 + Top20（44MB） | 106.74ms（v0.49 diag median of 3） |
| `sample N`（44MB，N=20..10000） | <1ms 洗牌开销 |
| DirIndex lax 二次（Linux） | 0.15ms |

> **门禁口径（v0.49 重锚定，2026-11-09 用户确认）**：后续版本同机 A/B 实测（diag median of 3），median 劣化 >10% 需归因；跨机绝对值不可比（比值 1.52-1.85×）。旧机 80/30/20ms 目标（i7-14700，v0.32-v0.45）降为历史参考（perf-baseline §1.1 机器变更说明）。实测数据权威源 `docs/performance-baseline.md` §1.1-1.2（44MB）+ §1.3（12GB 大数据多轮）。Windows `FindFirstFile` 单次 ~60µs（Linux 5-10×），物理上不可能达 0.15ms。Linux 接入按 CI 节奏推进，不门控。vs GNU grep/ripgrep 三方对照见 perf-baseline §1.3.2（本机 900MB count：rg 575ms < grep 1024ms < gxp 1454ms，gxp 裸扫最慢，赢面在 agent 往返与输出自包含；旧机 911MB v0.42 曾 2.42× 快于 grep，跨机不可比）。详见 Rust 方案 §11.5.4。

**内存/依赖**：零持久索引（D10 + 统一 `read_to_end` + thread-local buffer 复用）；不引入新 crate（Rust 方案 §13 列出）；不 fork；license MIT OR Apache-2.0

---

## 8. 协作流程

先读 AGENTS.md → 定位任务等级 → 检查 §2 硬约束 → 有歧义先问用户 → 改动顺序：设计→Rust 方案→代码→测试→commit。语义改动同步设计文档与 Rust 方案；硬约束改动必须先澄清，不允许"先改后说"。

---

## 9. 未决项与待验证

> V1-V7 全通过。流式 fallback + 多字段批量提取已落地（详见 Rust 方案 §11.4）。已发现 bug 见 §9.2。
> **D9 适用边界（v0.42 实测）**：v1 适用 ≤10GB 单目录（12GB/11603 文件 median 1.7-3.9s，--verify 100% PASS）。33GB 未实测不可声明 v1 适用——社区持久索引方案更合适。详见 `docs/performance-baseline.md` §1.3-1.4。

### 9.1 候选功能方向 + 暂不评估清单

> 权威表详见 `docs/rust-implementation-plan.md §11.3`（候选 7 项全部落地或转暂不评估 + 暂不评估 11 项 + 永久拒绝 2 项，含 re-triage 结论）。

### 9.2 已发现 bug（已全部修复，完整清单见 `docs/changelog.md`）

> - **v0.36 B1-B3**：终止 stage fail-loud / group by captures 优先 / 文档改"count group by"→"group by"。
> - **v0.43 B1'-B3'**：apply_where 漏 captures 优先 / percentiles 未走 pseudo_field_value / where 多 token 谓词解析断裂。
> - **v0.44 B6-B11**：单文件/流式漏填 m.path / capture 同字段谓词 L1 误淘汰 + Not 漏传 captures（同构遗漏 2 次）/ find_value 漏 JSON line + prefix_variants 缺变体 / detect_format 不 trim 空白。6 bug 均不靠 §6.5.2 发现，矩阵规则已补齐（§6.5.2 +3 行 + §6.5.3 镜像扫描 + §6.6 强制 e2e，详见 changelog v0.44）。
> - **v0.45-v0.47 B12-B14**：B12 扩展名过滤遗漏 `.jsx`；B13 `trace rust` 漏抓新 panic 格式；**B14 无括号混合 `&&`/`||` 与 Regex 组合 L1 部分组误杀 + 捷径 match-all（不变式违反，`--verify` 不可发现——暴力与优化共用 L1 同错对齐；修复：Or 组完整性 + `l1_exact` 捷径门槛，详见 changelog v0.47）**。
> - **v0.48 B15**：Or 分支内字段谓词的前缀被当必选粗筛（`level in [ERROR] \|\| "plain"` 丢 plain-only 行，`L1 ⊇ L2` 违反，`--verify` 报暴力独有）——修复：Or 子树前缀豁免（与 Not 豁免同构；B14 只审字面量组完整性未审前缀收集）。发现方式：字符串等值 e2e 独立期望集首跑暴露（B14 教训的直接回报），详见 changelog v0.48。

---

## 10. 版本历史

> 完整版本历史见 `docs/changelog.md`。**本文件结束。任何 agent 工作前必读。**
