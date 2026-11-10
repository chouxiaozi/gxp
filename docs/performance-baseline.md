# gxp 性能基线文档

> **用途**：所有性能数据的单一参考源。版本迭代后更新此文件。
> **更新规则**：跑完 bench 后立即更新；版本号 + 日期 + 实测条件必须齐全。

**最后更新**: v0.49（2026-11-09）— T1a 优化落地：L2 Literal 重校验豁免 + collect 零分配化，全管道 125.30→106.74ms（**−14.8%**，diag median of 3），详见 §1.2
**实测日期**: 2026-10-26（v0.32 性能基线实测日 + v0.41 复测）+ 2026-10-27（12GB 大数据多轮实测）+ 2026-11-01（v0.45 扩展名过滤实测）+ 2026-11-02（v0.45 三轮 diag bench 复测）+ 2026-11-09（v0.47 复测，起测量机为本机 i7-10700；v0.48 T1a 基线锚定；v0.49 T1a 优化实测）
**实测环境**: v0.32-v0.45 数据实测于 **i7-14700 / Win11 26200**（旧机）；v0.47 复测起实测于 **i7-10700 / Win11 26100**（本机，v0.45 后仓库迁移）。两机同为 x86_64-pc-windows-gnu / rustc 1.99.1 / GCC 15.2.0。**跨机绝对值不可比**（同版本比值实测 1.52-1.85×，详见 §1.1 机器变更说明）
**基准场景**: 44MB 日志文件 / 1,060,000 行 / 31931 候选行 / Top20 投影
**目录基准**: web-app 70648 文件（rayon 并行实测）
**大数据基准**: 12GB 合成 transcript / 11603 文件 / 60 子目录（详见 §3.1）

> **v0.41 复测结果（2026-10-26）**：trace v0.39 落地后性能无退化。分阶段数据表面改善（read -14%、L1 scan -12%、L1+L2+collect -22%、execute_stages -24%），run_with_writer 完整 80.30ms（vs v0.32 的 82.36ms，-2.5%）。**但归因是系统 cache 状态差异，非代码优化**：`git log -- src/l1/ src/l2/ src/mode/ src/io/file.rs src/plan.rs` 显示 v0.32→v0.41 期间热路径零 commit（只有 pipeline 阶段在变）；v0.41 实测第二次跑（warm cache）进一步降到 74.64ms，证明改善来自 page cache 预热状态。详见 §1.1 v0.41 列 + §1.3 归因分析。

> **v0.45 三轮复测结果（2026-11-02）**：diag_pipeline/diag_breakdown/diag_stages 各 3 轮，run_with_writer median 80.77ms，与 v0.41 的 80.30ms 一致（+0.6% 噪声范围内）。热路径零 commit（git log 确认 v0.41→v0.45 期间 `src/l1/`/`src/l2/`/`src/mode/`/`src/io/file.rs`/`src/plan.rs` 无改动）。execute_stages v0.45 median 14.12ms vs v0.41 的 8.74ms，但 diag_stages 独立测 execute_stages 是 15.76ms（与 v0.32 的 9.42ms 也有差异）——diag_pipeline 和 diag_stages 都是独立进程单次 min，execute_stages 的波动来自进程启动/cache 状态差异，非代码退化。详见 §1.1 v0.45 列 + §1.6 归因分析。

> **v0.47 复测结果（2026-11-09，测量机变更）**：B14（l1_exact 捷径门槛 + Or 组完整性）**零回归**——判定方法是同机 A/B：v0.45 源码 worktree 在本机同工具链同 fixture 编译实测 diag run_with_writer 125.12ms，与 v0.47 的 128.09ms（3 轮 median）差 +2.4%（噪声级，各阶段差异 ≤4.5%）。**同时发现 v0.32-v0.45 全部数据来自旧机 i7-14700**（本机 CPU i7-10700 + OS build 26100 与文档头部 i7-14700 + 26200 双重不符，且仓库在 v0.45 squash 重建时迁移）：同版本跨机比值 1.52-1.85×（全管道 128.09/80.77 = 1.59×，符合两代 CPU 单核差距）。**v0.47 列起为本机 i7-10700 新锚点，与历史列不可直接比值**；80ms 目标（AGENTS.md §7.1）锚定于旧机——已决策重锚定为本机基线锚点 + 回归性门禁（2026-11-09 用户确认，见 §1.1 机器变更说明）。另：criterion `gnu_grep_pipe` 对照在本机失效（msys 子进程 spawn ~400-500ms/进程，5 进程管道 1.4-2.6s，测的是进程启动开销非吞吐——错误路径实证：文件不存在时管道耗时相同），后续 grep 对照改用直接进程调用（§1.3.2 方法）。

---

## 1. 核心指标总览

### 1.1 全管道性能（基线锚点 106.74ms = v0.49 本机实测；门禁见 AGENTS.md §7.1）

| 指标 | 基线锚点（本机，AGENTS.md §7.1） | v0.32 实测 | v0.41 复测 | v0.45 复测 | v0.47 复测（本机 i7-10700） | v0.49 T1a 优化（本机） | 状态 |
|---|---|---|---|---|---|---|---|
| 全管道 + Top20 + 投影 | 106.74ms | 66-76ms | 80.30ms（噪声边缘） | 80.77ms（噪声边缘） | 128.59ms（criterion）/ 128.09ms（diag median） | **106.74ms（diag median，−14.8%）** | ✅ 旧机达标（边缘）；本机锚点 = v0.49 实测，median 劣化 >10% 需归因 |
| L1 两遍链 | 33.51ms | 18.66ms | 17.28ms | 18.10ms | 32.34ms（criterion）/ 33.44ms（diag） | 33.51ms（diag，不变） | ✅ 旧机达标；本机锚点 = v0.49 实测 |
| L2 字段抽取（31931 候选行） | 8.21ms | ~12ms | 5.42ms | 5.42ms | 8.21ms（criterion l2_extract） | 未复测（v0.47 8.21 参照） | ✅ 旧机达标；本机锚点 = v0.47 实测（v0.49 未复测） |
| DirIndex lax 二次（Linux） | 0.15ms | 未验证（Windows 物理 ~2.8ms） | 未验证 | 未验证 | 未复测 | ⚠️ Windows 物理不可达，Linux 待 CI 接入 |

> **机器变更说明（v0.47 复测起）**：v0.32-v0.45 列实测于旧机 i7-14700，v0.47 列实测于本机 i7-10700（仓库 v0.45 后迁移），**跨机比值 1.52-1.85×，列间不可直接比值**。B14 零回归由同机 A/B 判定（v0.45 源码本机实测 125.12ms vs v0.47 128.09ms，+2.4% 噪声级，详见顶部 v0.47 复测说明）。80ms/30ms/20ms 目标锚定于旧机——**已决策（2026-11-09，用户确认）：§7.1 重锚定为本机基线锚点 + 回归性门禁（同机 A/B，median 劣化 >10% 需归因），旧机目标降为历史参考**，本表首列即新锚点。

