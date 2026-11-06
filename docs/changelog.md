# gxp 版本历史（changelog）

> 本文件记录 gxp 版本演进。详细设计/实现细节见设计文档与 Rust 方案对应章节。
> 精简后的初始版本摘要见末尾"版本演进总结"。

## 版本演进总结

gxp 从 2026-10-20 启动到 v0.37（2026-10-26），4 天密集开发 + 2 天文档对抗评审。
核心演进路径：

1. **v0.1-v0.5 基础设施**（2026-10-20）：Python 原型 → Rust 脚手架 → DSL 解析器（nom 7.1 + AST + 管道 tokenizer + 42 集成测试）+ 门禁体系（fmt/clippy/test/missing_docs/rustfmt/hook）。
2. **v0.6-v0.12 核心查询层**（2026-10-20 → 10-21）：L1 编译器（aho-corasick + regex 双层粗筛，方向 B 字段名前缀下推）+ L2 字段抽取（11 维数值契约 + 四谓词 Not/In/Between/Exists）+ CLI 端到端串联（BARE ≡ grep -E）+ pipeline 管道动词执行器 + `--explain`/`--verify`/`--strict`/`--json`。
3. **v0.13-v0.16 性能 + 目录接入**（2026-10-21）：全管道达 <80ms 目标（实测见 performance-baseline §1.1）+ D10 改判（统一 read_to_end，mmap 废止）+ 目录参数 CLI 接入。
4. **v0.17-v0.22 L3 结构层**（2026-10-21 → 10-22）：ast-grep PoC（ts+rs）→ 五语言穷举（py/js/ts/rs/go）→ TSX 六语言 → Java/C/C++ 九语言 + V5 验证（ast-grep 不做预处理）+ --verify 覆盖矩阵（场景 A-G 全部落地）。
5. **v0.23-v0.28 v1.0 准备**（2026-10-22 → 10-23）：DRY 重构 + V1 验证（PreparedScan Send+Sync）+ rayon 并行跨文件扫描（2.8-3.9×）+ V2/V3/V4 三特性（capture 正则提取 / `_ts between` 时间谓词 / `stats` 流式聚合）+ MSRV 1.75 + `--json` NDJSON + shell completion + L1 纯字面量组跳过 regex verify。
6. **v0.29-v0.32 生产加固**（2026-10-23 → 10-24）：流式 fallback（>512MB BufReader）+ JSON 输出字段补全 + 多字段批量提取（batch_find_values，n≥3）+ MatchedLine.line Arc<[u8]> + AGENTS.md 解除 v1/v2 门控。
7. **v0.33 代表性抽样**（2026-10-24）：`sample N`/`sample P%` 管道动词（§9.1 候选 #1 落地）+ Fisher-Yates 部分洗牌 + 固定种子 0（--verify 天然对齐）+ 37 新测试（10 单元 + 9 DSL + 9 独立 e2e + 9 组合 e2e）+ criterion 基准（`sample 20/1000/10000` + `sample 10%/50%/90%` + head 对照）。基准结论：洗牌开销低于 L1+L2 扫描噪声水平（详细数据见 performance-baseline §1.3）。
8. **v0.34 全量排序**（2026-10-24）：`sort [by FIELD] [desc|asc]` 管道动词（§9.1 候选 #5 落地）+ 默认 asc（对齐 SQL ORDER BY / Unix sort / Python sorted）+ 省略 by 按整行字节序（聚集相同错误）+ 28 新测试（13 单元 + 10 DSL + 19 e2e 其中 11 组合矩阵）+ 确定性算法 --verify 天然对齐。
9. **v0.35 稳定去重**（2026-10-24）：`distinct [by FIELD]` 管道动词（§9.1 候选 #3 落地）+ 无 by 整行去重（对齐 Unix `sort | uniq`，但不排序——保持原序）+ `by FIELD` 按字段值去重（对齐 SQL `SELECT DISTINCT ON (field) *`，字段缺失归入空串键）+ 34 新测试（11 单元 + 7 DSL + 16 e2e 其中 7 组合矩阵 + 1 stdin 三路径覆盖）+ 确定性算法 --verify 天然对齐 + 自举 46 文件源码 3 种形态全 PASS。与 `group by` 区分：group by 输出 `value: N`（计数），distinct 输出去重后的行（保留原始字段/整行内容）——两者语义不重叠。同时补 `sort` criterion 基准（4 group：sort by FIELD / sort 整行 vs by FIELD / rank partial vs sort 全量 / sort|head vs rank topK）——基准结论：300K+ 行命中集上 `sort by FIELD` 全量排序显著超 80ms 全管道预算，Top-K 场景应选 rank 不选 `sort | head`（详细数据见 performance-baseline §1.3）。
10. **v0.36 LLM 友好原则 + bug 修复**（2026-10-24）：§0.1 LLM 友好原则写入 AGENTS.md（场景驱动 + 输出自包含 + 不增理解负担 + 文档先于代码）+ B1 修复（终止 stage 后非空 stage fail-loud 报错，`check_terminal_stages` 前置检查，三路径统一）+ B2 修复（`group by` captures 优先 + 行内 fallback，对齐 `fields` 投影）+ B3 修复（9 处文档 "count group by FIELD" → "group by FIELD" 独立动词）+ §9.1 候选 #7 落地（`--json` 单文件 path 字段断言补全，代码已实现仅缺测试）。+10 单元/e2e 测试（B1: 5 单元 + 5 e2e / B2: 4 单元 / #7: 3 ndjson 断言）。
11. **v0.37 非流式百分位**（2026-10-24）：`percentiles FIELD [p50,p95,p99]` 管道动词（§9.1 候选 #6 落地）+ nearest-rank 算法（idx = ceil(p/100 * len) - 1）+ 默认 p50/p95/p99（SLO 报告核心指标）+ 可选自定义列表（逗号分隔，`p` 前缀，如 `p90,p99,p999`）+ 终止 stage（与 stats/count/group by/files 一致）+ 9 单元测试 + 9 e2e 测试（7 正向 + 2 B1 终止 stage 报错）+ 确定性算法 --verify 天然对齐。与 `stats` 区分：stats 做流式 min/max/avg（O(1) 内存，逐行更新），percentiles 做非流式全量百分位（O(N) 内存缓冲 + 排序，与 sort 一致）。avg 掩盖长尾分布——SLO 报告需 p95/p99 衡量尾部延迟。`is_terminal`/`reorder_stages`/`stage_name` 三处扩展。

