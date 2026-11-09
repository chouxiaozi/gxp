//! gxp DSL 解析器（nom 7.1）。
//!
//! 设计文档 §2.2 EBNF 子集（方案 D' 语法）：
//! - `or_expr := and_expr ('||' and_expr)*`
//! - `and_expr := unary ('&&' unary)*`（邻接不隐含布尔）
//! - `unary := '!' unary | prim`（`!` 词法规则方向 C，设计文档 §3.4）
//! - `prim := '(' expr ')' | STRING | REGEX | ast | capture | BARE | pred`
//! - `capture := 'capture' '(' NAME ',' REGEX ')'`
//! - `pred := NAME (OP VALUE | 'in' '[' VALUES ']' | 'exists' | 'between' V 'and' V)`
//! - `_ts between "ISO" "and" "ISO"`（时间戳 between）
//!
//! 管道切分（Rust 方案 §7.5 风险表）：
//! - 引号外 `|` 是管道动词分隔符，交给 [`crate::dsl::pipeline`] 处理
//! - 引号内 `|` 是 ERE OR（BARE / REGEX / STRING 里的字面）
//! - 跟踪引号深度扫描，避免误切
//!
//! `!` 词法规则方向 C（设计文档 §3.4）：
//! - `!` 是操作符当且仅当前后是空白或非字母数字边界
//! - 作用于 pred 必须用括号：`!(cost>800)` 合法，`!cost>800` 语法错误
//! - `!ERROR` 中 `!` 后是字母 → 字面 `!ERROR`（ERE 里 `!` 本就是字面）

use crate::dsl::ast::{CmpOp, Expr, Pred, Prim, Query};
use crate::error::{GxpError, Result};
use nom::branch::alt;
use nom::bytes::complete::{tag, take_while1};
use nom::character::complete::{char, multispace0, multispace1};
use nom::combinator::{recognize, verify};
use nom::error::Error as NomError;
use nom::multi::{many1, separated_list1};
use nom::sequence::{delimited, pair, preceded, tuple};
use nom::IResult;

/// 判断 `!` 在当前位置是否是操作符（vs 字面量）。
///
/// 规则（设计文档 §3.4 方向 C）：前后必须有空白或非字母数字边界。
/// - `prev_char = None`（行首）→ 前边界成立
/// - `prev_char` 是空白或非字母数字 → 前边界成立
/// - `next_char = None`（行尾）→ 后边界成立（但语法上不应出现）
/// - `next_char` 是空白、`(`、`[` 或非字母数字 → 后边界成立
///
/// # 示例
///
/// ```
/// use gxp::dsl::parser::is_bang_operator;
///
/// // 行首 + 空白 → 操作符
/// assert!(is_bang_operator(None, Some(' ')));
/// // 字母前 + 字母后 → 字面
/// assert!(!is_bang_operator(Some('a'), Some('E')));
/// // 空白前 + `(` 后 → 操作符
/// assert!(is_bang_operator(Some(' '), Some('(')));
/// ```
#[inline]
pub fn is_bang_operator(prev_char: Option<char>, next_char: Option<char>) -> bool {
    let prev_boundary = match prev_char {
        None => true, // 行首
        Some(c) => c.is_whitespace() || !c.is_alphanumeric(),
    };
    let next_boundary = match next_char {
        None => true, // 行尾（不应出现，但保守接受）
        Some(c) => c.is_whitespace() || c == '(' || c == '[' || !c.is_alphanumeric(),
    };
    prev_boundary && next_boundary
}

/// 解析 gxp DSL 查询表达式为 [`Query`]。
///
/// 完整流程：先切管道、再解析表达式段、最后解析管道段。
///
/// # 错误
///
/// - [`GxpError::Parse`]：表达式或管道动词解析失败
pub fn parse_query(input: &str) -> Result<Query> {
    let (expr_str, stages_tokens) = split_pipeline(input)?;
    let expr = parse_expr(expr_str.trim())?;
    let stages = if stages_tokens.is_empty() {
        Vec::new()
    } else {
        crate::dsl::pipeline::parse_pipeline(&stages_tokens)?
    };
    Ok(Query { expr, stages })
}

/// 解析 `where` 后的单 token 表达式（pipeline.rs `where` 分支用）。
///
/// 限制：`where` 后只允许单个 token 表达式（简单 case）。复杂 where 由上层重组。
/// 当前实现只接受完整表达式字符串（trim 后）。
pub fn parse_query_inline(input: &str) -> Result<Expr> {
    parse_expr(input.trim())
}

