# gxp Rust 实现方案报告

**版本**: 见 `docs/changelog.md` 最新版本（v0.38 文档治理收尾）
**对应设计文档**: `gxp-design-full.md`
**版本历史**: `docs/changelog.md`

---

## 0. 执行摘要（一页内）

### 核心选型一览

| 层 | 选型 | Crate & 版本 | 关键理由 |
|---|---|---|---|
| **L1 引擎** | regex crate + regex-syntax literal extraction + aho-corasick | `regex = "1.10"`, `regex-syntax = "0.8"`, `aho-corasick = "1.0"` | regex crate 自带 literal extraction（内部 meta engine）；`regex_syntax::hir::literal::Extractor` 公开抽取必选字面量；aho-corasick 1.0 SIMD 化的多模式字面量匹配。**无需 fork grep 或 fork ripgrep。** |
| **L1 调度壳** | 直接用 ripgrep 的 `grep-searcher` 作为可选 L1 引擎 | `grep-searcher = "0.1"`, `grep-regex = "0.1"`, `grep-matcher = "0.1"` | 不 fork、只依赖：`Searcher::search_slice` + `Sink` trait 提供行级扫描 + 偏移；MPL-2.0 允许商业使用。 |
| **L2 字段抽取** | 手写状态机 + memchr 加速 + serde_json/simd-json for JSON | `memchr = "2.7"`, `simd-json = "0.13"` (optional) | 手写状态机用 memchr 的 SIMD `memchr2(b'=', b':', ...)` 找分隔符，设计目标 20ms（旧机口径，v0.49 起锚点见 §0 性能预期）。 |
| **L3 结构层** | ast-grep-language + tree-sitter fallback | `ast-grep-core = "0.45"`, `ast-grep-language = "0.45"` | ast-grep 的 `useQuery($A)` 语法比 tree-sitter 原生 S-expression 更接近 LLM 习惯；ast-grep-language 自带 ~20 种语言。 |
| **目录层** | ignore (Gitignore 匹配器) + blake3 + globset + rayon | `ignore = "0.4"`, `blake3 = "1.4"`, `globset = "0.4"`, `rayon = "1.10"` | 手写 `std::fs::read_dir` 栈式遍历（避开 WalkBuilder 固定开销，实测数据见 performance-baseline §1.4）；ignore 保留为 `.gitignore` 独立匹配器；globset 做 `--glob` 文件名过滤；rayon 跨文件并行扫描。 |
| **SIMD** | memchr（默认 SIMD）+ simd-json（JSON 路径 optional） | 同上 | 不碰 `std::simd`（nightly-only）也不碰 `packed_simd`（已弃用）。memchr 在 x86_64/aarch64 上自动 dispatch AVX2/SSE2/NEON。 |
| **内存** | 统一 `read_to_end`（D10）+ thread-local buffer 复用 + Arc<[u8]> | （无生产依赖；`memmap2` 降为 bench-only） | 实测：mmap page fault 分摊三阶段反而慢于 fs::read（详细数据见 performance-baseline §1.1 + §1.2）；MatchedLine.line 改 Arc<[u8]> 引用计数 clone。 |
| **CLI** | clap v4 derive + 手写 DSL 管道解析 + 入口解析（argv 扫描） | `clap = "4.5"`, `clap_complete = "4.5"`, `nom = "7.1"`, `thiserror = "1.0"` | gxp **不接受 grep 短选项**；入口扫描 argv 决定 BARE 模式（ERE 兼容）或 gxp 模式；grep 短选项报四段式错。 |
| **测试** | proptest + 穷举校验 + criterion 基准 | `proptest = "1.4"`, `criterion = "0.5"` | proptest 复现 0..60000 × 6 区间穷举；criterion 对照 GNU grep / ripgrep。 |

### 总体判断

**不 fork ripgrep，从头写。** 理由：
1. ripgrep 的 `grep-searcher` + `grep-matcher` 已作为 crates.io 上的库发布，直接作为依赖使用即可获得其 SIMD 加速的搜索循环。
2. gxp 的核心创新在 L1→L2 的下推不变式（必选组上提 + L1 下推字段名前缀粗筛、L2 精确数值比较），与 ripgrep 的核心逻辑平行而非冲突。
3. fork ripgrep 意味着承担 5 万行 MPL-2.0 文件级别的维护责任。

### 性能预期

> 当前实测数据详见 `docs/performance-baseline.md §1.1-1.2`（权威源）。基线锚点（v0.49 起本机 i7-10700，AGENTS.md §7.1 回归性门禁：median 劣化 >10% 需归因）：全管道 106.74ms（diag median）/ L1 两遍链 33.51ms / L2 字段抽取 8.21ms（v0.47 criterion）；旧机 80/30/20ms 目标降历史参考（跨机比值 1.52-1.85×，perf-baseline §1.1 机器变更说明）。

---

## 1. L1 引擎

### 1.1 选型

- **核心**: `regex = "1.10"` + `regex-syntax = "0.8"` + `aho-corasick = "1.0"`
- **可选加速**: `grep-searcher = "0.1"` + `grep-regex = "0.1"`（ripgrep 的搜索循环壳）

### 1.2 理由

**方向 B**（D4 决策）：L1 只下推字段名前缀（`cost_ms=`）粗筛，L2 精确数值比较。

1. **regex crate 自带 literal extraction**：`regex_syntax::hir::literal::Extractor` 是公开 API，从 HIR 抽出必选字面量序列。
2. **aho-corasick 1.0**：SIMD 加速的多模式字面量匹配器，把"必选组 1 OR 必选组 2"压成一次扫描。
3. **数值谓词走方向 B**：L1 只下推字段名前缀，不编译成正则。理由：正则表达力是 CFG 子集，无法覆盖任意数值比较。L2 抽取字段值后做精确数值比较。不变式 `L1(粗筛) ⊇ L2(精判)` 严格成立（定义见 design §3.2）。

### 1.3 实现落地

> 见 `src/l1/compiler.rs` + `src/l1/scan.rs`。

`CompiledL1` 包含：必选组（`LiteralGroup` = aho-corasick 粗筛 + `regex::bytes::Regex` 精判）+ `field_prefixes`（字段名前缀下推）+ `numeric_predicates`（L2 精判）。

**优化**：`LiteralGroup::is_pure_literal` 标记让纯 String/Bare 组跳过 `regex::is_match` 第二遍（aho-corasick 命中即等价 regex 命中）。

### 1.4 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| `regex_syntax::hir::literal::Extractor` API 可能在 0.8→0.9 变化 | 中 | pin 版本到 `=0.8.x`；CI 跑穷举校验 |
| L2 数值比较在浮点 NaN/Inf 处理边界 | 低 | 阈值 f64 解析时拒绝 NaN/Inf |
| aho-corasick 在零字面量模式（如 `.*`）下退化 | 低 | fallback 到确认型 aho-corasick |
| 数值字段格式多样 | 中 | L2 按 §3.3 11 维契约处理 |

---

## 2. L2 字段抽取

### 2.1 选型

- **手写状态机** + `memchr = "2.7"` 的 SIMD `memchr2`/`memchr3` 找分隔符
- JSON 路径：`serde_json = "1.0"`（simd-json 不启用，小 JSON 行反慢）
- 时间戳：`speedate = "0.16"`

### 2.2 理由

1. **不用 nom/winnow/chumsky**：固定 grammar 上没有性能优势，反而引入依赖。
2. **memchr 默认 SIMD 化**：`memchr2(b'=', b':', line)` 一次扫描定位所有分隔符。
3. **Late Materialization**：只解析查询引用到的字段（DSL 编译期收集 `referenced_fields`）。
4. **优化**：`find_value(line, name)` 懒单字段定位（跳过全量 extract + Vec 分配）。
5. **优化**：`batch_find_values` 多字段批量提取，n≥3 时一次 `memchr2` 调用覆盖所有分隔符。

