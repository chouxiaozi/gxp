# gxp 场景查询模板（A-H）

> 来源：AGENTS.md §0.2 LLM agent 工作流场景清单 + design §2.4 示例
> 覆盖 8 个一等公民场景

## A. 错误诊断

找 ERROR 行 + 上下文 + 异常聚合 + 完整 stack trace。

```bash
# ERROR 行 + 上下文
gxp 'ERROR | context 3' app.log                       # ≡ grep -C 3 ERROR

# 非对称上下文
gxp 'ERROR | context before:1 after:5' app.log       # ≡ grep -B 1 -A 5 ERROR

# 异常聚合（按 _tag 分组计数）
gxp 'ERROR | group by _tag' app.log                  # 输出 checkout: 42, billing: 17, ...

# 字段 + 上下文 + 投影组合
gxp 'cost_ms>800 | context 5 | fields _ts,_tag,cost_ms' app.log

# 完整 stack trace（6 语言 + 自动检测）
gxp 'panic | trace' app.log                          # 自动检测语言
gxp 'Exception | trace java' app.log                 # 显式 java
gxp 'Traceback | trace python' app.log               # 显式 python（或 py）
gxp 'panic | trace rust' app.log                     # 显式 rust（或 rs）
gxp 'throw | trace js' app.log                       # 显式 js（或 javascript）
gxp 'panic | trace go' app.log                        # 显式 go（或 golang）
gxp 'Exception | trace csharp' app.log               # 显式 csharp（或 cs / c#）

# JSON 模式（trace 续行进 extra_lines 数组）
gxp --json 'panic | trace' app.log

# 源码层错误诊断：Rust unwrap() 集中点
gxp 'ast(rs, "$OBJ.unwrap()")' src/
```

## B. 性能分析

慢查询定位 + SLO 长尾指标。

```bash
# Top-K 慢查询
gxp '"timeout" && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms' app.log

# SLO 长尾百分位
gxp 'cost_ms exists | percentiles cost_ms' app.log                    # 默认 p50,p95,p99
gxp 'cost_ms exists | percentiles cost_ms p90,p99,p999' app.log      # 自定义

# 流式聚合（min/max/avg）
gxp 'cost_ms>800 | stats cost_ms' app.log             # 输出 min/max/avg

# 组合：过滤 + Top-K + 投影
gxp '_tag in [checkout,billing] && cost_ms>500 | rank top 10 by cost_ms desc | fields _ts,_tag,cost_ms'
```

**stats vs percentiles 区别**：

- **stats FIELD**：流式 O(1) 内存，输出 min/max/avg。适合"快速看一眼整体水位"。
- **percentiles FIELD [p50,p95,p99]**：非流式 O(N) 全量排序，输出百分位。SLO 报告用（长尾指标必须 percentiles 而非 avg）。

## C. 代码理解

找某 API 调用 + AST 结构模式。

```bash
# TypeScript：找所有 useQuery 调用
gxp 'ast(ts, "useQuery($$$A)")' src/

# 单参数 useState 计数
gxp 'ast(ts, "useState($A)") | count' src/

# Rust：所有 .unwrap() 集中点
gxp 'ast(rs, "$OBJ.unwrap()")' src/

# Rust：println! 宏调用
gxp 'ast(rs, "println!($$$A)")' src/

# Python：requests.get 调用
gxp 'ast(py, "requests.get($$$A)")' src/

# JavaScript：fetch 调用
gxp 'ast(js, "fetch($$$A)")' src/

# Go：main 函数声明
gxp 'ast(go, "func main() { $$$B }")' src/

# Java：System.out.println 调用
gxp 'ast(java, "System.out.println($$$A)")' src/

# C：return 语句
gxp 'ast(c, "return $X;")' src/

# C++：namespace 声明
gxp 'ast(cpp, "namespace $NS { $$$B }")' src/

# 行内 + AST 组合
gxp '"hello" && ast(rs, "println!($$$A)")' src/      # 行含 hello 且是 println! 宏
```

**ast-grep 模式**：`$VAR` 单节点 / `$$$VAR` 多节点。字面量按 AST node 匹配（非 ERE）。

## D. 日志探查

时间窗口 + trace_id 追踪。

