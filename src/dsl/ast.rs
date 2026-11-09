//! gxp DSL AST（设计文档 §2 EBNF）。
//!
//! 类型层次：
//! - [`Query`]：根节点 = 表达式 + 管道动词序列
//! - [`Expr`]：布尔谓词树（or / and / not / prim）
//! - [`Prim`]：prim 节点（括号 / STRING / REGEX / BARE / ast / capture / pred）
//! - [`Pred`]：字段谓词（比较 / `in` / `exists` / `between`）
//! - [`Stage`]：管道动词（`rank` / `sort` / `distinct` / `head` / `fields` / `count` / `group` / `context` / `files` / `stats` / `sample`）
//!
//! 设计文档锚点：
//! - §2.2 EBNF（方案 D' 语法：邻接=字面空格，`&&`/`||`/`!` shell 符号，STRING 强制双引号）
//! - §3.4 `!` 词法规则方向 C（前后边界判定 + pred 强制括号 `!(cost>800)`）
//! - Rust 方案 §7.6 Stage enum（含 Context/Files 两变体）

/// 数值比较操作符（设计文档 §2.2 EBNF `pred` 规则）。
///
/// 用于 [`Pred::Compare`] 和下游 L2 数值比较器（`src/l2/numeric.rs::eval_numeric`）。
/// gxp 与 grep 有意分歧：BARE 里的 `>` `<` `>=` `<=` `==` `!=` 是谓词操作符，不是字面。
/// 字面 `>` 用 STRING（`">"`）或 REGEX（`/\>/`）或 ERE 转义（`\>`）表达（设计文档 §2.4 示例 10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    /// `>`（严格大于）
    Gt,
    /// `>=`（大于等于）
    Ge,
    /// `<`（严格小于）
    Lt,
    /// `<=`（小于等于）
    Le,
    /// `==`（相等）
    Eq,
    /// `!=`（不等）
    Ne,
    /// `=`（等价 `==`，设计文档 EBNF `pred` 规则接受 `=` 作为相等操作符）
    Assign,
}

/// gxp DSL AST 根节点（设计文档 §2.2 EBNF `query` 规则）。
///
/// 组成：一个布尔表达式 `expr` + 零或多个管道动词 `stages`。
/// 示例：`"timeout" && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms`
#[derive(Debug, Clone)]
pub struct Query {
    /// 查询表达式（`expr` 规则）。
    pub expr: Expr,
    /// 管道动词序列（`('|' stage)*` 规则，可能为空）。
    pub stages: Vec<Stage>,
}

/// 布尔谓词树（设计文档 §2.2 EBNF `expr` / `or_expr` / `and_expr` / `unary` 规则）。
///
/// 优先级（从低到高）：`||` < `&&` < `!` < prim。
/// 邻接不隐含布尔——邻接是 BARE 内的字面空格（设计文档 §2.1 方案 D'）。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `a || b`（`or_expr` 规则）。
    Or(Box<Expr>, Box<Expr>),
    /// `a && b`（`and_expr` 规则）。
    And(Box<Expr>, Box<Expr>),
    /// `! unary`（`unary` 规则；`!` 词法规则见设计文档 §3.4 方向 C）。
    ///
    /// 作用于 pred 必须用括号：`!(cost>800)` 合法；`!cost>800` 语法错误。
    Not(Box<Expr>),
    /// prim 节点（`prim` 规则：括号 / STRING / REGEX / BARE / pred）。
    Prim(Prim),
}

/// prim 节点（设计文档 §2.2 EBNF `prim` 规则）。
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    /// `( expr )`：括号分组。
    Paren(Box<Expr>),
    /// `"literal"`：双引号字面量 STRING（单引号禁用——避免与 shell 外层引号冲突）。
    String(String),
    /// `/pattern/`：REGEX 字面量。
    Regex(String),
    /// 裸字面量 BARE：ERE 兼容，含空格匹配连续子串（不拆成多个 token）。
    Bare(String),
    /// `ast(NAME, STRING)`：结构模式匹配（设计文档 §2.2 + §8.2）。
    ///
    /// NAME 是语言名（`py`/`js`/`ts`/`rs`/`go`/`tsx`/`java`/`c`/`cpp`/...，见 ast-grep `SupportLang`），
    /// STRING 是 ast-grep pattern（如 `useQuery($A)`）。
    ///
    /// L1 不下推（结构层查询无字节级下推），L1 视 ast() 为"可能命中"（不淘汰候选），
    /// L3 调 ast-grep `find_all` → 节点 range → 行号映射 → MatchedLine。
    ///
    /// PoC 只验 ts 通路；扩到 9 语言（py/js/ts/rs/go/tsx/java/c/cpp）穷举测试。
    Ast {
        /// 语言名（如 `"ts"`/`"py"`/`"rs"`）。
        lang: String,
        /// ast-grep pattern（如 `useQuery($A)`）。
        pattern: String,
    },
    /// `capture(NAME, REGEX)`：正则捕获组提取（设计文档 §2.2 EBNF `capture` 规则）。
    ///
    /// NAME 是提取结果的字段名，REGEX 是 `/pattern/` 形式正则字面量。
    /// 纯 L2：每行对正则做 find，首个捕获组（有则用组1，无则用全匹配）作为字段值。
    /// L1 不下推（正则捕获组无字节级字面量可提取），视为可能命中（不淘汰候选）。
    /// 不变式 L1 ⊇ L2：L1 不淘汰任何候选 → 所有行进 L2。
    Capture {
        /// 提取结果字段名（如 `trace_id`）。
        name: String,
        /// 正则表达式字面量（不含 `//` 分隔符，如 `req-\d+`）。
        pattern: String,
    },
    /// 字段谓词（`pred` 规则）。
    Pred(Pred),
}