### 2.3 实现落地

> 见 `src/l2/extract.rs` + `src/l2/format.rs` + `src/l2/numeric.rs` + `src/l2/time.rs`。

- `LineFormat`（Plain/Logfmt/JsonLine/BracketTag/IsoTimestamp/Mixed）基于首字符 + 分隔符密度探测
- `ExtractedFields` 零拷贝视图：`kv: Vec<(&[u8], &[u8])>` + `tags: Vec<&[u8]>` + `timestamp`
- `find_value(line, name)` 懒单字段定位
- logfmt 带引号值（v0.50）：`k="v"` 值为引号内字节、值终点为闭合 `"`；`extract_logfmt_kv` 与 `find_value` **镜像同步**（§6.5.3 同构约束）；未闭合引号回退空白切分（设计文档 §3.3 带引号值契约）
- `batch_find_values(line, names)` 多字段批量提取（`BATCH_THRESHOLD=3`）
- `parse_numeric_value(v: &[u8]) -> Option<f64>`：11 维契约，拒绝千分位/单位/空白 trim/NaN/布尔
- `eval_numeric(value_bytes, op, threshold) -> bool`：parse 失败 → false（fail-safe）

### 2.4 数值字段格式契约（11 维，D5）

| 维度 | 支持 | 示例 |
|---|---|---|
| 整数 | ✅ | `cost_ms=800` |
| 负数 | ✅ | `cost_ms=-5` |
| 正号 | ✅ | `cost_ms=+800` |
| 前导零 | ✅ | `cost_ms=0950` |
| 浮点 | ✅ | `cost_ms=800.5` |
| 科学计数法 | ✅ | `cost_ms=8e2` |
| 千分位 | ❌ | `cost_ms=1,000` |
| 单位 | ❌ | `cost_ms=800ms` |
| 空白 trim | ❌ | `cost_ms= 800 ` |
| 布尔 | ❌ | `ok=true` |
| NaN/Inf | ❌ | `cost_ms=NaN` |

### 2.5 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| `k:v` 与 ISO 时间戳冒号打架 | 中 | 词边界约束：key 前是空白或行首 |
| speedate 不支持 RFC 3339 全部变体 | 低 | fallback 到 `time::OffsetDateTime::parse` |

---

## 3. L3 结构层

### 3.1 选型

- **主选**: `ast-grep-core = "0.45"` + `ast-grep-language = "0.45"`（feature `builtin-parser`）
- **降级**: `tree-sitter` + 单独 grammar crate（v2 评估）

### 3.2 语言覆盖三层结构

| 层 | 语言数 | 语言 | 落地机制 | 状态 |
|---|---|---|---|---|
| **层 1：穷举测试** | 9 | py/js/ts/tsx/rs/go/java/c/cpp | ast-grep 原生 + 穷举测试 | ✅ 完成 |
| **层 2：ast-grep 内置** | ~20 | +C#/Ruby/PHP/Bash/SQL/HTML/CSS/JSON/YAML/TOML/Lua/Scala/Kotlin | `ast-grep-language` feature `builtin-parser` | 天然可用，不穷举测试 |
| **层 3：tree-sitter fallback** | ~10 | Swift/Haskell/Erlang/Clojure/Dart/R/Julia + 其他 | 单独 grammar crate + feature flag | v2 评估 |

**关键边界**：
- L1+L2 对所有文本文件通用，不区分语言
- CSS/SCSS/Less/Markdown 不进 L3（BARE 模式已覆盖文本检索需求）
- Vue SFC 需预处理提取 `<script>` 块，v2 评估

### 3.3 实现落地

> 见 `src/l3/structural.rs`。

- `AstLineSet = HashMap<String, HashSet<usize>>`（key = `"{lang}\x00{pattern}"`）
- `run_single(query, content)` ast-grep find_all + memchr 行号映射
- `run_all_ast(prepared, content)` 递归收集 + 同语言多 pattern 复用 AstGrep root
- `lang_from_str` 九语言映射 + 大小写不敏感 + 全拼别名 + c++/cxx/cc
- feature `l3-structural` 条件编译：默认关闭，关闭时 `PreparedPrim::Ast` 视为永真

### 3.4 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| tree-sitter grammar 版本锁定 | 中 | fail-loud + `--explain` 输出 ast-grep/tree-sitter 版本号 |
| 解析失败静默返回空集合 | 中 | v1.0 应检测解析错误并 fail-loud |
| ast-grep pattern 语义依赖语言 | 低 | 穷举测试覆盖 |

---

## 4. 目录层

### 4.1 选型

- **目录遍历**: 手写 `std::fs::read_dir` 栈式遍历（避开 WalkBuilder 固定开销，实测见 performance-baseline §1.4）
- **.gitignore**: `ignore = "0.4"` 的 `Gitignore` 独立匹配器
- **文件名过滤**: `globset = "0.4"`（`--glob`）
- **目录指纹**: `blake3 = "1.4"`（截断到 16 字节）
- **文件读取**: 统一 `std::fs::read_to_end`（D10）；>512MB 走流式 fallback
- **跨文件并行**: `rayon = "1.10"`（PreparedScan Send+Sync 已验证）

### 4.2 实现落地

> 见 `src/dir/{mod,index,plan,tail,git}.rs`。

- `FreshnessMode`（Strict/Lax/Off）三档语义
- `PathPlan`：`file_ok` 单文件判定 + `dir_eligible` pre-descent 剪枝 + `glob_patterns` + `respect_gitignore`
- `DirIndex::list(plan)`: off 模式零 syscall 返回缓存 → 手写栈遍历 → file_ok 过滤 → stat/lax/off 三档
- `TailIndex::check(path)` 对比 `FileState { inode, size, mtime_ns }` 前后状态
- `GitignoreChecker` 从 walk 根向上查找 .gitignore + 每层目录加载

### 4.3 跨平台路径