### 能力概览快照（v0.37，历史保留 — 当前能力见 `docs/capability-matrix.md` §1）

| 层 | 能力 | 测试基线 |
|---|---|---|
| BARE 模式 | ERE 兼容（`gxp 'pattern' file` ≡ `grep -E`） | 30 项 ERE 对账 |
| gxp DSL | 布尔 + 字段作用域 + Top-K + 管道（where/context/count/group/rank/sort/distinct/head/sample/fields/files/stats/percentiles） | 758 默认 + 853 l3 全绿 |
| L1 | aho-corasick + regex 双层；字段名前缀下推；纯字面量组跳过 regex verify | 穷举 L1 ⊇ L2 |
| L2 | 11 维数值契约 + Not/In/Between/Exists + capture 正则提取 + `_ts between` 时间谓词 | 1.44M 次穷举 0 错 |
| L3 | 九语言 AST 结构查询（py/js/ts/tsx/rs/go/java/c/cpp），feature `l3-structural` 条件编译 | 9 内联单测 + 9 e2e |
| 目录 | rayon 并行跨文件扫描 + `--glob` + `.gitignore` 尊重 + DirIndex 三档（strict/lax/off）+ TailIndex 增量 | 31 项目录行为 |
| 流式 | >512MB BufReader 逐行扫描（避免 OOM） | 15 项 stdin + streaming |
| CLI | `--explain`/`--verify`/`--strict`/`--json`/`--glob`/`--no-gitignore`/`--generate-completion` | 完整覆盖 |
| 性能 | 全管道 <80ms 目标达标；vs GNU grep ~2.8× 快（详细数据见 performance-baseline §1.1） | criterion + diag bench |
| CI | Windows + Linux + macOS 三平台 matrix + cargo-deny + jscpd + llvm-cov + cargo-mutants | 226 行 ci.yml |

### 关键决策里程碑