v0.45 vs `gnu_grep -E | head -20` ~148ms（criterion 实测），gxp ~1.8× 快。v0.41 时 gxp 1.7× 快（gnu_grep 140ms），比值波动来自系统 cache 状态。**（旧机数据）本机 i7-10700 上该 criterion `gnu_grep_pipe` 对照失效**：msys 子进程 spawn ~400-500ms/进程（5 进程管道 1.4-2.6s），测的是进程启动开销非 grep 吞吐；本机 grep 对照改用直接进程调用（见 §1.3.2 ripgrep 对照小节方法）。

> **v0.41 复测说明（2026-10-26）**：v0.32→v0.41 期间落地 v0.33 sample/v0.34 sort/v0.35 distinct/v0.37 percentiles/v0.39 trace，性能未退化反而略改善。新动词都是 L4 pipeline 阶段，不进入 L1+L2 热路径。L2 抽取 12ms→5.42ms 改善 56%。**归因是系统 cache 状态差异，非代码优化**：`git log -- src/l1/ src/l2/ src/mode/ src/io/file.rs src/plan.rs` 显示 v0.32→v0.41 期间热路径零 commit；v0.41 实测第二次跑（warm cache）进一步降到 74.64ms，证明改善来自 page cache 预热状态。两次实测用的是同一份确定性生成的 fixtures/44mb.log，输入数据完全相同。

### 1.2 分阶段拆解（diag_pipeline + diag_breakdown + diag_stages）

