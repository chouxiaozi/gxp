# gxp 功能能力矩阵

> **用途**：按功能维度（非时间线）纵览当前能力边界。agent 工作前读此文件快速定位"已实现 / 未实现 / 不做"。
> **更新规则**：每次功能落地或变更后更新对应行；更新时修改底部"最后更新"时间。
> **与路线图的关系**：本文档是横向能力快照；`docs/changelog.md` 是纵向版本演进。两者互补。

**最后更新**: 见 `docs/changelog.md` 最新版本（v0.49 — 发版收尾·skill 分发 plugin 化 + T1a 性能优化，详见 changelog v0.49 行）
**测试基线**: 见 §4 代码规模表

---

## 1. 五层流水线 + 目录层 + CLI

| 层 | 能力 | 状态 | 实现位置 | 测试 |
|---|---|---|---|---|
| **L0 DSL** | EBNF 子集解析：`&&`/`||`/`!`/括号/谓词/管道动词/`ast(NAME, STRING)` | ✅ | `src/dsl/{ast,parser,pipeline}.rs` | `tests/dsl_parser.rs` |
| **L1 粗筛** | aho-corasick 多模式字面量 + regex 精判 + 字段名前缀存在性粗筛（方向 B），对所有文本文件通用 | ✅ | `src/l1/{compiler,scan}.rs` | `tests/l1_scan.rs` + lib 内联 |
| | Not 子树豁免（不变式 L1 ⊇ L2） | ✅ | `compiler.rs::collect_field_predicates` | lib 内联 |
| | ast() 豁免（L1 不下推，L3 精判） | ✅ | `compiler.rs::collect_prim_literals` | cli_e2e ast 测试 |
| | 伪字段豁免（`_ts`/`_tag` 不参与前缀粗筛） | ✅ | `compiler.rs::is_pseudo_field` | lib 内联 |
| | 纯字面量组跳过 regex verify（aho-corasick 命中即等价 regex 命中） | ✅ | `compiler.rs::LiteralGroup::is_pure_literal` | lib 内联 |
| | Or 组完整性（B14）：任一分支不推进字面量（And/Not/Regex/Pred/Ast/Capture）→ 整组放弃，L2 全树精判；scan 捷径以 `l1_exact` 为门槛 | ✅ v0.47 | `compiler.rs::collect_or_literals` + `detect.rs::l1_exact` | compiler 4 + detect 4 单元 + cli_pipeline 7 e2e |
| | Or 子树前缀豁免（B15）：Or 分支内字段谓词不参与前缀粗筛（AND 语义的前缀会误杀另一分支候选） | ✅ v0.48 | `compiler.rs::collect_field_predicates_inner` | compiler 3 单元 + cli_string_eq 4 e2e |
| **L2 精判** | 4 格式：logfmt k=v/k:v / JSON line / [tag] / ISO 8601 | ✅ | `src/l2/{extract,format}.rs` | `tests/l2_extract.rs` |
| | JSON line 仅顶层字段过滤；嵌套路径 / 多行合并 / Array 展开委托 `jq -c` + `gron` 组合（B10/B11 后边界） | ✅ | `src/l2/extract.rs::find_value`（含 JSON 兜底） + `src/l2/format.rs::detect_format`（B11 trim 空白） | `tests/l2_extract.rs` |
| | 11 维数值契约（D5） | ✅ | `src/l2/numeric.rs` | `tests/numeric_eval_exhaustive.rs` (0..60000 × 6 区间) |
| | `Compare`（6 op: > < >= <= == !=） | ✅ | `src/mode/detect.rs::eval_prepared` | cli_e2e |
| | `In`（集合成员判定） | ✅ | 同上 | cli_e2e |
| | 字符串等值 `NAME="STRING"`（`=`/`==` 带引号 RHS 解析期脱糖单元素 In；`!=`/`>` 带引号 fail-loud + hint；单 `=` 非数值 RHS 回退 ERE 字面量） | ✅ v0.48 | `dsl/parser.rs::pred_compare`（脱糖）+ `mode/gxp.rs::enrich_string_eq_hint`（hint）+ `detect.rs::has_field_predicate`（前瞻） | cli_string_eq 20 e2e |
| | Literal 重校验豁免（v0.49 Opt A）：单元素纯字面量必选组成员（`verified_literals`）L2 免 `regex.is_match` 重跑；Or 组（≥2 元素）/Paren(And)（组放弃）/Not 侧不豁免 | ✅ v0.49 | `compiler.rs::verified_literals` + `prepared.rs::l1_verified` + `eval.rs` 双镜像豁免分支 | compiler 6 单元 + cli_l1_verified 6 e2e（独立期望集） |
| | `Between`（闭区间） | ✅ | 同上 | cli_e2e |
| | `Exists`（字段存在性） | ✅ | 同上 | cli_e2e |
| | `Not`（三态取反：True→False / False→True / NotNumeric→True） | ✅ | 同上 | cli_e2e |
| | `find_value` 懒单字段定位（跳过全格式 extract） | ✅ | `extract.rs::find_value` | l2_extract.rs |
| | logfmt 带引号值提取（`k="v"`：值为引号内字节、终点为闭合引号，与 JSON 通路对齐；未闭合回退空白切分） | ✅ v0.50 | `extract.rs::extract_logfmt_kv` + `find_value`（镜像同步） | tests/l2_extract.rs + `tests/cli_quoted_logfmt.rs` e2e |
| | `batch_find_values` 多字段批量提取（n≥3 走 batch） | ✅ | `extract.rs::batch_find_values` | l2_extract.rs |
| | `ast_lines` 集合查询（L3 预计算结果） | ✅ | `detect.rs::EvalCtx` | cli_e2e (l3-structural) |
| | `capture(NAME, /REGEX/)` 正则捕获组提取 | ✅ | `src/mode/eval.rs::eval_prepared_with_captures` | cli_e2e |
| | `_ts between "ISO" "and" "ISO"` 时间戳区间判定 | ✅ | `src/l2/time.rs` + `src/mode/eval.rs` | cli_e2e |
| **L3 结构层** | `ast(ts, ...)` TypeScript 通路 | ✅ | `src/l3/structural.rs` | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(rs, ...)` Rust 通路 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(py, ...)` Python 通路 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(js, ...)` JavaScript 通路 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(go, ...)` Go 通路 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | 同语言多 pattern 复用 AstGrep root | ✅ | `structural.rs::run_all_ast` | lib 内联 |
| | `ast(tsx, ...)` JSX 支持 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(java, ...)` Java 通路 | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(c, ...)` C 通路（V5 验证：不做预处理） | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | `ast(cpp, ...)` C++ 通路（V5 验证：不做预处理） | ✅ | 同上 | structural 3 内联 + cli_e2e 3 e2e |
| | 大小写不敏感 + 全拼别名（`Java`/`java`/`JAVA`；`c++`/`cpp`/`cxx`/`cc`） | ✅ | `structural.rs::lang_from_str` | cli_e2e |
| | 按扩展名过滤（`ast(ts)` 只扫 `.ts`/`.mts`/`.tsx`/`.vue`；`ast(tsx)` 扫 `.tsx`/`.jsx`；`ast(js)` 扫 `.js`/`.mjs`/`.cjs`/`.jsx`/`.vue`；不解析 `.json`/`.md`/`.css` 等） | ✅ v0.45 | `structural.rs::lang_matches_ext` + `detect.rs::prepare_l3` | cli_e2e + perf-baseline §1.3 |
| | 解析失败 fail-loud（`Pattern::try_new` → `L3Error::PatternParse`） | ✅ | `src/l3/structural.rs::run_single` | cli_e2e l3-structural |
| | `--glob PATTERN` 文件名过滤 | ✅ | `src/dir/plan.rs::PathPlan::walk` + `compile_globset` | cli_e2e 6 项 glob 测试 |
| | .gitignore 尊重（walk 层过滤） | ✅ | `src/dir/plan.rs::GitignoreChecker` | cli_e2e 3 项 gitignore 测试 |
| | tree-sitter fallback（Swift/Haskell/Erlang 等） | v2 | — | — |
| | CSS/SCSS/Less L3 | 不做 | — | — |
| **L4 管道** | `where`（二次过滤） | ✅ | `src/pipeline/mod.rs::apply_where` | cli_e2e |
| | `context N`（等价 grep -A/-B/-C） | ✅ | `src/pipeline/context.rs` | cli_e2e |
| | `count` / `count by FIELD`（等价 grep -c） | ✅ | `src/pipeline/count.rs` | cli_e2e |
| | `group by FIELD`（按字段值分组计数，输出 `value: N`；独立动词） | ✅ | `src/pipeline/count.rs::apply_count_group_by` | lib 内联 + cli_e2e |
| | `rank top K by FIELD [desc\|asc]`（partial-sort） | ✅ | `src/pipeline/rank.rs` | cli_e2e |
| | `sort [by FIELD] [desc\|asc]`（全量稳定排序） | ✅ | `src/pipeline/sort.rs` | cli_e2e 19 项 |
| | `sample N` / `sample P%`（固定种子 0 抽样） | ✅ | `src/pipeline/sample.rs` | cli_e2e |
| | `head N` | ✅ | `src/pipeline/head.rs` | cli_e2e |
| | `fields F1,F2,...`（投影） | ✅ | `src/pipeline/project.rs` | cli_e2e |
| | `files` / `files --invert`（等价 grep -l/-L） | ✅ | `src/pipeline/files.rs` | cli_e2e |
| | `stats FIELD`（流式聚合 min/max/avg） | ✅ | `src/pipeline/stats.rs` | cli_e2e |
| | `distinct [by FIELD]`（稳定去重，保留首次出现） | ✅ v0.35 | `src/pipeline/distinct.rs` | cli_e2e |
| | `percentiles FIELD [p50,p95,p99]`（非流式 nearest-rank） | ✅ v0.37 | `src/pipeline/percentiles.rs` | cli_e2e |
| | `trace [LANG]`（多行 stack trace；6 语言 + 自动检测；rust 新旧 panic 格式 + 帧间 at 行 v0.46） | ✅ v0.39 | `src/pipeline/trace.rs` | cli_trace 19 项 |
| | 管道链式组合（如 `context 3 | fields _ts,cost_ms`） | ✅ | `pipeline/mod.rs::execute_stages` | cli_e2e |
| | 目录多文件聚合（全局 rank 归并 + context 不跨文件） | ✅ | `pipeline/mod.rs::execute_stages_directory` | cli_e2e |
| **目录层** | `PathPlan::walk`（手写栈式 readdir） | ✅ | `src/dir/plan.rs` | `tests/dir_pruning.rs` |
| | `DirIndex`（strict/lax/off 三档 + Arc 复用） | ✅ | `src/dir/index.rs` | `tests/dir_index.rs` |
| | `TailIndex`（增量追加检测） | ✅ | `src/dir/tail.rs` | `tests/dir_tail.rs` |
| | `commit_key`（git commit 指纹） | ✅ | `src/dir/git.rs` | `tests/dir_git.rs` |
| | 正则目录过滤（`dir_re`） | ✅ | — | `tests/dir_regex.rs` |
| | freshness off 模式 | ✅ | — | `tests/dir_off.rs` |
| | `gxp 'pattern' dir/` CLI 接入 | ✅ | `mode/detect.rs::run_*_directory` | cli_e2e 10 项目录测试 |
| | stdin 与 file/dir 互斥 | ✅ | `mode/detect.rs` | cli_e2e |
| | rayon 并行跨文件扫描 | ✅ | `mode/{bare,gxp}.rs::par_iter` | cli_e2e |
| **CLI** | BARE 模式（ERE 兼容 `≡ grep -E`） | ✅ | `mode/detect.rs::run_bare_mode` | `tests/mode_bare_ere_compat.rs` |
| | gxp 模式（DSL + 管道 + 字段谓词） | ✅ | `mode/detect.rs::run_gxp_mode` | cli_e2e |
| | `--explain`（执行计划：文本 + JSON） | ✅ | `src/plan.rs::build_plan` | cli_e2e |
| | `--verify`（暴力对账 + 与 `--explain` 联动） | ✅ | `mode/detect.rs::run_verify` | cli_e2e |
| | `--strict`（字段探活 fail-loud） | ✅ | `mode/detect.rs::strict_field_probe` | cli_e2e 5 项 `--strict` 测试 |
| | `--json` flag（NDJSON 输出 + 与 `--explain` 联动） | ✅ | `src/cli.rs` | cli_e2e |
| | `--glob PATTERN`（文件名过滤） | ✅ | `src/cli.rs` + `src/dir/plan.rs` | cli_e2e |
| | `--no-gitignore`（覆盖默认 .gitignore 尊重） | ✅ | `src/cli.rs` | cli_e2e |
| | `--generate-completion SHELL`（shell completion） | ✅ | `src/cli.rs` + `clap_complete` | cli_e2e |
| | `--query-file PATH`（查询从文件读取，免 shell 引号层；空文件/多行/与位置 pattern 互斥 fail-loud） | ✅ v0.50 | `src/cli.rs` + `mode/detect.rs` | `tests/cli_query_file.rs` e2e（三路径） |
| | 四段式报错（11 个 grep 短选项全覆盖） | ✅ | `mode/reject_grep_opts.rs` + `error.rs` | `tests/cli_bare.rs`（10 短选项 e2e）+ `src/mode/reject_grep_opts.rs` 内联单测 |
| | stdin 管道（`some_cmd | gxp 'pattern'` / `gxp 'pattern' -`） | ✅ | `io/file.rs::read_stdin_for_scan` + `mode/detect.rs::dispatch_stdin` | cli_e2e 15 项 stdin 测试 |
| | 流式扫描（文件 > 512MB 走 BufReader） | ✅ | `src/l1/scan.rs::scan_stream_lines_cb` | cli_e2e streaming 测试 |