| 版本 | 决策 | 意义 |
|---|---|---|
| v0.1 | D1-D11 硬约束确立 | 不是 grep 超集；方案 E 单一 gxp 模式；方向 B L1 字段名前缀；11 维数值契约 |
| v0.10 | Python 原型删除 | Rust 全面承担跨平台职责 |
| v0.15 | D10 改判 | 统一 read_to_end，mmap 废止（实测见 performance-baseline §1.1） |
| v0.22 | --verify 覆盖矩阵场景 A-G 全部落地 | 交叉验证方法论成型 |
| v0.24 | V1 通过 + rayon 并行 | PreparedScan Send+Sync 验证 + 2.8-3.9× 提速 |
| v0.28 | v1.0 准备完成 | MSRV + NDJSON + shell completion + L1 优化 |
| v0.32 | Arc<[u8]> 基础设施铺设 | 大文件场景自动受益 |
| v0.33 | `sample N`/`sample P%` 代表性抽样 | §9.1 候选 #1 落地；固定种子 0 → --verify 天然对齐；Fisher-Yates 部分洗牌 O(take) |
| v0.34 | `sort [by FIELD] [desc\|asc]` 全量排序 | §9.1 候选 #5 落地；默认 asc（对齐 SQL/Unix sort）；省略 by 按整行字节序（聚集相同错误） |
| v0.35 | `distinct [by FIELD]` 稳定去重 | §9.1 候选 #3 落地；保留首次出现（对齐 SQL DISTINCT）；与 group by 语义区分（计数 vs 行列表） |
| v0.36 | LLM 友好原则 + B1/B2/B3 修复 + #7 落地 | §0.1 原则写入 AGENTS.md；终止 stage fail-loud；group by captures 对齐；#7 path 断言补全 |
| v0.37 | `percentiles FIELD [p50,p95,p99]` 非流式百分位 | §9.1 候选 #6 落地；nearest-rank；终止 stage；SLO 长尾指标；与 stats 流式 min/max/avg 互补 |
| v0.38 | 文档治理规则落地 | 4 agent 对抗评审发现 6 阻断 + ~20 中严重过程性噪声（Python 原型残留/digit-DP 历史描述/PoC 措辞/选型实验过程感/清单漂移/性能数据复述）→ R1-R5 规则 + 写文档 checklist + 版本发布体检；新增 `docs/document-governance.md` |
| v0.39 | `trace [LANG]` 多行 stack trace 捕获 | §9.1 候选 #2 落地（L0 对抗评审 + 方案 B'）；6 语言（python/java/js/rust/go/csharp）+ 自动检测 + `MatchedLine.extra_lines` 扩展 + NDJSON `extra_lines` 数组字段 + 文本 `  → ` 前缀视觉区分 + 终止 stage（fail-loud 后置动词）+ 流式路径不支持（提示用 `context N` 替代）+ 目录场景每文件独立 trace（跨文件不连续，参照 context 规则）+ `--explain` 渲染 + 25 单元测试 + 17 e2e 测试 |
| v0.40 | §8.2 多行 JSON 判决理由修正 | L3 文档改动：原"破坏 grep 行级扫描模型，与 D9 冲突"是误判（D9 是拒绝守护进程，与行级扫描无关）。改为"需引入 preprocessor 概念（与 Vue SFC 同类）"；并明确该条只覆盖场景 B（某低代码平台 内嵌源码字符串），不覆盖候选 #4 场景 A（pretty-printed JSON 日志合并，与 trace 同构，pipeline 阶段问题）。候选 #4 仍标"待评审"，场景 A 升级路径独立 |
| v0.41 | 候选 #4 多行 JSON re-triage → 转暂不评估 | L0 对抗评审：起源是 2026-10-22 某低代码平台 评估副产品，场景 A 是事后合理化，不是真实 LLM 工作流驱动需求（违反 §0.1 第 1 条场景驱动）；K8s/Docker 生产日志默认单行 JSONL 不是多行，配置文件 LLM 直接 Read，真实场景稀疏；替代路径 `jq -c '.' multiline.log \| gxp 'cost_ms>500'`（jq 预处理转单行 JSONL，gxp 仍是 grep-aligned 不跨界）；候选清单从 7 项缩为 6 项，暂不评估从 11 项扩为 12 项；§8.1 JSON 行注明仅单行 JSONL；§8.2 line 586 补充多行 JSON 合并不做的判决 |
| v0.42 | 文档同步 + 性能复测 + 归因澄清 | capability-matrix §4 代码规模同步（src 43→48 文件 / ~10K→~15.6K 行 / tests 24→25 个）+ 头部版本 v0.39→v0.41；performance-baseline §1.1/1.2 增加 v0.41 复测列 + §1.4 归因分析（v0.32→v0.41 期间热路径零 commit，性能改善是 page cache 状态差异非代码优化；同会话 warm cache 实测 74.64ms 进一步降 7%；trace v0.39 落地后无退化）；测试 802/897 全绿（v0.41 实测 2026-10-26） |
| v0.43 | apply_where captures 优先 + percentiles 伪字段对齐 + where 多 token 谓词解析修复 + 防回归机制 | 三处 bug 修复（自动发现自动修复第 10-12 轮）：B1 apply_where 漏 captures 优先（P0 不变式违反：`capture(cost, /req-(\d+)/) \| where cost>50000` 返回 0 行——L1 前缀粗筛误淘汰 captures 提供的字段值；修复：`collect_pred_fields` 助手递归收集谓词字段名，若 captures 已含任一字段跳过 L1 粗筛直接走 L2；commit 2932aa5）；B2 percentiles 未走 `pseudo_field_value`（与 stats/sort/rank/count/distinct/project 不一致，`_match`/`_line`/`_file` 伪字段无法识别；修复：改用 `crate::pipeline::pseudo_field_value(m, field)`；commit 643b060）；B3 where 多 token 谓词解析断裂（P0 parser bug：`where cost between 100 and 500` / `where cost in [100, 500]` / `where cost exists` 全部报"未知管道动词 between/in/exists"——`where` 分支只取单个 token 作 EXPR，剩余 token 被当作下一 stage 动词；修复：循环收集所有 token 直到遇到 `is_verb` 判定的下一个 stage 动词或 token 列表结束，用空格重组为完整 EXPR 字符串再调 `parse_query_inline`）；**防回归机制**（根因诊断：5 bug 全在跨数据来源契约，单元层用 `MatchedLine::new` 手工构造绕过真实 pipeline 流，组合覆盖只 5%）：+12 e2e 补 §6.4 矩阵缺口（capture × rank/sort/distinct/stats/percentiles 5 项 + 伪字段 × sort/rank/distinct/stats/percentiles/file 7 项；commit 80b339c）；AGENTS.md §6.4 加第 3 条"数据来源 × stage 矩阵"+ §6.5.2 加自举组合覆盖检查（commit 617d5da）；governance §4.1.8 加测试基线数字自动对账（commit c5316d3）；+10 e2e 测试（capture+where 2 项 + where between/in/exists 多 token 5 项含 capture 前置和链式 stage 终止验证 + apply_where 单元 5 项 + percentiles 伪字段 3 项）；测试基线 891 默认/984 l3 全绿（v0.43 实测 2026-10-28），自举 `gxp '"is_verb" \|\| "parse_one_stage"' src/ --verify` 0 FAIL |
| v0.44 | execute_stages 单文件模式补填 m.path + emit_remaining 加 show_path_prefix 参数 + B7 流式路径同构修复 + B8 capture 同字段谓词 L1 粗筛豁免 + 暴力路径 capture 提取对齐 + B9 Not 分支漏传 captures 修复 + dead_code 清理 + B10 find_value 漏 JSON line 通路修复 + L1 prefix_variants 补 "name": 变体 + JSON 场景能力边界评估 + jq 组合分工落地 + B11 detect_format JSON 探测 trim 首尾空白 | B6/B7/B8/B9/B10/B11 bug 修复（自动发现自动修复第 13-18 轮，§6.5.2 自举组合覆盖检查 + B9 同构遗漏深探 + B10 jq 组合验证发现 + B11 对抗评审发现）：**B6**（第 13 轮）`gxp 'ERROR \| group by _file' single_file.txt` 单文件模式返回 `: 3` 空键（目录模式正常返回 `path: N`）。根因：`execute_stages`（单文件路径）取 `file_path: &str` 参数但从未填入 `m.path`；`pseudo_field_value(m, "_file")` 返回 None → 空键。`mode/gxp.rs::run_gxp_mode` 仅在 `opts.json = true` 时填 `m.path`（JSON 输出需要），文本模式漏填。修复：在 `execute_stages` 入口对每行 `m.path` 填 `file_path`（与目录模式 `execute_stages_directory` mod.rs:347 对齐）；同时为 `emit_remaining` 加 `show_path_prefix: bool` 参数，单文件模式传 `false` 抑制文本输出路径前缀（保持历史 UX：`line_no:line` 而非 `path:line_no:line`），目录模式传 `true`（保持 `path:line_no:line`），JSON 模式不受影响（已用 `if let Some(p) = m.path` 处理）；同步更新 `execute_stages_streaming`（单文件流式 `false`）+ `execute_stages_directory`（`true`）+ 2 处单元测试 helper（`emit_text` 单文件 + `emit_text_dir` 目录）。**B7**（第 14 轮）流式路径（>512MB 文件）`run_gxp_mode_streaming` 同构 bug：同样只在 `opts.json=true` 时填 `m.path`，文本模式漏填 → 大文件 `group by _file` / `distinct by _file` 返回空键。修复：`run_gxp_mode_with_content` 和 `run_gxp_mode_streaming` 两处都改为无条件填 `m.path`（与 `execute_stages` 入口对齐）。**B8**（第 15 轮）`capture(rid, /req-(\d+)/) && rid>10000` 返回 0 行——L1 `collect_field_predicates` 收集了 `rid>10000` 的 `rid=` 前缀粗筛，行内无 `rid=`（值由 capture 提供）→ L1 误淘汰所有行。B1 修了 pipeline `where` 路径（`apply_where` 前 `captures_has_field` 检查），但 L2 `&&` 路径同构遗漏。修复两层：（1）`l1/compiler.rs::collect_field_predicates` 先收集所有 `Prim::Capture` 的 name，谓词字段名 == capture name 时跳过前缀收集（与 B1 同构）；（2）`mode/verify.rs` 暴力路径改调 `is_hit_with_captures`（原调 `is_hit` 走 `eval_prepared` 无 capture 提取 → `capture(X) && X>Y` 暴力 0 行，--verify 报"优化独有 N 行"反向违反 L1 ⊇ L2）；同步删除 `PreparedScan::eval` / `is_hit`（无 capture 提取版本，dead_code）+ 清理 `eval_prepared` / `eval_prepared_with_ctx` 的 import。**B9**（第 16 轮）`capture(rid, /req-(\d+)/) && !(rid>50000)` 返回 3 行（应返回 2 行——67890 > 50000 应被 Not 排除却未被排除）。根因：`mode/eval.rs::eval_prepared_with_captures` 的 `Not(e)` 分支原调 `eval_prepared(e, line)`（无 captures）——Not 内部谓词 `rid>50000` 找不到 rid 值（rid 值由外部 capture 提供，行内无 `rid=`）→ NotNumeric → Not 映射为 True → 所有行命中。原注释"capture 提取无意义"部分正确——Not 内部新提取的 capture 不应注入（取反的谓词匹配了也不注入）。但**外部已提取的 captures** 必须传递（如 `capture(rid) && !(rid>50000)` 中 rid 值由外部 capture 提供，Not 内部谓词需用此值）。B8 修复了暴力路径用 `is_hit_with_captures`，但 L2 Not 分支内部仍调 `eval_prepared` 不传 captures → B9 同构遗漏。修复：`eval_prepared_with_captures` 的 `Not(e)` 分支改调 `eval_prepared_with_captures(e, line, captures)` 传递 captures；l3-structural 镜像 `eval_prepared_with_ctx_and_captures` 的 `Not(e)` 分支同步改调 `eval_prepared_with_ctx_and_captures`；修复后 `eval_prepared` / `eval_prepared_prim` / `eval_prepared_with_ctx` / `eval_prepared_prim_with_ctx` 四个无 capture 提取版本全部 dead_code，一并删除（统一走 `eval_prepared_with_captures` / `eval_prepared_with_ctx_and_captures`，无 capture 调用方传空 `CaptureMap` O(1) 哈希查询无性能损耗）；同步清理 10 处文档引用（mod.rs/gxp.rs/detect.rs/verify.rs/compiler.rs/extract.rs/structural.rs/prepared.rs 的注释指向新函数名）。+7 测试（3 单元 + 4 e2e）：单元覆盖 `capture(X) && !(X>Y)` 三谓词（Compare/In/Between）；e2e 覆盖三谓词 + --verify 对账；修正旧测试 `test_not_does_not_extract_capture`（原断言 `captures.is_empty()` 基于 Not 不传 captures 的旧行为，B9 后允许 Not 子树求值副产物写入 captures，断言改为只断言整体返回 False）。**B10**（第 17 轮，2026-10-29 jq 1.8.2 安装组合验证时暴露）：`gxp 'cost_ms exists' /tmp/t1.jsonl`（含 `{"cost_ms":900}`）返回 0 行（logfmt 行 `cost_ms=900` 正常命中 1 行）。**初步定位错**（L2 `extract_json_kv_via_memchr` 数值 value 边界 bug）——单元测试 `test_extract_json_numeric_last_field_no_comma` 通过证伪。**真实根因**：L2 `find_value`（`src/l2/extract.rs:112`）只扫 logfmt 的 `=` / `:` 分隔符，未识别 JSON line 的 `"name":` 模式；找到 `:` 时向前查 key 起点遇 `"`（非字母数字）→ `key.iter().all(...)` 拒绝 → 跳过 → 返回 None；L1 `prefix_variants` 同构 bug：只生成 `name=` / `name:`，未生成 `"name":`，L1 aho-corasick 在 JSONL 行上 0 命中。**修复三层**：（1）L1 `prefix_variants`（`src/l1/compiler.rs:163-180`）返回 `Vec<Vec<u8>>`，加第三个变体 `"name":`（JSON line）；（2）L2 `find_value`（`src/l2/extract.rs:112-180`）logfmt 通路未命中时走 `extract_json_kv_via_memchr` 兜底；（3）单元测试 `test_compile_compare_produces_field_prefix` 期望从 `[0,0]` 改为 `[0,0,0]`（3 变体）。**+2 单元测试**：`test_extract_json_numeric_last_field_no_comma` + `test_extract_json_numeric_single_field_no_space`。**JSON 场景能力边界评估**（B10 修复后闭环）：gxp 独占行级过滤/Top-K/聚合/时间窗口/trace/AST；jq 委托 schema 探查/嵌套路径/多行合并/Array 展开；gron 委托嵌套路径拍平（与 gxp logfmt 直扫天然兼容）；不新增 `keys` 动词（jq `keys` 语法 LLM 易稳定生成，组合已覆盖）。**B11**（第 18 轮，2026-10-29 对抗评审发现）：B10 修复后对抗评审深探 JSONL 边角场景，发现 `detect_format` 的 JSON 探测不 trim 首尾空白——`{"cost_ms":900} `（尾随空格）/ `{"cost_ms":900}\r\n`（CRLF）/ ` {"cost_ms":900}`（行首空格）均 0 命中（应 1 命中）。根因：`detect_format` 用 `line[0]` / `line[len-1]` 判 JSON 首尾字符，尾随空白/CRLF/行首空格导致 `last` 非 `}` 或 `first` 非 `{` → 不识别为 JsonLine → `find_value` 走不到 JSON 兜底 → 0 命中。**--verify PASS [0 行命中]**——L1 和 L2 都漏，形式上 L1 ⊇ L2 保持，但**语义上应命中而未命中**（正确性 bug，非不变式违反）。Windows 环境 CRLF 常见，此 bug 影响真实 LLM 工作流。修复：`detect_format` 入口加 `trim_ascii_whitespace`（trim 空格/制表符/CR/LF），trim 后的行用于所有后续探测（JSON/Bracket/Logfmt/ISO）；`[tag]` 行不会因 trim 被误判为 JSON（trim 后首字符 `[` 但尾字符非 `]` 仍排除）。+5 单元测试（trailing space / CRLF / leading space / both sides whitespace / bracket not misjudged after trim）。**测试基线 922 默认/1015 l3 全绿**（v0.44 实测 2026-10-29，governance §4.1.8 自动对账；+5 = B11 detect_format trim 空白边角测试）；**§6.5.2 自举组合覆盖验证持续生效**：B6/B7 靠 `gxp '"_file" && "group"' tests/` 0 命中发现，B8 靠诚实态度（"还能测出来问题"质疑）+ 深探矩阵（capture × 谓词同字段组合）发现——B8 不是靠自举组合检查发现，是用户质疑驱动的深探发现，B9 靠 B8 修复后深探 `capture × Not(同字段谓词)` 同构盲区发现——B8/B9 均不靠 §6.5.2 发现，B10 靠 jq 安装组合验证发现（不是自举、不是单元层、不是用户质疑），B11 靠对抗评审深探 JSONL 边角场景发现——B6/B7/B8/B9/B10/B11 均不靠 §6.5.2 发现，说明矩阵规则仍需迭代（§6.5.2 模板未含 "capture × 同字段谓词"、"capture × Not(同字段谓词)"、"JSON 格式 × 字段值类型 × 首尾空白" 检查项；详见 `docs/_tmp_auto_fix_rounds_issues.md` §5，该临时文件已于 2026-11-01 清理，内容迁入 AGENTS.md §6.5.2-6.5.3 / §6.6 + governance §4.1.9）。**§6.5.2 模板扩展（2026-11-01，L3 文档改动）**：§6.5.2 检查表新增 3 行（capture × 同字段谓词 [B8] / capture × Not [B9] / JSON 格式 × 首尾空白 [B10/B11]）；§6.6 加强制 e2e 约束段；§9.1/§9.2 精简控制行数 ≤320。**§6.5.3 同构镜像扫描 + governance §4.1.9 cargo doc lint（2026-11-01，L3 文档改动）**：§6.5.3 新增镜像扫描表（eval_prepared_with_captures vs _with_ctx_and_captures / execute_stages 三路径 / run_gxp_mode 三路径）；governance §4.1.9 加 `cargo doc -D rustdoc::broken_intra_doc_links -D rustdoc::invalid_html_tags` 检查（防函数名漂移，覆盖 B9 清理 10 处场景）；修复 7 处 broken intra-doc links（`[tag]` → `` `[tag]` `` 6 处 + `[run_l3_ast]` → `` `run_all_ast` `` 1 处）+ 3 处 unclosed HTML tags（`<PathBuf>`/`<u8>` 加 backtick）；AGENTS.md 行数上限 320→330（§6.5.3 新增 10 行）；`_tmp` 临时文件清理（P1+P2 五项全部迁入正式文档）；测试基线不变。**L3 ast() 大目录验证补齐**（2026-11-01）：web-app 3378 文件（2329 .ts + 46 .js，537MB）× 3 ast pattern（ts function decl / ts arrow fn / js function decl）× --verify = 0 FAIL，~1.1-1.4s；capability-matrix §2 + performance-baseline §1.3 验证缺口标注从 ⚠️ 待补改为 ✅ |
| v0.45 | L3 ast() 扩展名过滤 — 消除非目标语言文件的无效 ast-grep 解析 + .jsx 回归修复 | `prepare_l3` 按文件扩展名跳过非目标语言文件（`ast(ts, ...)` 只解析 `.ts`/`.mts`/`.tsx`/`.vue`，不再对 `.json`/`.md`/`.css` 等无谓跑 TypeScript parser）。新增 `lang_matches_ext(SupportLang, &str) -> bool`（l3/structural.rs）+ `expr_has_matching_ast_lang(PreparedExpr, &str) -> bool`（递归检查查询中是否有 ast() 语言匹配扩展名）+ `ext_from_path(&str) -> String`（mode/detect.rs，前导点文件如 `.tmpXXXXXX` 返回空串不过滤）。`PreparedScan::build` / `build_per_file` / `run_verify` 签名加 `file_ext: &str` 参数。行为变更：`ast(ts, ...)` 不再解析 `.js` 文件（TypeScript parser 只扫 `.ts`/`.mts`/`.tsx`/`.vue`；`.vue` 仍匹配因 SFC 内嵌 `<script lang="ts">` 容错解析）；`.js` 文件应使用 `ast(js, ...)`。**B12 回归修复**：扩展名过滤遗漏 `.jsx`——v0.44 `ast(tsx/js)` 能解析 `.jsx`，v0.45 被跳过返回 0 行；修复：`lang_matches_ext` 的 `Tsx` 和 `JavaScript` 加 `.jsx`（ast-grep 两个 parser 都能解析 JSX 语法）。--no-gitignore 大目录（web-app 72K 文件）实测提速 40-53%（ast(ts function) 19.7→9.2s / ast(ts arrow) 17.1→8.7s / ast(js function) 20.2→12.1s）；--verify 0 FAIL（3 pattern × 72K 文件）；测试基线 922 默认/1017 l3 全绿（v0.45 实测 2026-11-01，+2 l3 e2e = .jsx 回归测试）；交叉验证：默认模式 `.vue` 426 命中不变 / `.js` 0 命中（正确不再用 TS parser 解析）|

