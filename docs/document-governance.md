# gxp 文档治理规则

> **用途**：防止"过程性噪声堆积、清单多源不一致、历史与现状混嵌"复发。
> **适用范围**：`docs/*.md` + `AGENTS.md` + `CONTRIBUTING.md`。
> **更新规则**：本文件优先级低于 AGENTS.md §3 文档规范，但高于其他文档内部约定。改本文件 = L1 实现改动（AGENTS.md §5.1）。
> **创建背景**：2026-10-26 文档对抗评审发现 6 阻断 + ~20 中严重，根因是缺护栏。本文件是护栏。

---

## 0. 一页内

**5 条规则 + 5 项执行 checklist**：

| ID | 规则 | 一句话 |
|---|---|---|
| R1 | 文档定位唯一性 | 每文档单一角色，不互相重叠 |
| R2 | 主题单点锚定 | 每主题只有一个权威锚点 |
| R3 | 历史与现状分离 | 历史沉到 changelog，正文只留当前态 |
| R4 | 清单单点权威 | 每清单只有一个权威源 |
| R5 | 门槛与内容绑定 | 高门槛文档只含高稳定内容 |

**执行护栏**：每次写文档改动前过一遍 §3 checklist；新增主题前先 grep 现有锚点；新版本发布前跑 §4 体检。

---

## 1. 规则定义

### R1 文档定位唯一性

**定义**：每份文档承担一个角色，不与其它文档职责重叠。**角色互斥指职责层互斥，不指信息维度互斥**——高阶文档（AGENTS）可保留指针性 1-2 行快照（"当前状态"），详细清单只放权威源。这等同于"目录扉页 vs 索引"——目录扉页印一行"本书 12 章 300 页"不算与索引职责重叠。

**角色清单**（与 AGENTS.md §3.1 文档分类对齐）：

| 文档 | 单一角色 |
|---|---|
| `AGENTS.md` | 协作约束（agent 工作前必读，硬约束清单 + 流程 + 指针性状态快照） |
| `docs/gxp-design-full.md` | 设计基线（语义/语法/EBNF/执行模型/不变式） |
| `docs/rust-implementation-plan.md` | 实现方案（crate 选型/模块结构/路线图） |
| `docs/performance-baseline.md` | 性能数据单一参考源 |
| `docs/capability-matrix.md` | 横向能力快照 |
| `docs/changelog.md` | 版本历史 |
| `CONTRIBUTING.md` | 贡献者上手指南 |
| `docs/document-governance.md`（本文件） | 文档治理规则（R1-R5 + 体检 checklist） |

**反例**（已修）：
- ❌ design §5 复述 performance-baseline 的实测数字 → 改链接（属职责重叠，非信息维度互补）
- ❌ rust-plan §11.1 里程碑表与 changelog 重叠 → 改链接
- ❌ AGENTS §7.1 性能表复述 performance-baseline 实测列 → 改链接

**判据**：
- **职责层互斥**：两份文档都把"详细能力清单"作为自己职责 → 重叠，违规
- **信息维度互补**：AGENTS §0 1-2 行指针性状态（"853/758 全绿"）+ capability-matrix §4 详细能力清单 → 互补，合规
- 如某节内容能在另一文档找到同源详细数据 → 改链接不复述

### R2 主题单点锚定

**定义**：每个主题（不变式、决策、性能指标、测试基线、能力清单、版本历史）只有一个权威锚点。

**当前锚点表**：

| 主题 | 权威锚点 |
|---|---|
| `L1 ⊇ L2` 不变式 | design §3.2 |
| D1-D11 决策 | AGENTS.md §2 + rust-plan §15（决策表） |
| 性能数据 | performance-baseline §1 |
| 测试基线数字 | capability-matrix §4 |
| 候选功能清单 | rust-plan §11.3 |
| Bug 清单 | AGENTS.md §9.2（已发现→修完→空） |
| 版本历史 | changelog（按版本号） |
| Cargo.toml 依赖 | rust-plan §13 |

