# gxp 语法速查

> 来源：AGENTS.md §2 语法锚点 + capability-matrix §1 + design §2.4 示例
> 更新：2026-11-09（v0.48 全量）

## BARE 模式（与 `grep -E` 等价）

```bash
gxp 'ERROR' app.log                          # ≡ grep -E 'ERROR' app.log
gxp 'ERROR timeout' app.log                  # ≡ grep -E 'ERROR timeout'（连续子串）
gxp 'a|b' app.log                            # ERE 的 OR
```

**不支持 grep 短选项**（D1 硬约束）：`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r`/`--color`/`-q`/`-s`。等价功能走 gxp 原生语法。

**纯 stage 查询会被 BARE 模式吞掉**：查询只含管道动词（如 `distinct by trace_id`、`count`）时被判为 BARE 模式，当成 ERE 字面量去搜文件（0 命中、退出码 1）——**谓词先行**：`trace_id exists | distinct by trace_id`、`"ERROR" | count`。

## gxp DSL 谓词

| 类别 | 语法 | 示例 |
|---|---|---|
| 字面量（STRING，非 ERE） | `"..."` | `"error"` / `"cost>800"`（字面 `>`） |
| 正则字面量 | `/.../` | `/req-\d+/` |
| 数值比较 | `field op N` | `cost_ms>800` / `latency<=50` / `count==0` / `status!=200` |
| 集合成员 | `field in [a,b,c]` | `_tag in [checkout,billing]` |
| 字符串等值（v0.48） | `field="VALUE"` / `field=="VALUE"` | `level="ERROR"`（解析期脱糖单元素 In；`!=`/`>` 带引号 fail-loud） |
| 区间 | `field between X and Y` | `_ts between "T1" and "T2"` / `cost_ms between 100 and 500` |
| 字段存在 | `field exists` | `cost_ms exists` |
| 逻辑 | `&&` / `||` / `!` | `"error" && !timeout` / `!(cost>800)` |
| 优先级 | 括号 | `("a" || "b") && cost>10` |
| 正则捕获 | `capture(NAME, /REGEX/)` | `capture(trace_id, /req-\d+/) \| fields trace_id` |

**11 维数值字段格式契约（D5）**：支持整数/浮点/负数/前导零/正号/科学计数法；拒绝千分位/单位/空白 trim/布尔/NaN。

**`|` 歧义规则**：
- 单 `|` 两侧空白/边界 → 管道
- 紧贴字母（`a|b`）→ ERE OR
- `||` → 逻辑 OR（恒定）

**字面 `>` 表达**：`"cost>800"`（STRING）/ `/cost>800/`（REGEX）/ `cost\>800`（ERE 转义）。

## 伪字段（无需 L2 字段抽取）

| 字段 | 含义 | 示例 |
|---|---|---|
| `_ts` | 行首 ISO 8601 时间戳 | `_ts between "T1" and "T2"` |
| `_tag` | 行内 `[tag]` 标记 | `_tag in [checkout,billing]` |

## 管道动词（v0.48 全量 14 个）

| 动词 | 语义 | 等价 grep / 备注 |
|---|---|---|
| `where PRED` | 二次过滤（管道后谓词） | — |
| `context N` | 上下文窗口（前后 N 行） | ≡ `grep -C N` |
| `context before:A after:B` | 非对称上下文 | ≡ `grep -B A -A B` |
| `count [by FIELD]` | 行数（可按字段分组） | ≡ `grep -c`（单文件） |
| `group by FIELD` | 按字段分组计数 | 输出 `value: N` |
| `rank top K by FIELD [desc\|asc]` | Top-K（partial-sort） | 默认 desc |
| `sort [by FIELD] [desc\|asc]` | 全量稳定排序 | 默认 asc；对齐 SQL ORDER BY |
| `distinct [by FIELD]` | 稳定去重（保留首次出现） | 对齐 SQL DISTINCT |
| `head N` | 取前 N 行 | ≡ `head -n N` |
| `sample N` / `sample P%` | 固定种子 0 随机抽样 | LLM context window 物理限制 |
| `fields F1,F2,...` | 投影（输出指定字段） | — |
| `files [--invert]` | 列文件名 | ≡ `grep -l` / `grep -L`（--invert） |
| `stats FIELD` | 流式聚合 min/max/avg | O(1) 内存 |
| `percentiles FIELD [p50,p95,p99]` | 非流式 nearest-rank 百分位 | O(N) 内存 + 排序 |
| `trace [LANG]` | 多行 stack trace 捕获 | 6 语言 + 自动检测 |

**终止 stage**（输出摘要后链终止）：`count` / `group by` / `stats` / `percentiles` / `trace`。终止 stage 后置动词 fail-loud。

## AST 结构查询

```bash
gxp 'ast(LANG, "PATTERN")' <dir>
```

**前置条件**：ast() 由 `l3-structural` feature 提供，**自 v0.49.0 起默认启用**——预编译二进制与 `cargo install gxp` 开箱可用；仅 `--no-default-features` 裁剪构建不含（遇 `ast()` fail-loud 提示重建）；查询侧没有 `--features` 旗标。

**支持语言**（v0.48 全量 9 种）：`ts` / `rs` / `py` / `js` / `go` / `tsx` / `java` / `c` / `cpp`。大小写不敏感 + 别名（`c++`/`cxx`/`cc` → `cpp`）。