/// 字段谓词（设计文档 §2.2 EBNF `pred` 规则）。
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    /// `NAME OP VALUE`：数值/字符串比较。
    ///
    /// L1 下推字段名前缀（`cost_ms=`），L2 抽取字段值后调用 `eval_numeric`（方向 B，设计文档 §3.2）。
    /// VALUE 以字节字面量存储（不做语义解析），L2 负责按 11 维格式契约解析（设计文档 §3.3）。
    Compare {
        /// 字段名（如 `cost_ms`）。
        name: String,
        /// 比较操作符。
        op: CmpOp,
        /// 字面量阈值（字节形式，保留原样便于 L2 解析）。
        value: String,
    },
    /// `NAME in [v1, v2, ...]`：集合成员判定。
    ///
    /// v0.48 起 `NAME ("="|"==") "STRING"` 字符串等值在解析期脱糖为单元素本变体
    /// （设计文档 §3.2 字符串等值脱糖——构造性等价，L1/L2 零新增语义分支）。
    In {
        /// 字段名。
        name: String,
        /// 候选值列表（字节字面量）。
        values: Vec<String>,
    },
    /// `NAME exists`：字段存在判定。
    Exists {
        /// 字段名。
        name: String,
    },
    /// `NAME between LO and HI`：区间判定。
    ///
    /// `and` 是 `between` 的固定关键字，不与布尔操作符冲突（gxp 布尔用 `&&`，无 `and` 关键字）。
    /// 如需与外部布尔组合，用括号：`(cost between 100 and 200) && ...`。
    Between {
        /// 字段名。
        name: String,
        /// 下界（字节字面量）。
        lo: String,
        /// 上界（字节字面量）。
        hi: String,
    },
}

/// `sample` 的抽取规格：绝对数或比例。
///
/// `Ratio(p)` 在执行时按命中行数换算为 `ceil(len * p)`。
#[derive(Debug, Clone, PartialEq)]
pub enum SampleSpec {
    /// `sample N`：抽 N 条。
    Count(usize),
    /// `sample P%`：抽 `ceil(len * P/100)` 条（p ∈ [0.0, 1.0]）。
    Ratio(f64),
}