/// 按未配对的 `|` 切分管道（引号内的 `|` 视为 ERE OR，不切）。
///
/// 返回 `(expr_part, pipeline_tokens)`：
/// - `expr_part`：管道前的完整表达式（含引号）
/// - `pipeline_tokens`：所有管道段按空白切的 token 列表（已扁平化，`|` 分隔符被丢弃）
///
/// # 实现策略
///
/// 状态机扫描字符流，跟踪引号深度（`"` 和 `/` 两种）和括号深度。
/// 引号外、括号外的 `|` 视为管道分隔，但**仅当两侧都是空白或边界**时（设计文档 §2.4
/// 示例 1 vs 示例 8 的区分）：`a|b` 是 ERE OR（单 `|` 紧贴字母），`a | b` 才是管道。
/// `||` 总是 ERE OR（逻辑或），跳过两字符。
/// 未终止的 STRING / REGEX 报 fail-loud 错误（设计文档 §2.1 原则 4）。
fn split_pipeline(input: &str) -> Result<(&str, Vec<String>)> {
    let bytes = input.as_bytes();
    let mut split_pos = bytes.len();
    let mut i = 0;
    let mut in_string = false; // 双引号 STRING
    let mut in_regex = false; // /REGEX/
    let mut paren_depth = 0i32; // 括号内的 | 不是管道
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'"' {
                in_string = false;
            }
        } else if in_regex {
            // ERE 转义 `\|` 不算结束；未转义 `/` 结束
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == b'/' {
                in_regex = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'/' => in_regex = true, // 注意：BARE 里的 `/` 也被识别为 REGEX 起始
                b'(' | b'[' => paren_depth += 1,
                b')' | b']' => paren_depth -= 1,
                b'|' if paren_depth == 0 => {
                    // `||` 是 ERE OR（逻辑或），不是管道——跳过两字符
                    if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                        i += 2;
                        continue;
                    }
                    // 单 `|` 是管道当且仅当两侧都是空白或边界（ERE OR 紧贴字母）
                    let prev_is_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
                    let next_is_boundary =
                        i + 1 == bytes.len() || bytes[i + 1].is_ascii_whitespace();
                    if prev_is_boundary && next_is_boundary {
                        split_pos = i;
                        break;
                    }
                    // 否则视为 ERE OR 字面，继续扫描
                }
                _ => {}
            }
        }
        i += 1;
    }
    // fail-loud：扫描结束仍在 STRING/REGEX 内 → 未终止
    if in_string {
        return Err(GxpError::Parse {
            detail: "未终止的双引号 STRING（缺少闭合的 `\"`）".into(),
        });
    }
    if in_regex {
        return Err(GxpError::Parse {
            detail: "未终止的 REGEX（缺少闭合的 `/`）".into(),
        });
    }
    let mut expr_end = split_pos;
    while expr_end > 0 && input.as_bytes()[expr_end - 1].is_ascii_whitespace() {
        expr_end -= 1;
    }
    let expr_part = &input[..expr_end];
    let mut pipeline_tokens = Vec::new();
    if split_pos < bytes.len() {
        // 对管道剩余段按 `|` 切分（忽略引号/正则/括号内的 `|`），每段按空白切 token，扁平化
        let rest = &input[split_pos + 1..];
        pipeline_tokens = split_pipeline_stages(rest);
    }
    Ok((expr_part, pipeline_tokens))
}