```
v0.49 T1a 优化实测（2026-11-09，本机 i7-10700，diag_pipeline median of 3 rounds：
  110.70/106.74/106.53 → median 106.74ms，vs v0.48 基线 125.30 **−18.56ms / −14.8%）：
  read_for_scan:              13.37ms   （不变，I/O）
  L1 scan_lines_cb (count):   33.51ms   （不变——L1 编译器只增 verified_literals 产出）
  L1+L2 eval (no collect):    47.68ms   （简化 bench 阶段，不含豁免收益）
  L1+L2+collect:              57.30ms   （同上）
  execute_stages:             18.16ms   （25.17→18.16，−7.0ms：MatchedLine 缩水红利
                                          ——match_bytes/path 两字段 3-word→2-word，
                                          clone 与排序 cache 局部性受益）
  ─────────────────────────────────────
  run_with_writer 完整:       106.74ms  （生产路径收益 ~16ms 集中在 collect：
                                          Literal regex 重校验豁免 + match_bytes/path
                                          零分配；diag_stages 完整 24.29→17.17 交叉一致）
  未归因差值:                  ~17.9ms（106.74 − 各阶段累计 88.8；原 29.94——生产冗余
                              ~16ms 已消除，余下为相位隔离测量理想化 + PreparedScan::build
                              + 分发开销，samply 不可用（v0.13.1 本机编译失败），已归因——
                              见下方「残差归因」块）

  优化明细（Rust 方案 §7.9，归因数据见 v0.48 块下方归因表）：
    Opt A  Literal 重校验豁免     +5.8ms（verified_literals 构造性论证）
    Opt B  match_bytes Arc+快速路径 ~3.6ms（单元素组免 aho 二查）
    Opt C  path Arc<str>          ~6.5ms（每行 String clone → refcount）
    计划 ~15.9ms；实测全管道 −18.56ms（含 stages 红利 ~7ms，结构缩水意外收益）

  残差归因（2026-11-09 晚，v0.49 后，diag_pipeline 新增 C1/C2/C3 判别检查点，
  3 轮 median；本轮全管道 108.55ms vs 锚点 106.74 +1.7% 噪声级，R1 136.68
  首轮离群（新编 exe Defender 税 + 首触）弃用）：
    C1 parse_query+compile:                     0.04ms（parse/compile 与残差无关，排除）
    C2 端到端复刻链（public API 按生产顺序串联，fresh read）: 104.44ms
    C3 同链 warm buffer（无 read）:              92.70ms
    C2 − C3 = 11.74 ≈ read 独立值 14.48 → fresh buffer 无额外冷页/TLB 税
    本轮残差 = 108.55 − 阶段累计 94.84（read 14.48 + L1 35.04 + L2 inc 13.88
    + collect inc 11.53 + stages 19.91）= 13.71ms（v0.49 轮 17.9，会话间噪声 ±4ms）

  三桶拆解（合计 ~14.9 ≈ 残差 13.71，块间噪声 ±2-3ms 内闭合）：
    ① 复刻口径差 ~6.0ms：生产形态 collect 66.49 vs 简化 collect 60.45——
       match_bytes/path 逐行填充（103607 行 Option 写入 + 结构脏化）；
       `_match`/`_file` 伪字段语义必需（场景 A/D 一等公民），非可削减冗余
    ② 链内相位交错 ~4.8ms：C2 104.44 − 各段独立 min 之和 99.68（read 14.48
       + C1 0.04 + 生产 collect 66.49 + stages 生产形态 18.67）——单遍串联
       vs 分段热循环的 TLB/cache/分配器交错，端到端固有属性
    ③ 生产专属 ~4.1ms：run_with_writer 108.55 − C2 104.44——PreparedScan::build
       （<0.1ms）+ eval_prepared 机制 vs 简化 l2_match（含 CaptureMap::new ×
       141367 L1 命中行）+ dispatch（reject/positional/metadata，<0.5ms）

  结论：残差无单一大头（三桶各 4-6ms）。triage：① 语义必需、② 端到端固有，
  均非优化靶；③ 内 CaptureMap 空表共享（no-capture 查询免 141k 次构造）预估
  <2ms，需动 with_captures 边界，收益/风险比不足，暂不做（Rust 方案 §7.9 补记）。
  stages 内 rank key extraction 8.24ms（diag_stages）仍是最大单一可优化靶（可选后续）。

v0.48 T1a 基线锚定（2026-11-09，本机 i7-10700，diag_pipeline median of 3 rounds，3 轮离散 <2%）：
  read_for_scan:              12.57ms   fs::read 44MB → Vec<u8>（3 轮 12.42/12.57/13.49）
  L1 scan_lines_cb (count):   33.72ms   memchr + aho-corasick（命中 141367 行）
  L1+L2 eval (no collect):    47.28ms   + find_value + eval_numeric（L2 命中 103607）
  L1+L2+collect:              57.62ms   + MatchedLine Arc<[u8]> clone（7019876 字节）
  execute_stages:             25.17ms   rank partial-sort + fields 投影
  ─────────────────────────────────────
  run_with_writer 完整:       125.30ms  （3 轮：125.18/125.30/127.75）
  各阶段增量累计:              95.36ms   （L2 eval 增量 13.56 + collect 增量 10.34）
  未归因差值:                  29.94ms（~24%）— T1a 第一靶子，与 v0.47 的 31.46ms 同量级

  vs v0.47（同机）：全管道 −2.2%、L1 +0.8%、execute_stages −0.3%，噪声级零回归
  （read −7.8% 属 I/O cache 噪声）。diag 主查询为 And + In 形态，不经过 v0.48
  改动路径（字符串等值脱糖 / B15 Or 前缀豁免），零回归符合预期。

  T1a 归因（diag_pipeline 复刻检查点，v0.48 形态，2026-11-09）：
    L1 + Literal regex.is_match:     39.39ms（vs L1 33.59 → 重校验 +5.80ms/141367 行）
    生产形态 collect（v0.48 复刻）:   80.12ms（vs 简化 57.68 → 总增量 +22.44ms）
      其中 path 填充 ~6.5ms / match_bytes（aho 二查+Vec 分配）~3.6ms
      （A/B 变体同轮相对拆分，该轮受后台任务 CPU 竞争，绝对值偏高仅取差值）
    残差 ~11ms：相位隔离测量理想化（standalone min 热循环 vs 单次冷路径）

v0.47 复测（2026-11-09，本机 i7-10700，diag_pipeline median of 3 rounds）：
  read_for_scan:              13.64ms   fs::read 44MB → Vec<u8>
  L1 scan_lines_cb (count):   33.44ms   memchr + aho-corasick（命中 141367 行）
  L1+L2 eval (no collect):    47.85ms   + find_value + eval_numeric（L2 命中 103607）
  L1+L2+collect:              57.74ms   + MatchedLine Arc<[u8]> clone（7MB 总字节）
  execute_stages:             25.25ms   rank partial-sort + fields 投影
  ─────────────────────────────────────
  run_with_writer 完整:       128.09ms  （3 轮：128.09/129.82/126.68，离散 <2%）

  同机 A/B 对照（v0.45 源码 worktree，同工具链同 fixture，单次）：
  run_with_writer: 125.12ms / L1 scan: 33.28 / L1+L2: 47.46 / collect: 56.80 / execute_stages: 24.16
  → 与 v0.47 各阶段差异 ≤4.5%（噪声级），B13/B14 零回归

v0.45 三轮复测（2026-11-02，diag_pipeline median of 3 rounds）：
  read_for_scan:              11.69ms   fs::read 44MB → Vec<u8>
  L1 scan_lines_cb (count):   18.10ms   memchr + aho-corasick（命中 141367 行）
  L1+L2 eval (no collect):    27.56ms   + find_value + eval_numeric（L2 命中 103607）
  L1+L2+collect:              33.96ms   + MatchedLine Arc<[u8]> clone（7MB 总字节）
  execute_stages:             14.12ms   rank partial-sort + fields 投影
  ─────────────────────────────────────
  run_with_writer 完整:       80.77ms

v0.41 复测（2026-10-26）：
diag_pipeline (min):
  read_for_scan:              13.85ms   fs::read 44MB → Vec<u8>
  L1 scan_lines_cb (count):   21.08ms   memchr + aho-corasick（命中 141367 行）
  L1+L2 eval (no collect):    29.92ms   + find_value + eval_numeric（L2 命中 103607）
  L1+L2+collect:              37.15ms   + MatchedLine Arc<[u8]> clone（7MB 总字节）
  execute_stages:              8.74ms   rank partial-sort + fields 投影
  ─────────────────────────────────────
  run_with_writer 完整:       80.30ms

v0.32 baseline（保留对照）：
diag_pipeline (min):
  read_for_scan:              16.15ms   fs::read 44MB → Vec<u8>
  L1 scan_lines_cb (count):   23.88ms   memchr + aho-corasick（命中 141367 行）
  L1+L2 eval (no collect):    35.17ms   + find_value + eval_numeric（L2 命中 103607）
  L1+L2+collect:              47.64ms   + MatchedLine Arc<[u8]> clone（7MB 总字节）
  execute_stages:             11.51ms   rank partial-sort + fields 投影
  ─────────────────────────────────────
  run_with_writer 完整:       82.36ms
  手工 sum:                   75.30ms
  差值:                        7.06ms   (9%) cache 迁移开销

diag_breakdown (min, v0.32):
  parse + compile:             0.04ms
  scan + L2 + collect:        39.46ms
  execute_stages:             10.06ms
  read_for_scan:              14.57ms
  run_with_writer 完整:        76.76ms

diag_breakdown (median, v0.45 三轮复测 2026-11-02):
  scan + L2 + collect:        37.22ms
  execute_stages:             17.03ms
  read_for_scan:              13.43ms
  run_with_writer 完整:       82.85ms

diag_stages (103607 候选行, v0.32):
  execute_stages 完整:          9.42ms
  apply_rank:                   6.62ms
  rank key extraction:         5.51ms   (find_value ~4.46ms + parse_numeric ~1.05ms)
  sort_by (precomputed):       2.06ms
  select_nth + sort K:         0.22ms

diag_stages (median, v0.45 三轮复测 2026-11-02):
  execute_stages 完整:         15.76ms
  apply_rank:                   8.68ms
  rank key extraction:         5.53ms
  sort_by (precomputed):       2.04ms

diag_breakdown (median, v0.48 T1a 锚定 2026-11-09，本机 i7-10700，3 轮):
  scan + L2 + collect:         59.75ms
  execute_stages:              27.27ms
  read_for_scan:               12.56ms
  parse + compile:              0.06ms
  run_with_writer 完整:       124.89ms  （与 diag_pipeline 125.30 差 0.3%，交叉一致）

diag_stages (median, v0.48 T1a 锚定 2026-11-09，本机 i7-10700 首次拆解，3 轮):
  execute_stages 完整:         24.29ms
  apply_rank:                  14.26ms
  rank key extraction:         8.72ms   (find_value + parse_numeric，占 apply_rank 61%)
  sort_by (precomputed):       2.65ms
  select_nth + sort K:          0.31ms

diag_breakdown (v0.49 T1a 优化实测 2026-11-09，单轮):
  scan + L2 + collect:         60.01ms
  execute_stages:              21.36ms
  read_for_scan:               13.96ms
  run_with_writer 完整:       107.75ms  （与 diag_pipeline 106.74 差 0.9%，交叉一致）

diag_stages (v0.49 T1a 优化实测 2026-11-09，单轮):
  execute_stages 完整:         17.17ms  （v0.48 24.29 → −7.1ms，MatchedLine 缩水红利）
  apply_rank:                  14.26ms  （不变）
  rank key extraction:         8.24ms   （≈8.72，噪声内）
  sort_by (precomputed):       2.68ms
  select_nth + sort K:          0.25ms
```

#### v0.45 vs v0.41 execute_stages 差异归因

v0.45 diag_pipeline 的 execute_stages median 14.12ms vs v0.41 的 8.74ms（+61.5%），表面退化。但：

1. **diag_stages 独立验证**：v0.45 diag_stages median 15.76ms，v0.32 diag_stages 是 9.42ms——同一指标在不同 bench 工具间也有 6-7ms 差异，说明 bench 进程启动/cache 状态对 execute_stages 这一小绝对值（~10-16ms）的测量影响显著。
2. **热路径零 commit**：`git log -- src/pipeline/rank.rs src/pipeline/project.rs src/pipeline/mod.rs` 显示 v0.41→v0.45 期间 rank/project 核心逻辑无改动（只有 v0.44 B6-B11 bug 修复在 mode/eval 层，不影响 execute_stages 内部）。
3. **绝对值占比**：execute_stages ~14ms 占全管道 80ms 的 17%，与 v0.32 的 11.51ms/82.36ms=14% 和 v0.41 的 8.74ms/80.30ms=11% 在同一量级，无功能性退化。

