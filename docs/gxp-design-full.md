# gxp: grep-aligned 查询语言设计文档

**版本**: 见 `docs/changelog.md` 最新版本（v0.38 文档治理收尾）；正文各节内嵌日期为对应章节落地日（历史性，不更新）
**状态**: 设计完成，Rust 实现验证  
**作者**: AI-assisted design

---

## 0. 执行摘要

gxp 是一个面向 LLM Agent 的文本查询工具。**不是 grep 的严格超集**——支持 grep 的 ERE 语法作为 BARE 模式，但不接受 grep 的短选项参数（`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c` 等）；等价功能用 gxp 原生语法表达（`||` / `!` / `context N` / `fields` / `files` / `count`）。核心思想：**对齐 LLM 预训练分布（grep ERE + shell 操作符），在 grep 表达力之上增强字段谓词、数值比较、Top-K 投影，而非追求形式上的超集**。

**关键成果**：
- 语法：grep ERE 兼容（BARE 模式）+ shell 符号布尔（`&&`/`||`/`!`）+ 字段谓词 + 结构化匹配 + 管道投影（含 `context N` 上下文行）；无隐式 AND，邻接即字面空格
- 性能：详见 `docs/performance-baseline.md`（grep vs gxp 实测对比）
- 缓存：零持久索引；v1 只做单次查询内缓存（T0/T1）；T2 sidecar / T3 daemon 因 D9（grep 一次性命令模型）按需评估启动（详见 `docs/rust-implementation-plan.md §11.3.2`）
- 实现：Rust（详见 `docs/rust-implementation-plan.md`）；穷举验证数值谓词语义正确性（0 错误）

**核心约束**：
- 不换数据库（用户明确要求）
- 不持久索引（缓存做不到就内存处理）
- ERE 兼容（`gxp 'pattern' file` 与 `grep -E 'pattern' file` 在 BARE 模式下行为一致）
- **不是 grep 超集**：不接受 grep 短选项参数；等价功能用 gxp 原生语法表达

---

## 1. 背景与动机

### 1.1 问题陈述

LLM Agent 用 grep 检索文本/日志/代码时面临三个痛点：

1. **Token 爆炸**：grep 返回所有匹配行，无相关度排序，无投影。实测：106k 行匹配 → 310万 token，直接撑爆上下文窗口。
2. **表达力天花板**：grep 只能字节匹配，无法表达字段作用域（`service=checkout`）、数值比较（`cost>800`）、时间区间（`ts between T1 and T2`）。
3. **静默失败**：grep 对不存在的模式返回空结果，LLM 无法区分"数据不存在"和"查询写错了"，只能反复瞎重试。

### 1.2 为什么不用现成方案

| 方案 | 排除原因 |
|---|---|
| ripgrep | 更快但仍无语义、无排序、无字段谓词 |
| ast-grep / semgrep | 只解决代码结构问题，不解决日志/文本流 |
| SQLite FTS5 / DuckDB | 用户明确要求"不换成带索引的数据库" |
| LogQL / KQL | 需要索引后端，不符合"零持久索引"约束 |
| Sourcegraph Code Search | 需要 trigram 索引，同上 |

### 1.3 设计目标

- **精确**：谓词语义完整（字段作用域、数值、时间、布尔组合）
- **快速**：复用 grep 引擎（Boyer-Moore / Aho-Corasick / DFA），不重新发明轮子
- **LLM 友好**：语法基于 grep ERE + shell 符号操作符预训练分布，`--explain` 输出可自我纠错
- **零持久索引**：不落数据库文件；缓存做不到就纯内存单遍

### 1.4 工具定位（诚实声明）

**gxp 不是 grep 的严格超集。** 设计取舍：

1. **支持**：grep -E 的 ERE 语法（BARE 模式）。`gxp 'pattern' file` 与 `grep -E 'pattern' file` 在 BARE 模式下行为一致；含空格的 BARE 匹配连续子串（与 grep -E 一致）。
2. **不接受**：grep 短选项参数（`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r`/`--color`/`-q`/`-s` 等）。出现这些参数时报错并给出 gxp 等价语法提示。
3. **等价 gxp 语法**：

| grep 用法 | gxp 等价 | 备注 |
|---|---|---|
| `grep -e a -e b file` | `gxp 'a \|\| b' file` | ERE 的 OR 已在 BARE 支持 |
| `grep -v PATTERN file` | `gxp '!PATTERN' file` | BARE 模式下 `!` 是谓词级否定，对纯字面匹配等价；对字段谓词有细微语义差异见 §3.4 |
| `grep -A N PATTERN file` | `gxp 'PATTERN \| context after:N' file` | 新增管道动词 |
| `grep -B N PATTERN file` | `gxp 'PATTERN \| context before:N' file` | 新增管道动词 |
| `grep -C N PATTERN file` | `gxp 'PATTERN \| context N' file` | 默认前后各 N |
| `grep -o PATTERN file` | `gxp 'PATTERN \| fields _match' file` | 语义略不同：输出匹配的字段值而非字节范围 |
| `grep -l PATTERN *.log` | `gxp 'PATTERN *.log \| files' file` | 新增管道动词 |
| `grep -L PATTERN *.log` | `gxp 'PATTERN *.log \| files --invert' file` | 新增管道动词 |
| `grep -c PATTERN file` | `gxp 'PATTERN file \| count' file` | 单文件时等价 |
| `grep -r PATTERN dir` | `gxp 'PATTERN dir'` | gxp 默认递归 |
| `grep -i PATTERN file` | `gxp '(?i)PATTERN' file` | ERE 内联标志 |
| `grep -w PATTERN file` | `gxp '\bPATTERN\b' file` | ERE 边界 |
| `grep -x PATTERN file` | `gxp '^PATTERN$' file` | ERE 锚定 |
| `grep --color` | （无等价）| gxp 输出给 LLM，不需要颜色 |
| `grep -q` / `-s` | （无等价）| gxp 总是输出，不提供 quiet/silent |

4. **BARE 里的 `>` `<` 默认是谓词操作符**，不是 ERE 字面。这是与 grep 的**有意分歧**——字段谓词是 gxp 核心功能。匹配字面 `>` 用 STRING（`">"`）或 REGEX（`/\>/`）或转义（`\>`）。

5. **stdin 管道支持**（已落地）：`some_cmd | gxp 'pattern'` 或 `gxp 'pattern' -` 从 stdin 读取输入，与 grep 行为一致。stdin 是"文件参数的流式变体"——全量读入 `Vec<u8>` 后与文件场景走同一 L1/L2/pipeline 链路。`--verify` 在 stdin 下支持（D10 `read_to_end` 模型下可多次扫描）；`--explain` 显示 `<stdin>` 作为输入源；`files` stage 输出 `<stdin>`。stdin 与 file/dir 参数互斥（§11.5.1）。

### 1.5 入口解析与报错

启动时扫描 argv：
- 出现 gxp 专属参数（`--explain`/`--verify`/`--strict`/`--json`）或表达式含 `&&`/`||`/`!`/`>`/`</`/`>=`/`<=`/`==`/`!=`/`[`/`]`/`|`（管道）→ gxp 模式
- 否则 → BARE 模式（ERE 兼容）
- 出现 grep 短选项参数（`-A`/`-B`/`-C`/`-e` 等）→ 报错并给出 gxp 等价语法提示