### 决策依据历史（已废弃方案保留供 D 系改判时参考）

> 本节记录已废弃方案的详细实测数据与论证，按 governance R3 三档"决策依据历史"沉底。
> 对应设计文档仅保留结论 + 1 句理由 + 指向本节的链接。

#### T2/T3 跨查询增量（D9 永久拒绝，详见 rust-plan §11.3.2）

| 实验 | 结果 | 真实场景有效性 |
|---|---|---|
| 块摘要构建成本 | 32k-bit/块：846 ms ≈ 9 次 grep 全扫（本地盘） | — |
| 小布隆（4k-bit） | **完全饱和失效**，不存在的词也 100% 命中块 | — |
| 跳块收益 vs 词频 | 稀有词可跳 74–84% I/O；**常见词跳 0%** | — |
| zone-map 在交错日志 | 跳过 0% | **真实生产日志几乎都是交错的，zone-map 基本无效** |
| zone-map 在聚簇数据 | 跳过 93%（cost_ms>2900）/47%（>1500） | **非典型场景**：只在人为排序或按字段分区写入时出现 |
| 追加写增量更新 | 尾部 32 ms vs 全量 3392 ms = **107×** | **单进程内多次 check 测得**；一次性命令模型下每次启动都是 cold，无跨查询增量 |