**反例**（已修）：
- ❌ AGENTS §6.2 + design §3.2 + rust-plan §1.2 三处都复述 `L1 ⊇ L2` → AGENTS/rust-plan 改链接到 design §3.2
- ❌ AGENTS §0 + rust-plan §0 都列 D9 永久拒绝措辞 → rust-plan §11.3.2 是权威，AGENTS §2 改"按需评估启动"

**判据**：写某主题前 grep 现有文档找锚点；若已存在 → 改链接不新建。

**操作性引用 vs 定义复述判据**（"操作性"无操作性定义 → 补三条客观判据）：

1. **判据 A（语义层）**：含"不变式符号 + 操作后果"算定义复述（如 `L1 ⊇ L2` + "绝不漏答案"），单含"操作后果命令"算操作性引用（如"绝不漏答案" alone）
2. **判据 B（读者层）**：高门槛文档（design/rust-plan）禁复述，必须链接；中门槛协作基线（AGENTS/CONTRIBUTING）可保留"不变式 + 一句操作后果"作为 agent 执行时当场可见的提示，但**禁止理由论证**（理由论证放 design）
3. **判据 C（链接优先）**：能改成 `详见 design §X` 且不破坏 agent 执行流（不需跳转就能继续工作）的，一律改链接；需跳转才能继续工作的，保留操作性引用

### R3 历史与现状分离

**定义**：历史信息沉到 changelog，正文只留当前态 + 链接。"历史"分三档（避免一刀切误删）：

| 内容类型 | 判据 | 处置 |
|---|---|---|
| **已废弃历史** | 已被后续版本否决、不再影响当前决策（如 digit-DP、Python 原型） | 沉 changelog，原文保留 1 行链接 |
| **决策依据历史** | 当前决策的对抗评审证据（"为什么不做 X"，如 design §6 "D8 封死 GNU grep fork 路线"） | 保留结论 + 1 句理由，详细数据沉 changelog |
| **进度看板** | 含完成日期的里程碑/版本表（如 rust-plan §11.1） | 沉 changelog；权威表只留"当前阶段" |

**沉到 changelog 的内容类型**：
- 已废弃算法的详细描述（如 digit-DP 完整正则）
- 原型期实测数据（如 Python 原型 327ms）
- 历史决策过程的详细论证（D8/D10 改判的完整理由论证）
- 已修 bug 的修复细节
- 已删除模块（如 Python 原型）
- 含完成日期的里程碑明细表

**正文可保留**：
- 当前态描述（"方向 B：L1 字段名前缀粗筛 + L2 精确数值比较"）
- 单行链接（"决策记录见 changelog v0.1"）
- 仍然有效的设计依据（"gxp 真正优势是让字段谓词可表达"）
- 决策依据历史的一句结论 + 链接（"§6 D8 封死 GNU grep fork 路线，详见 changelog v0.1"）
- 进度看板的"当前阶段"行（无历史日期）

**反例**（已修）：
- ❌ design §5 整章复述原型期实测 → 删整章，改链接到 changelog v0.1-v0.5
- ❌ design §3.2 "历史算法（已废弃）"详细描述 digit-DP → 改单行链接
- ❌ design §12.4 "gxp.py 未打通"已不适用 → 删条目
- ❌ rust-plan §12 "Python 原型"独立章节 → 改一行链接

**判据**：写"曾经/原本/最初/早期/已废弃/PoC 落地"前自问——这内容现在还有决策价值吗？
- 没有决策价值 → 沉 changelog（已废弃历史）
- 有决策价值但需详细论证 → 保留结论 + 1 句理由，详细数据沉 changelog（决策依据历史）
- 是含完成日期的进度看板 → 整个表沉 changelog，原文改"当前阶段 = X"

### R4 清单单点权威

**定义**：每清单（依赖列表、bench 列表、测试基线数字、命令清单、命令示例）只有一个权威源。

**当前清单权威表**：