**位置参数判定**（接入目录参数后适用，详见 Rust 方案 §11.5）：
- `positional[1]` 是文件路径 → 单文件模式
- `positional[1]` 是目录路径（`std::fs::metadata().is_dir()`）→ 目录模式：走 `PathPlan::walk` + `DirIndex` 三档 + 逐文件 L1+L2 扫描 → 聚合到全局 `Vec<MatchedLine>` → `execute_stages_directory`（§7.7.1）
- `positional` 含 `-`（stdin 标记）与其他路径参数同时出现 → 报错（stdin 与 file/dir 互斥，§11.5.1）
- `positional` 仅 1 个元素（pattern）无文件参数 → 隐式 stdin（与 `grep 'pattern'` 行为一致）
- `positional[1]` 是 `-` → 显式 stdin（与 `grep 'pattern' -` 行为一致）
- 目录模式下 `--verify`：逐文件对账（§11.5.2），跨文件 rank Top-K 全局归并（§11.5.3）
- stdin 模式下 `--verify`：与文件场景一致（D10 `read_to_end` 全量读入 Vec 后可多次扫描）

报错模板（四段式）：
```
gxp: 不接受选项 {PARAM}（{原因}）

【当前意图】
  {用户原始命令}

【gxp 不接受的原因】
  {为什么这个参数不接受}

【等价 gxp 语法】
  {转换后的 gxp 命令，含完整示例}

【或直接用 grep】
  {等价 grep 命令}
```

---

## 2. 语法规范

### 2.1 设计原则

1. **复用预训练分布**：LLM 已写过数十亿次 grep/shell，不让它学新语言。语法对齐 grep ERE + shell 符号操作符（`&&`/`||`/`!`），不引入 SQL 的 `and`/`or`/`not` 关键字（避免与 BARE 含空格冲突）。
2. **可下沉优先**：富语义谓词先编译成字节级模式，喂给 grep 引擎。
3. **超集不变式**：`L1(下推) ⊇ L2(精确)`（详见 §3.2）。
4. **大声失败**：引用不存在的字段必须报错并列出真实字段。
5. **引号零转义**：整个表达式用 shell 单引号包，表达式内 STRING 强制双引号——bash/PowerShell 单引号内所有字符字面化，无需转义 `&&`/`||`/`!`/`>`/`(`。LLM 误用未引用的 shell 元字符时由 shell 报错（fail-loud at shell layer）。

### 2.2 EBNF

```ebnf
query    := expr ( '|' stage )*
stage    := 'where' expr
         | 'rank' 'top' INT 'by' KEY ['desc'|'asc']
         | 'head' INT
         | 'sample' INT                                   // 随机抽 N 条（固定种子 0）
         | 'sample' RATIO '%'                             // 按比例抽 ceil(len * R/100) 条
         | 'fields' NAME (',' NAME)*
         | 'count'
         | 'group' 'by' NAME
         | 'context' INT                                  // 前后各 N 行，等价 grep -C N
         | 'context' ('before:' INT) ('after:' INT)      // 分开控制，等价 -B N -A M
         | 'files' ['--invert']                           // 只输出文件名（--invert 反向）
         | 'stats' NAME                                   // 流式聚合 min/max/avg
         | 'percentiles' NAME [PLIST]                     // 非流式百分位（默认 p50,p95,p99；PLIST 形如 p50,p95,p99）
         | 'sort' ['by' KEY] ['desc'|'asc']                // 全量稳定排序（对齐 SQL ORDER BY / Unix sort）
         | 'distinct' ['by' KEY]                          // 稳定去重（保留首次出现，对齐 SQL DISTINCT；不排序——保持原序）
         | 'trace' [LANG]                                 // 多行 stack trace 捕获（6 语言：python/java/js/rust/go/csharp；省略 LANG 自动检测；终止 stage）

expr     := or_expr
or_expr  := and_expr ( '||' and_expr )*
and_expr := unary ( '&&' unary )*           // 邻接不隐含布尔；邻接是 BARE 内的字面空格
unary    := '!' unary | prim               // ! 是操作符当且仅当前后有空白或非字母数字边界（见 §3.4）；作用于 pred 必须用括号
prim     := '(' expr ')'
         | STRING        // "literal"（双引号字面量，单引号禁用——避免与 shell 外层引号冲突）
         | REGEX         // /pattern/
         | BARE          // 裸字面量（ERE 兼容，含空格匹配连续子串；> < >= <= == != 是谓词操作符）
         | ast           // 结构匹配（ast(NAME, STRING) 走 ast-grep，§8.2）
         | capture       // 正则捕获组（capture(NAME, REGEX) 纯 L2 每行提取，不走 L1 下推）
         | pred

ast      := 'ast' '(' NAME ',' STRING ')'     // ast(ts, "useQuery($$$A)")：语言名 + ast-grep pattern
                                              // NAME ∈ {py,js,ts,rs,go,tsx,java,c,cpp,...}（§8.2 层 1/层 2 支持列表）
                                              //   大小写不敏感：Java/JAVA/java 均可；c++/C++/cpp/cxx/cc 均可
                                              //   全拼也可：Python/javascript/TypeScript/golang/rust
                                              // NAME 是 bare identifier（非 STRING 字面量），允许 `+`（c++）
                                              // STRING 是双引号字面量 ast-grep pattern
                                              //   fail-loud：单引号/无引号 pattern 报错（不静默落 BARE）
                                              // L1 不下推（结构层查询无字节级下推），L3 调用 ast-grep 解析 + find_all
                                              // pattern 通配符：$A 单节点匹配；$$$A 任意参数列表
                                              //   例：useQuery($A) 只匹配单参数调用；useQuery($$$A) 匹配任意参数列表
                                              //   Rust method call 必须 $OBJ.unwrap()（有接收者节点）
capture  := 'capture' '(' NAME ',' REGEX ')'    // capture(trace_id, /req-\d+/)：字段名 + 正则
                                              // NAME 是 bare identifier（提取结果的字段名）
                                              // REGEX 是 /pattern/ 形式的正则字面量
                                              // 纯 L2：每行对正则做 find，首个捕获组/全匹配作为字段值
                                              // L1 不下推（正则捕获组无字节级字面量可提取），视为可能命中
                                              // 不变式 L1 ⊇ L2：L1 不淘汰任何候选 → 所有行进 L2
pred     := NAME ('>'|'>='|'<'|'<='|'=='|'!='|'=') VALUE
         | NAME 'in' '[' VALUE (',' VALUE)* ']'
         | NAME 'exists'
         | NAME 'between' VALUE 'and' VALUE     // 数值 between：VALUE 是数值字面量（D5 11 维契约）
         | '_ts' 'between' STRING 'and' STRING // 时间戳 between：_ts 伪字段特例，STRING 是 ISO 8601 时间戳
                                              // D5 不适用（_ts 是伪字段，不是数值字段）
                                              // speedate 解析时间戳为 epoch seconds 比较

// BARE 含空格时等价 grep -E 的多字符 ERE（匹配连续子串），不拆成多个 token
// 想表达多条件必须用显式 && / ||，无隐式 AND/OR
// BARE 里的 > < >= <= == != 默认是谓词操作符（与 grep 有意分歧）
// 匹配字面 > 用 STRING（">"）或 REGEX（/\>/）或转义（\>）
// between 的 and 只是该谓词的固定连接关键字，不与布尔操作符冲突；如需与外部布尔组合，用括号 (cost between 100 and 200) && ...
```