/// 将管道剩余段按 `|` 切分成多个 stage 子串，每个子串按空白切 token（保留引号/正则内空格），
/// 然后扁平化成一个 `Vec<String>`。
///
/// 同 [`split_pipeline`] 的规则：单 `|` 是 stage 边界当且仅当两侧都是空白或边界。
/// `||` 是 ERE OR（逻辑或），跳过。引号/正则/括号内的 `|` 不切。
fn split_pipeline_stages(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut stage_start = 0usize;
    let mut i = 0usize;
    let mut in_string = false;
    let mut in_regex = false;
    let mut paren_depth = 0i32;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'"' {
                in_string = false;
            }
        } else if in_regex {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == b'/' {
                in_regex = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'/' => in_regex = true,
                b'(' | b'[' => paren_depth += 1,
                b')' | b']' => paren_depth -= 1,
                b'|' if paren_depth == 0 => {
                    // `||` 是 ERE OR，跳过两字符
                    if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                        i += 2;
                        continue;
                    }
                    // 单 `|` 是 stage 边界当且仅当两侧都是空白或边界
                    let prev_is_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
                    let next_is_boundary =
                        i + 1 == bytes.len() || bytes[i + 1].is_ascii_whitespace();
                    if prev_is_boundary && next_is_boundary {
                        let stage = &input[stage_start..i];
                        tokens.extend(split_by_whitespace_respecting_quotes(stage));
                        stage_start = i + 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    // 处理最后一段
    if stage_start < bytes.len() {
        let stage = &input[stage_start..];
        tokens.extend(split_by_whitespace_respecting_quotes(stage));
    }
    tokens
}

/// 按空白切 token，但保留引号内/正则内的空格（这些 token 不再被拆分）。
///
/// 设计文档 §2.1 原则 5：整个表达式用 shell 单引号包，表达式内 STRING 强制双引号。
/// 到达本函数时，shell 外层单引号已被剥离。
fn split_by_whitespace_respecting_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut cur = String::new();
    let mut in_string = false;
    let mut in_regex = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            cur.push(c as char);
            if c == b'"' {
                in_string = false;
            }
        } else if in_regex {
            cur.push(c as char);
            if c == b'\\' && i + 1 < bytes.len() {
                cur.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == b'/' {
                in_regex = false;
            }
        } else if c == b'"' {
            cur.push('"');
            in_string = true;
        } else if c == b'/' {
            cur.push('/');
            in_regex = true;
        } else if c.is_ascii_whitespace() {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c as char);
        }
        i += 1;
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// 解析表达式（递归下降入口）。
fn parse_expr(input: &str) -> Result<Expr> {
    match or_expr(input) {
        Ok(("", e)) => Ok(e),
        Ok((rest, _)) => Err(GxpError::Parse {
            detail: format!("表达式尾部未消费：{:?}", rest),
        }),
        Err(nom::Err::Error(e) | nom::Err::Failure(e)) => {
            // ast()/capture() 解析失败时给出 LLM 友好的错误消息
            let trimmed = input.trim_start();
            if trimmed.starts_with("ast") {
                Err(GxpError::Parse {
                    detail: format!(
                        "ast() 语法错误。正确语法：ast(NAME, \"PATTERN\")\n\
                         NAME 支持大小写不敏感：ts/rs/py/js/go/tsx/java/c/cpp/c++\n\
                         PATTERN 必须用双引号包裹（不支持单引号/无引号）\n\
                         示例：ast(java, \"logger.info($$$A)\")\n\
                         输入：{input}"
                    ),
                })
            } else if trimmed.starts_with("capture") {
                Err(GxpError::Parse {
                    detail: format!(
                        "capture() 语法错误。正确语法：capture(NAME, /REGEX/)\n\
                         NAME 是字段名（如 cost_ms）\n\
                         REGEX 必须用 /.../ 包裹\n\
                         示例：capture(cost_ms, /(\\d+)/)\n\
                         输入：{input}"
                    ),
                })
            } else {
                Err(GxpError::Parse {
                    detail: format!("解析失败：{}", e),
                })
            }
        }
        Err(nom::Err::Incomplete(_)) => Err(GxpError::Parse {
            detail: "输入不完整（nom Incomplete）".into(),
        }),
    }
}

// or_expr := and_expr ('||' and_expr)*
fn or_expr(input: &str) -> IResult<&str, Expr> {
    let (mut input, first) = and_expr(input)?;
    let mut acc = first;
    loop {
        let res: IResult<&str, (&str, &str)> = tuple((multispace0, tag("||")))(input);
        let (rest, _) = match res {
            Ok((rest, consumed)) => (rest, consumed),
            Err(_) => break,
        };
        let (rest, rhs) = and_expr(rest)?;
        acc = Expr::Or(Box::new(acc), Box::new(rhs));
        input = rest;
    }
    Ok((input, acc))
}

// and_expr := unary ('&&' unary)*
fn and_expr(input: &str) -> IResult<&str, Expr> {
    let (mut input, first) = unary(input)?;
    let mut acc = first;
    loop {
        let res: IResult<&str, (&str, &str)> = tuple((multispace0, tag("&&")))(input);
        let (rest, _) = match res {
            Ok((rest, consumed)) => (rest, consumed),
            Err(_) => break,
        };
        let (rest, rhs) = unary(rest)?;
        acc = Expr::And(Box::new(acc), Box::new(rhs));
        input = rest;
    }
    Ok((input, acc))
}

// unary := '!' unary | prim
// `!` 词法规则方向 C：前后必须是空白或非字母数字边界
fn unary(input: &str) -> IResult<&str, Expr> {
    // 跳过前导空白（&& || 之间的空格）
    let (input, _) = multispace0(input)?;
    // 检查 `!` 是否是操作符
    let bytes = input.as_bytes();
    if !bytes.is_empty() && bytes[0] == b'!' {
        let prev = None; // 当前位置已是 token 起始（前必为分隔或行首）
        let next = bytes.get(1).copied().map(|b| b as char);
        if is_bang_operator(prev, next) {
            // 操作符：跳过 `!` + 空白，解析 unary
            let (rest, _) = pair(char('!'), multispace0)(input)?;
            let (rest, inner) = unary(rest)?;
            return Ok((rest, Expr::Not(Box::new(inner))));
        }
    }
    // 否则按 prim 解析（`!ERROR` 走 BARE 分支）
    prim(input).map(|(rest, p)| (rest, Expr::Prim(p)))
}

// prim := '(' expr ')' | STRING | REGEX | ast | pred | BARE
fn prim(input: &str) -> IResult<&str, Prim> {
    // 1. 括号
    // v0.42 修复：未闭合括号静默吞 bug。原 `if let Ok` 失败时 fall through
    // 到 pred/BARE 分支，导致 `!(cost_ms>800`（缺 `)`）解析为 Not(Pred) 而非
    // 报错"未闭合括号"。修复：input 以 `(` 开头但 `delimited` 失败时 fail-loud。
    let trimmed = input.trim_start();
    if trimmed.starts_with('(') {
        match delimited(
            pair(char('('), multispace0),
            or_expr,
            pair(multispace0, char(')')),
        )(input)
        {
            Ok((rest, e)) => return Ok((rest, Prim::Paren(Box::new(e)))),
            Err(_) => {
                return Err(nom::Err::Error(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Char,
                )));
            }
        }
    }
    // 2. ast(NAME, STRING) 结构模式匹配（设计文档 §2.2 + §8.2）
    //    必须在 STRING/REGEX/BARE 之前——`ast` 关键字优先级高于 BARE 兜底
    //    fail-loud：输入以 `ast` 关键字开头但解析失败时报错，
    //    不静默落 BARE 兜底（否则 `ast(java, 'pattern')` 单引号/无引号 pattern 静默返回空）
    if input.trim_start().starts_with("ast") {
        return ast_literal(input).map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
        });
    }
    // 2a. capture(NAME, REGEX) 正则捕获组（设计文档 §2.2 EBNF `capture` 规则）
    //     必须在 STRING/REGEX/BARE 之前——`capture` 关键字优先级高于 BARE 兜底
    //     fail-loud：同 ast，`capture` 关键字开头但解析失败时报错
    if input.trim_start().starts_with("capture") {
        return capture_literal(input).map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
        });
    }
    // 3. STRING（强制双引号）
    if let Ok((rest, s)) = string_literal(input) {
        return Ok((rest, Prim::String(s)));
    }
    // 4. REGEX（/pattern/）
    if let Ok((rest, r)) = regex_literal(input) {
        return Ok((rest, Prim::Regex(r)));
    }
    // 5. pred（字段名 + 操作符/in/exists/between）
    if let Ok((rest, p)) = pred(input) {
        return Ok((rest, Prim::Pred(p)));
    }
    // 5a. v0.48：`NAME <非等值操作符> "STRING"` 形态 fail-loud，不落 BARE 兜底。
    // `level>"ERROR"` / `level!="ERROR"` 若落 BARE 会静默变成子串搜索（v0.42 静默吞
    // 同类）。仅在 pred 失败后调用——REGEX/STRING/ast/capture 已在前面试过，
    // 不会误伤 `/re>"x/` 这类正则字面量。定向 hint 由 mode 层 enrich（四段式模板）。
    if quoted_wrong_op_shape(input) {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }
    // 6. BARE 兜底（ERE 兼容，含空格匹配连续子串——但解析层只消费到下一个 `&&`/`||`/`|`/`)`）
    bare_literal(input).map(|(rest, b)| (rest, Prim::Bare(b)))
}