```bash
# 时间窗口（_ts 伪字段 + ISO 8601）
gxp '_ts between "2026-10-27T10:00:00Z" and "2026-10-27T11:00:00Z" | fields _ts' app.log

# 带时区偏移
gxp '_ts between "2026-10-27T10:00:00+08:00" and "2026-10-27T11:00:00+08:00" && cost_ms>800' app.log

# trace_id 追踪（正则捕获组）
gxp 'capture(trace_id, /req-\d+/) | fields trace_id' app.log

# ERROR 行提取 user_id
gxp '"ERROR" && capture(user_id, /uid=\d+/) | fields user_id' app.log

# trace_id + 时间窗口组合
gxp '_ts between "2026-10-27T10:00:00Z" and "2026-10-27T11:00:00Z" && capture(trace_id, /req-\d{8}/) | fields _ts,trace_id' app.log
```

## E. 文件清单

哪些文件含某模式。

```bash
# 列含 ERROR 的文件（≡ grep -l）
gxp 'ERROR *.log | files'

# 列不含 ERROR 的文件（≡ grep -L）
gxp 'ERROR *.log | files --invert'

# 配合 --glob 过滤
gxp 'TODO | files --glob "*.rs"'

# 覆盖默认 .gitignore
gxp 'TODO | files --no-gitignore'
```

## F. 抽样审计

大结果集代表性采样（context window 物理限制）。

```bash
# 固定数量样本
gxp 'ERROR | sample 50' app.log              # 50 条随机样本（固定种子 0，可复现）

# 百分比样本
gxp 'ERROR | sample 10%' app.log             # 10% 命中行（ceil(len*0.10)）

# 与 head 区别
gxp 'ERROR | head 50' app.log               # 取前 50 条（漏后段）
gxp 'ERROR | sample 50' app.log              # 全局洗牌后取 50（代表性）

# 大目录采样（多文件聚合后抽样）
gxp 'ERROR | sample 100' *.log

# 抽样 + 排序组合
gxp 'ERROR | sample 100 | sort by _ts' app.log
```

## G. 去重列表

唯一错误/唯一用户/唯一 trace。

> ⚠️ **谓词先行**：纯 stage 查询（`distinct by trace_id` 单独出现）会被 BARE 模式吞掉当 ERE 搜文件——查询必须以谓词开头（下例均含谓词，安全）。

```bash
# 整行去重（相同错误只留首条）
gxp 'ERROR | distinct' app.log

# 按字段去重
gxp 'ERROR | distinct by trace_id' app.log    # 每个 trace 只留首条

# 聚集相同错误 + 去重（= sort | uniq）
gxp 'ERROR | sort | distinct' app.log

# 去重 + 排序组合
gxp 'ERROR | distinct by user | sort by cost desc' app.log

# 字段缺失归入空串键（多条缺失只留首条）
```

**distinct vs group by vs sort**：
- `distinct`：输出去重后的行（保留首次出现顺序）
- `group by`：输出 `value: N`（计数）
- `sort`：改变顺序（聚集相同键相邻，不删除）

## H. 全量排序

全量有序输出（rank 之外的场景）。

```bash
# 按字段升序（默认 asc，数值优先）
gxp 'cost_ms>500 | sort by cost_ms' app.log

# 按字段降序
gxp 'cost_ms>500 | sort by cost_ms desc' app.log

# 按整行字节序（聚集相同错误）
gxp 'ERROR | sort' app.log

# 整行降序
gxp 'ERROR | sort desc' app.log

# 多字段组合：先去重再排序
gxp 'ERROR | distinct by user | sort by cost desc' app.log
```

**sort vs rank**：
- `sort`：全量有序（默认 asc，O(N log N)）
- `rank top K`：只取 Top-K（默认 desc，partial-sort O(N log K)）

## 跨场景组合

```bash
# 时间窗口 + Top-K + 投影（D + B + 投影）
gxp '_ts between "T1" and "T2" && cost_ms>500 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms' app.log

# ERROR + 上下文 + trace（A 错误诊断全链路）
gxp 'panic | context 1 | trace rust | fields _ts,extra_lines' app.log

# 多文件聚合 + 抽样 + 排序（E + F + H）
gxp 'ERROR | sample 100 | sort by _ts' *.log
```