**§4.3 实测数据的意义修正**：TailIndex 的 107× 增量收益在**单进程内多次 check** 时测得；一次性命令模型下每次启动都是 cold，无跨查询增量。该数字仅说明「单次查询内多阶段（L0 采样 → L1 grep → L2 求值 → --verify 对账）可避免重复 I/O」这一场景的价值。

**摘要缓存回本条件（保留供将来 D9 改判时参考）**：大文件 × 慢 IO × 稀有词 × 高频查询四个条件同时成立。在真实交错日志上 zone-map 跳过 0%（见上表），该策略对生产日志基本无效。

#### GNU grep 改造路线（D8 永久拒绝）

**v1 不走 GNU grep 改造路线**（P0/P1/P2 全部不做）。理由：
1. gxp 不承诺"grep 超集"（§0、§1.4），不需要绑死 GNU grep 二进制
2. §12.3 实测：git 调用比目录列举贵 50–130 倍——加重的状态管理不能进热路径
3. 跨平台：Windows 无原生 grep，依赖 WSL/ripgrep 不可靠
4. Rust 从头实现更省事（见 `rust-implementation-plan.md`）：regex crate 自带 literal extraction，memchr SIMD，tree-sitter 一等绑定

#### digit-DP 编译数值谓词为 ERE（D4 永久拒绝）