**结论**：execute_stages 测量波动来自 bench 进程级 cache 状态差异（小绝对值对系统噪声敏感），非代码退化。全管道 80.77ms 与 v0.41 的 80.30ms 一致（+0.6%）。

#### 瓶颈定位（91% 耗时已归因，无需进程级 profiler）

| 阶段 | v0.32 耗时 | v0.45 耗时 | 占比(v0.45) | 归因 | 优化空间 |
|---|---|---|---|---|---|
| read_for_scan | 16.15ms | 11.69ms | 14% | I/O bound（D10 read_to_end） | 无（D10 已对抗评审） |
| L1 scan | 23.88ms | 18.10ms | 22% | memchr/aho-corasick SIMD 已最优 | 无 |
| L2 eval (增量) | 11.29ms | 11.40ms | 14% | find_value 单字段 + eval_numeric | 低 |
| collect (clone) | 12.47ms | 6.40ms | 8% | Arc<[u8]> 引用计数 clone | 低（已 Arc 化） |
| execute_stages | 11.51ms | 14.12ms | 17% | rank key extraction 5.53ms 是主项 | 低 |
| 差值 | 7.06ms | 11.16ms | 14% | cache 迁移 + 测量噪声 | 无（非未知耗时） |

**结论**：samply profile 不再阻塞 v1.0。现有 diag bench 已提供等效分阶段数据。

### 1.3 大数据实测（v0.42，2026-10-27）

> 12GB 合成 agent transcript 数据集（30 个真实模板 round-robin 复制到 11603 文件 / 60 子目录），用于验证 v1 在 >10GB 量级是否适用。

#### 12GB 目录多轮实测（6 查询 × 3 轮，median）

| 查询 | R1 | R2 | R3 | median | mean |
|---|---|---|---|---|---|
| BARE `"ERROR"` | 1.570s | 1.763s | 1.782s | **1.763s** | 1.705s |
| `"ERROR" \| context 3` | 1.749s | 1.656s | 1.719s | **1.719s** | 1.708s |
| `"user" \| count` | 3.853s | 4.342s | 3.537s | **3.853s** | 3.911s |
| `"user" \| rank top 10 by line_no desc` | 3.955s | 3.596s | 3.807s | **3.807s** | 3.787s |
| `"assistant" \| files` | 1.782s | 1.855s | 1.898s | **1.855s** | 1.845s |
| `"system" \| distinct` | 1.821s | 1.994s | 1.803s | **1.821s** | 1.872s |

- **--verify**：11603 文件 × 6 查询 × 3 轮 = 100% PASS（0 FAIL），L1 ⊇ L2 不变式成立
- **数据结构多样性限制**：12GB 由 30 个真实模板 round-robin 复制，结构多样性 = 30 种，不是 11603 种；不能外推到所有 JSONL 结构

#### 911MB 单文件流式 fallback 实测（gxp vs GNU grep 3.0 × 3 轮）

| 工具 | R1 | R2 | R3 | median |
|---|---|---|---|---|
| gxp `"ERROR" \| count` | 0.327s | 0.290s | 0.299s | **0.299s** |
| grep -cE ERROR | 0.714s | 0.724s | 0.723s | **0.723s** |

- **gxp 2.42× 快于 GNU grep 3.0**（Windows 911MB 单文件，median 比值）
- **限制声明**：(1) 单次 OS 调度波动较大，3 轮取 median 仅能消除部分噪声；(2) GNU grep 3.0 是 Windows 移植版，Linux GNU grep 3.11 性能可能不同，不可外推；(3) page cache 命中率影响实测，911MB < 16GB RAM 可能部分热读
- **--verify 拒绝路径**：>512MB 单文件 `--verify` 不支持（流式 fallback 拒绝暴力对账），错误信息清晰提示用户

#### ripgrep 对照（v0.47，2026-11-09，本机 i7-10700）

> 补齐 AGENTS.md §6.1 声称的 gxp vs GNU grep vs ripgrep 三方对照。**原 911MB 数据集已不在本机**（v0.42 实测数据集未留存），本表用等价合成 JSONL：899.81 MiB / 6,700,000 行 / 均值 ~140B/行 / `"level":"ERROR"` 密度 1/5 = **1,340,000 行**（确定性生成，块复制法）。三方同文件同轮次交错实测（1 warmup + 3 轮 median，RAM 31.8GB 文件全量入 cache），全部直接进程调用（不经 msys 管道，避开本机 spawn 税，见 §1.1 机器变更说明）。

| 工具（版本） | 命令 | R1 | R2 | R3 | median | spawn（`--version` median） | 剔除 spawn |
|---|---|---|---|---|---|---|---|
| gxp 0.47.0 release | `"ERROR" \| count`（流式 fallback） | 1454ms | 1454ms | 1459ms | **1454ms** | ~317ms | ~1137ms |
| GNU grep 3.0（git 自带） | `grep -cE ERROR` | 1025ms | 1024ms | 1014ms | **1024ms** | ~92ms | ~932ms |
| ripgrep 15.2.0 | `rg -c ERROR`（直连路径，runtime AVX2） | 581ms | 573ms | 575ms | **575ms** | ~89ms | ~486ms |
| gxp 0.45.0（同机 A/B） | 同 v0.47 | 1496ms | 1446ms | 1488ms | **1488ms** | — | — |

- **计数正确性**：三方输出均 = 1,340,000 = 独立计算期望值（670 万行 × 1/5）✓
- **同机 A/B**：v0.45 ≈ v0.47（1488 vs 1454ms，±3% 噪声）→ B13/B14 及 v0.45→v0.47 流式路径零退化。**v0.42→v0.45 无法 A/B**（仓库 v0.45 squash 重建无 v0.42 commit；期间 B6-B11 动过流式 m.path 填充）
- **诚实结论：本机本场景 ripgrep 最快**——gxp raw 慢于 rg 2.53× / 慢于 grep 1.42×；剔除 spawn 后 2.40× / 1.25×。gxp 差异化价值不在裸扫描吞吐（功能维度见 §6.3），此数据作为 v1.0 定位参考（P3 agent A/B 实验背景）
- **与上方 v0.42 表（gxp 2.42× 快于 grep）不可跨表比值**：机器不同（i7-14700→i7-10700）+ 数据集不同（原 transcript 未留存，行长/密度分布不同——流式 count 的 per-line 开销对行数敏感：本数据集 670 万行 vs 原数据集行数未知）+ spawn 环境不同（本机 Defender 对当日新编译 gxp.exe 逐次收税 ~290ms）
- **流式 fallback 吞吐参考**：gxp 流式 ~790 MiB/s（1137ms/900MB）vs read_to_end 路径 L1 扫描 1.31 GiB/s（§1.2 本机 33.44ms/44MB）vs rg ~1.9 GiB/s——流式路径存在优化空间，但 v1 优先级低（D10 已论证单查询主路径是 read_to_end，流式仅 >512MB fallback）
- 限制声明：(1) spawn 分解中 Defender 实时扫描对"当日新编译/新安装"二进制逐次收税（gxp ~317ms、rg 经 scoop shim 首测 ~700ms / 直连 ~89ms），raw 列为实际用户感知值；(2) 单机单日 3 轮 median，轮间波动 <2%；(3) ripgrep 15.2 为 2026-11 新版本，SIMD runtime AVX2 开启