/// 检测 `NAME <op> "STRING"` 且 op 非等值（≠ `=` / `==`）的形态（v0.48 fail-loud）。
///
/// prim 的 pred 分支失败后调用：正常路径下 `=`/`==` 带引号 RHS 已被
/// `pred_compare` 脱糖为 In，到达这里说明要么操作符非法，要么 STRING 畸形
/// （如 `level=""` 空串——op 是 `=`，本函数返回 false，落 BARE 由 v0.42
/// guard 兜底报错）。
fn quoted_wrong_op_shape(input: &str) -> bool {
    let t = input.trim_start();
    // NAME：字母/数字/下划线（与 field_name 一致，允许前导 _ 伪字段）
    let name_end = t
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    if name_end == 0 {
        return false;
    }
    let after_name = t[name_end..].trim_start();
    // 操作符（长度优先，双字符在前）
    let Some(op) = ["!=", ">=", "<=", "==", ">", "<", "="]
        .into_iter()
        .find(|o| after_name.starts_with(o))
    else {
        return false;
    };
    let rest = after_name[op.len()..].trim_start();
    // 带引号 RHS + 非等值操作符 → 误用形态
    rest.starts_with('"') && op != "=" && op != "=="
}

/// `ast(NAME, STRING)` 解析（设计文档 §2.2 EBNF `ast` 规则）。
///
/// 语法：`ast ( NAME , STRING )`
/// - NAME：语言名（`py`/`js`/`ts`/`rs`/`go`/`c++`/...，字母数字 + 下划线 + `+`，大小写不敏感）
/// - STRING：双引号字面量 ast-grep pattern（如 `"useQuery($A)"`）
fn ast_literal(input: &str) -> IResult<&str, Prim> {
    // tuple 元素依次：tag("ast"), ms0, '(', ms0, NAME(identifier), ms0, ',', ms0, STRING(pattern), ms0, ')'
    let (rest, (_, _, _, _, lang, _, _, _, pattern, _, _)) = tuple((
        tag("ast"),
        multispace0,
        char('('),
        multispace0,
        // NAME：语言名（ts/py/js/rs/go/c++/...），字母数字 + 下划线 + `+`（支持 c++/C++）
        take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '+'),
        multispace0,
        char(','),
        multispace0,
        // STRING：双引号字面量 ast-grep pattern
        delimited(char('"'), take_while1(|c: char| c != '"'), char('"')),
        multispace0,
        char(')'),
    ))(input)?;
    Ok((
        rest,
        Prim::Ast {
            lang: lang.to_string(),
            pattern: pattern.to_string(),
        },
    ))
}

/// `capture(NAME, REGEX)` 解析（设计文档 §2.2 EBNF `capture` 规则）。
///
/// 语法：`capture ( NAME , /REGEX/ )`
/// - NAME：提取结果字段名（如 `trace_id`）
/// - REGEX：`/pattern/` 形式正则字面量（ERE）
fn capture_literal(input: &str) -> IResult<&str, Prim> {
    let (rest, (_, _, _, _, name, _, _, _, regex_pattern, _, _)) = tuple((
        tag("capture"),
        multispace0,
        char('('),
        multispace0,
        // NAME：字段名（字母数字下划线，允许前导 _）
        take_while1(|c: char| c.is_alphanumeric() || c == '_'),
        multispace0,
        char(','),
        multispace0,
        // REGEX：/pattern/ 形式正则字面量
        delimited(
            char('/'),
            recognize(many1(alt((
                preceded(char('\\'), nom::character::complete::anychar),
                verify(nom::character::complete::none_of("\\/"), |_| true),
            )))),
            char('/'),
        ),
        multispace0,
        char(')'),
    ))(input)?;
    Ok((
        rest,
        Prim::Capture {
            name: name.to_string(),
            pattern: regex_pattern.to_string(),
        },
    ))
}

/// 双引号 STRING 字面量。
fn string_literal(input: &str) -> IResult<&str, String> {
    let (rest, (_, body, _)) =
        tuple((char('"'), take_while1(|c: char| c != '"'), char('"')))(input)?;
    Ok((rest, body.to_string()))
}

/// `/pattern/` REGEX 字面量（ERE；`\/` 转义支持）。
fn regex_literal(input: &str) -> IResult<&str, String> {
    let (rest, (_, body, _)) = tuple((
        char('/'),
        recognize(many1(alt((
            preceded(char('\\'), nom::character::complete::anychar),
            verify(nom::character::complete::none_of("\\/"), |_| true),
        )))),
        char('/'),
    ))(input)?;
    Ok((rest, body.to_string()))
}

/// 字段名：字母/数字/下划线，允许前导 `_`（`_ts` / `_tag` 伪字段）。
fn field_name(input: &str) -> IResult<&str, String> {
    let (rest, name) = take_while1(|c: char| c.is_alphanumeric() || c == '_')(input)?;
    Ok((rest, name.to_string()))
}

/// 谓词值：数字、标识符、字面串（不含空白、`)`、`]`、`,`）。
fn pred_value(input: &str) -> IResult<&str, String> {
    let (rest, v) =
        take_while1(|c: char| !c.is_whitespace() && c != ')' && c != ']' && c != ',')(input)?;
    Ok((rest, v.to_string()))
}

/// BARE 字面量：消费到下一个 `&&`/`||`（逻辑操作符）/`)`/`]`/行尾。
///
/// 设计文档 §2.4 示例 1：`gxp 'ERROR timeout' app.log` ≡ `grep -E 'ERROR timeout'`
/// BARE 含空格时匹配连续子串，所以空格保留在 BARE 内。
///
/// **关键区分**：单个 `|` 在 BARE 内是 ERE OR（字面），不是逻辑 OR；只有 `||` 才是逻辑 OR。
/// 因此 BARE 终止条件是 `&&` 或 `||`（双竖线），而不是单个 `|`。
/// 管道 `|`（单竖线 + 两侧空白）已在 [`split_pipeline`] 阶段切走，到达本函数时剩余的单 `|` 一定是 ERE OR。
fn bare_literal(input: &str) -> IResult<&str, String> {
    let bytes = input.as_bytes();
    let mut end = 0usize;
    while end < bytes.len() {
        let c = bytes[end];
        // `&&` 是逻辑 AND，终止 BARE
        if c == b'&' {
            break;
        }
        // `||` 是逻辑 OR，终止 BARE；单个 `|` 是 ERE OR，保留在 BARE 内
        if c == b'|' {
            // 只在双竖线时终止
            if end + 1 < bytes.len() && bytes[end + 1] == b'|' {
                break;
            }
            // 单个 `|` 是 ERE OR，继续消费
        }
        // 结构性括号终止 BARE
        if c == b')' || c == b']' {
            break;
        }
        end += 1;
    }
    if end == 0 {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::NonEmpty,
        )));
    }
    Ok((&input[end..], input[..end].trim_end().to_string()))
}