| 清单 | 权威源 | 副本要求 |
|---|---|---|
| Cargo.toml 依赖 | `Cargo.toml`（代码文件） | rust-plan §13 骨架与代码同步 |
| bench 文件清单 | performance-baseline §5 | capability-matrix §4 引用 |
| 测试基线数字 | capability-matrix §4 | AGENTS §0 引用 |
| 候选 + 暂不评估清单 | rust-plan §11.3 | AGENTS §9.1 引用 |
| 命令示例 | design §2.4 | 不复述到其他文档 |

**反例**（已修）：
- ❌ rust-plan §13 Cargo.toml 骨架缺 sample/sort bench → performance-baseline §5 + capability-matrix §4 报"13 个"实际 15 个 → 同步补齐
- ❌ AGENTS §0 复述测试基线数字 → 改链接到 capability-matrix §4

**判据**：清单改动必须先改权威源，再 grep 副本同步；副本用链接而非复述。

**权威源更新触发器**（R4 核心补丁——防"权威源失真 → 下游全部跟着错"复发）：

每次版本迭代必须同步更新对应权威源：

| 触发事件 | 必须同步的权威源 |
|---|---|
| 新增 `[[bench]]` / 新增依赖 crate | rust-plan §13 Cargo.toml 骨架（同步 diff） + Cargo.toml（权威源本身） |
| 新增功能 / 落地候选 | capability-matrix §1 + rust-plan §11.3.1 候选表（标记 ✅） |
| 跑完 bench | performance-baseline §1.1 + §5 索引 + 头部日期 |
| 新增 L3 语言 | capability-matrix §1 L3 行 + design §8.2 支持矩阵 |
| 修复 bug | changelog（修复细节）；若新增/移除测试用例 → 同步 capability-matrix §4 测试基线计数 |
| 修改 EBNF 语法 | design §2.2（权威源）+ AGENTS §2 语法锚点表 + changelog |
| 改 D 系决策措辞 | AGENTS §2 + rust-plan §15 + design 引用点（措辞漂移是已识别反模式） |
| 改 crate 版本号 | Cargo.toml（权威源）+ changelog 版本号同步 |
| 删除已废弃模块 | src/ + capability-matrix §1 + changelog |
| 改 MSRV | Cargo.toml `rust-version` + CONTRIBUTING §3 + ci.yml msrv job |
| 改 CI matrix | ci.yml + AGENTS §6.1 + CONTRIBUTING §3 |

**滞后传播修复规则**：权威源滞后时下游报错指向权威源；下游发现权威源已失真，**必须先修权威源再修下游，不允许下游独立修复**（避免同步漂移）。这一条给 B-4 类阻断划定时序边界——必须先修 rust-plan §13，再让 capability-matrix §4 / performance-baseline §5 跟随。

**代码本体不复述规则**（R4 子规则——用户 2026-10-26 提的"直接指向代码不行吗"）：

文档不复述代码本体（`Cargo.toml` / `Cargo.toml [profile.release]` / 完整函数体），改用链接指向代码文件 + 文字说明决策理由。代码改了文档不会自动跟，复述即同步漂移源。

| 内容 | 处置 |
|---|---|
| `Cargo.toml` 完整骨架 | 链接到 `Cargo.toml`，正文列依赖分组 + 决策理由表 |
| `Cargo.toml [profile.release]` 配置 | 链接到 `Cargo.toml`，正文列配置 + 理由表 |
| 函数体完整源码 | 不在文档复述；用 `src/path.rs::function_name` 引用 |
| EBNF 语法定义（design §2.2） | **例外**：保留——设计基线是权威源本身，代码是实现而非反过来 |

**判据**：写代码块前自问——这代码改了文档会跟着改吗？会自动跟 → 可以保留（如自动生成的 API 文档）；不会自动跟 → 改链接到代码文件。

### R5 门槛与内容绑定

**定义**：门槛绑定到**内容稳定性**，不是绑定到文档（同一文档内不同章节稳定性差异巨大，按文档绑门槛无法落地）。

**内容稳定性三档**：