数值谓词（`cost_ms>800`）**不再**编译成 digit-DP 生成的 ERE 区间正则。理由：正则表达力是 CFG 子集，无法覆盖任意数值比较（9 位以上大数、前导零定宽、浮点、负数都漏匹配）。替代策略：L1 只下推字段名前缀（`cost_ms=`）做粗筛，L2 做精确数值比较。Rust 预估净收益为正（L1 节省的 digit-DP 编译/执行开销大于 L2 候选增加的开销，详细实测见 performance-baseline §1.2）。

#### prototype 期目录层 TDD 规模化验证数据（已删 prototype，数据保留作 Rust 实现参照基线）

> 数据来自 Python prototype v0.1-v0.10 阶段；prototype 已删除（v0.10），Rust `tests/dir_{pruning,index,off,tail,regex,git}.rs` 承担跨平台职责。本数据保留作 Rust 实现参照基线。

语料：30 天 × 2 分片 × 4 万行 = **361 文件 / 35 目录 / 154 MB**（overlay 文件系统，4 核）

**路径剪枝（设计 §1 第 1 节结论得证）**

| 谓词 | 目录访问 | 剪掉 | 需读 | 占全树 |
|---|---|---|---|---|
| `ext=log` 全树 | 35/35 | 0 | 154.0 MB | 100% |
| `dir_prefix=logs/2026-10-20` | 3/33 | **30** | 5.1 MB | **3.3%** |
| `dir_re=2026-10-(1[5-9]\|20)` | 35/35 | 0 | 30.8 MB | 20.0%（**不可剪枝**） |
| `ext=py + dir_prefix=src` | 4/5 | 1 | ~0 MB | 0% |
| `max_depth<=1` | 3/34 | 31 | ~0 MB | 0% |