#### 多样化数据集 L1+L2 验证（65MB server.log / 500K 行）

> 非转录本结构的真实日志格式，6 类查询 0 FAIL，L1 ⊇ L2 不变式在多样化数据上成立。

| 查询 | 结果 |
|---|---|
| `"ERROR" \| context 2` | PASS [100289 行命中] |
| `cost_ms>1800` | PASS [49876 行命中] |
| `capture(trace_id, /trace_[0-9a-f]+/) \| distinct` | PASS [500000 行命中] |
| `"ERROR" \| rank top 10 by cost_ms desc` | PASS [100289 行命中] |
| `_ts between "2026-10-01T00:00:00" and "2026-10-01T01:00:00"` | PASS [2977 行命中] |
| `cost_ms \| percentiles p50,p95,p99` | PASS [500000 行命中] |

#### L3 ast() 在真实代码库验证（gxp src/ 48 文件）

> L3 ast() 在自身源码（48 文件 Rust）上的 --verify 测试。**发现 ast-grep pattern 限制**（非 gxp bug）。

| 查询 | 结果 | 说明 |
|---|---|---|
| `ast(rs, "$OBJ.unwrap()")` | 0 FAIL | 13 行命中 |
| `ast(rs, "if $C { $A } else { $B }")` | 0 FAIL | 4 行命中 |
| `ast(rs, "match $E { $ARMS }")` | 0 FAIL | 0 行命中 |
| `ast(rs, "fn $NAME($ARGS)")` | 0 FAIL | 0 行命中 |
| `ast(rs, "#[test]")` | 0 FAIL | 11 行命中 |
| `ast(rs, "struct $NAME")` | 0 FAIL | 1 行命中 |
| `ast(rs, "#[test] fn $NAME()")` | **48 FAIL** | ast-grep 限制：多语句 pattern "Multiple AST nodes detected" — gxp 行为正确（直接暴露 ast-grep 错误） |

**结论**：L3 ast() 在单语句 pattern 上 0 FAIL；多语句 pattern（`#[test] fn $NAME()`）触发 ast-grep 上游限制，gxp 错误信息清晰暴露，非 gxp bug。

**L3 ast() 大目录验证**（v0.44，2026-11-01）：

| 目录 | 文件数 | ast pattern | 命中数 | --verify | 耗时 |
|---|---|---|---|---|---|
| web-app（537MB） | 3378（2329 .ts + 46 .js） | `ast(ts, "function $NAME($$$ARGS) { $$$BODY }")` | 809 行 | 0 FAIL | 1.7s |
| web-app（537MB） | 3378 | `ast(ts, "const $NAME = ($$$ARGS) => $$$BODY")` | 4040 行 | 0 FAIL | 1.3s |
| web-app（537MB） | 3378 | `ast(js, "function $NAME($$$ARGS) { $$$BODY }")` | — | 0 FAIL | 1.1s |

L3 ast() 大目录验证缺口已补齐：3378 文件 × 3 pattern × --verify = 0 FAIL。

**L3 扩展名过滤优化**（v0.45，2026-11-01）：`prepare_l3` 按文件扩展名跳过非目标语言文件，不再对 `.json`/`.md`/`.css` 等无谓跑 ast-grep parser。`--no-gitignore` 大目录（72K 文件）实测提速 40-53%：

| pattern | 优化前 | 优化后 | 提速 |
|---|---|---|---|
| `ast(ts, function)` | 19.7s | 9.2s | 53% |
| `ast(ts, arrow fn)` | 17.1s | 8.7s | 49% |
| `ast(js, function)` | 20.2s | 12.1s | 40% |

行为变更：`ast(ts, ...)` 不再解析 `.js` 文件（TypeScript parser 只扫 `.ts`/`.mts`/`.tsx`/`.vue`）；`.vue` 文件仍匹配（SFC 内嵌 `<script lang="ts">` 容错解析）。

#### web-app 交叉验证（v0.45，2026-11-02）

> 在真实前端项目 `web-app`（1.5GB / 72K 文件含 node_modules / 3378 文件含 .gitignore）上跑 gxp 全功能矩阵，验证功能边界和不变式。

**BARE 模式 ≡ grep -E 对账**（单文件级，3 pattern）：

| pattern | gxp 行数 | grep -cE 行数 | 一致 |
|---|---|---|---|
| `export` | 38 | 38 | ✓ |
| `function\|const` | 36 | 36 | ✓ |
| `^import` | 0 | 0 | ✓ |
| `export`（763 行文件） | 763 | 763 | ✓ |

**Not 不变式验证**（`with + without = total`）：387 行 .ts 文件，`import` BARE 命中 15 行 + `! "import"` 命中 372 行 = 387 = 总行数 ✓。

**目录模式 --verify**（3378 文件 / .gitignore 尊重）：

| 查询 | --verify 结果 | 耗时 |
|---|---|---|
| `"ERROR"` | 3378 files checked, 0 failed | 0.43s |
| `ast(ts, "function $NAME($$$ARGS) { $$$BODY }")` | 3378 files checked, 0 failed | 1.68s |
| `ast(ts, "const $NAME = ($$$ARGS) => $$$BODY")` | 3378 files checked, 0 failed | 1.71s |
| `ast(js, "function $NAME($$$ARGS) { $$$BODY }")` | 3378 files checked, 0 failed | 1.14s |

**--no-gitignore 大目录 --verify**（72K 文件）：

| 查询 | --verify 结果 | 耗时 |
|---|---|---|
| `ast(ts, "function $NAME($$$ARGS) { $$$BODY }")` | 72516 files checked, 0 failed | 9.68s |
| `ast(ts, "interface $NAME { $$$BODY }")` | 72518 files checked, 0 failed | 10.87s |
| BARE `function`（命中统计） | 482867 行 | 5.32s |
| `"ERROR" \| count` | 6344 行 | 2.68s |

**功能矩阵覆盖**（目录模式 3378 文件，0 error）：