/// 管道动词（设计文档 §2.2 EBNF `stage` 规则 + Rust 方案 §7.6）。
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// `where EXPR`：对上游结果再过滤。
    Where(Expr),
    /// `rank top K by FIELD [desc|asc]`：BM25 Top-K 排序（gxp 独有，非 grep 等价）。
    Rank {
        /// Top-K 的 K。
        top: usize,
        /// 排序字段名。
        by: String,
        /// `desc` 为 true（默认），`asc` 为 false。
        desc: bool,
    },
    /// `head N`：限制输出行数（等价 `head -n N`）。
    Head(usize),
    /// `sample N` / `sample P%`：随机抽样（固定种子 0，可复现）。
    ///
    /// - `sample 50`：从命中行随机抽 50 条
    /// - `sample 50%`：按比例抽 50%（解析时换算为 `ceil(len * p)`）
    ///
    /// 与 `head N` 区别：head 取前 N（漏后段，无代表性）；sample 全局洗牌后取 N（代表性）。
    /// 场景：LLM context window 物理限制下需代表性样本而非前 N 条（§9.1 候选 #1，L2 等级）。
    /// 固定种子 0 → --verify 暴力路径同种子重跑，100% 一致。
    Sample {
        /// `Count(N)`：抽 N 条；`Ratio(p)`：抽 `ceil(len * p)` 条（p ∈ [0.0, 1.0]）。
        /// ratio 在执行时按实际命中数换算（解析时 len 未知）。
        spec: SampleSpec,
    },
    /// `distinct [by FIELD]`：去重投影（§9.1 候选 #3 落地）。
    ///
    /// - `distinct`：整行去重，保留首次出现的行（对齐 Unix `sort | uniq`，但不排序）
    /// - `distinct by FIELD`：按字段值去重，每个字段值只留首次出现的行
    ///   （对齐 SQL `SELECT DISTINCT ON (field) *`，字段缺失的行归入空串键）
    ///
    /// 与 `group by FIELD` 区别：group by 输出 `value: N`（计数），
    /// distinct 输出去重后的行（保留原始字段/整行内容）——两者语义不重叠。
    /// 与 `sort` 区别：sort 改变顺序（聚集相同键相邻），distinct 删除重复（保持原序）。
    ///
    /// 稳定去重：相同键只保留首次出现，后续出现的行删除（对齐 SQL DISTINCT 语义）。
    Distinct {
        /// 去重字段名。`None` 按整行字节；`Some(field)` 按字段值。
        by: Option<String>,
    },
    /// `sort [by FIELD] [desc|asc]`：全量稳定排序（§9.1 候选 #5 落地）。
    ///
    /// - `sort`：按整行字节序升序（聚集相同错误行，对齐 Unix `sort` 默认）
    /// - `sort by FIELD`：按字段值排序（数值优先 11 维契约，非数值沉底）
    /// - `sort by FIELD desc`：降序；`asc` 升序（默认）
    ///
    /// 与 `rank top K by FIELD` 区别：rank 只取 Top-K（partial-sort，默认 desc）；
    /// sort 是全量有序（默认 asc，对齐 SQL ORDER BY / Unix sort / Python sorted）。
    /// 两者默认方向不同：rank 是"最大的 K 个"（desc 语义），sort 是"排成有序"（asc 原型）。
    /// 稳定排序：相等键保持原顺序（与 SQL ORDER BY 一致）。
    Sort {
        /// 排序字段名。`None` 按整行字节序；`Some(field)` 按字段值（数值优先）。
        by: Option<String>,
        /// `desc` 为 true，`asc` 为 false（默认 false/asc）。
        desc: bool,
    },
    /// `fields a, b, ...`：字段投影（等价 `awk '{print $1,$2}'`）。
    Fields(Vec<String>),
    /// `count`：计数（等价 `grep -c`）。
    Count,
    /// `group by FIELD`：按字段分组。
    GroupBy(String),
    /// `context N` / `context before:N after:M`：上下文行（等价 `grep -C` / `-B` / `-A`）。
    Context {
        /// 前文行数（`before:N` 或 `context N` 的 N）。
        before: usize,
        /// 后文行数（`after:N` 或 `context N` 的 N）。
        after: usize,
    },
    /// `files` / `files --invert`：只输出文件名（等价 `grep -l` / `-L`）。
    Files {
        /// `--invert` 为 true（等价 `grep -L`），否则 false（等价 `grep -l`）。
        invert: bool,
    },
    /// `stats FIELD`：流式聚合 min/max/avg（设计文档 §2.2 EBNF `stage` 规则）。
    ///
    /// 只做 min/max/avg（可流式）；p50/p95/p99 由 [`Stage::Percentiles`] 承担。
    /// 逐行更新 min/max/sum/count，不缓冲全量数据。
    /// 输出格式：`min: X, max: Y, avg: Z`（一行摘要）。
    Stats {
        /// 聚合字段名（如 `cost_ms`）。
        field: String,
    },
    /// `percentiles FIELD [p50,p95,p99]`：非流式百分位计算（§9.1 候选 #6 落地）。
    ///
    /// - `percentiles FIELD`：默认 p50/p95/p99
    /// - `percentiles FIELD p50,p95,p99`：指定百分位列表（逗号分隔，无方括号）
    ///
    /// 与 `stats` 区别：stats 做流式 min/max/avg（O(1) 内存），
    /// percentiles 做非流式全量百分位（O(N) 内存缓冲，与 sort 一致）。
    /// avg 掩盖长尾分布——SLO 报告需 p95/p99 衡量尾部延迟。
    ///
    /// 输出格式：`p50: X, p95: Y, p99: Z`（与 stats 的 `min: X, max: Y, avg: Z` 格式一致）。
    /// 终止 stage（输出摘要后链终止，与 stats/count 一致）。
    Percentiles {
        /// 聚合字段名（如 `cost_ms`）。
        field: String,
        /// 百分位列表（如 `[50.0, 95.0, 99.0]`）。默认 `[50, 95, 99]`。
        percentiles: Vec<f64>,
    },
    /// `trace [LANG]`：多行 stack trace 捕获（§9.1 候选 #2 落地）。
    ///
    /// - `trace`：自动检测语言（按命中首行特征）
    /// - `trace java` / `trace python` / `trace rust` / `trace js` / `trace go` / `trace csharp`
    ///   指定语言规则
    ///
    /// 场景：LLM agent 异常分析（§0.2 场景 A）——`gxp 'panic | trace rust' app.log`
    /// 匹配 panic 首行 + 按语言规则拉后续 stack 帧，输出一条带 `extra_lines` 的记录。
    ///
    /// 实现模型：L1 不变（仍只匹配首行模式），pipeline 阶段对每个命中行调用
    /// 语言边界规则，拉到 stack trace 结束 → 输出 `MatchedLine { line, extra_lines, ... }`。
    ///
    /// 终止 stage（与 stats/count/percentiles 一致）——输出多行记录后链终止。
    /// 流式路径（>512MB）不支持，fail-loud 提示用 `context N` 替代。
    Trace {
        /// 语言名。`None` = 自动检测；`Some(name)` = 指定语言规则。
        lang: Option<String>,
    },
}