**与 grep 的关系**（非超集，有意分歧）：
- `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（单 BARE，含空格也是 ERE）
- `gxp 'a|b' file` ≡ `grep -E 'a|b' file`（ERE 的 OR，`|` 在 BARE 内是 ERE 元字符）
- `gxp 'cost>800' file` ≠ `grep -E 'cost>800' file`（gxp 是谓词，grep 是字面）
- 不接受 grep 短选项参数（`-A`/`-B`/`-e`/`-v`/`-o`/`-l` 等）；等价功能用 gxp 管道动词（`context N` / `files` / `count` / `fields`）

**与 SQL `and`/`or` 的区别**：gxp 用 `&&`/`||`/`!`（shell/JS/Python 风格），不用 `and`/`or`/`not`（SQL 风格）。理由：`and`/`or`/`not` 是英语常用词，在日志/代码文本里频繁出现作为字面量，作为关键字会冲突；`&&`/`||`/`!` 在文本里很少作为字面字符串出现，冲突概率低。

### 2.3 内置伪字段

| 字段 | 探测方式 | 示例 |
|---|---|---|
| `_ts` | ISO 8601 时间戳 | `2026-10-20T08:00:00Z` |
| `_tag` | `[xxx]` 方括号标签 | `[ERROR]`, `[checkout]` |

### 2.4 语法示例

```bash
# 1. 纯 grep 兼容（裸模式，行为与 grep -E 一致；含空格的 BARE 匹配连续子串）
gxp 'ERROR' app.log                          # ≡ grep -E 'ERROR' app.log
gxp 'ERROR timeout' app.log                   # ≡ grep -E 'ERROR timeout' app.log（匹配连续子串）
gxp 'a|b' app.log                             # ≡ grep -E 'a|b' app.log（ERE 的 OR）

# 2. 布尔 + 字段作用域 + 数值域 + Top-K
gxp '("timeout" || "latency") && _tag in [checkout,billing] && cost_ms>800
     | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms' app.log

# 3. 聚合与可解释执行计划
gxp '"timeout" && cost_ms>900 | group by _tag' app.log
gxp --explain --verify '<query>' app.log

# 4. 代码：结构模式（对接 ast-grep）
gxp 'ast(python, "useQuery($A)") && !"test" | sym ref(getUserById) | head 30' src/

# 4a. 结构查询（九语言穷举：ts/rs/py/js/go/tsx/java/c/cpp；大小写不敏感 + c++/cxx/cc 别名 + fail-loud）
gxp 'ast(ts, "useQuery($$$A)")' src/               # 找所有调用 useQuery 的 TS 代码（$$$A 匹配任意参数列表）
gxp 'ast(ts, "useState($A)") | count' src/          # 计数单参数 useState 调用（$A 只匹配单参数）
gxp 'ast(rs, "$OBJ.unwrap()")' src/                  # Rust：找所有 .unwrap() 方法调用（method call 必须有接收者 $OBJ）
gxp 'ast(rs, "println!($$$A)")' src/                 # Rust：找所有 println! 宏调用
gxp '"hello" && ast(rs, "println!($$$A)")' src/      # 行含 "hello" 且是 println! 宏调用
gxp 'ast(py, "requests.get($$$A)")' src/             # Python：找所有 requests.get() 调用
gxp 'ast(js, "fetch($$$A)")' src/                    # JavaScript：找所有 fetch() 调用
gxp 'ast(go, "func main() { $$$B }")' src/           # Go：找 main 函数声明（Go expression statement 需匹配完整 node）
gxp 'ast(java, "System.out.println($$$A)")' src/     # Java：找所有 println 调用
gxp 'ast(c, "return $X;")' src/                       # C：找所有 return 语句（C/C++ function call 需带分号）
gxp 'ast(cpp, "namespace $NS { $$$B }")' src/          # C++：找所有 namespace 声明（不做预处理，行号准确）

# 5. Fail-loud（字段写错直接报错）
gxp --strict 'ERROR && http_status>400' app.log
# 错误：字段 ['http_status'] 在候选集中从未出现；实际探测到的字段：['cost_ms', 'msg', 'req']

# 6. 引号与转义：整个表达式用 shell 单引号包，STRING 内用双引号
# bash/PowerShell 单引号内所有字符字面化，无需转义 && || ! > ( ) | [ ]
gxp 'cost_ms>800 && _tag in [checkout,billing]' app.log    # 零转义
gxp '"error" && !timeout' app.log                           # 字面量用双引号
gxp '"line with && in it"' app.log                          # 匹配字面 "&&"（在双引号 STRING 内）

# 7. LLM 忘记引号的 fail-loud
gxp cost_ms>800 app.log                # bash: > 被解释为重定向，立即报错
gxp ERROR && timeout app.log           # bash: && 是 shell 链操作符，立即报错

# 8. 上下文行（等价 grep -A/-B/-C，gxp 原生管道动词）
gxp 'ERROR | context 3' app.log                              # ≡ grep -C 3 ERROR app.log
gxp 'ERROR | context before:1 after:3' app.log                # ≡ grep -B 1 -A 3 ERROR app.log
gxp 'cost_ms>800 | context 5 | fields _ts,_tag,cost_ms' app.log   # 谓词 + 上下文 + 投影组合

# 9. 只列文件名 / 计数
gxp 'ERROR *.log | files'                         # ≡ grep -l ERROR *.log
gxp 'ERROR *.log | files --invert'                # ≡ grep -L ERROR *.log
gxp 'ERROR app.log | count'                       # ≡ grep -c ERROR app.log（单文件）

# 10. 字面 > < 的表达（BARE 里 > 是谓词操作符，与 grep 分歧）
gxp '"cost>800"' app.log              # STRING 字面量，匹配字面 "cost>800"
gxp '/cost>800/' app.log              # REGEX 字面量
gxp 'cost\>800' app.log               # ERE 转义（grep 习惯）

# 10a. 正则捕获组提取
gxp 'capture(trace_id, /req-\d+/) | fields trace_id' app.log   # 提取 req-12345 形式的 trace_id
gxp '"ERROR" && capture(user_id, /uid=\d+/) | fields user_id' app.log  # ERROR 行提取 user_id

# 10b. 时间范围查询（_ts 伪字段特例）
gxp '_ts between "2026-10-20T08:00:00Z" and "2026-10-20T09:00:00Z" | fields _ts' app.log
gxp '_ts between "2026-10-20T08:00:00+08:00" and "2026-10-20T09:00:00+08:00" && cost_ms>800' app.log