---

## 2. 不变式与对账

| 不变式 | 状态 | 验证方式 |
|---|---|---|
| `L1(粗筛) ⊇ L2(精判)` | ✅ 100% | lib 内联双集成测试 + `--verify` 暴力对账 |
| `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（BARE 模式） | ✅ | `tests/mode_bare_ere_compat.rs` |
| 31 项目录层行为全绿 | ✅ | `tests/dir_*.rs` |
| `--verify` 与精确结果 100% 一致 | ✅ | `run_verify` 行集合 diff |
| `--verify` 暴力路径 = 优化路径语义链对齐 | ✅ | 暴力路径走 L1 粗筛 → L2 精判 → L3 集合（详见 Rust 方案 §8.5.1） |
| 交叉验证真实代码库 0 FAIL | ✅ | web-app 1020 .ts + 35 Rust 源文件 |
| 12GB 大数据 --verify 0 FAIL（v0.42，2026-10-27） | ✅ | 12GB/11603 文件 × 6 查询 × 3 轮 = 100% PASS，median 1.7-3.9s（详见 perf-baseline §1.3） |
| 65MB server.log 多样化数据集 --verify 0 FAIL | ✅ | 6 查询（含 cost_ms>X / capture / _ts between / percentiles）100% PASS |
| L3 ast() 单语句 pattern 0 FAIL | ✅ | gxp src/ 48 Rust 源文件 × 6 单语句 pattern |
| L3 ast() 多语句 pattern 触发 ast-grep 上游限制 | ⚠️ 已记录 | `#[test] fn $NAME()` 触发 "Multiple AST nodes detected"，非 gxp bug；gxp 错误信息清晰暴露 |
| L3 ast() 大目录验证 0 FAIL（v0.44，2026-11-01） | ✅ | web-app 3378 文件（2329 .ts + 46 .js）× 3 ast pattern × --verify = 0 FAIL，~1.1-1.4s（详见 perf-baseline §1.3） |
| L3 ast() --no-gitignore 大目录验证 0 FAIL（v0.45，2026-11-01） | ✅ | web-app 72K 文件 × 3 ast pattern × --verify = 0 FAIL，扩展名过滤后 40-53% 提速（详见 perf-baseline §1.3） |
| 911MB 合成 JSONL 双法 GT 对账（v0.47，2026-11-09） | ✅ | 12 项基准答案 gxp × grep/构造模型全部精确闭合（含边界含端点 + 非法时间戳拒绝语义，详见 agent-ab-experiment.md §3） |
| agent A/B 实验 16/16 全对（v0.47，2026-11-09） | ✅ | 8 场景 × 2 臂独立 agent，答案对照冻结 GT 全对；结论"混合→按场景收敛"，详见 `docs/agent-ab-experiment.md` |