| 内容稳定性 | 判据 | 处置 |
|---|---|---|
| **超稳定**（EBNF / D1-D11 / 谓词语义） | 改动须对抗评审 | 高门槛文档（design/rust-plan §0-§10） |
| **中稳定**（crate 选型 / 模块结构 / 决策依据历史） | 改动须更新决策表 | 中门槛文档（rust-plan §11+ / AGENTS） |
| **易变**（性能数字 / 测试基线 / bench 索引 / 里程碑日期 / 当前状态行） | 版本迭代同步 | 易变内容**单点维护**，权威源可以是任意文档，下游只链接 |

**易变内容单点维护规则**：易变内容（如"853 l3 + 758 默认全绿"）无论放 AGENTS / capability-matrix / changelog，**只能存在于一处**，其他处只能链接。这直接解决 capability-matrix §4 + 头部双点维护问题——保留一处，另一处改链接。

**文档门槛分级**（与 AGENTS.md §3.1 对齐，作为内容归属参考）：

| 文档 | 门槛 | 允许内容 | 禁止内容 |
|---|---|---|---|
| `gxp-design-full.md` | 高 | EBNF/语义/不变式/决策 | 实测数字/版本号/测试基线 |
| `rust-implementation-plan.md` | 中 | crate 选型/模块结构/路线图 | 性能数字/测试基线/版本号 |
| `AGENTS.md` | 中 | 硬约束/流程/规范 + 指针性 1-2 行状态快照 | 实测数字/版本行/详细清单 |
| `capability-matrix.md` | 低 | 测试基线数字 + 横向能力 | - |
| `performance-baseline.md` | 低 | 性能数据 + bench 索引 | - |
| `changelog.md` | 低 | 版本历史 + 已废弃决策 | - |

**反例**（已修）：
- ❌ AGENTS §0 写"853 l3 + 758 默认全绿（v0.37）" → 改链接到 capability-matrix §4
- ❌ AGENTS §7.1 性能表带"实测"列 → 删实测列，改链接到 performance-baseline §1
- ❌ design §5 写"Python 实现 327ms" → 删整章，链接到 performance-baseline §1

**判据**：写易变数字前自问——这数字下个版本会变吗？会 → 下沉到低门槛文档，正文改链接。同一易变内容**只能存在于一处**，其他处只链接不复述。

---

## 2. 反模式清单

### 2.1 过程性噪声（已识别并清理）

> 这正是用户问"还有其他类似内容，过于过程信息的噪声吗"的答案。

| 噪声类型 | 表现 | 修法 |
|---|---|---|
| 历史算法详细描述 | "早期原型用 digit-DP..." | 改单行链接到 changelog |
| 原型期实测数据 | "Python 实现 327ms" | 删，链接到 performance-baseline |
| PoC 措辞 | "PoC 落地，穷举测试" | 改"落地，穷举测试" |
| 选型实验记录 | "选型实验结论" | 改"SIMD 配置参考"（结论而非过程） |
| 改判措辞 | "D10 改判后" | 改"D10"（决策已是当前态） |
| 时刻快照 | "v0.32 时刻快照" | 改"v0.32 实测"（去掉"时刻"过程感） |
| 历史选择表 | "Python ⭐⭐ ⭐⭐⭐⭐⭐ 原型用" | 删已淘汰语言行 |
| 未验证项残留 | "gxp.py 未打通" | 删条目 |
| 实验过程描述 | "PoC 只验通路；扩到 5 语言..." | 改"落地进度见 changelog" |

**自检清单**：grep "PoC\|改判\|时刻\|早期\|曾经\|原本\|最初\|已废弃\|实验后\|选型实验" → 评估每处是否值得保留。

**"改判"两用法判据**：
- ❌ 过程性（必改）："D10 改判后" / "实测改判" — 决策已是当前态，去 "改判" 二字
- ✅ 规则性（保留）："除非 D9 改判" / "保留供将来 D8 改判时参考" — 这是变更条件，是规则性引用