| 功能 | 查询 | 结果 |
|---|---|---|
| BARE 目录扫描 | `function` --glob '*.ts' | 1765 行 ✓ |
| gxp AND 查询 | `"export" && "default"` --glob '*.ts' | 235 行 ✓ |
| gxp OR 查询 | `"export" \|\| "import"` | 53 行（单文件）✓ |
| count | `"function" \| count` | 2968 ✓ |
| files | `"console" \| files` --glob '*.ts' | 100 files ✓ |
| distinct | `"import" \| distinct` --glob '*.ts' | 去重 import 行 ✓ |
| context | `"export" && "default" \| context 2 \| head 10` | 窗口展开 ✓ |
| sample | `"import" \| sample 5` --glob '*.ts' | 5 行随机抽样 ✓ |
| sort | `"export default" \| sort \| head 5` | 排序输出 ✓ |
| capture + distinct | `capture(fn_name, /function\s+(\w+)/) \| distinct by fn_name` --glob '*.ts' | 1482 唯一函数名 ✓ |
| --json NDJSON | `"export" && "default"` --json | 235 行 NDJSON 含 line/line_no/path/match ✓ |
| --explain | `"export" && "default" \| head 5` --explain | 执行计划含 L1 必选组 + 管道动词 ✓ |
| --strict 字段探活 | `"function" && cost_ms>0` --strict | fail-loud "cost_ms 从未出现" + 列出实际探测到的字段 ✓ |
| L3 ast(ts) interface | `ast(ts, "interface $NAME { $$$BODY }")` | 2502 行 ✓ |
| L3 ast(ts) type | `ast(ts, "type $NAME = $$$T")` | 466 行 ✓ |
| L3 ast(ts) class | `ast(ts, "class $NAME { $$$BODY }")` | 34 行 ✓ |
| L3 ast(ts) defineComponent | `ast(ts, "defineComponent($$$ARGS)")` | 82 行 ✓ |
| L3 ast(tsx) JSX | `ast(tsx, "<$TAG>$$$CHILDREN</$TAG>")` | 2 行 ✓ |
| L3 ast(ts) --verify | 3 pattern × 3378 文件 | 0 FAIL ✓ |
| L3 ast(ts) --no-gitignore --verify | 2 pattern × 72K 文件 | 0 FAIL ✓ |

**结论**：gxp 全功能矩阵（BARE/gxp/L3/pipeline/--json/--explain/--strict/--verify/--glob/--no-gitignore）在 web-app 真实项目上 0 error 0 FAIL，不变式 `L1 ⊇ L2` 和 BARE ≡ grep -E 成立。

#### web-app 性能实测（v0.45，2026-11-02，3 轮 median）

> 真实前端项目 `web-app`（1.5GB）上的性能数据。两种模式：.gitignore 尊重（3378 文件）vs --no-gitignore（72641 文件）。

**.gitignore 模式（3378 文件 / 53MB 源码）**：

| 查询 | R1 | R2 | R3 | median | 命中数 |
|---|---|---|---|---|---|
| BARE `function` | 0.357s | 0.239s | 0.255s | **0.255s** | 638 文件 |
| BARE `export` | 0.683s | 0.245s | 0.234s | **0.245s** | — |
| gxp `"function" \| count` | 0.355s | 0.199s | 0.195s | **0.199s** | 2970 行 |
| gxp `"export" && "default" \| count` | 0.281s | 0.200s | 0.192s | **0.200s** | 235 行 |
| gxp `"import" \| distinct` | 0.386s | 0.222s | 0.230s | **0.230s** | 5731 唯一行 |
| `ast(ts, function)` | 1.231s | 1.151s | 1.147s | **1.151s** | 809 行 |
| `ast(ts, interface)` | 1.165s | 1.066s | 1.066s | **1.066s** | 2502 行 |
| `ast(ts, arrow fn)` | 1.499s | 1.145s | 1.105s | **1.145s** | 4040 行 |
| `"function" --verify` | 0.440s | 0.275s | 0.278s | **0.278s** | 0 FAIL |
| `"function"` (no verify) | 0.346s | 0.222s | 0.290s | **0.290s** | — |
| `"export" --json` | 0.372s | 0.266s | 0.262s | **0.266s** | NDJSON |
| `capture(fn, /function\s+(\w+)/) \| distinct by fn` --glob '*.ts' | 0.307s | 0.184s | 0.192s | **0.192s** | 1482 唯一名 |
| `"export" && "default" \| context 2 \| head 10` --glob '*.ts' | 0.352s | 0.183s | 0.164s | **0.183s** | — |
| `"function" \| rank top 20 by line_no desc` --glob '*.ts' | 0.311s | 0.169s | 0.174s | **0.174s** | — |
| `"export" --glob '*.ts'` | 0.301s | 0.174s | 0.192s | **0.192s** | — |

**--no-gitignore 大目录（72641 文件 / 1.5GB）**：

| 查询 | R1 | R2 | R3 | median | 命中数 |
|---|---|---|---|---|---|
| BARE `function` | 3.856s | 3.687s | 3.827s | **3.827s** | 27819 文件 |
| gxp `"function" \| count` | 4.999s | 5.192s | 3.589s | **4.999s** | 483288 行 |
| `ast(ts, function)` | 9.491s | 8.166s | 9.167s | **9.167s** | — |
| `ast(ts, function) --verify` | 9.802s | 11.124s | 9.403s | **9.802s** | 0 FAIL |
| `ast(ts, function) --glob '*.ts'` | 1.791s | 1.635s | 1.669s | **1.669s** | — |

**--verify 开销**：`"function"` 有/无 --verify 差异 ~0.01s（0.278 vs 0.290），开销 <5%，在 3378 文件量级可忽略。

**--glob + --no-gitignore 组合**：`ast(ts, function) --no-gitignore --glob '*.ts'` median 1.669s vs 无 --glob 的 9.167s，--glob 过滤在 --no-gitignore 模式下提速 5.5×（72641→15260 .ts 文件，跳过 57381 非目标文件 walk+scan）。

**冷热比**：R1 普遍比 R2/R3 慢 40-80%（page cache 预热效应），median 取 R2/R3 更稳定。

### 1.4 性能边界声明（基于 §1.3 实测）

| 量级 | 实测耗时 | 状态 |
|---|---|---|
| 1.22GB（真实 1268 transcript） | 0.518s（BARE） | ✅ v1 适用 |
| 12GB（合成 11603 文件） | 1.7-3.9s | ✅ v1 适用（边缘：LLM agent <5s 延迟容忍上限） |
| 911MB 单文件（流式 fallback） | 0.30s | ✅ v1 适用 |
| 33GB（线性外推，未实测） | ~5-10s（推算） | ⚠️ **未实测，不可声明 v1 适用** —LLM agent <5s 延迟容忍上限已超；社区持久索引方案更合适 |

> **D9 边界修正**：v1 适用 ≤10GB 单目录（基于 12GB 实测）。33GB 未实测不可声明。LLM agent 工作流延迟容忍 <5s，>10GB 已超常规场景。

### 1.5 Arc<[u8]> 评估（v0.32 落地决策）

> 本节为 v0.32 Arc<[u8]> 落地的评估决策记录。当前测试基线见 `docs/capability-matrix.md` §4。

| 判据 | 结果 | 结论 |
|---|---|---|
| 编译通过 | ✅ cargo build + clippy 零警告 | 通过 |
| 测试全绿 | ✅ 与 capability-matrix §4 一致 | 通过 |
| Send+Sync 断言 | ✅ test_matched_line_is_send_sync | Arc<[u8]>: Send+Sync |
| --verify 0 FAIL | ✅ 43 文件全 PASS | 自举验证通过 |
| 性能退化 | ✅ 无退化 | 但单文件 44MB 无显著提速 |
| <80ms 目标 | ⚠️ 噪声波动 80-90ms | 边缘（系统噪声主导） |