---

## 3. 明确不做的事（v1 / v1.0 不做）

> 权威表详见 `docs/rust-implementation-plan.md §11.3.2`（暂不评估清单 + re-triage）。
> 本节不重复清单，仅保留与能力矩阵强相关的"v1 不做"摘要：T2 sidecar / T3 daemon（D9 永久拒绝）、grep 短选项（D1）、simd-json 强制启用（D10 永久拒绝）、自定义 SIMD intrinsics（memchr 已自动 dispatch）、跨压缩文件（D10 暂不评估）。

---

## 4. 代码规模

| 维度 | 数量 |
|---|---|
| Rust 源文件 | 48 个 |
| 源码总行数 | ~17,500 行 |
| 集成测试文件 | 29 个 |
| 基准 bench 文件 | 15 个 |
| 默认 feature 测试 | 1197 全绿（v0.50.0 实测 2026-11-11，+63 vs v0.49.0 的 1134：--query-file 25[11 单元+14 e2e 三路径] + logfmt 带引号值 24[9 单元+15 e2e 谓词全家] + 引号提示四挂点 15[7 单元+8 e2e]；governance §4.1.8 自动对账；l3-structural 自 crates.io 首发起默认开启——与预编译二进制全功能开箱即用一致，见 Cargo.toml [features] 注释） |
| l3-structural feature 测试 | 1197 全绿（与默认集相同，l3 已默认开启；裁剪模式 --no-default-features 上次实测 1030，v0.49 2026-11-09，未随 v0.50 +63 复测） |
| 生产依赖 crate | 14 个（+ 4 optional/dev） |