/// 谓词分支：尝试 pred 规则。
///
/// pred := NAME ('>'|'>='|'<'|'<='|'=='|'!='|'=') VALUE
///       | NAME 'in' '[' VALUE (',' VALUE)* ']'
///       | NAME 'exists'
///       | NAME 'between' VALUE 'and' VALUE
fn pred(input: &str) -> IResult<&str, Pred> {
    // 先消费 NAME
    let (rest, name) = field_name(input)?;
    // 跳过空白
    let (rest, _) = multispace0(rest)?;
    // 顺序尝试各分支（avoid alt+闭包捕获生命周期问题）
    if let Ok((rest, p)) = pred_compare(&name, rest) {
        return Ok((rest, p));
    }
    // v0.42 修复：未闭合 in 列表静默吞 bug。
    // 原 `if let Ok` 失败时 fall through 到 exists/between/BARE 兜底，
    // 导致 `_tag in [a, b`（缺 `]`）静默当字面量搜索（exit 0 无输出）。
    // 修复：检测到 `NAME in [` 但 pred_in 失败时 fail-loud（同 ast/capture 模式）。
    if rest.trim_start().starts_with("in") {
        return pred_in(&name, rest).map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
        });
    }
    if let Ok((rest, p)) = pred_in(&name, rest) {
        return Ok((rest, p));
    }
    if let Ok((rest, p)) = pred_exists(&name, rest) {
        return Ok((rest, p));
    }
    // v0.42 修复：未闭合 between 静默吞 bug（同 in）。
    // `cost between 100`（缺 `and 200`）原 fall through 到 BARE 当字面量。
    if rest.trim_start().starts_with("between") {
        return pred_between(&name, rest).map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
        });
    }
    if let Ok((rest, p)) = pred_between(&name, rest) {
        return Ok((rest, p));
    }
    Err(nom::Err::Error(NomError::new(
        rest,
        nom::error::ErrorKind::Alt,
    )))
}

/// 数值前瞻（粗筛，v0.48）：首字符数字/`-`/`+`/`.` 视为数值候选。
/// 精确校验由编译期 D5 11 维契约做；误放行（如 `+x`）仍会被 D5 拒绝，fail-loud 不变。
fn looks_numeric(v: &str) -> bool {
    v.chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')
}

fn pred_compare<'a>(name: &str, input: &'a str) -> IResult<&'a str, Pred> {
    // 按长度优先匹配 >=/<=/==/!=/>/<
    let (rest, op) = alt((
        tag(">="),
        tag("<="),
        tag("=="),
        tag("!="),
        tag("="),
        tag(">"),
        tag("<"),
    ))(input)?;
    let op = match op {
        ">=" => CmpOp::Ge,
        "<=" => CmpOp::Le,
        "==" => CmpOp::Eq,
        "!=" => CmpOp::Ne,
        "=" => CmpOp::Assign,
        ">" => CmpOp::Gt,
        "<" => CmpOp::Lt,
        _ => unreachable!(),
    };
    let (rest, _) = multispace0(rest)?;
    // v0.48 字符串等值脱糖（设计文档 §3.2）：`NAME ("="|"==") "STRING"` → 单元素 In。
    // 构造性等价（同一 In 节点）——L1 下推 / Or 组完整性 / Not 豁免 / 伪字段 /
    // captures 优先级全部继承 `in` 现有路径，零新增语义分支。
    if matches!(op, CmpOp::Eq | CmpOp::Assign) && rest.starts_with('"') {
        let (rest, s) = string_literal(rest)?;
        return Ok((
            rest,
            Pred::In {
                name: name.to_string(),
                values: vec![s],
            },
        ));
    }
    // v0.48：非等值操作符 + 带引号 RHS → fail-loud（字符串无大小序；`!=` 的正确写法
    // 是 `!(NAME="...")`）。返回 Err 后由 prim 的 shape guard 拦截（不落 BARE 兜底），
    // 定向 hint 在 mode/gxp.rs::enrich_string_eq_hint 附加。
    if rest.starts_with('"') {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }
    let (rest, value) = pred_value(rest)?;
    // v0.48：`=`（单等号）+ 无引号非数值 RHS → 返回 Err 落 BARE 兜底（ERE 字面量）。
    // 设计文档 §2.2：`=` 不在 BARE 谓词分歧操作符列表（> < >= <= == !=），`a=b` 应与
    // grep -E 一致；v0.47 实测却是 runtime 数值错误（文档-实现不一致，本处对齐）。
    // `==` 是有意分歧操作符：非数值 RHS 保持 Compare（编译期 D5 拒绝），不走 BARE。
    if matches!(op, CmpOp::Assign) && !looks_numeric(&value) {
        return Err(nom::Err::Error(NomError::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }
    Ok((
        rest,
        Pred::Compare {
            name: name.to_string(),
            op,
            value,
        },
    ))
}

fn pred_in<'a>(name: &str, input: &'a str) -> IResult<&'a str, Pred> {
    let (rest, _) = tag("in")(input)?;
    let (rest, _) = multispace1(rest)?;
    let (rest, _) = char('[')(rest)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, values) =
        separated_list1(tuple((multispace0, char(','), multispace0)), pred_value)(rest)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, _) = char(']')(rest)?;
    Ok((
        rest,
        Pred::In {
            name: name.to_string(),
            values,
        },
    ))
}

fn pred_exists<'a>(name: &str, input: &'a str) -> IResult<&'a str, Pred> {
    let (rest, _) = tag("exists")(input)?;
    Ok((
        rest,
        Pred::Exists {
            name: name.to_string(),
        },
    ))
}