**"已废弃"两用法判据**：
- ❌ 过程性（必改）：正文整段描述已废弃算法 → 改单行链接到 changelog
- ✅ 当前态（保留）："digit-DP 已废弃" 一行标注 + 链接 → 是当前态描述

**"早期"两用法判据**：
- ❌ 过程性（必改）："早期原型用 digit-DP" → 改单行链接
- ✅ 比较性（保留）："是 gxp 里 X 的前身/早期形态" 作为对比参考 → 可保留（或改"前身"中性化）

### 2.2 清单漂移

**症状**：同一清单在多文档出现，更新一处其他不跟。

**修法**：每清单指定权威源，副本改链接，副本改动须先改权威源。

### 2.3 性能数据复述

**症状**：design + rust-plan + AGENTS + performance-baseline 四处都写"全管道 66-76ms"。

**修法**：performance-baseline §1 是唯一权威；其他文档改链接。

### 2.4 决策复述

**症状**：AGENTS §2 + rust-plan §15 + design 正文三处都写 D 系决策措辞，措辞漂移。

**修法**：AGENTS §2 是面向 agent 的硬约束清单（一行）；rust-plan §15 是决策表（含替代方案 + 理由）；design 正文引用 AGENTS §2。

### 2.5 易变数字放高门槛文档

**症状**：AGENTS §0 写测试基线数字"853 l3 + 758 默认全绿"。

**修法**：易变数字下沉到 capability-matrix §4，AGENTS §0 改链接。

### 2.6 死链反模式

**症状**：文档 A 引用文档 B 的 §X.Y，但 B 中 §X.Y 不存在。

**修法**：跨文档引用前先 `grep -n "§X.Y" target_doc.md` 验证目标章节存在。CI 跑 §4.1.6 死链检查。

### 2.7 同文档双点反模式

**症状**：同一文档内头部摘要 + 正文 §X 在两处复述同一易变数字（如 capability-matrix 头部 + §4 都列 853/758）。

**修法**：同文档内部也遵守 R5 单点——头部摘要 1 行指针 + 链接到正文 §X；或删正文 §X 改链接到头部。

### 2.8 日期多点反模式

**症状**：同一完成日期（如 2026-10-26）在 design / rust-plan / perf-baseline / capability-matrix / changelog / governance 6 处文档复述，每次版本迭代都要多点同步。

**修法**：日期权威源是 changelog v0.X 版本号；其他文档改"详见 changelog v0.X"链接，不写绝对日期。例外：性能基线实测日期（performance-baseline §1 头部）是必要环境标注，保留。

### 2.9 元评审反模式（governance 自身违反）

**症状**：governance 文档自身引用其他文档的章节/引文，但目标章节不存在或引文是凭空捏造（如"governance §90 引用 design §6.3 P0 旁路是最安全的起点"，但 design §6 无 §6.3 子节也无此句话）。

**修法**：governance 也要跑 §4 体检；元评审反模式自身即阻断，必须修复。

### 2.10 范围链接模糊反模式

**症状**："详见 changelog v0.17-v0.22" — 读者需自己猜对应行号；changelog 不按版本范围分段。

**修法**：范围链接改"详见 changelog §11 v0.17-v0.22 行"或 changelog 增加分段锚点。

---

## 3. 写文档前的 checklist

每次写文档改动前过一遍（AGENTS.md §5.1 L2 代码改动级别适用）：

- [ ] **R1 角色唯一**：这节内容是否属于本文档角色？不属于 → 改文档
- [ ] **R2 主题锚点**：grep 现有文档，这主题已有锚点吗？有 → 改链接不新建
- [ ] **R3 历史沉底**：这内容是"曾经/早期/已废弃"吗？是 → 沉到 changelog
- [ ] **R4 清单权威**：这是清单吗？是 → 找权威源，副本改链接
- [ ] **R5 门槛匹配**：这内容易变吗？易变 → 下沉到低门槛文档

每条都过 → 写；任一卡 → 改方向，不要硬写。

---

## 4. 版本发布前的体检 checklist