# 10c. 流式聚合统计（只做 min/max/avg）
gxp 'cost_ms>800 | stats cost_ms' app.log          # 输出 min/max/avg
gxp '"timeout" | stats cost_ms | head 20' app.log  # stats 在 head 前（流式）

# 10d. 代表性抽样（LLM context window 物理限制）
gxp 'ERROR | sample 50' app.log                   # 50 条随机样本（固定种子 0，可复现）
gxp 'ERROR | sample 10%' app.log                  # 10% 命中行（ceil(len*0.10)）
# 与 head 区别：head 取前 N（漏后段）；sample 全局洗牌后取 N（代表性）

# 10e. 全量排序（对齐 SQL ORDER BY / Unix sort，默认升序）
gxp 'cost_ms>500 | sort by cost_ms' app.log       # 按字段升序（默认 asc，数值优先）
gxp 'cost_ms>500 | sort by cost_ms desc' app.log  # 按字段降序
gxp 'ERROR | sort' app.log                         # 按整行字节序升序（聚集相同错误）
gxp 'ERROR | sort desc' app.log                    # 按整行字节序降序
# 与 rank 区别：rank 只取 Top-K（默认 desc）；sort 全量有序（默认 asc）

# 10f. 稳定去重（保留首次出现，对齐 SQL DISTINCT；不排序——保持原序）
gxp 'ERROR | distinct' app.log                     # 整行去重（相同错误只留首条）
gxp 'ERROR | distinct by trace_id' app.log         # 按 trace_id 去重（每个 trace 只留首条）
gxp 'ERROR | sort | distinct' app.log              # 先聚集相同错误，再去重（= sort | uniq）
gxp 'ERROR | distinct by user | sort by cost desc' # 先按 user 去重，再按 cost 降序排
# 与 group by 区别：group by 输出 `value: N`（计数），distinct 输出去重后的行
# 与 sort 区别：sort 改变顺序（聚集相同键相邻），distinct 删除重复（保持原序）
# 字段缺失的行归入空串键（多条缺失只留首条）

# 10g. 非流式百分位（SLO 报告核心指标；avg 掩盖长尾分布）
gxp 'cost_ms exists | percentiles cost_ms' app.log          # 默认 p50,p95,p99
gxp 'cost_ms exists | percentiles cost_ms p90,p99,p999' app.log  # 自定义百分位列表
# 与 stats 区别：stats 做流式 min/max/avg（O(1) 内存，逐行更新）；
# percentiles 做非流式全量百分位（O(N) 内存缓冲 + 排序，与 sort 一致）。
# 算法：nearest-rank 法——idx = ceil(p/100 * len) - 1。
# 输出格式：`p50: X, p95: Y, p99: Z`（对齐 stats 的 `min: X, max: Y, avg: Z`）。
# 终止 stage：输出摘要后链终止（与 stats/count 一致）。

# 10h. 多行 stack trace 捕获（错误诊断一等公民；L1 不变，pipeline 阶段判定续行边界）
gxp 'panic | trace' app.log              # 自动检测语言（python/java/js/rust/go/csharp）
gxp 'Exception | trace java' app.log     # 显式指定 java，拉所有 `^\s+at\s` 帧
gxp 'Traceback | trace python' app.log   # python，拉所有 `  File "..."` + 缩进代码行
gxp 'panic | trace go' app.log            # go，拉 `goroutine N [running]:` 头 + 函数调用 + .go:N 帧
gxp 'panic | trace rust' app.log          # rust，拉 `note:` / `stack backtrace:` / `   N:` 帧
gxp --json 'panic | trace' app.log        # NDJSON 输出：每行含 `extra_lines` 数组字段
# 输出形态：文本模式每行用 `  → ` 前缀视觉区分续行；JSON 模式加 `extra_lines` 字段（数组，仅非空时）
# 与 context 区别：context 拉固定窗口（无语义边界），trace 按语言规则判定 stack trace 边界
# 限制：trace 是终止 stage（与 stats/percentiles 一致），后置动词 fail-loud
# 限制：流式路径（文件 > 512MB）不支持 trace（跨行边界判定需"看到下一行"），提示用 `context N` 替代

# 11. 用 grep 短选项时的报错与转换提示
$ gxp -A 3 'ERROR' app.log
gxp: 不接受选项 -A（上下文行）

【当前意图】
  gxp -A 3 'ERROR' app.log

【gxp 不接受的原因】
  gxp 不接受 grep 短选项参数。上下文行功能用 gxp 原生管道动词 context。

【等价 gxp 语法】
  gxp 'ERROR | context 3' app.log
  gxp 'ERROR | context before:1 after:3' app.log   # 等价 -B 1 -A 3

【或直接用 grep】
  grep -A 3 ERROR app.log
```

---

## 3. 执行模型

### 3.1 五层流水线

```
parse → compile ─┬─ 必选组 groups（组内 OR，组间 AND）
                 ├─ 字段谓词 → L1 下推字段名前缀（cost_ms=）；数值比较交给 L2 精判
                 └─ 排序/投影计划