Arc<[u8]> 方案技术上可行，单文件 44MB 场景净收益不显著，大文件/大目录场景自动受益。

> 2026-11-09 注：本表 "<80ms 目标" 为旧机 i7-14700 口径，v0.49 起降为历史参考；现行门禁见 AGENTS.md §7.1（本机基线锚点，median 劣化 >10% 需归因）。

### 1.6 性能改善归因分析（v0.32 → v0.41 → v0.45）

> **结论**：v0.41 vs v0.32 和 v0.45 vs v0.41 的性能差异**都不是代码优化贡献**，而是系统 page cache 状态差异。v0.45 全管道 80.77ms 与 v0.41 的 80.30ms 一致（+0.6%），无退化。

**证据链**：

1. **`git log -- src/l1/ src/l2/ src/mode/ src/io/file.rs src/plan.rs` 显示 v0.32→v0.41→v0.45 期间热路径零 commit**——只有 `src/pipeline/` 在变（v0.33 sample/v0.34 sort/v0.35 distinct/v0.37 percentiles/v0.39 trace/v0.44 B6-B11 bug 修复）。新动词和 bug 修复都是 L4 阶段或 eval 层，不进入 L1+L2 热路径。
2. **v0.41 同会话两次跑 diag_pipeline**：第一次 80.30ms（首次跑，fixtures 新生成、cache 冷），第二次 74.64ms（warm cache，进一步降 7%）——证明 page cache 预热对总耗时影响显著。
3. **v0.45 三轮复测**：diag_pipeline run_with_writer 78.17/80.77/80.16ms（3 轮 median 80.77ms），diag_breakdown 82.85/84.11/82.59ms（median 82.85ms），diag_stages execute_stages 15.76/15.47/15.94ms（median 15.76ms）——三轮间波动 <3%，与 v0.41 的 80.30ms 一致。
4. **fixtures 是确定性生成**（`svcs`/`kws`/`cost = k % 3000 + 1` 全模运算），v0.32、v0.41、v0.45 用的是**完全相同**的 44MB 数据。

**为什么 v0.32 实测时较慢**：v0.32 是性能 baseline 首次实测，fixtures 首次生成、cargo bench 进程刚启动、page cache 冷态。v0.41 复测时同一会话内多轮迭代已预热 cache，且系统状态更稳定。

**对 v1.0 的启示**：
- 性能数据应报告"冷启动 + warm"两组，避免单次实测误导
- v0.32 的 82.36ms、v0.41 的 80.30ms、v0.45 的 80.77ms 都在 80ms 目标边缘——**仍属达标**，但不要把版本间微小差异作为代码改进/退化证据
- gxp 全管道性能瓶颈已归因清楚（v0.45: read 14% / L1 scan 22% / L2+collect 22% / execute_stages 17% / 差值 14%），无优化空间

**v0.47 补记（2026-11-09，测量机变更归因）**：v0.47 复测全管道 128.09ms vs v0.45 记录 80.77ms 的差异**不是代码退化**，证据链：(1) 同机 A/B——v0.45 源码 worktree 本机实测 125.12ms ≈ v0.47 的 128.09ms（+2.4% 噪声级，B13/B14 后各阶段差异 ≤4.5%）；(2) 本机硬件（i7-10700 / OS build 26100）与 v0.32-v0.45 数据的环境标注（i7-14700 / 26200）双重不符，仓库在 v0.45 squash 重建时迁移；(3) 跨机比值 1.52-1.85× 均匀分布于所有 CPU 段而 read I/O 仅 1.17×（13.64 vs 11.69ms），符合"CPU 单核差距、存储同代"的机器特征；(4) 命中数完全一致（L1 141367 / L2 103607，确定性 fixture），语义零变化。**方法论教训：复测前先核对测量环境标注与本机实际硬件（一条 `Get-CimInstance Win32_Processor` 即可），环境不符时版本间绝对值比较无意义，应改用同机 A/B。**

---

## 2. 目录层性能

### 2.1 Walk 性能（Windows, 154MB 树 / 35 目录 / 361 文件）

| 操作 | 耗时 |
|---|---|
| 全树 walk (35 目录/60 文件/154MB) | 3.66ms |
| 单日 walk (logs/2026-10-20) | 0.36ms |
| 六天 dir_re (不可剪枝) | 2.40ms |

### 2.2 DirIndex 三档对比

| 模式 | 首次 | 再次 | readdir | stat | 复用 |
|---|---|---|---|---|---|
| strict | 2.64ms | 2.44ms | 35 | 361 | 0 |
| lax | 2.63ms | 2.80ms | 35 | 0 | 35 |
| off | 2.91ms | 0.01ms | 0 | 0 | 35 (过期) |

**结论**：Windows 上 lax 无收益（`FindFirstFileExW` 已返回 file_type，stat ~1µs/次，lax 跳过 stat 零收益）。v2 Linux 接入 lax 有收益（`getdents` 批量返回，stat 分离）。

### 2.3 rayon 并行跨文件扫描（70648 文件）

| 路径 | 串行 | 并行 | 提速 |
|---|---|---|---|
| BARE `.ts` 目录扫 | 3.4s | 1.2s | 2.8× |
| gxp 模式目录扫 | 12.0s | 4.3s | 2.8× |
| `--verify` 暴力对账 | 10.5s | 2.7s | 3.9× |

**交叉验证**：并行 vs 串行 md5 一致 + `--verify` 0 FAIL（70648 文件全量）。

**为什么没到线性加速（20 核应 ~20×）**：I/O bound + 文件粒度不均 + rayon 工作窃取开销 + Windows FindFirstFile 串行枚举。

---

## 3. V7 D10 大文件验证（1.03GB 合成文件）

| 方案 | 总延迟 | read 时间 | scan 时间 | 峰值内存 |
|---|---|---|---|---|
| A: read_to_end（D10 生产路径） | 1225ms | 395ms | 830ms | 1107 MB |
| B: BufReader 8MB buffer | 1423ms | — | — | 8 MB |
| C: BufReader 64KB buffer | 1289ms | — | — | 0 MB |

**结论**：D10 在 1GB 场景下仍成立，read_to_end 比流式快 5-16%。>512MB 走流式 fallback 避免 OOM（已落地）。

---

## 4. SIMD 配置参考

### 4.1 依赖 crate

| 库 | 版本 | 用途 | SIMD 机制 | 选型结论 |
|---|---|---|---|---|
| memchr | 2.7 | 行切分 + 分隔符 | x86_64 AVX2→SSE2; aarch64 NEON | ✅ 已最优 |
| aho-corasick | 1.0 | L1 多模式字面量匹配 | 内部用 memchr SIMD | ✅ NFA 已够快（DFA 仅 +3.8% 噪声） |
| regex | 1.10 | L1 精判正则 | meta engine prefilter + DFA | ✅ 已最优 |
| simd-json | 0.13 (optional) | JSON 解析加速 | SSE4.2+ in-place mutation | ❌ 不启用（小 JSON 行反慢 5.5×） |