| 平台 | 路径分隔 | inode 源 | mtime 源 |
|---|---|---|---|
| Linux/macOS | `/` | `MetadataExt::ino()` | `MetadataExt::mtime_nsec()` |
| Windows | `\` | `MetadataExt::creation_time()`（FILETIME 100ns，伪 inode） | `SystemTime::duration_since(UNIX_EPOCH)` |

Windows 用 `creation_time()` 作伪 inode（stable Rust 下 `file_index()` 需要 unstable feature `windows_by_handle`）。

### 4.4 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| Windows 上 lax 无收益 | 低 | Windows `FindFirstFileExW` 已返回 file_type，stat ~1µs/次；Linux 接入后 lax 有收益 |
| blake3 截断到 16B 抗碰撞下降 | 低 | 单进程内单次查询有效，T1 不跨查询 |

---

## 5. SIMD 加速

### 5.1 选型

- **`memchr = "2.7"`**：默认 SIMD 化，所有热点字节扫描走它
- **`simd-json = "0.13"`**（optional feature，不启用）
- **不碰** `std::simd`（nightly-only）、`packed_simd`（已弃用）

### 5.2 理由

memchr crate 在 x86_64 上自动 dispatch AVX2（fallback SSE2），aarch64 上 dispatch NEON。关键性能点都走 memchr：行扫描、字段分隔符、必选组字面量、行号计数。

### 5.3 SIMD 配置结论

- **aho-corasick NFA vs DFA**：DFA 边际 +3.8%，噪声范围。不接入 DFA。
- **simd-json**：~97 字节/行的小 JSON 反慢 32%。不启用。
- **自定义 AVX2 多字段批扫**：2 字段时 find_value 两遍更快；n≥3 时 batch 反超（已落地 `batch_find_values`）。

---

## 6. 内存管理

### 6.1 选型

- **所有文件统一 `read_to_end`**（D10）
- **>512MB 走流式 `BufReader`**（`STREAMING_THRESHOLD = 512MB`）
- **stdin 读取**：`read_stdin_for_scan()` 全量读入 Vec
- **MatchedLine.line: Arc<[u8]>**（引用计数 clone）

### 6.2 D10 决策理由

实测数据见 `docs/performance-baseline.md` §1.1。决策：mmap 惰性映射触发首次扫描 page fault 集中爆发，分摊在 L1+collect+execute_stages 三阶段；fs::read 预付读取后所有阶段零页错误，净收益为正。详细毫秒数 + 软页错误计数 + TLB miss 估算见 perf-baseline。

### 6.3 流式 fallback

- `FileContent::Stream(PathBuf)` 变体 + `STREAMING_THRESHOLD = 512MB`
- `scan_stream_lines_cb`：`BufReader::read_until(b'\n')` 逐行扫描
- `apply_context_streaming`：二次遍历文件，`HashSet<usize>` 收集 context 窗口内行号
- `execute_stages_streaming`：流式管道执行链
- `--verify`/`--strict`/`--explain`/`ast()` 在流式模式下 fail-loud

### 6.4 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| 超大文件（1GB+）占进程堆 | 低 | >512MB 走流式 fallback；V7 验证 1GB read_to_end 比流式快 5-16% |

---

## 7. CLI 层

### 7.1 选型

- **CLI 解析**: `clap = "4.5"`（derive + cargo）
- **DSL 管道解析**: 手写 tokenizer（8 个动词：`where | rank | head | fields | count | group | context | files`）+ `stats`/`sample`/`sort`/`distinct`/`percentiles`
- **表达式解析**: `nom = "7.1"`
- **入口解析**: 手写 argv 扫描（决定 BARE 模式 vs gxp 模式 + grep 短选项拦截）
- **shell completion**: `clap_complete = "4.5"`（`--generate-completion <SHELL>`）

### 7.2 理由

gxp **不是 grep 的严格超集**（D1），不接受 grep 短选项参数。等价功能用 gxp 原生语法表达：

| grep 用法 | gxp 等价 |
|---|---|
| `grep -e a -e b file` | `gxp 'a \|\| b' file` |
| `grep -v PATTERN file` | `gxp '!PATTERN' file` |
| `grep -A N PATTERN file` | `gxp 'PATTERN \| context after:N' file` |
| `grep -B N PATTERN file` | `gxp 'PATTERN \| context before:N' file` |
| `grep -C N PATTERN file` | `gxp 'PATTERN \| context N' file` |
| `grep -o PATTERN file` | `gxp 'PATTERN \| fields _match' file` |
| `grep -l PATTERN *.log` | `gxp 'PATTERN *.log \| files' file` |
| `grep -c PATTERN file` | `gxp 'PATTERN file \| count' file` |
| `grep -r PATTERN dir` | `gxp 'PATTERN dir'` |

### 7.3 实现落地

> 见 `src/cli.rs` + `src/mode/detect.rs` + `src/mode/reject_grep_opts.rs`。

- `Cli`（clap derive）：`query: String` + `paths: Vec<String>` + `--explain`/`--verify`/`--strict`/`--json`/`--glob`/`--no-gitignore`/`--generate-completion` flags
- `is_gxp_mode(pattern)` 检测双引号/`&&`/`||`/`!`/管道 `|`/字段谓词特征 + `ast(`/`capture(` 前缀
- `detect_and_dispatch` 入口分派 BARE vs gxp 模式
- `format_reject_error` 四段式报错
- `--query-file PATH`（v0.50）：查询从文件读取（剥 BOM + trim）后**前置为 `args[0]`**，`detect_and_dispatch` 对此透明；前置形态由 clap 解析，后置双形态（`--query-file P` / `--query-file=P`）进 `extract_trailing_flags` + `KNOWN_LONG_FLAGS`；空文件/多行/与位置 pattern 互斥均 fail-loud（设计文档 §1.5 + §2.4 示例 12）

### 7.4 `!` 词法规则（D6，方向 C）

`!` 是操作符当且仅当前后有空白或非字母数字边界；作用于 pred 必须用括号 `!(cost>800)`。

### 7.5 `context` / `files` 管道动词

> 见 `src/dsl/pipeline.rs` + `src/pipeline/context.rs` + `src/pipeline/files.rs`。

`Stage::Context { before, after }`：`context N` 前后各 N 行；`apply_context` 预扫行偏移表 + 窗口扩展 + 去重。`Stage::Files { invert }`：`files` 列命中文件名。

### 7.6 目录多文件聚合

`execute_stages_directory` 函数（目录场景专用）：每文件独立调 `apply_context`（规避跨文件边界），所有文件的扩展后 `MatchedLine` 聚合到全局 Vec → 跑剩余 stages。

### 7.7 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| shell pipe `\|` 截断 | 中 | 文档强约束：用引号包整个查询 |
| ERE OR `\|` vs gxp 管道歧义 | 中 | 规则：引号内 `\|` 是 ERE OR；引号外是 gxp 管道 |
| `!` 词法规则嵌套 | 低 | 递归下降解析器自然处理 |

### 7.8 字符串等值脱糖（v0.48）

> 见 `src/dsl/parser.rs::pred_compare` + `src/mode/detect.rs::has_field_predicate` + `src/mode/gxp.rs::enrich_string_eq_hint`。设计文档 §3.2「字符串等值脱糖」+ §2.4 示例 2a。动机：agent A/B 实验第一摩擦源（3/8 agent 自然写出 `level="ERROR"` 被 v0.47 runtime 数值错误拒绝，见 `docs/agent-ab-experiment.md`）。

- **脱糖**：`NAME ("="|"==") "STRING"` 在 `pred_compare` 解析期直接构造 `Pred::In { values: [STRING] }`——AST/L1/L2 无新增节点与分支，Or 组完整性（B14）/ Not 子树豁免 / 伪字段 / captures 优先级全部自动继承 `in` 现有路径
- **消歧规则**（实测钉死 v0.47 基线后确定）：
  - 带引号 RHS + `=`/`==` → 字符串等值（单元素 In）
  - 带引号 RHS + `!=`/`>`/`>=`/`<`/`<=` → `pred_compare` 返回 Err + prim 层 shape guard 拦截（不落 BARE 兜底，v0.42 静默吞同类），hint 由 `enrich_string_eq_hint` 附加（字符串无大小序；`!=` 正确写法是 `!(NAME="...")`）
  - 无引号 RHS 数值开头 → 数值等值（Compare，D5 原样）
  - 无引号 RHS 非数值 + `=`（单等号）→ pred 返回 Err 落 BARE 兜底 = ERE 字面量（**对齐设计文档"`=` 不在 BARE 谓词分歧操作符列表"**；v0.47 实测 `a=b` 报 runtime 数值错误，属文档-实现不一致，v0.48 修正）
  - 无引号 RHS 非数值 + `==` → 保持 D5 runtime 拒绝（`==` 在文档中是有意分歧操作符，不回退 BARE）
- **模式检测**：`has_field_predicate` 对 `字母=` 增加 RHS 前瞻——越过连续 `=` 与空白后首字符为数字/`-`/`+`/`.` 才判谓词特征；`==`（两个及以上）仍无条件触发。`a=b` 由此回归 BARE/ERE 通道（`level="ERROR"` 经双引号规则照常进 gxp 模式）
- **B14 教训落实**：e2e 独立期望集（`tests/cli_string_eq.rs`：等值三元等价 / Not 补集 / Or 组混字面量 / capture 字段 / 伪字段 / BARE `a=b` 回归 / 两条 hint 错误路径），不依赖 `--verify`（暴力与优化共用 L1 编译同错对齐）

### 7.9 L2 Literal 重校验豁免 + collect 零分配化（v0.49 T1a）

> 见 `src/l1/compiler.rs::verified_literals` + `src/mode/prepared.rs` + `src/mode/eval.rs` + `src/mode/detect.rs::find_match_bytes`。动机：v0.48 T1a 基线锚定（perf-baseline §1.2）发现 125.30ms 中 **~30ms 未归因差值**，diag 复刻检查点归因为 collect 路径三项冗余（合计 ~16ms 实测增量）+ 相位隔离测量理想化残差 ~11ms。

**归因数据**（diag_pipeline 复刻阶段，2026-11-09，i7-10700）：

| 冗余项 | 实测增量 | 机理 |
|---|---|---|
| L2 Literal `regex.is_match` 重校验 | +5.8ms / 141367 行 | L1 必选组 aho 命中已保证字面量存在，L2 全树精判再跑一次转义正则 |
| path 填充 `String::clone` | ~6.5ms / 103607 行 | 单文件模式所有命中行共享同一路径，却每行堆分配一个 String |
| `find_match_bytes` 二次 aho + `Vec::clone` | ~3.6ms / 103607 行 | 单元素纯字面量组命中即确定 `_match` 值，无需再 find + 每行分配 |

**Opt A（Literal 重校验豁免）**：`compile()` 产出 `verified_literals: Vec<Vec<u8>>`——仅收**单元素纯字面量必选组**的成员；`prepare_expr` 据此给 `PreparedPrim::Literal` 置 `l1_verified` 标志；eval 命中标志直接返回 `True` 不跑 regex。

健全性论证（构造性，三条）：
1. **单元素组 any-hit ≡ 字面量存在**：必选组全部命中才进 L1 回调；组内仅一个 `regex::escape` 转义字面量，any-hit ⟺ 该子串在行内 ⟺ L2 的 `regex.is_match`（同 pattern）。这是 §15「L1 纯字面量组跳过 regex verify」决策在 L2 侧的同一论证。
2. **Or 分支组永不单元素**：`collect_or_literals` 的每个 complete 叶子（String/Bare，含 Paren 递归）至少推进 1 个字面量，Or 有两支 → 组 ≥2 元素（compiler.rs Or 分支推组处不可能产出单元素组）。Or/Not 侧字面量因而不入集——B15 场景（`level in [ERROR] || "plain"`）plain 不豁免。
3. **Paren(And) 陷阱**：`"a" && ("b" && "c")` 的 Paren 内 And 组因 B14 完整性规则**整组放弃**（collect_or_literals 对 And 返回 false），b/c 无 L1 组——朴素的「路径只经 And/Paren 即已验证」规则不健全。故验证集必须由 `compile()` 按实际推组结果产出，不做树路径平行实现（§6.5.3 同构镜像教训）。

同串多出现语义：某串入集 ⟺ 每条 L1 幸存行必含该子串 ⟺ 树中**任意位置**的同串 Literal 节点求值为 True（含 Not 内——`Not(已保证存在的字面量)` 恒 False，与跑 regex 结果一致）。集合按串去重语义健全。

**B14 教训落实**：`run_verify` 暴力与优化路径共用 `PreparedScan::build`（verify.rs:34）——豁免标志不健全时两侧同错对齐，`--verify` 不可发现。必须走独立期望集 e2e（`tests/cli_l1_verified.rs`：And 单例豁免 / Paren(And) 不豁免 / Or 侧不豁免 / Not 侧语义 / 同串跨子树）。

**Opt B（match_bytes 零分配）**：`LiteralGroup.literals: Vec<Vec<u8>>` → `Vec<Arc<[u8]>>`；`MatchedLine.match_bytes: Option<Vec<u8>>` → `Option<Arc<[u8]>>`；`find_match_bytes` 对单元素纯字面量组跳过 aho find 直接 `Arc::clone`（命中即该字面量，无需鉴别）。多元素组保留 find（需鉴别命中的是哪个 pattern），返回 `Arc::from(bytes)`。

**Opt C（path 零分配）**：`MatchedLine.path: Option<String>` → `Option<Arc<str>>`，填充/克隆点全部 refcount 化（gxp.rs 两处填充循环 / pipeline/mod.rs 入口补填 / context 复制点 / dir 层收集）。

**范围外**：残差 ~11ms（相位隔离理想化 + PreparedScan::build + 分发开销）不在本轮；samply 在本机编译失败（v0.13.1），归因以 diag 复刻检查点为准。

**残差归因补记（2026-11-09 晚，C1/C2/C3 判别实验，diag_pipeline 新增检查点）**：残差 ~13.7ms（108.55 − 阶段累计 94.84）拆为三桶各 4-6ms——① 生产 collect 口径差 ~6.0（match_bytes/path 逐行填充，`_match`/`_file` 语义必需）② 链内相位交错 ~4.8（端到端固有）③ 生产专属 ~4.1（eval 机制 + CaptureMap::new × 141k + dispatch；build <0.1ms）——无单一大头。CaptureMap 空表共享预估 <2ms 且需动 with_captures 边界，收益/风险比不足，**暂不做**。数据与判别式见 perf-baseline §1.2 残差归因块。

---

## 8. 测试与验证

### 8.1 选型

- **属性测试**: `proptest = "1.4"`
- **基准**: `criterion = "0.5"`
- **CI**: GitHub Actions matrix：`ubuntu-latest`, `macos-latest`, `windows-latest`

### 8.2 不变性校验（必须 100% 通过）

- `L1(粗筛) ⊇ L2(精判)`：每个 L1 改动必须对账验证
- `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（BARE 模式）
- `tests/dir_*.rs` 31 项全绿
- `--verify` 暴力对账：100% 一致

### 8.3 测试金字塔

| 层级 | 目标 | 工具 |
|---|---|---|
| 单元 | ≥80% 行覆盖 | `cargo test` + `proptest` |
| 穷举 | 数值谓词 0..60000 × 6 区间 0 错 | explicit test |
| 集成 | 31 项目录层行为 | `tests/dir_*.rs` |
| 基准 | gxp vs GNU grep vs ripgrep | `criterion` + `benches/scale_dir.rs` |
| CI | Windows + Linux + macOS | GitHub Actions |

### 8.4 测试覆盖盲区记录

#### 8.4.1 --verify 暴力路径 vs 优化路径覆盖矩阵

**发现方式**：对真实代码库跑 `--verify ast(ts, "ref($A)") && "dict"`，142 个 FAIL → 根因是暴力路径跳过 L1 正则粗筛。

**修复后已覆盖的回归测试**：场景 A-G 全部落地，包括 ast+literal 正向/负向路径、目录、stdin、ast+field_pred、OR、Not、多 ast 节点。

#### 8.4.2 单文件测试掩盖目录路径

**发现方式**：`run_gxp_mode_directory` 和 `run_verify` 完全缺失 L3 `ast()` 预计算路径。所有 e2e 测试用单文件 `write_temp`，目录路径从未被测试执行过。

**已修复** + 3 项目录模式 L3 e2e 测试。防再发生规则见 AGENTS.md §6.6 并行路径交叉覆盖规范。

#### 8.4.3 防止新功能漏测试的检查清单

1. **所有代码路径都有测试入口**：单文件/目录/stdin 三条路径都必须有 e2e 测试
2. **组合条件矩阵**：对涉及多个条件的查询，列出条件矩阵，每个非空交集至少一个测试
3. **正向 + 负向双路径**：不只测"不命中"路径，也要测"命中"路径
4. **暴力路径语义对齐**：`--verify` 的暴力路径必须与优化路径走相同的语义链
5. **交叉验证**：新功能落地后，用 gxp 二进制对自身源码或真实代码库跑一次查询 + `--verify`

---

## 9. 与 ripgrep 的关系

### 9.1 不 fork ripgrep，用依赖

| 路线 | 代价 | 收益 | gxp 选 |
|---|---|---|---|
| **从头写** | 自己实现 L1 调度壳、L2 字段抽取、DSL 编译器 | 完全控制，无 MPL 责任 | ✅ v1 |
| **依赖 ripgrep crates** | `grep-searcher` + `grep-regex` + `ignore` 作为 crates.io 依赖 | 复用 ripgrep 的 SIMD 搜索循环 | ✅ v1 |
| **fork ripgrep** | 维护 5 万行 MPL-2.0 代码 | 直接复用 core/args/main.rs | ❌ v1，留 P2 选项 |

### 9.2 ripgrep crates 可复用性

- `grep-searcher = "0.1"`：**高**。`Searcher::search_slice` 对 `read_to_end` 的 `&[u8]` 直接扫
- `grep-regex = "0.1"`：**中**。`RegexMatcherBuilder` + 内置 literal extraction
- `grep-matcher = "0.1"`：**高**。`Matcher` trait + `Captures` 抽象
- `ignore = "0.4"`：**高**。独立设计，已选（§4）

### 9.3 何时考虑 fork

如果 v2 需要：跨压缩文件搜索（ugrep 兼容）/ 跨多个 L1 必选组并行调度 / 相关度排序改 `Sink` trait。

---

## 10. 性能基准

### 10.1 选型

- **基准库**: `criterion = "0.5"`
- **对照点**: GNU grep 3.11, ripgrep 13.x
- **数据**: 44MB / 1,060,000 行合成日志 + 154MB 目录树 + 70648 文件目录

### 10.2 基准矩阵

| 基准名 | 输入 | 指标 |
|---|---|---|
| `l1_scan_only` | 44MB 单文件 | 行/秒 |
| `l1_chain_2_groups` | 两遍必选组 | 端到端 ms |
| `l2_extract_31k` | 31931 候选行 | µs/行 |
| `l2_numeric_eval` | 0..60000 6 区间 | µs/比较 |
| `dir_walk_154mb` | 361 文件 / 35 目录 | ms |
| `full_query_top20` | 44MB + Top20 + 投影 | 端到端 ms |

### 10.3 实测结果

> 详见 `docs/performance-baseline.md §1.1`（权威源）。摘要：所有目标达标，具体数字不再复述（防同步漂移）。

---

## 11. 实现路线图

### 11.1 已完成里程碑

> 完整版本演进 + 里程碑 + 决策记录见 `docs/changelog.md` §1（版本演进总结）+ §3（关键决策里程碑）。本节不再复述表格，避免双点维护漂移。

### 11.2 待验证项清单

| ID | 验证项 | 验证方法 | 状态 |
|---|---|---|---|
| V1 | PreparedScan Send+Sync | 四路交叉验证 + 编译断言 | ✅ 通过 |
| V2 | capture 正则提取与 L1⊇L2 | 设计评审：选项 A（纯 L2） | ✅ 通过 |
| V3 | `_ts between` 与 D5 兼容性 | 设计评审：选项 A（`_ts` 伪字段特例） | ✅ 通过 |
| V4 | `stats` 流式聚合与 pipeline 兼容性 | 设计评审：选项 A（只做 min/max/avg） | ✅ 通过 |
| V5 | C/C++ ast-grep 预处理行为 | 11 项实验 | ✅ 通过 |
| V6 | 三平台 CI | CI matrix windows + ubuntu + macos | ✅ 通过 |
| V7 | D10 大文件流式 fallback | 1GB 合成文件四方案对比 | ✅ 通过（D10 维持） |

### 11.3 候选功能方向 + 暂不评估清单（权威表，2026-10-26 合并）

> **权威性声明**：本节是 gxp "候选功能"与"暂不评估/永久拒绝"清单的**单一权威表**。其他文档（AGENTS.md §9.1、capability-matrix §3）只保留链接 + 一句话摘要，不重复清单内容。
> **更新规则**：每次候选功能落地、re-triage、永久拒绝决策变更时，更新本表。changelog 同步追加版本行。

#### 11.3.1 候选功能方向（对抗评审排序后，2026-10-24）

| # | 功能 | 场景 | 不变式 | 等级 |
|---|---|---|---|---|
| 1 | `sample N`/`sample RATIO` ✅ v0.33 落地 | LLM context window 物理限制 | 无 | L2 |
| 2 | 多行 stack trace ✅ v0.39 落地 | 异常分析当前只抓首行 | L1 不变（pipeline 阶段判定续行边界） | L0 |
| 3 | `distinct` 去重投影 ✅ v0.35 落地 | 唯一 user_id/error_code（与 group by 语义区分：行列表 vs 计数） | 无 | L2 |
| 4 | 多行 JSON 合并 | re-triage 后转暂不评估（2026-10-26） | — | L0 → 暂不评估 |
| 5 | `sort [by FIELD] [desc\|asc]` ✅ v0.34 落地 | rank top-K 外的全量有序 | 无 | L2 |
| 6 | `percentiles FIELD [p50,p95,p99]` ✅ v0.37 落地 | SLO 报告核心指标；avg 掩盖长尾 | 非流式（与 sort 一致） | L2 |
| 7 | `--json` 单文件 path 字段 ✅ v0.36 落地 | search→edit 自包含；schema 与目录一致 | 无 | L1 |

> **候选 #4 re-triage 决策（2026-10-26）**：候选 #4 起源是 2026-10-22 某低代码平台 代码库评估副产品（场景 B），场景 B 在 v0.40 修正 §8.2 判决理由后独立出去（需 preprocessor 概念，与 Vue SFC 同类）。剩余场景 A（pretty-printed JSON 日志合并）是事后补的合理化场景，不是真实 LLM 工作流驱动需求——违反 §0.1 第 1 条场景驱动原则。场景频率评估：K8s/Docker 生产日志默认单行 JSONL（不是多行），配置文件 LLM 通常直接 Read，多行 JSON 在真实 LLM 工作流中稀疏。替代路径：`jq -c '.' multiline.log | gxp 'cost_ms>500'`。详见 §11.3.2 暂不评估清单。

#### 11.3.2 暂不评估清单 + re-triage（2026-10-26，场景驱动）

判据三问：(a) 是否有真实 LLM 工作流需求（§0.1 场景驱动，非功能完备性）？(b) 是否触及 D1-D11 硬约束或 `L1 ⊇ L2` 不变式？(c) 是否有等价组合可替代？

| 项 | (a) 场景驱动力 | (b) 不变式风险 | (c) 等价替代 | 结论 |
|---|---|---|---|---|
| `--type NAME` | 中（语言过滤是高频，但 `--glob *.rs` 已覆盖） | 无（CLI 层） | `--glob *.rs` / `--glob *.{rs,toml}` | **仍保持暂不评估**：场景弱（`--glob` 等价），实现成本低（clap arg + 内置扩展名映射表），但 v1.0 后无强驱动力。升级条件：用户报告 `--glob` 语法负担 |
| 时间窗口聚合 | 中（按小时/天 group 是真实需求） | 无（B2 修复后 capture+group by 组合可达） | `capture(hour, /\d{4}-\d{2}-\d{2}T\d{2}/) \| group by hour` | **仍保持暂不评估**：组合等价。但若用户反馈 capture 正则写法负担重，可升级为一等公民动词 `bucket FIELD by hour` |
| `--json` byte_offset | 弱（LLM 编辑场景需 byte 定位，但 path+line_no 已够；byte_offset 仅 VLQ 编辑协议需要） | 无 | path + line_no + match_bytes 已自包含 | **仍保持暂不评估**：场景驱动不足。升级条件：明确出现需要 byte 精确定位的下游工具集成 |
| T2/T3 缓存 | 弱（gxp 一次性命令模型与守护进程冲突，D9） | **D9 硬约束**（T0/T1 only，当前实现） | 无 | **永久拒绝**（除非 D9 改判）：跨查询增量需要守护进程，与 grep 一次性命令模型根本冲突。设计文档 §4.3 已有实测数据支撑（zone-map 在生产日志跳过率 0%） |
| L3 Vue SFC/CSS/Markdown | 弱（Vue SFC 需预处理提取 `<script>` 块，CSS/Markdown AST 查询场景稀疏） | L3 模型需加 `preprocessor` 概念（中等改动） | 无 | **仍保持暂不评估**：场景稀疏。升级条件：Vue 项目用户明确报告无法查 SFC 内 JS 结构 |
| L3 Swift/Haskell/Erlang 等 | 弱（按文件量排 P1，主语言已九语言覆盖） | 无（tree-sitter fallback） | 无 | **仍保持暂不评估**：场景稀疏。升级条件：某语言文件量在真实代码库 >5% |
| 跨压缩文件 | 弱（gzip log 是真实场景，但需引入新 crate 与 D10 流式 fallback 冲突） | **D10 流式 fallback 兼容性**（gzip decoder 是状态机，需 stream 接入） | 预解压到临时文件后扫描 | **仍保持暂不评估**：D10 兼容性 + 新 crate 风险。升级条件：压缩 log 占比 >30% 且预解压开销不可接受 |
| SIMD JSON 强制启用 | 弱（simd-json 已 optional feature，默认 off；强制启用违反 D10 read_to_end） | **D10 硬约束**（simd-json 需要 &[u8] 整体，与 BufReader 流式不兼容） | simd-json feature 门控（已落地） | **永久拒绝**（除非 D10 改判）：simd-json 强制启用与 D10 流式 fallback 根本冲突 |
| `tail N` | 弱（"最后 N 行"场景，但日志按时间戳排序更常见） | 无 | `sort by _ts desc \| head N \| sort by _ts asc`（三段组合） | **仍保持暂不评估**：组合等价。三段管道开销 ~150ms（sort 全量），若场景频繁可升级 |
| `fields` 自定义分隔符 | 弱（默认空格已覆盖 90%，自定义分隔符场景稀疏） | 无 | 无 | **仍保持暂不评估**：场景稀疏。升级条件：CSV/TSV 日志场景频繁 |
| 多行 JSON 合并（原候选 #4） | 弱（K8s/Docker 生产日志默认单行 JSONL；配置文件 LLM 直接 Read；多行 JSON 在真实 LLM 工作流稀疏） | 无（方案 c：L4 合并与 trace 同构；方案 b：L2 改造风险中） | `jq -c '.' multiline.log \| gxp 'cost_ms>500'`（jq 预处理转单行 JSONL） | **仍保持暂不评估**：re-triage 后从候选 #4 转入。起源是 2026-10-22 某低代码平台 评估副产品，场景 A 是事后合理化，不是真实 LLM 工作流驱动需求（违反 §0.1 第 1 条）。升级条件：用户明确报告多行 JSON 日志场景频繁 + jq 预处理负担重 |
| `--verify-large` 分块 verify | 弱（>512MB 单文件 --verify 拒绝场景，但 --verify 是 gxp 开发者不变式校验，非 LLM agent 工作流驱动） | 中（边界跨行处理：context N / trace 续行跨块易出 bug；run_verify 暴力路径 Vec<(usize, Vec<u8>)> 全量命中行 clone 内存模型不变） | 用户 `split -C 500M bigfile.log chunk_` + 逐块 `gxp 'pattern' chunk_* --verify`；或 `head -c 500M bigfile.log \| gxp 'pattern' /dev/stdin --verify` | **仍保持暂不评估**（v0.42 评估，2026-10-27）：§0.1 第 1 条场景驱动不通过（非 LLM agent 工作流）+ 第 4 条等价组合替代已存在（用户 split 即可）。升级条件：用户报告 split 语法负担重 + 频繁需要 >512MB 单文件 verify |
| 流式性能三件套（性能项，非功能）：#1 块式流式扫描（8MB 块 + 块内 `scan_lines_cb` + 跨块行 carry，替代 `read_until` 逐行）/ #2 聚合 stage 免收集（count/files/stats 命中即计数，跳过 MatchedLine 构造）/ #3 单字面量 AC→memmem 特化 | 中（流式仅 >512MB 单文件触发，场景 A/D 大文件变体；P2 实测本机流式 ~790 MiB/s 慢于 rg ~1.9 GiB/s 达 2.4×，perf-baseline §1.3.2；但 P2 诚实结论"gxp 价值不在裸吞吐"——排期应取决于 P3 agent A/B 实验是否要求大文件吞吐竞争力） | #1 低（语义不变，块边界/CRLF/末行语义已有测试锚定）；#2 中（B1 终止 stage 语义 + §6.4 组合矩阵需 e2e）；#3 低（L1 编译期特化，`L1 ⊇ L2` 不变） | 无等价组合（性能优化）；用户侧缓解 = 数据源头分卷（同 `--verify-large` 行的 split 思路） | **暂不评估**（2026-11-09 记录，P2 副产品）：预期收益为推算非承诺——#1 估 1137→850-950ms、#+2 估→650-800ms（900MB count 口径，反超 grep 但仍慢 rg ~1.4×，残余差距是专用扫描器护城河）。升级条件：P3 agent A/B 结论要求大文件场景竞争力，或用户报告 >512MB 单文件查询频繁且延迟不可接受 |
| DRY 重复率整改（重构项，非功能） | 无（代码质量项，不对应 LLM 工作流场景） | 中（热点是 §6.5.3 同构镜像的受控结构：dir/index.rs↔plan.rs 双遍历 12 处 / pipeline/count.rs 8 / mode/eval.rs captures 双镜像 8 / mode 三路径——B8/B9 同构遗漏史证明镜像一致性靠纪律维护，贸然合并反而放大不变式风险） | jscpd threshold 7 防恶化门禁（scripts/check-dry.sh + ci.yml 调用，本地可复现；jscpd 5.x 参数陷阱——--exit-code 是"发现即退"独立开关会覆盖 --threshold 判定，2026-11-09 修复时抽脚本固化） | **仍保持暂不评估**（2026-11-09 CI 首次真实运行）：基线 lines 6.09%（判定口径）/ 182 clones（src 5.34%/93 + tests 6.39%/76 + 跨目录 13，min-tokens 50）。tests 重复是对照断言/夹具模板属可接受；src 重复主体是同构镜像 + `#[cfg(test)]` 内联夹具。升级条件：CI 红线（lines 超 7%）或内联测试外迁（§6.6 e2e 约束）顺带消化 |

**re-triage 结论**：17 项中 2 项永久拒绝（T2/T3、SIMD JSON 强制启用，均因 D 系硬约束根本冲突），15 项仍保持暂不评估（含 v0.42 评估新增的 `--verify-large` + 2026-11-09 新增的流式性能三件套与 DRY 重复率整改，后两者为性能/质量项非功能项）。无升级为候选。判据核心是 §0.1 场景驱动 + D 系硬约束根本冲突检查。

---

## 12. 已删除节

> Python 原型 v0.1-v0.5 数据 + 决策记录已沉到 `docs/changelog.md`。当前实现见 §1-§11。

---

## 13. Cargo.toml 依赖说明

> **代码本体**：见 `Cargo.toml`（根目录，权威源）。本节只列依赖选型决策，不复述 `Cargo.toml` 全文（防止代码改了文档不跟的同步漂移）。

**依赖分组**（对应 `Cargo.toml [dependencies]` 段）：

| 层 | 依赖 | 用途 | 决策理由 |
|---|---|---|---|
| L1 引擎 | `regex` 1.10 + `regex-syntax` 0.8 + `aho-corasick` 1.0 | 正则 + literal extraction + SIMD 多模式字面量 | regex crate 自带 literal extraction；aho-corasick 1.0 SIMD 化 |
| L1 加速壳 | `grep-searcher` 0.1 + `grep-regex` 0.1 + `grep-matcher` 0.1（均 optional） | ripgrep 的搜索循环壳 | 不 fork、只依赖；MPL-2.0 商业可用 |
| L2 字段抽取 | `memchr` 2.7 + `serde`/`serde_json` 1.0 + `speedate` 0.16 | SIMD 字节扫描 + JSON 解析 + 时间戳 | memchr 默认 SIMD；simd-json 不启用（小 JSON 反慢） |
| L3 结构层 | `ast-grep-core` 0.45 + `ast-grep-language` 0.45（均 optional，feature `l3-structural`） | AST 结构查询 | ast-grep pattern 语法接近 LLM 习惯；feature 门控默认关闭 |
| 目录层 | `ignore` 0.4 + `globset` 0.4 + `blake3` 1.4 + `rayon` 1.10 | .gitignore + 文件名过滤 + 目录指纹 + 并行 | 手写栈式遍历比 WalkBuilder 快（无 6ms 固定开销） |
| CLI | `clap` 4.5 + `clap_complete` 4.5 + `nom` 7.1 + `thiserror` 1.0 | CLI 解析 + DSL 解析 + 错误派生 | 不接受 grep 短选项（D1）；nom 解析 DSL |

**dev-dependencies**：`proptest` 1.4（穷举测试）+ `criterion` 0.5（基准）+ `tempfile` 3.10（测试夹具）+ `memmap2` 0.9（仅 `benches/diag_mmap.rs` 对照基准，不进生产）。

**feature flags**：
- `default = []`：默认不启用任何 optional
- `ripgrep-engine`：启用 ripgrep 搜索壳（L1 加速）
- `simd-json`：启用 simd-json JSON 解析（不推荐，小 JSON 反慢）
- `l3-structural`：启用 L3 AST 结构查询（ast-grep 重依赖，默认关闭）

**bench 文件清单**（15 个）：详见 `docs/performance-baseline.md §5`（权威源）。`l3_scan` 需 `required-features = ["l3-structural"]`。

**release profile**：见 `Cargo.toml [profile.release]`。关键配置：`lto = "fat"`（跨模块内联，memchr SIMD 循环内联到调用点）+ `codegen-units = 1`（最大化内联）+ `panic = "abort"`（减少分支干扰 SIMD 流水线）+ `strip = true`。决策理由见 `docs/performance-baseline.md §4.3`。

### 13.1 L3 feature flag 设计

**feature `l3-structural`（已落地）**：
- `ast-grep-core` + `ast-grep-language` 声明为 `optional = true`（不进默认编译）
- 启用时：DSL `ast(NAME, STRING)` 走完整 L3 求值路径（L1 豁免 → L2 查 `ast_lines` 集合 → L3 `find_all` 精判）
- `lang_from_str` 九语言映射（py/js/ts/tsx/rs/go/java/c/cpp）+ 大小写不敏感 + 全拼别名 + c++/cxx/cc
- fail-loud：不支持语言报 `L3Error::UnsupportedLang`
- 关闭时：`PreparedPrim::Ast` 视为永真（L1 ⊇ L2 不变式成立）
- **不直接依赖 tree-sitter**：ast-grep-core v0.45 内部依赖 tree-sitter 0.26，直接声明会触发 `links` 冲突

**层 3（tree-sitter fallback，v2 设计）**：按语言单独 feature flag，默认不启用。已确认 `L3Query::TreeSitter` 分支不需要（ast-grep 覆盖 20+ 种语言足够）。

---

## 14. 模块结构

```
gxp/
├── Cargo.toml
├── src/
│   ├── main.rs                 # 入口，clap + 调度
│   ├── lib.rs                  # 库导出
│   ├── cli.rs                  # Cli struct（gxp 专属参数，无 grep 短选项）
│   ├── error.rs                # 错误类型统一（含四段式报错）
│   ├── plan.rs                 # ExecutionPlan (--explain)
│   ├── dsl/
│   │   ├── mod.rs
│   │   ├── ast.rs              # EBNF AST
│   │   ├── parser.rs           # nom 解析（含 ! 词法规则）
│   │   └── pipeline.rs         # 手写管道 tokenizer
│   ├── mode/
│   │   ├── mod.rs
│   │   ├── detect.rs           # 入口解析：扫描 argv 决定 BARE/gxp 模式
│   │   ├── reject_grep_opts.rs # 四段式报错实现
│   │   ├── bare.rs             # BARE 模式（ERE 兼容）
│   │   ├── gxp.rs              # gxp 模式
│   │   ├── eval.rs             # eval_prepared / eval_prepared_with_ctx
│   │   ├── prepared.rs         # PreparedScan / PreparedExpr / PreparedPrim
│   │   ├── strict.rs           # --strict 字段探活
│   │   └── verify.rs           # --verify 暴力对账
│   ├── l1/
│   │   ├── mod.rs
│   │   ├── compiler.rs         # 必选组编译 + 字段名前缀下推（方向 B）
│   │   └── scan.rs             # 行扫描入口
│   ├── l2/
│   │   ├── mod.rs
│   │   ├── extract.rs          # 字段抽取（手写状态机 + batch_find_values）
│   │   ├── numeric.rs          # 数值解析与比较（11 维契约）
│   │   ├── format.rs           # 格式探测
│   │   └── time.rs             # _ts between 时间谓词
│   ├── l3/
│   │   ├── mod.rs
│   │   └── structural.rs       # ast-grep + tree-sitter
│   ├── dir/
│   │   ├── mod.rs
│   │   ├── plan.rs             # PathPlan
│   │   ├── index.rs            # DirIndex
│   │   ├── tail.rs             # TailIndex
│   │   └── git.rs              # commit_key
│   ├── io/
│   │   ├── mod.rs
│   │   └── file.rs             # FileContent (Vec/Stream) + read_for_scan + read_stdin_for_scan
│   └── pipeline/
│       ├── mod.rs              # execute_stages + execute_stages_directory + execute_stages_streaming
│       ├── rank.rs              # partial-sort Top-K
│       ├── project.rs          # fields 投影
│       ├── head.rs             # head N
│       ├── count.rs            # count / group by
│       ├── context.rs          # context N / before:N / after:M
│       ├── files.rs            # files / files --invert
│       ├── stats.rs            # stats FIELD（min/max/avg）
│       └── percentiles.rs      # percentiles FIELD [p50,p95,p99]（nearest-rank）
├── tests/
│   ├── cli_ast.rs / cli_bare.rs / cli_completion.rs / cli_detect_unit.rs
│   ├── cli_directory.rs / cli_glob.rs / cli_ndjson.rs / cli_pipeline.rs
│   ├── cli_stdin.rs / cli_trailing_flags.rs
│   ├── common/mod.rs
│   ├── dir_git.rs / dir_index.rs / dir_off.rs / dir_pruning.rs / dir_regex.rs / dir_tail.rs
│   ├── dsl_parser.rs / l1_scan.rs / l2_extract.rs
│   ├── mode_bare_ere_compat.rs / numeric_eval_exhaustive.rs
│   ├── streaming.rs / v5_cpp_experiment.rs
├── benches/
│   ├── l1_scan.rs / l2_extract.rs / full_pipeline.rs / scale_dir.rs
│   ├── diag_pipeline.rs / diag_stages.rs / diag_breakdown.rs / diag_writeonly.rs
│   ├── diag_mmap.rs / diag_v7_streaming.rs / l3_scan.rs / verify_overhead.rs / diag_batch_projection.rs
└── fixtures/                    # .gitignored，基准数据不入库
    ├── 44mb.log / 44mb_json.log / 1gb.log / batch_projection.log / l2_candidates.log
```

---

## 15. 关键决策记录

| 决策 | 选择 | 替代方案 | 理由 |
|---|---|---|---|
| 不 fork ripgrep | 依赖 + 头写 | fork ripgrep | crates.io 依赖已够用；fork 意味着 5 万行 MPL 维护 |
| 不 fork GNU grep | Rust 头写 | GNU grep P1 改造（C） | Rust 生态（tree-sitter/memchr/regex）比 C 强 |
| regex + aho-corasick 而非 grep 子进程 | 进程内 | grep 子进程 | 进程内零开销；grep 子进程每次有 fork+exec 开销（实测见 performance-baseline §1.5） |
| 手写字段抽取状态机 而非 nom/winnow | 手写 | nom 7 / winnow 0.6 | 固定 grammar 上手写 + memchr 比 combinator 快且依赖少 |
| blake3 截断 16B 而非 blake2b | blake3 | blake2 | 5× 速度，单进程内抗碰撞足够 |
| PathPlan::walk 改手写栈替代 WalkBuilder | 手写 `std::fs::read_dir` 栈 | ignore::WalkBuilder | WalkBuilder 有固定开销导致小场景偏慢（实测见 performance-baseline §1.4） |
| ast-grep 而非裸 tree-sitter | ast-grep-language | tree-sitter Query | pattern 语法接近 LLM 预训练分布 |
| clap derive + 入口解析 | clap derive + 手写 argv 扫描 | 手写 argv / grep 兼容短选项 | gxp 不接受 grep 短选项；入口解析决定 BARE/gxp 模式 |
| T0/T1 only（D9） | 单进程内 + 单次查询内 | T2 sidecar / T3 daemon | grep 一次性命令模型；T2/T3 永久拒绝（除非 D9 改判，详见 §11.3.2）。**v0.42 实测边界（2026-10-27）**：v1 适用 ≤10GB 单目录（12GB/11603 文件 median 1.7-3.9s + --verify 100% PASS，详见 performance-baseline §1.3）；33GB 未实测不可声明 v1 适用，LLM agent <5s 延迟容忍已超 |
| D10：统一 read_to_end | 所有文件走 `read_to_end` | mmap 阈值 64KB / 全部 mmap | 实测：mmap page fault 分摊三阶段反而慢于 fs::read（详细数据见 performance-baseline §1.1 + §1.2） |
| 方向 B：L1 字段名前缀粗筛 | L1 只下推 `cost_ms=`，L2 精确数值比较 | digit-DP 编译成正则 | 正则表达力是 CFG 子集，无法覆盖任意数值比较 |
| 不是 grep 超集 | grep-aligned（BARE 模式 ERE 兼容） | grep 严格超集 | 声称超集会引发语义破裂点 |
| 不接受 grep 短选项 | 报错 + 四段式转换提示 | 接受并转换 | 等价功能用 gxp 原生管道动词 |
| `!` 词法规则方向 C | 前后边界判定 + pred 强制括号 | 优先级歧义 / ERE 字面 | `!` 前后必须有空白或非字母数字边界才是操作符 |
| 11 维数值字段格式契约 | 支持整数/浮点/负数/前导零/正号/科学计数法；拒绝千分位/单位/空白 trim/布尔/NaN | 任意格式 | 避免 `1,000` 与字段分隔歧义 |
| 不做 MCP server | gxp 核心只暴露 CLI + `--json`；MCP 集成由外围独立子项目 | gxp 自身成为 MCP server | D9：MCP server 长驻进程模型与 grep 一次性命令模型冲突 |
| L1 纯字面量组跳过 regex verify | `LiteralGroup::is_pure_literal` 标记 | 全部保留两遍 | aho-corasick 命中即等价 regex 命中 |
| 流式 fallback（512MB 阈值） | `FileContent::Stream(PathBuf)` + `STREAMING_THRESHOLD = 512MB` | 全部走流式 / 阈值 256MB / 阈值 1GB | V7 验证：1GB read_to_end 比流式快 5-16%，但 >512MB 走流式避免 OOM |
| JSON 输出字段补全 | `MatchedLine` 新增 `path` + `captures` + `match_bytes` | 只补 path | 7 个 `--json` 输出问题修复 |
| 多字段批量提取（算法层） | `batch_find_values` n≥3 走 batch | SIMD intrinsics / 全部走 find_value | bench 实测：n=5 +137%、n=8 +288%、n=12 +344% |
| MatchedLine.line: Arc<[u8]> | `line: Vec<u8>` → `Arc<[u8]>` | 借用 `&'a [u8]`（破坏 Send+Sync）/ 全面 Arc 化 | 唯一不破坏 V1 Send+Sync 的 L1 改动；大文件场景自动受益 |
| 字符串等值 `NAME="STRING"` 解析期脱糖为单元素 In（v0.48） | 脱糖（零新 AST 节点） | 新增 Pred::EqString 变体 + L1/L2 分支 | A/B 实验第一摩擦源；构造性等价消除 L1/L2 镜像风险（§6.5.3 B14 类）；`=` 单等号非数值 RHS 同时回归 ERE 字面量（对齐设计文档分歧操作符列表） |
| L2 Literal 重校验豁免 + collect 零分配化（v0.49） | `verified_literals`（单元素纯字面量必选组成员）+ `l1_verified` 标志 + match_bytes/path Arc 化 | 树路径平行判定（Paren(And) 陷阱不健全）/ 惰性 match_bytes（需 compiled 穿透 stage 层） | T1a 归因 ~16ms 三项冗余实测；豁免健全性构造性论证见 §7.9；--verify 共用 build 同错对齐 → 独立期望集 e2e 强制 |
| logfmt 带引号值提取（v0.50） | 值以 `"` 包裹时值为引号内字节、终点为闭合引号；`extract_logfmt_kv` + `find_value` 镜像同步；未闭合回退空白切分 | eval 层比较时剥引号（投影值仍带引号，`fields`/`distinct`/`stats` 自相矛盾） | 提取层单一事实源，与 JSON 通路对齐；string equality 跨格式一致；数值谓词语义外溢显式声明（changelog v0.50）；L1 前缀下推在字段名侧不受影响（`L1 ⊇ L2` 保持） |
| `--query-file PATH` 免引号通道（v0.50） | 查询文件读取后前置为 `args[0]`，分发层透明 | 环境变量 `GXP_QUERY`（两步调用/会话状态泄漏/不可组合）；DSL 引入单引号字符串（触 D3 硬约束，与 ast pattern 内 `'a'` 字面歧义） | 引号丢失后查询与合法字面量逐字节相同、产品侧不可检测；查询文件即 agent Write 工具产物（场景 §0.2-I）；备选对比与对抗评审记录见 changelog v0.50 |

---

**报告结束**