L0  采样估计选择性（读 512KB，给 groups 排序，成本 ~1ms）
L1  grep 链：每个必选组一条 grep -E；字段谓词下推字段名前缀（粗筛），全部跑在 grep 引擎上（第一遍 -n 保行号）
L2  精确求值：字段抽取(仅引用字段) + not/or/作用域判定 + 数值比较（支持整数/浮点/负数/前导零/科学计数法）
L3  结构层：ast-grep 扇出（九语言穷举落地，feature `l3-structural` 条件编译）
L4  rank(BM25 over 候选集) → top-k → fields/json → trace(多行 stack trace) → emit
```

### 3.2 关键算法

#### L1 下推策略（数值谓词不再编译成正则）

数值谓词（`cost_ms>800`）**不再**编译成 digit-DP 生成的 ERE 区间正则。理由：正则表达力是 CFG 子集，无法覆盖任意数值比较（9 位以上大数、前导零定宽、浮点、负数都漏匹配）。

替代策略：**L1 只下推字段名前缀**（`cost_ms=`）做粗筛，L2 做精确数值比较。

- L1 候选集：所有含 `cost_ms=` 字样的行（粗筛超集）
- L2 精判：字段抽取后数值比较（支持整数、浮点、负数、前导零、科学计数法、千分位）

**不变式**：`L1(下推) ⊇ L2(精确)` 严格成立——L1 只看字段名是否存在，L2 做精确比较。无漏检窗口。

**性能**（Rust 预估，方向 B 净收益为正——L1 节省 digit-DP 开销大于 L2 候选增加开销，详细实测见 `docs/performance-baseline.md` §1.2）：

> 历史算法（digit-DP 编译数值谓词为 ERE）已废弃，决策记录见 changelog v0.1。

#### 必选组上提

`A and (B or C)` → 必选组 = `{A, B, C}`（至少命中一个，保守超集）。

**不变式**：OR 内部的谓词一律不下沉（下沉就变 AND，破坏超集不变式）。

#### L1 不下推场景（维持 `L1 ⊇ L2`）

两类谓词字段**不参与 L1 前缀粗筛**——否则会破坏 `L1 ⊇ L2` 不变式（L1 漏候选，L2 倒希望命中）：

1. **Not 子树豁免**：`!(cost_ms>800)` 对无 `cost_ms` 字段的行视为"谓词假→取反为真→命中"（§3.3 字段不存在语义）。若 L1 对 Not 内部的 `cost_ms>800` 下推 `cost_ms=` 前缀粗筛，会淘汰所有无 `cost_ms` 字段的行——而 L2 的 Not 语义恰恰希望命中它们。故 Not 子树完全不收集字段谓词，L1 对 Not 内部不做前缀粗筛，把全部判定交给 L2 `eval_prepared` 递归求值。

2. **伪字段豁免**：伪字段 `_ts`（ISO 时间戳）和 `_tag`（方括号标签）不是 logfmt kv 字段——它们由 L2 抽取器从行结构（ISO 时间戳、方括号标签）推断，行里没有 `_ts=` / `_tag=` 子串可粗筛。若下推，L1 会淘汰所有含真实时间戳/标签的行，破坏 `L1 ⊇ L2`。故伪字段不参与前缀粗筛，L2 抽取后判定。

### 3.3 字段抽取（Late Materialization）

只解析查询引用到的字段，只在候选行上解析。

**支持格式**：
- `k=v` / `k:v`
- `[tag]` 方括号标签
- `"k": v` JSON 风格
- ISO 8601 时间戳

**数值字段格式契约（v1）**：

| 维度 | 支持 | 示例 | 备注 |
|---|---|---|---|
| 整数 | ✅ | `cost_ms=800` | 无符号、无前导零 |
| 负数 | ✅ | `cost_ms=-5` | 识别前导 `-` |
| 正号 | ✅ | `cost_ms=+800` | 识别前导 `+`，与 800 等价 |
| 前导零 | ✅ | `cost_ms=0050` | 按字面解析为 50（不视为八进制）|
| 浮点 | ✅ | `cost_ms=8.5` / `.5` / `8.` | 标准浮点格式 |
| 科学计数 | ✅ | `cost_ms=8e2` / `1.5E-3` | 支持 e/E |
| 千分位 | ❌ | `cost_ms=1,000` | v1 不支持（逗号视为字段分隔符）|
| 单位 | ❌ | `cost_ms=800ms` | v1 不支持（单位视为字面）|
| 空白 | ❌ | `cost_ms= 800` | v1 不 trim（视为字段不存在）|
| 空值 | ❌ | `cost_ms=` / `cost_ms=null` | 视为字段不存在 |
| 布尔 | ❌ | `cost_ms=true` | v1 不支持布尔数值比较 |

**字段不存在的语义**（对 `!` 和数值谓词的影响）：
- 行 `INFO started`（无 cost 字段），`cost>800` 谓词为假
- `!(cost>800)` 谓词为真 → **这行被命中**
- 如要表达"cost 字段存在且 ≤800"，必须显式写：`cost exists && !(cost>800)`
- 与 grep `-v` 的差异：grep `-v PATTERN` 是行级否定，不区分"字段存在/不存在"；gxp `!` 是谓词级否定，对无字段行视为谓词为假再取反

### 3.4 `!` 操作符的词法规则

`!` 是操作符当且仅当：前一个字符是空白、`(`、`[`、`&&`、`||`、或行首；且后一个字符是空白、`(`、`[`、`"`、`/`。其他情况下 `!` 是 BARE 的字面字符。

示例：
```bash
gxp '!ERROR' file              # ! 前是行首但后是字母 → 字面 "!ERROR"
gxp '! ERROR' file             # ! 前是行首，后是空白 → 操作符（否定 BARE ERROR）
gxp '!("ERROR")' file          # ! 前是行首，后是 ( → 操作符（否定 STRING）
gxp '!/ERROR/' file            # ! 前是行首，后是 / → 操作符（否定 REGEX）
gxp 'a!b' file                 # ! 前后都是字母 → 字面 "a!b"
gxp 'a && ! b' file            # ! 前是空白（&& 后），后是空白 → 操作符
gxp 'a && !b' file             # ! 前是空白但后是字母 → 字面（用户意图是 !b 字面）
```

**`!` 优先级**：`!` 绑定 `unary`，等价 `!(unary)`。与谓词操作符的组合：
- `!cost>800` → `(!cost) > 800` ❌（语法错误，`!cost` 不是合法 pred 左值）
- `!(cost>800)` → `!(cost>800)` ✅（`!` 绑定括号内整个 pred）
- `!cost exists` → `!(cost exists)` ✅（`!` 绑定 `cost exists` 这个 unary）

强制规则：`!` 作用于 pred 时**必须用括号**。`!cost>800` 是语法错误，强制 LLM 写成 `!(cost>800)`。

---

## 4. 缓存策略

### 4.1 核心原则

**缓存只允许产生"跳过整块"的决策，且该决策依赖不可变证据（块内容哈希）**。

缓存失效的代价只能是变慢，不能是变错。

### 4.2 分层策略

| 层 | 内容 | 失效方式 | 何时启用 |
|---|---|---|---|
| **T0 纯内存单遍** | 默认，永远正确 | — | 文件小/查询少/IO 快 |
| **T1 进程内 shape cache** | 字段布局/字段名探测 + 单次查询内 TailIndex 增量 | 随进程销毁 | 总是 |
| **T2 块摘要 sidecar** | 稀有词的 32k-bit 布隆 + 有序数据的 zone-map | 块内容哈希证据；前缀块哈希不变则不重建；仅尾部增量 | **永久拒绝**（D9）—详见 rust-plan §11.3.2 |
| **T3 常驻 daemon** | mmap + inotify + 跨查询块哈希缓存 | 事件驱动 | **永久拒绝**（D9）—详见 rust-plan §11.3.2 |

> T2/T3 决策依据见 §4.3 实测数据；永久拒绝理由见 `docs/rust-implementation-plan.md §11.3.2`。

### 4.3 实测数据（T2/T3 决策依据）

> 详细实测数据 + 意义修正 + 摘要缓存回本条件已沉 changelog v0.1（"决策依据历史"小节）。本节保留结论：在真实交错日志上 zone-map 跳过 0%；单进程内多次 check 测得 107× 增量收益不适用于一次性命令模型。

### 4.4 结论

**v1 默认纯内存单遍（T0/T1）。** T2/T3 永久拒绝理由见 `docs/rust-implementation-plan.md §11.3.2`（D9 硬约束：跨查询增量需要守护进程，与 grep 一次性命令模型根本冲突）。决策依据详细数据见 changelog v0.1。

---

## 5. 性能基线

> 当前性能数据详见 `docs/performance-baseline.md` §1.1。原型期实测与决策记录见 changelog v0.1-v0.5。

**关键结论（保留作设计依据）**：
- gxp 的真正优势不是"更精确"，而是**让字段谓词查询可表达、可正确实现**。grep 在字节匹配上仍然是更快更精确的工具。
- `--verify` 暴力对账与优化路径 100% 一致（不变式 §3.2）。

---

## 6. 接近 GNU grep 的改造方案（已废弃，D8 封死）

> D8 决策（不 fork GNU grep，Rust 从头写）已封死该路线。决策依据详细论证（P0/P1/P2 拒绝理由 4 条）已沉 changelog v0.1（"决策依据历史"小节）。本节保留 1 句结论：v1 不走 GNU grep 改造路线，详见 changelog v0.1 + `rust-implementation-plan.md` §9.1。

---

## 7. 开发语言选择

| 语言 | 性能 | 生态 | 关键判据 | 选择 |
|---|---|---|---|---|
| **Rust** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | regex crate 自带 literal extraction（L1 下沉天然支持）、tree-sitter 一等绑定、SIMD | **生产首选** |
| Go | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | 无 literal extraction、GC 在大文件上有抖动 | 次选 |
| C | ⭐⭐⭐⭐⭐ | ⭐⭐ | 可 upstream grep，但无 tree-sitter/AST 生态 | 仅 P2 改造 |

**推荐路线**：Rust 实现（regex crate literal extraction + tree-sitter 一等绑定 + memchr SIMD + 内存安全）。决策记录详见 changelog v0.1。

---

## 8. 格式识别

### 8.1 应该识别的格式（自动探测）

| 格式 | 探测方式 | 字段抽取 |
|---|---|---|
| 纯文本（默认） | 行分隔 | 无 |
| JSON / JSON Lines | 首字符 `{` `[`（**仅单行 JSONL**；多行 JSON 用 `jq -c '.'` 预处理） | 顶层字段 |
| CSV / TSV | 首行 header + 分隔符探测 | 列名 |
| Logfmt | `k=v` 模式密度 | 键 |
| Apache Combined Log | 正则模式 | 预定义字段 |
| Syslog (RFC 3164/5424) | 时间戳 + 标签模式 | 预定义字段 |
| XML | `<` 开头 + 标签 | 元素/属性（SAX 流式） |
| YAML | `---` 或 `key: value` | 顶层键 |
| Markdown | `#` heading | heading-aware |
| 源代码（tree-sitter） | 文件扩展名 + shebang | AST 节点 |
| Bracket tags `[level]` | 方括号模式 | `_tag` |
| ISO timestamps | RFC 3339 变体 | `_ts` |

### 8.2 源代码语言（tree-sitter 支持 ~30 种）

Python, JavaScript, TypeScript, Go, Rust, Java, C, C++, C#, Ruby, PHP, Bash, SQL, HTML, CSS, JSON, YAML, TOML, Markdown, Lua, Swift, Kotlin, Scala, Haskell, Elixir, Erlang, Clojure, Dart, R, Julia

#### 文件类型 × 查询层支持矩阵（2026-10-22 补充）

> **核心原则**：L1+L2 对**所有文本文件**通用（字面量/正则匹配 + 字段抽取 + 数值比较），不区分语言。L3（`ast(NAME, PATTERN)` AST 结构查询）需要语言专用 parser，按语言分档。`--glob '*.ext'` 只做文件名过滤，不影响查询层。

| 文件类型 | L1+L2 文本搜索 | L3 `ast(lang, ...)` | 说明 |
|---|---|---|---|
| `.ts` | ✅ | ✅ `ast(ts, ...)` | 落地，穷举测试 |
| `.tsx` | ✅ | ✅ `ast(tsx, ...)` | 落地，`SupportLang::Tsx` 原生解析 JSX |
| `.js` | ✅ | ✅ `ast(js, ...)` | 落地 |
| `.jsx` | ✅ | ✅ `ast(tsx, ...)` | 落地，同 TSX，`SupportLang::Tsx` 覆盖 JSX |
| `.vue` | ✅ | ⏳ 暂不评估 | Vue SFC 需预处理提取 `<script>` 块（详见 rust-plan §11.3.2） |
| `.rs` | ✅ | ✅ `ast(rs, ...)` | 落地，穷举测试 |
| `.py` | ✅ | ✅ `ast(py, ...)` | 落地 |
| `.go` | ✅ | ✅ `ast(go, ...)` | 落地 |
| `.java` | ✅ | ✅ `ast(java, ...)` | 落地 |
| `.c` / `.h` | ✅ | ✅ `ast(c, ...)` | 落地，V5 验证通过（不做预处理） |
| `.cpp` / `.cc` / `.hpp` | ✅ | ✅ `ast(cpp, ...)` | 落地，V5 验证通过（不做预处理） |
| `.css` | ✅ | 不做 | 样式语言，AST 结构查询非 gxp 目标场景 |
| `.scss` | ✅ | 不做 | 同 CSS，用 stylelint/sass 等专业工具 |
| `.less` | ✅ | 不做 | 同 CSS |
| `.html` | ✅ | 层 2 天然可用 | ast-grep 内置 HTML parser，不穷举测试 |
| `.md` | ✅ | 不做 | 结构松散，AST 价值低（§8.2 决策）；BARE 模式已覆盖文本检索 |
| `.json` | ✅ | 层 2 天然可用 | ast-grep 内置 JSON parser |
| `.yaml`/`.yml` | ✅ | 层 2 天然可用 | ast-grep 内置 YAML parser |
| `.toml` | ✅ | 层 2 天然可用 | ast-grep 内置 TOML parser |

> **gxp L3 启用范围 ≠ tree-sitter 全集**。tree-sitter 支持 ~30 种是能力上限，gxp L3 按 `rust-implementation-plan.md §3.1.1` 三层结构落地：
> - 层 1（v1.0 验收 5 种）：py/js/ts/rs/go
> - 层 2（ast-grep 内置 ~20 种，天然可用）：java/c/cpp 等（落地 java/c/cpp，见 `rust-implementation-plan.md §3.2`）
> - 层 3（tree-sitter fallback ~10 种，v1.0 后评估）：Swift/Haskell/Erlang/Clojure/Dart/R/Julia 等
>
> **Markdown 不进 L3**：结构松散，AST 价值低；gxp BARE 模式（§2.4 示例 1）已覆盖文本检索需求。
> **Vue**：ast-grep 不原生支持 Vue SFC（需 `<template>`/`<script>`/`<style>` 三段预处理提取 script 块，L3 模型需加 `preprocessor` 概念）。决策详见 `docs/rust-implementation-plan.md §11.3.2`（暂不评估：升级条件 = Vue 项目用户明确报告无法查 SFC 内 JS 结构）。
> **CSS/SCSS/Less 不进 L3**：样式语言，AST 结构查询（如"找所有嵌套选择器"）是 linter/stylelint 的职责，不是 gxp 的目标场景。L1+L2 文本搜索完全覆盖。
>
> **九语言穷举落地**：py/js/ts/tsx/rs/go/java/c/cpp。feature `l3-structural` 条件编译（ast-grep 重依赖默认关闭）。落地进度详见 changelog v0.17-v0.22。
>
> **求值模型**：DSL `ast(NAME, STRING)` → L1 豁免不下推（视为"可能命中"，不淘汰候选）→ 扫描前对整个文件预计算 `ast_grep::find_all` 得 `ast_lines` 行号集合 → L2 逐行查集合（`ast_line_contains`）。不变式 `L1 ⊇ L2 ⊇ L3`。
>
> **ast-grep pattern 语义**（九语言穷举发现）：
> - `useQuery($A)` 只匹配**单参数**调用；`useQuery($$$A)` 匹配**任意参数列表**
> - Rust method call 需 `$OBJ.unwrap()`——不能写裸 `unwrap()`（method call 表达式必须有接收者节点）
> - `println!($$$A)` 匹配宏调用任意参数
> - `SupportLang::TypeScript` **不解析 JSX**——含 JSX 的 `.tsx` 代码需 `SupportLang::Tsx`（落地 `ast(tsx, ...)` 语法）
> - Go `fmt.Println($$$A)` **不单独命中**——Go tree-sitter grammar 把 expression statement 解析为含行尾的完整 node，selector expression pattern 不匹配；用 `func main() { $$$B }` func declaration pattern 避开此问题
> - C/C++ function call pattern **需带分号**——`printf($$$A)` 不命中，`printf($$$A);` 命中（C/C++ tree-sitter 把 expression statement 解析为含分号的完整 node，与 Go 类似）（V5 验证发现）
> - C/C++ function definition pattern **需带函数体**——`int $F()` 不命中，`int $F() { $$$B }` 命中（V5 验证发现）
> - C/C++ **不做预处理**——ast-grep tree-sitter 解析预处理前源码，不展开 `#include`/`#define`/`#ifdef`，AST 行号准确对应源码行（V5 验证结论 2026-10-23，11 项实验全部通过）
>
> **tree-sitter grammar 版本兼容性**（已知限制）：
> - ast-grep 内部用 tree-sitter 的 grammar 解析源代码，grammar 版本在 `ast-grep-language` crate 编译时锁定
> - 目标代码用了比该 grammar 更新的语法特性时，可能解析失败或产生不完整 CST，导致 pattern 匹配漏报
> - gxp 策略：**fail-loud**（语言不支持 → `L3Error::UnsupportedLang`）+ **诚实声明**（不静默容错语法版本差异；解析失败目前返回空集合，v1.0 应改为 fail-loud 报错）
> - v1.0 改进方向：解析失败 → fail-loud + `--explain` 输出 ast-grep/tree-sitter 版本号 + pattern 语义穷举测试
>
> 九语言穷举落地进度详见 Rust 方案 §11.2。
>
> **代码库评估（2026-10-22，web-app）**：
> - JS/Python/Go 支持已落地（详见 changelog v0.17-v0.22）
> - TSX 支持已落地（`SupportLang::Tsx` 原生解析 JSX，无需预处理）；Vue SFC 支持暂不评估（详见 rust-plan §11.3.2）
> - JSON 文件内嵌源码字符串（某低代码平台 `.Script` 文件内嵌多行 TS 源码）不做：需引入 preprocessor 概念（与 Vue SFC 同类，见上条），与 D9 无关（D9 是拒绝守护进程）。建议用 `jq '.Script' file | gxp 'ast(ts, ...)'` 组合（preprocessor 委托 jq，gxp 只做 AST 查询）
> - 多行 JSON 合并不做（2026-10-26 re-triage）：起源是 某低代码平台 评估副产品，场景 A（pretty-printed JSON 日志）是事后合理化，K8s/Docker 生产日志默认单行 JSONL 不是多行，配置文件 LLM 通常直接 Read，真实 LLM 工作流稀疏。替代路径：`jq -c '.' multiline.log | gxp 'cost_ms>500'`（jq 预处理转单行 JSONL，gxp 仍是 grep-aligned 不跨界）。升级条件：用户明确报告多行 JSON 日志场景频繁 + jq 预处理负担重。详见 rust-plan §11.3.2

### 8.3 不该识别的格式（边界）

- 二进制文件（图片、可执行）→ grep -I 行为
- 压缩归档（zip/tar）→ 委托 ugrep -z
- 数据库文件（SQLite/Parquet）→ 不在范围
- 富文本（PDF/DOCX）→ 不在范围
- 协议流（HTTP/WebSocket）→ 不在范围

---

## 9. 社区对标与可借鉴项目

### 9.1 语法最接近

**ugrep**：原生支持 `--and` `--or` `--not` 布尔谓词和括号分组，保留 grep 兼容（POSIX BRE/ERE）。

**ack**：`--match` + `--output` 分离，本质上是 gxp 里"表达式 + fields 投影"的前身。

### 9.2 架构最接近

**LogQL**（Grafana Loki）：分层管道（stream selector → line filter → parser → label filter），语法结构几乎同构。

**Sourcegraph Stream Query Syntax**：`pattern and/or/not` + `repo:` `file:` `type:` 限定符，面向代码场景。

### 9.3 谓词下沉/索引加速

**Google Code Search / Russ Cox 的 trigram 论文**（2012）：正则表达式 → trigram 索引 → 候选集 → 精确验证。gxp 的 L1→L2 下推不变式（L1 粗筛 ⊇ L2 精判）借鉴此"候选集 → 精确验证"思想；区别在 gxp 不建索引（D9），L1 用 regex crate 的 literal extraction 做 byte-level 粗筛，L2 做 field-level 精判（方向 B，§3.2）。

### 9.4 预建索引的代码理解工具（与 gxp L3 的分工）

**codebase-mcp / codegraph 类工具**：预建 `.codegraph/` AST 索引，跨查询复用，返回符号源码 + 调用路径 + 依赖爆炸半径。与 gxp 形成互补，不是替代关系。

| 维度 | gxp | codebase-mcp 类 |
|---|---|---|
| 索引模型 | 零持久索引（§1.3 核心约束 D9） | 预建 `.codegraph/` 索引，跨查询复用 |
| 查询粒度 | 行级 / 字段级 / 数值级 | 符号级 / AST 节点级 |
| 返回形式 | 匹配行 + 字段投影 + Top-K | 节点源码 + call path + blast radius |
| 跨查询复用 | 不做（D9，v1 保持 grep 一次性命令模型） | 核心机制 |
| 日志/文本流 | 一等公民（logfmt/JSON/[tag]/ISO 时间戳） | 不支持 |
| 字段/数值比较 | 一等公民（11 维契约，`cost_ms>800`） | 不支持 |
| Top-K/排序 | 一等公民（`rank top 20 by cost_ms desc`） | 不支持 |
| 调用图/依赖爆炸 | 不支持（需跨文件符号索引，与 D9 冲突） | 核心能力 |
| L3 重叠区 | 行级 AST 命中集合（用于 pipeline 后续 rank/fields 投影） | 符号定义源码 + 调用路径 |

**分工边界**（"不建议做跨文件符号解析，让 ast-grep 干，gxp 只做扇出调度"原则的延伸）：

- gxp L3 定位为**行级 AST 扇出调度器**：L1 粗筛出候选文件 → `L3Matcher::match_file` → 行号集合 → 进 pipeline（rank/fields/context）。不做跨文件符号解析、调用图、依赖爆炸半径——这是 codebase-mcp 的领域。
- codebase-mcp 定位为**预建索引的代码理解工具**：跨查询复用 AST 索引，提供符号级 + 调用图能力。
- LLM agent 工具路由：按返回形式选择——要行号 + 字段/数值/Top-K 走 gxp；要符号定义源码 + 调用路径走 codebase-mcp。路由由 agent harness（如 CodeAgent tool descriptor）完成，gxp 核心不内嵌 MCP server（D9 + §15 决策表"不做 MCP server，外围封装才做"）。
- **gxp 核心聚焦能力**：行级检索 + 字段抽取 + 数值比较 + Top-K 投影 + L3 行级 AST 扇出。MCP/agent harness 集成属外围封装，不进 gxp 仓库；v2 若有 MCP 需求，落地形式是独立子项目（如 `gxp-mcp` crate）。

---

## 10. 已知局限

- `k:v` 分隔符和时间戳冒号打架，`T01` 被误当字段名——需要词边界约束。
- BM25 的 df 在**候选集**上统计而非全库，Top-K 够用，但不是严格全库 IDF。
- 多文件/递归已覆盖（目录参数 CLI 接入）；`--include`/`--glob` 文件名过滤已落地（见 §11 第 9 项）；二进制判定、`-z` 仍未覆盖，建议直接用 grep。
- L3 结构层九语言穷举落地（py/js/ts/tsx/rs/go/java/c/cpp，feature `l3-structural` 条件编译）：DSL 解析 → L1 豁免 → L2 查 ast_lines 集合 → L3 ast-grep find_all 精判 + memchr 行号映射。已知限制：tree-sitter grammar 版本编译时锁定，目标代码用更新语法可能漏报（§8.2）。
- `context N` / `files` / `before:N after:M` 管道动词已落地（详见 changelog v0.6-v0.12）。
- **数值字段格式契约**：v1 支持整数/浮点/负数/前导零/科学计数法，不支持千分位逗号/单位/空白 trim/布尔值（见 §3.3 表）。
- **`!` 操作符的词法规则**：`!` 前后必须有空白或非字母数字边界才是操作符，否则是字面；作用于 pred 必须用括号 `!(cost>800)`，不允许 `!cost>800`（见 §3.4）。
- **`--verify` 对账基准**：当前用 gxp 自身暴力求值，不是独立实现；真正的独立对账应该用 DuckDB 临时表或手写无代码复用的实现（独立对账方案仍未落地，列入未决项 §12.4 + §12.5）。

---

## 11. 下一步

1. **Rust 实现（v1）**：按 `rust-implementation-plan.md` 从头实现。不走 GNU grep P1 改造路线（D8 决策，详见 §6 + changelog v0.1）。
2. **L2 精度修复**：数值谓词上界漏匹配问题（§3.2）选方向 B（L1 粗筛 + L2 精判）落地。Rust 实现时 L2 字段抽取 + 数值比较同时做，无需 digit-DP 编译。
3. **格式探测**：实现 JSON/CSV/logfmt/syslog 自动探测 + shape cache。
4. **结构层**：九语言穷举落地（py/js/go/tsx/java/c/cpp，§8.2）。C/C++ V5 验证通过（不做预处理，行号准确）。Vue SFC 暂不评估（详见 rust-plan §11.3.2）。解析失败 fail-loud 已落地。
5. **context / files 动词**：在 L4 投影阶段实现上下文行保留和只列文件名。
6. **`!` 词法规则测试**：覆盖 §3.4 的 7 种 `!` 边界 case（操作符 vs 字面）。
7. **字段格式契约测试**：覆盖 §3.3 表格的支持/不支持项，特别是前导零、浮点、负数、科学计数法。
8. **独立对账基准**：用 DuckDB 临时表做 `--verify` 的独立实现，避免用 gxp 自身暴力求值循环论证。
9. **文件名过滤**：`--glob PATTERN`（globset crate，支持 `*.ts`/`*.{ts,vue}`/`**/*.json`）+ .gitignore 尊重（`ignore` crate gitignore 匹配器，不换回 WalkBuilder）。代码库评估（web-app，20+ 扩展名 + node_modules 噪声目录）识别为阻断性缺失。`--type NAME` 预设类型映射推迟 v2。详见 Rust 方案 §11.2.9。

---

## 12. 验证结果（2026-10-20，TDD）

### 12.1 测试

原 `tests/dir_*.rs` 扩充至 **31 项全绿**。开发顺序严格 red→green：
- 首批 10 项因 `gxp_dir` 缺失而失败 → 实现 `PathPlan` / `DirIndex`
- 深度谓词测试的断言算术写错（用 `count(sep)` 当作嵌套深度）→ 改用 `relpath` 表达意图
- 第二批 8 项（TailIndex / commit_key）先红后绿
- `off` 档 3 项先红后绿（要求"零 readdir"且"返回过期列表"）
- **发现真实缺陷**：`dir_re` 被当成祖先约束参与剪枝，导致命中 0 文件。补 3 个回归测试（含与暴力遍历的等价性对照）后修复。原测试套件没覆盖正则路径谓词，是真实覆盖率缺口。

> **当前态**：Rust `tests/dir_{pruning,index,off,tail,regex,git}.rs` 承担跨平台职责。本节保留规模数据作为 Rust 实现参照基线。prototype/ 删除决策见 changelog v0.10。

### 12.2 规模化验证

> prototype 期 TDD 规模化验证数据（路径剪枝 / 三档新鲜度 / append-only 尾部增量 / 分区并集 / git 锚点成本）已沉 changelog v0.10 "决策依据历史" 小节。本节保留 1 句结论：路径剪枝是唯一有效的跳过手段（单日谓词把需读字节压到 3.3%）；`dir_re` 不可剪枝（无锚定正则无法证明后代不命中）；默认 `lax` 档（`off` 档只能用于显式 TTL 场景）；git 调用比目录列举贵 50–130 倍，不能进热路径。

### 12.3 对前文结论的修正

> 实测修正表已沉 changelog v0.10 "决策依据历史" 小节。本节保留 1 句结论：5 项 prototype 期前文说法中 3 项得证（路径剪枝/lax 复用/dir_re 不可剪枝）、2 项修正（git status 便宜不成立 / `off` 档不可作默认）。

### 12.4 仍需讨论（未验证项）

1. 跨文件 `rank` 的分层归并 vs 全局精确排序，只定了语义未做实测
2. 4 核并行 + 单文件切块读取的实际收益
3. 是否需要 `--pre` 解压钩子（与 ugrep `-z` 的边界划分）

**已验证/已移出**：
- "Rust 化时 regex crate 的 literal extraction 能否替代外部 grep 子进程"——已验证可行，见 `rust-implementation-plan.md` §1（regex-syntax 0.8 `hir::literal::Extractor` + aho-corasick 1.0 并行匹配）
- "10 万+ 文件目录下指纹哈希的内存上限与分片策略"——移入 §12.5 v1 不做项

### 12.5 v1 明确不做

> 暂不评估 + 永久拒绝清单详见 `docs/rust-implementation-plan.md §11.3.2`。摘要：
> 1. 跨查询增量缓存（T2 sidecar / T3 daemon）：D9 永久拒绝
> 2. 大文件目录指纹哈希内存分片：随 T2/T3 永久拒绝

---

**文档结束**

---

内容由 AI 生成