每次版本发布前跑（changelog 落新版本后立即跑）：

### 4.1 清单同步体检

```bash
# 4.1.1 测试基线数字一致性
grep -rn "l3\|默认" docs/ AGENTS.md | grep -E "[0-9]{3,}" | head

# 4.1.2 bench 文件清单一致性
grep -n "bench" docs/performance-baseline.md docs/capability-matrix.md | head

# 4.1.3 Cargo.toml 版本号与 changelog 最新版本一致
grep '^version' Cargo.toml
grep '^## v' docs/changelog.md | head -1
# Cargo.toml version 必须 ≥ changelog 最新版本号

# 4.1.4 D 系决策措辞一致性
grep -rn "D1\|D2\|D3\|D4\|D5\|D6\|D7\|D8\|D9\|D10\|D11" docs/ AGENTS.md | head

# 4.1.5 代码本体不复述检查（R4 子规则）
grep -rn '```toml\|```rust$' docs/ AGENTS.md CONTRIBUTING.md 2>/dev/null
# 应返回空（design §2.2 EBNF 是例外，不算代码本体复述）

# 4.1.6 跨文档引用死链检查
# 收集所有 "§X.Y" 引用，与目标文档章节对比
grep -rn '详见.*§\|见.*§\|§[0-9]' docs/ AGENTS.md CONTRIBUTING.md 2>/dev/null \
  | grep -v "^docs/changelog" | head -20

# 4.1.7 src/ 注释 §X.Y 引用 vs docs/AGENTS/CONTRIBUTING 章节锚点交叉校验
# 机器能查：§X.Y.Z 子节号 → docs/AGENTS/CONTRIBUTING 中是否有对应 #/##/### X.Y.Z 锚点
# 机器查不出：§X.Y 锚点存在但语义不对应（需 agent 自觉 + code review）
# 实现在 scripts/check_governance.sh §4.1.7（自动跑，<1s）
# 反例：src/foo.rs:10 `// 见 Rust 方案 §11.2.5` 而 rust-plan §11.2 只有 V1-V7（无 V8+）→ 死链
# 人工评估每处：目标 §X.Y 是否真实存在

# 4.1.8 测试基线数字对账（防漂移）
# capability-matrix §4 表写的 "默认/l3-structural feature 测试" 数字必须与 cargo test --list 实际计数一致
# 实现在 scripts/check_governance.sh §4.1.8（自动跑，<3s 复用 target/release/）
# 跳过条件：CARGO_SKIP_TEST_COUNT=1（用于离线/无 cargo 工具链环境）
# 反例：capability-matrix §4 写 802 全绿，实际 cargo test --list 979 → 阻断，更新文档

# 4.1.9 cargo doc broken intra-doc links（防函数名漂移）
# src/**/*.rs 的 /// 注释中 [fn_name] 形式被 rustdoc 解析为 intra-doc link，
# 指向不存在的函数/类型 → error。覆盖 B9 清理 10 处函数名漂移的场景。
# 实现在 scripts/check_governance.sh §4.1.9（自动跑，~5s）
# 跳过条件：CARGO_SKIP_DOC_CHECK=1
# 反例：src/mode/prepared.rs 注释 [`run_l3_ast`] 而实际函数名是 `run_all_ast` → error
```

### 4.2 历史沉底体检

```bash
# 4.2.1 过程性噪声词扫描
grep -rn "PoC\|改判\|时刻\|早期\|曾经\|原本\|最初\|已废弃\|实验后\|选型实验" \
  docs/gxp-design-full.md docs/rust-implementation-plan.md \
  docs/performance-baseline.md AGENTS.md CONTRIBUTING.md 2>/dev/null \
  | grep -v "^docs/changelog"

# 4.2.2 已删除模块残留扫描
grep -rn -i "python\|prototype\|原型" \
  docs/gxp-design-full.md docs/rust-implementation-plan.md \
  docs/performance-baseline.md AGENTS.md CONTRIBUTING.md 2>/dev/null \
  | grep -v "^docs/changelog"