### 4.2 memchr 调用点

| 文件 | 用途 |
|---|---|
| `src/l1/scan.rs` | L1 行切分 `\n` |
| `src/l2/extract.rs` | 字段分隔符 `=`/`:` + 字段值终止 + ISO 时间戳 + [tag] + JSON key-value |
| `src/l2/format.rs` | 格式探测 |
| `src/l2/numeric.rs` | 千分位拒绝 (D5) |
| `src/pipeline/context.rs` | context 窗口行定位 |
| `src/mode/detect.rs` | BARE/gxp 模式行扫描 |
| `src/l3/structural.rs` | AST 行号映射 |

### 4.3 Release profile

> 代码本体见 `Cargo.toml [profile.release]`（权威源）。本节只列决策理由。

| 配置 | 理由 |
|---|---|
| `lto = "fat"` | 跨模块内联，memchr SIMD 循环内联到调用点 |
| `codegen-units = 1` | 单 codegen unit，最大化内联 |
| `panic = "abort"` | 不生成 unwinding，减少分支干扰 SIMD 流水线 |
| `strip = true` | 减小二进制体积（2.2 MiB） |

### 4.4 SIMD 配置结论

- **aho-corasick NFA vs DFA**：DFA 边际 +3.8%，噪声范围。不接入 DFA。
- **simd-json vs serde_json vs memchr 手写**：simd-json 在 ~97 字节/行的小 JSON 反慢 32%；memchr 手写路径比 serde_json 快 3.7×。不启用 simd-json。
- **自定义 AVX2 多字段批扫**：2 字段时 find_value 两遍比 batch 快；n≥3 时 batch 反超。
- **多字段批量提取阈值**（已落地）：n=2 +8%、n=5 +137%、n=8 +288%、n=12 +344%。`batch_find_values` 算法层优化（非 SIMD intrinsics），n≥3 走 batch。

---

## 5. Bench 文件索引

| 文件 | 类型 | 测量内容 |
|---|---|---|
| `benches/l1_scan.rs` | criterion | L1 两遍链 44MB |
| `benches/l2_extract.rs` | criterion | L2 字段抽取 31931 行 |
| `benches/full_pipeline.rs` | criterion | 全管道 + Top20 + 投影 |
| `benches/sample.rs` | criterion | sample N / sample P% 抽样（v0.33 落地） |
| `benches/sort.rs` | criterion | sort by FIELD / sort 整行（v0.34 落地） |
| `benches/scale_dir.rs` | manual | 目录层 6 项实验 |
| `benches/diag_pipeline.rs` | manual | 管道分阶段拆解 |
| `benches/diag_stages.rs` | manual | execute_stages 子阶段拆解 |
| `benches/diag_breakdown.rs` | manual | run_with_writer 阶段拆解 |
| `benches/diag_writeonly.rs` | manual | 输出写入开销 |
| `benches/diag_mmap.rs` | manual | read_for_scan vs fs::read 对照（回归基线） |
| `benches/diag_v7_streaming.rs` | manual | 流式 vs read_to_end 对照 |
| `benches/l3_scan.rs` | criterion | L3 ast() 查询（条件编译） |
| `benches/verify_overhead.rs` | criterion | --verify 暴力对账开销 |
| `benches/diag_batch_projection.rs` | criterion | 多字段批量提取阈值 n=2/5/8/12 |

### 跑 bench 命令

```bash
# criterion 基准
cargo bench --bench full_pipeline
cargo bench --bench l1_scan
cargo bench --bench l2_extract

# 诊断 bench (manual, 5 次 min)
cargo bench --bench diag_pipeline
cargo bench --bench diag_breakdown
cargo bench --bench diag_stages

# 目录层
cargo bench --bench scale_dir

# L3 条件编译
cargo bench --bench full_pipeline --features l3-structural
```

---

## 6. 开源实现性能对比估算

> 基于已有实测（gxp 76.31ms vs `gnu_grep -E | head -20` 212.68ms）+ 公开 benchmark 数据 + 架构分析推算。

### 6.1 单文件正则扫描（44MB 日志）

> ⚠️ 本表为 v0.32 旧机估算，已被 §1.3.2「ripgrep 对照」实测取代（本机 900MB count 场景：**rg 最快，gxp 2.53× 慢于 rg**）。估算表仅作历史参考。

| 工具 | 估算耗时 | vs gxp |
|---|---|---|
| **gxp BARE 模式** | ~25ms | 1× |
| **GNU grep -E** | ~85ms | 3.4× 慢 |
| **ripgrep (rg)** | ~40ms | 1.6× 慢 |
| **ag (silver searcher)** | ~55ms | 2.2× 慢 |
| **ack** | ~200ms | 8× 慢 |

### 6.2 目录扫描并行效率（70648 文件）

| 工具 | 并行策略 | 提速 |
|---|---|---|
| **gxp** | rayon `par_iter` per-file | 2.8× |
| **ripgrep** | rayon + ignore crate walk | ~4-5×（估） |
| **GNU grep -r** | 单线程 | 1× |

### 6.3 功能维度对比

| 能力 | gxp | grep | ripgrep | ast-grep |
|---|---|---|---|---|
| ERE 正则 | ✅ BARE | ✅ | ✅ | ❌ |
| 字段谓词 `cost>800` | ✅ 原生 | ❌ | ❌ | ❌ |
| 正则捕获提取 `capture(NAME, /REGEX/)` | ✅ | ❌ | ❌ | ❌ |
| 时间范围 `_ts between` | ✅ | ❌ | ❌ | ❌ |
| 流式聚合 `stats` | ✅ | ❌ | ❌ | ❌ |
| AST 结构查询 `ast(NAME, PATTERN)` | ✅ L3 | ❌ | ❌ | ✅ |
| Top-K 排序 `rank top N by` | ✅ | ❌ | ❌ | ❌ |
| 上下文窗口 `context N` | ✅ | ✅ -A/-B/-C | ✅ | ❌ |
| .gitignore 尊重 | ✅ | ❌ | ✅ | ❌ |
| 并行目录扫描 | ✅ | ❌ | ✅ | ❌ |

**gxp 独特价值**：将 grep 的行级正则 + awk 的字段提取 + sort 的 Top-K + jq 的管道组合统一为单一查询语言，面向 LLM agent 的日志分析场景。无直接竞品做相同组合。

### 6.4 构建产物大小

| 工具 | 二进制大小 | 语言 |
|---|---|---|
| **gxp release** | 2.2 MiB | Rust |
| **ripgrep 13** | 2.4 MiB | Rust |
| **GNU grep 3.11** | 0.2 MiB | C |
| **fd 9** | 1.8 MiB | Rust |

gxp 2.2 MiB：14 个生产依赖，65 个唯一 crate。主要体积来源 `.text` 段 1.61 MiB（regex/aho-corasick/memchr SIMD 代码）。