fn pred_between<'a>(name: &str, input: &'a str) -> IResult<&'a str, Pred> {
    let (rest, _) = tag("between")(input)?;
    let (rest, _) = multispace1(rest)?;
    // _ts between "ISO" "ISO" 支持双引号 STRING 边界值
    // 时间戳含空格/冒号/T/Z，需要双引号包裹
    if name == "_ts" {
        let (rest, lo) = string_literal(rest)?;
        let (rest, _) = multispace1(rest)?;
        let (rest, _) = tag("and")(rest)?;
        let (rest, _) = multispace1(rest)?;
        let (rest, hi) = string_literal(rest)?;
        Ok((
            rest,
            Pred::Between {
                name: name.to_string(),
                lo,
                hi,
            },
        ))
    } else {
        // 数值 between：VALUE 是未引号数值
        let (rest, lo) = pred_value(rest)?;
        let (rest, _) = multispace1(rest)?;
        let (rest, _) = tag("and")(rest)?;
        let (rest, _) = multispace1(rest)?;
        let (rest, hi) = pred_value(rest)?;
        Ok((
            rest,
            Pred::Between {
                name: name.to_string(),
                lo,
                hi,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    //! Parser 单测：核心 `!` 词法 + 递归下降。
    //! 完整 11 个示例查询断言放集成测试 `tests/dsl_parser.rs`。

    use super::*;

    #[test]
    fn test_is_bang_operator_boundary_rules() {
        // 行首 + 空白 → 操作符
        assert!(is_bang_operator(None, Some(' ')));
        assert!(is_bang_operator(None, Some('(')));
        // 字母前 + 字母后 → 字面
        assert!(!is_bang_operator(Some('a'), Some('E')));
        // 空白前 + 字母后 → 字面（`!b` 中 ! 是字面）
        assert!(!is_bang_operator(Some(' '), Some('b')));
        // 空白前 + `(` 后 → 操作符
        assert!(is_bang_operator(Some(' '), Some('(')));
    }

    #[test]
    fn test_parse_string_eq_desugar_single_element_in() {
        // v0.48 字符串等值脱糖：level="ERROR" ≡ level in [ERROR]（设计文档 §3.2）
        let q = parse_query(r#"level="ERROR""#).unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::In { name, values })) => {
                assert_eq!(name, "level");
                assert_eq!(values, vec!["ERROR".to_string()]);
            }
            other => panic!("期望单元素 In，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_string_eq_double_equals_and_spaces() {
        // == 同样接受带引号 RHS；STRING 允许空格（单一记号，强于 in 裸词列表）
        let q = parse_query(r#"level == "WARN INFO""#).unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::In { name, values })) => {
                assert_eq!(name, "level");
                assert_eq!(values, vec!["WARN INFO".to_string()]);
            }
            other => panic!("期望单元素 In，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_numeric_eq_unchanged() {
        // 无引号数值：`=`/`==` 数值等值（D5 原样，v0.47 行为不变）
        let q = parse_query("cost_ms=800").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Compare { name, op, value })) => {
                assert_eq!(
                    (name.as_str(), op, value.as_str()),
                    ("cost_ms", CmpOp::Assign, "800")
                );
            }
            other => panic!("期望 Compare，得到 {:?}", other),
        }
        let q = parse_query("cost_ms==800").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Compare { op, .. })) => assert_eq!(op, CmpOp::Eq),
            other => panic!("期望 Compare，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_single_eq_non_numeric_falls_back_to_bare() {
        // v0.48：`=` 单等号非数值 RHS → BARE ERE 字面量（对齐设计文档分歧操作符列表）
        let q = parse_query("a=b").unwrap();
        match q.expr {
            Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "a=b"),
            other => panic!("期望 BARE，得到 {:?}", other),
        }
        // `==` 是有意分歧操作符：非数值 RHS 保持 Compare（编译期 D5 拒绝）
        let q = parse_query("a==b").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Compare { op, value, .. })) => {
                assert_eq!(op, CmpOp::Eq);
                assert_eq!(value, "b");
            }
            other => panic!("期望 Compare，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_quoted_rhs_wrong_op_fails_loud() {
        // 带引号 RHS + 非等值操作符 → 解析失败（不落 BARE 兜底）
        assert!(parse_query(r#"level>"ERROR""#).is_err());
        assert!(parse_query(r#"level!="ERROR""#).is_err());
        assert!(parse_query(r#"level >= "x""#).is_err());
        // 合法对照（不应误伤）
        assert!(parse_query(r#"level="ERROR""#).is_ok());
        assert!(parse_query(r#"level=="ERROR""#).is_ok());
    }

    #[test]
    fn test_parse_string_eq_in_bool_context() {
        // && / Not 上下文中的字符串等值（§3.4 D6：作用于 pred 必须括号）
        let q = parse_query(r#""timeout" && level="ERROR""#).unwrap();
        match q.expr {
            Expr::And(_, rhs) => {
                assert!(matches!(*rhs, Expr::Prim(Prim::Pred(Pred::In { .. }))));
            }
            other => panic!("期望 And，得到 {:?}", other),
        }
        let q = parse_query(r#"!(level="ERROR")"#).unwrap();
        match q.expr {
            // `!(...)` 的 prim 是 Paren 包裹（§3.4：! 作用于 pred 必须用括号）
            Expr::Not(inner) => match *inner {
                Expr::Prim(Prim::Paren(inner2)) => {
                    assert!(
                        matches!(*inner2, Expr::Prim(Prim::Pred(Pred::In { .. }))),
                        "Paren 内应为 In：{:?}",
                        inner2
                    );
                }
                other2 => panic!("期望 Paren 包裹，得到 {:?}", other2),
            },
            other => panic!("期望 Not，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_bare_literal_simple() {
        // BARE 兜底：ERROR 走 Prim::Bare
        let q = parse_query("ERROR").unwrap();
        match q.expr {
            Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "ERROR"),
            other => panic!("期望 Prim::Bare，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_string_literal() {
        let q = parse_query(r#""timeout""#).unwrap();
        match q.expr {
            Expr::Prim(Prim::String(s)) => assert_eq!(s, "timeout"),
            other => panic!("期望 Prim::String，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_regex_literal() {
        let q = parse_query(r"/cost>800/").unwrap();
        match q.expr {
            Expr::Prim(Prim::Regex(s)) => assert_eq!(s, "cost>800"),
            other => panic!("期望 Prim::Regex，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_and_of_two_strings() {
        let q = parse_query(r#""a" && "b""#).unwrap();
        match q.expr {
            Expr::And(l, r) => {
                assert!(
                    matches!(*l, Expr::Prim(Prim::String(_))),
                    "左枝不是 String：{:?}",
                    l
                );
                assert!(
                    matches!(*r, Expr::Prim(Prim::String(_))),
                    "右枝不是 String：{:?}",
                    r
                );
            }
            other => panic!("期望 And，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_or_of_two_strings() {
        let q = parse_query(r#""a" || "b""#).unwrap();
        assert!(matches!(q.expr, Expr::Or(_, _)));
    }

    #[test]
    fn test_parse_not_with_paren_pred() {
        // !(cost>800) —— 方向 C 要求括号
        let q = parse_query("!(cost>800)").unwrap();
        match q.expr {
            Expr::Not(inner) => match *inner {
                Expr::Prim(Prim::Paren(_)) => {}
                other => panic!("期望 Paren，得到 {:?}", other),
            },
            other => panic!("期望 Not，得到 {:?}", other),
        }
    }

    // v0.42 修复：未闭合括号静默吞 bug——`!(cost>800` 缺 `)` 原静默匹配所有行
    #[test]
    fn test_parse_unclosed_paren_after_bang_fails_loud() {
        // `!(cost>800`（缺 `)`）应 fail-loud，不应静默解析为 Not(Pred)
        let r = parse_query("!(cost>800");
        assert!(r.is_err(), "未闭合括号应报 parse error，得到: {:?}", r);
        let err = format!("{}", r.unwrap_err());
        assert!(
            err.contains("解析失败") || err.contains("Char"),
            "错误消息应含 parse error 上下文，得到: {err}"
        );
    }

    #[test]
    fn test_parse_unclosed_paren_with_pred_fails_loud() {
        // `(cost>800`（缺 `)`）应 fail-loud
        let r = parse_query("(cost>800");
        assert!(r.is_err(), "未闭合括号应报 parse error");
        let err = format!("{}", r.unwrap_err());
        assert!(
            err.contains("解析失败") || err.contains("Char"),
            "错误消息应含 parse error 上下文，得到: {err}"
        );
    }

    #[test]
    fn test_parse_unclosed_paren_in_and_fails_loud() {
        // `"x" && (cost>800` 应 fail-loud
        let r = parse_query(r#""x" && (cost>800"#);
        assert!(r.is_err(), "未闭合括号在 && 后应报 parse error");
    }

    #[test]
    fn test_parse_unclosed_paren_in_or_fails_loud() {
        // `"x" || (cost>800` 应 fail-loud
        let r = parse_query(r#""x" || (cost>800"#);
        assert!(r.is_err(), "未闭合括号在 || 后应报 parse error");
    }

    #[test]
    fn test_parse_closed_paren_still_works() {
        // 闭合括号正常解析（确认修复未误伤）
        let q = parse_query("(cost>800)").unwrap();
        match q.expr {
            Expr::Prim(Prim::Paren(_)) => {}
            other => panic!("期望 Paren，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_nested_closed_paren_still_works() {
        let q = parse_query("((cost>800))").unwrap();
        match q.expr {
            Expr::Prim(Prim::Paren(_)) => {}
            other => panic!("期望 Paren，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_pred_compare_gt() {
        let q = parse_query("cost_ms>800").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Compare { name, op, value })) => {
                assert_eq!(name, "cost_ms");
                assert_eq!(op, CmpOp::Gt);
                assert_eq!(value, "800");
            }
            other => panic!("期望 Pred::Compare，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_pred_in_list() {
        let q = parse_query("_tag in [checkout, billing]").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::In { name, values })) => {
                assert_eq!(name, "_tag");
                assert_eq!(values, vec!["checkout".to_string(), "billing".into()]);
            }
            other => panic!("期望 Pred::In，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_pred_exists() {
        let q = parse_query("cost_ms exists").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Exists { name })) => {
                assert_eq!(name, "cost_ms");
            }
            other => panic!("期望 Pred::Exists，得到 {:?}", other),
        }
    }

    #[test]
    fn test_parse_pred_between() {
        let q = parse_query("cost_ms between 100 and 200").unwrap();
        match q.expr {
            Expr::Prim(Prim::Pred(Pred::Between { name, lo, hi })) => {
                assert_eq!(name, "cost_ms");
                assert_eq!(lo, "100");
                assert_eq!(hi, "200");
            }
            other => panic!("期望 Pred::Between，得到 {:?}", other),
        }
    }

    #[test]
    fn test_split_pipeline_respects_quotes() {
        // 引号内 `|` 不切；引号外 `|` 切
        let (expr, toks) = split_pipeline(r#""a|b" | count"#).unwrap();
        assert_eq!(expr, r#""a|b""#);
        assert_eq!(toks, vec!["count".to_string()]);
    }

    #[test]
    fn test_split_pipeline_respects_regex() {
        let (expr, toks) = split_pipeline(r"/a\|b/ | count").unwrap();
        assert_eq!(expr, r"/a\|b/");
        assert_eq!(toks, vec!["count".to_string()]);
    }

    #[test]
    fn test_split_pipeline_no_pipe_returns_empty_tokens() {
        let (expr, toks) = split_pipeline("ERROR").unwrap();
        assert_eq!(expr, "ERROR");
        assert!(toks.is_empty());
    }

    #[test]
    fn test_split_pipeline_paren_depth_protects_pipe() {
        // 括号内的 | 不是管道
        let (expr, toks) = split_pipeline("(a | b) && c | count").unwrap();
        assert_eq!(expr, "(a | b) && c");
        assert_eq!(toks, vec!["count".to_string()]);
    }

    #[test]
    fn test_split_pipeline_multiple_stages_flattened() {
        // 多个 stage 按 `|` 切分后扁平化成一个 token 列表
        let (expr, toks) = split_pipeline("ERROR | context 3 | count").unwrap();
        assert_eq!(expr, "ERROR");
        assert_eq!(toks, vec!["context", "3", "count"]);
    }

    #[test]
    fn test_split_pipeline_unterminated_string_fails_loud() {
        // 未终止的 STRING → fail-loud
        let err = split_pipeline(r#""unterminated"#).unwrap_err();
        assert!(format!("{err}").contains("STRING"));
    }

    #[test]
    fn test_parse_ast_literal() {
        // ast(ts, "useQuery($$$A)") → Prim::Ast { lang: "ts", pattern: "useQuery($$$A)" }
        let q = parse_query(r#"ast(ts, "useQuery($$$A)")"#).expect("ast() 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Ast { lang, pattern }) => {
                assert_eq!(lang, "ts", "lang 应为 ts");
                assert_eq!(pattern, "useQuery($$$A)", "pattern 应为 useQuery($$$A)");
            }
            other => panic!("应为 Prim::Ast，实际: {other:?}"),
        }
        assert!(q.stages.is_empty(), "无管道 stage");
    }

    #[test]
    fn test_parse_ast_with_spaces() {
        // ast( ts , "useQuery($A)" ) → 容忍空白
        let q = parse_query(r#"ast( ts , "useQuery($A)" )"#).expect("ast() 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Ast { lang, pattern }) => {
                assert_eq!(lang, "ts");
                assert_eq!(pattern, "useQuery($A)");
            }
            other => panic!("应为 Prim::Ast，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parse_ast_combined_with_literal() {
        // "data" && ast(ts, "useQuery($$$A)")
        let q =
            parse_query(r#""data" && ast(ts, "useQuery($$$A)")"#).expect("ast + 字面量组合应解析");
        match &q.expr {
            Expr::And(l, r) => {
                assert!(
                    matches!(**l, Expr::Prim(Prim::String(_))),
                    "左应是 String，实际: {l:?}"
                );
                if let Expr::Prim(Prim::Ast { lang, .. }) = &**r {
                    assert_eq!(lang, "ts", "右应为 Ast(ts, ...)");
                } else {
                    panic!("右应为 Ast，实际: {r:?}");
                }
            }
            other => panic!("应为 And，实际: {other:?}"),
        }
    }

    // capture(NAME, /REGEX/)

    #[test]
    fn test_parse_capture_basic() {
        // capture(trace_id, /req-\d+/) → Prim::Capture { name: "trace_id", pattern: "req-\\d+" }
        let q = parse_query(r"capture(trace_id, /req-\d+/)").expect("capture() 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Capture { name, pattern }) => {
                assert_eq!(name, "trace_id", "name 应为 trace_id");
                assert_eq!(pattern, "req-\\d+", "pattern 应为 req-\\d+");
            }
            other => panic!("应为 Prim::Capture，实际: {other:?}"),
        }
        assert!(q.stages.is_empty(), "无管道 stage");
    }

    #[test]
    fn test_parse_capture_with_spaces() {
        // capture( trace_id , /req-\d+/ ) → 容忍空白
        let q = parse_query(r"capture( trace_id , /req-\d+/ )").expect("capture() 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Capture { name, pattern }) => {
                assert_eq!(name, "trace_id");
                assert_eq!(pattern, "req-\\d+");
            }
            other => panic!("应为 Prim::Capture，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parse_capture_combined_with_literal() {
        // "ERROR" && capture(user_id, /uid=\d+/)
        let q = parse_query(r#""ERROR" && capture(user_id, /uid=\d+/)"#)
            .expect("capture + 字面量组合应解析");
        match &q.expr {
            Expr::And(l, r) => {
                assert!(
                    matches!(**l, Expr::Prim(Prim::String(_))),
                    "左应是 String，实际: {l:?}"
                );
                if let Expr::Prim(Prim::Capture { name, .. }) = &**r {
                    assert_eq!(name, "user_id", "右应为 Capture(user_id, ...)");
                } else {
                    panic!("右应为 Capture，实际: {r:?}");
                }
            }
            other => panic!("应为 And，实际: {other:?}"),
        }
    }

    // _ts between "ISO" "ISO"

    #[test]
    fn test_parse_ts_between_quoted_strings() {
        let q = parse_query(r#"_ts between "2026-10-20T08:00:00Z" and "2026-10-20T09:00:00Z""#)
            .expect("_ts between 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Pred(Pred::Between { name, lo, hi })) => {
                assert_eq!(name, "_ts");
                assert_eq!(lo, "2026-10-20T08:00:00Z");
                assert_eq!(hi, "2026-10-20T09:00:00Z");
            }
            other => panic!("应为 Pred::Between，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parse_ts_between_with_offset() {
        let q = parse_query(
            r#"_ts between "2026-10-20T08:00:00+08:00" and "2026-10-20T09:00:00+08:00""#,
        )
        .expect("_ts between with offset 应解析成功");
        match &q.expr {
            Expr::Prim(Prim::Pred(Pred::Between { name, .. })) => {
                assert_eq!(name, "_ts");
            }
            other => panic!("应为 Pred::Between，实际: {other:?}"),
        }
    }

    #[test]
    fn test_parse_numeric_between_still_works() {
        // 确保数值 between 仍用 pred_value（非引号）
        let q = parse_query("cost_ms between 100 and 200").unwrap();
        match &q.expr {
            Expr::Prim(Prim::Pred(Pred::Between { name, lo, hi })) => {
                assert_eq!(name, "cost_ms");
                assert_eq!(lo, "100");
                assert_eq!(hi, "200");
            }
            other => panic!("应为 Pred::Between，实际: {other:?}"),
        }
    }
}