```

**判据**：grep 结果非空 → 评估每处是否值得保留，不值得 → 修。

### 4.3 性能数据单点体检

```bash
# 4.3.1 性能数字只在 performance-baseline 出现
grep -rn "66.*76ms\|18\.66ms\|12ms\|327ms\|835ms" \
  docs/gxp-design-full.md docs/rust-implementation-plan.md \
  AGENTS.md 2>/dev/null

# 4.3.2 性能表只列目标不列实测（除 performance-baseline）
grep -n "实测" docs/gxp-design-full.md docs/rust-implementation-plan.md AGENTS.md
```

**判据**：4.3.1 应返回空（除链接到 performance-baseline 的引用）；4.3.2 应只返回链接式引用。

### 4.4 R1 角色唯一体检

```bash
# 4.4.1 design 不应含测试基线数字 / crate 版本号
grep -n "v0\.\|0\.33\|0\.38\|853\|758" docs/gxp-design-full.md | grep -v "详见\|见.*changelog\|见.*capability"

# 4.4.2 capability-matrix 不应含 EBNF / 决策措辞
grep -n 'EBNF\|:=\|D[0-9]' docs/capability-matrix.md | grep -v "见.*design"

# 4.4.3 AGENTS §0 不应含易变数字（除 1-2 行指针性快照）
grep -n "[0-9]\{3,\}.*全绿\|v0\.[0-9]" AGENTS.md | head
```

**判据**：4.4.1 / 4.4.2 应返回空；4.4.3 只允许 §0 1-2 行指针。

### 4.5 R2 主题锚点体检

```bash
# 4.5.1 同一不变式措辞在多文档 grep
grep -rn "L1.*⊇.*L2\|L1.*粗筛.*L2.*精判" docs/ AGENTS.md

# 4.5.2 同一决策措辞在多文档复述（应只在 AGENTS §2 + rust-plan §15）
grep -rn "D[0-9].*决策\|D[0-9].*硬约束" docs/ AGENTS.md \
  | grep -v "^docs/changelog\|^docs/rust-implementation-plan.md.*§15\|^AGENTS.md.*§2"
```

**判据**：4.5.1 应只在 design §3.2 + 单点引用；4.5.2 应只在 AGENTS §2 + rust-plan §15。

---

## 5. 评审流程

### 5.1 文档对抗评审触发条件

| 改动类型 | 触发评审 |
|---|---|
| 改 R1-R5 规则本身 | 必跑（本文件 §3 checklist + AGENTS.md §5.1 L1 流程） |
| 改文档角色定位（§1 表） | 必跑 |
| 新增文档 | 必跑（先定位角色） |
| 改清单权威源（§4 表） | 必跑 |
| L2 代码改动 | 不触发（跑 §4 体检即可） |
| L3 文档改动 | 不触发（跑 §4 体检即可） |

### 5.2 评审维度（与上次会话 5 agent 对抗评审对齐）

| 维度 | 检查 |
|---|---|
| T1 文档定位 | 改动是否破坏 R1 角色唯一性 |
| T2 主题锚定 | 改动是否引入 R2 重复主题 |
| T3 历史分离 | 改动是否引入 R3 过程性噪声 |
| T4 清单权威 | 改动是否破坏 R4 清单单点 |
| T5 门槛绑定 | 改动是否违反 R5 门槛与内容匹配 |

### 5.3 阻断判据

任一以下情况 = 阻断，必须先修再 commit：

- 改动后 grep §4 体检出现"应返回空"的项目非空
- 改动引入过程性噪声词（§2.1 表）
- 改动复述了已在另一文档锚定的主题
- 改动在高门槛文档中放易变数字

---

## 6. 历史与现状

**本文件创建背景**：2026-10-26 文档对抗评审发现 6 阻断 + ~20 中严重，根因是缺护栏。本文件落 R1-R5 规则防止复发。修复历史详见 changelog v0.38。

**未来扩展**：发现新反模式 → 加到 §2 反模式清单 + §4 体检 checklist。