**ast-grep 模式语法**：
- `$VAR` — 单节点捕获
- `$$$VAR` — 多节点捕获（任意参数列表 / 任意语句块）
- 字面量按 AST node 匹配（不是 ERE）

**示例**：
```bash
gxp 'ast(rs, "$OBJ.unwrap()")' src/                 # Rust .unwrap() 调用
gxp 'ast(rs, "println!($$$A)")' src/                # println! 宏调用
gxp 'ast(ts, "useState($A)") | count' src/          # 单参数 useState 计数
gxp 'ast(py, "requests.get($$$A)")' src/             # Python requests.get
gxp 'ast(java, "System.out.println($$$A)")' src/     # Java println
gxp 'ast(c, "return $X;")' src/                       # C return 语句
gxp 'ast(cpp, "namespace $NS { $$$B }")' src/         # C++ namespace 声明
```

## CLI flag

| flag | 作用 |
|---|---|
| `--verify` | 暴力对账（与优化路径结果 diff，0 FAIL 才可信） |
| `--explain` | 执行计划（文本 + JSON） |
| `--json` | NDJSON 输出（每行含 path/line_no/line/captures 字段） |
| `--strict` | 字段探活 fail-loud（写错字段名直接报错） |
| `--glob PATTERN` | 文件名过滤（如 `--glob '*.rs'`） |
| `--no-gitignore` | 覆盖默认 .gitignore 尊重 |
| `--query-file PATH` | 查询从文件读取（v0.50，免 shell 引号层；单查询单行；与位置 pattern 互斥） |
| `--generate-completion SHELL` | shell completion（bash/zsh/fish/ps） |

## Windows PowerShell 引号（高危坑，v0.50 实测矩阵）

> 实测环境：pwsh 7.6.9（`PSNativeCommandArgumentPassing=Windows`）/ Windows PowerShell 5.1.26100 / Git-Bash 类单层引号 shell；gxp 0.49+，2026-11-11。shell 行为会随版本变化——异常时先用 `--explain` 自检（见下）。

查询含**内层双引号**（`level="ERROR"`、`ast(rs, "PATTERN")`）时，PowerShell 的正确习惯在 5.1 与 7+ 之间**互为颠倒**：

| 写法 | pwsh 7.3+ | PS 5.1 | bash 类 |
|---|---|---|---|
| `gxp 'level="ERROR" \| count' f`（单引号+裸内层引号） | ✅ | ❌ 引号被吃 → 静默退化 | ✅ |
| `gxp 'level=\"ERROR\" \| count' f`（`\\"` 转义） | ❌ 反斜杠入 pattern → **0 命中 exit 0 无警告** | ✅（恰好被还原） | ❌ 同 pwsh |
| `gxp --% "level=\"ERROR\" \| count" f` | ❌（脚本内 argv 碎裂） | ✅ | — |
| `cmd /c 'gxp "level=\"ERROR\" \| count" f'` | ✅ | ✅ | — |
| 查询写入文件 + `gxp --query-file q.gxp f` | ✅ | ✅ | ✅ |

**规则**：
1. **首选 Bash 等单层引号 shell**（Claude Code 用 Bash 工具跑 gxp 含引号查询）。
2. pwsh 7.3+：单引号包裹 + 裸内层 `"` 即可；**永远不要写 `\"`**（仅 `cmd /c` 内层合法）。
3. PS 5.1 / 不确定版本：`cmd /c` 包装，或查询写入文件走 `--query-file`（彻底绕开引号层，agent 用 Write 工具落盘即用）。
4. `ast()`/`capture()` 被剥引号时会 fail-loud（多文件参数错 / ast 语法错），v0.50 起错误文案含 shell 根因提示。

**自检协议（引号是否完整到达解析器）**：给查询挂 `--explain`——
- ✅ 正确：`L2 字段谓词:` 下出现 `level in [ERROR]`
- ❌ 引号被剥：`L1 必选组:` 下出现字面量 `"level=ERROR"`（且 v0.50 起渲染 `bare 字面量含 '='` 提示行）
- 注意：gxp 0 命中恒 exit 0（无区分信号），**不要**用退出码判断。

## 输出字段（JSON 模式自包含）

```json
{"path":"app.log","line_no":42,"line":"...","captures":{"trace_id":"req-123"}}
```

终止 stage 输出形态：
- `count` / `group by`：`value: N`
- `stats`：`min: X, max: Y, avg: Z`
- `percentiles`：`p50: X, p95: Y, p99: Z`
- `trace`：每行 `  → ` 前缀（文本）/ `extra_lines` 数组（JSON）

## fail-loud 行为

| 错误 | 行为 |
|---|---|
| grep 短选项（如 `-A 3`） | 四段式报错：当前意图 / 不接受原因 / 等价 gxp 语法 / 示例 |
| 字段写错（`--strict`） | 字段从未出现 + 实际探测到的字段清单 |
| ast() 解析失败 | `L3Error::PatternParse` + 修正建议 |
| 终止 stage 后置动词 | fail-loud（终止 stage 后不能再接管道） |
| 流式路径用 trace | 提示用 `context N` 替代（跨行边界判定需"看到下一行"） |