遍历本身耗时 0.05–0.39 ms，可忽略；剪枝收益完全体现在"少读多少字节"。

**三档新鲜度（同一棵树第二次列举）**

| 模式 | 首次 | 再次 | 再次时 syscall | 正确性 |
|---|---|---|---|---|
| `strict` | 1.30 ms | 1.19 ms | readdir 35 + stat 361 | 永远正确 |
| `lax` | 1.07 ms | **0.45 ms** | readdir 35 + **stat 0**，复用 35 | 新增文件仍可见（复用 34 / 变更 1） |
| `off` | 0.98 ms | **0.06 ms** | **readdir 0 + stat 0** | **返回过期列表**（已有测试固化） |

**append-only 尾部增量**：追加 8 万行 → 只读尾部 5.4 MB，跳过 32.1% 重复扫描；无新写时 `delta=0`（0.009 ms）；`truncate`（日志轮转）→ `start=0` 强制全扫；同尺寸重写、inode 替换均判为全扫。

**分区并集 == 全树**（剪枝不改变结果）：30 次按天分区扫描并集 **944,032 行**，与全树一次扫描 **完全一致**；代价 213 ms vs 181 ms（多 32 ms 固定开销）。

**git 锚点成本（重要修正）**：`rev-parse HEAD` 36.8 ms、`status --porcelain` 85.8 ms、`-uno` 91.6 ms、`diff-index` 65.3 ms，而 lax 列树只要 0.64–1.8 ms → **git 调用比目录列举贵 50–130 倍，不能进热路径**。

#### prototype 期 §12.3 对前文结论的修正（保留作 Rust 实现参照）

| 前文说法 | 实测修正 |
|---|---|
| "lax 先比目录指纹，变了才重建" | ✅ 成立，但省的是**逐文件 stat**，**readdir 每次仍要做**（不做 readdir 就无法发现改名/新增） |
| "git status 一次调用拿到全量变更集，比 inotify 便宜" | 可靠性成立，**便宜不成立**（86 ms）。改用法：`head` 作为会话级缓存 key，目录指纹做每次校验，`dirty` 只在写操作前查 |
| "路径剪枝是唯一有效的跳过手段" | ✅ 得证：单日谓词把需读字节压到 3.3%，而块级 zone-map 在交错数据上是 0% |
| `dir_re` 可用于剪枝 | ❌ 不可。无锚定正则无法证明后代目录不命中，一律进入；要剪枝必须用 `dir_prefix` |
| `off` 档可作默认 | ❌ 只能用于显式的 TTL 场景，默认必须 `lax` |



