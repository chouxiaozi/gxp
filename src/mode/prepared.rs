//! L2 谓词预编译产物（与 AST `Pred`/`Expr`/`Prim` 一一对应，阈值/边界已预解析为 f64）。
//!
//! 设计文档 §3.3 + D5：`Compare`/`Between` 的数值阈值在编译期解析一次，
//! 避免每行求值时重复 `f64::from_str`。`In`/`Exists` 不涉及数值解析。

use crate::dsl::ast::{CmpOp, Expr, Pred, Prim};
use crate::l2::build_between_bounds;
use crate::l2::time::parse_timestamp;

/// L2 谓词预编译产物（与 AST `Pred` 一一对应，阈值/边界已预解析为 f64）。
///
/// 设计文档 §3.3 + D5：`Compare`/`Between` 的数值阈值在编译期解析一次，
/// 避免每行求值时重复 `f64::from_str`。`In`/`Exists` 不涉及数值解析。
#[derive(Debug, Clone)]
pub(crate) enum PreparedPred {
    /// `NAME OP VALUE`：数值比较。
    Compare {
        /// 字段名。
        field: String,
        /// 比较操作符。
        op: CmpOp,
        /// 阈值（已按 11 维契约解析为 f64）。
        threshold: f64,
    },
    /// `NAME in [v1, v2, ...]`：字节字面量集合判定。
    In {
        /// 字段名。
        field: String,
        /// 候选值列表（字节字面量，原样保留）。
        values: Vec<String>,
    },
    /// `NAME exists`：字段存在性。
    Exists {
        /// 字段名。
        field: String,
    },
    /// `NAME between LO and HI`：闭区间数值判定。
    Between {
        /// 字段名。
        field: String,
        /// 下界（已按 11 维契约解析为 f64）。
        lo: f64,
        /// 上界（已按 11 维契约解析为 f64）。
        hi: f64,
    },
    /// `_ts between "ISO" "and" "ISO"`：时间戳区间判定（设计文档 §2.2 EBNF）。
    ///
    /// `_ts` 伪字段特例：D5 不适用（`_ts` 是伪字段不是数值字段）。
    /// speedate 解析时间戳为 epoch seconds 比较。
    TimeBetween {
        /// 字段名（固定为 `_ts`）。
        field: String,
        /// 下界（已解析为 epoch seconds）。
        lo: i64,
        /// 上界（已解析为 epoch seconds）。
        hi: i64,
    },
}

/// L2 预编译表达式树（与 AST `Expr` 同构，`Prim::Pred` 节点替换为 `PreparedPred`）。
///
/// 递归求值时消费此树，不再回头解析 AST 字节字面量。
#[derive(Debug, Clone)]
pub(crate) enum PreparedExpr {
    /// `a || b`。
    Or(Box<PreparedExpr>, Box<PreparedExpr>),
    /// `a && b`。
    And(Box<PreparedExpr>, Box<PreparedExpr>),
    /// `! unary`：三态取反（见 [`crate::mode::eval::eval_prepared_with_captures`]）。
    Not(Box<PreparedExpr>),
    /// prim：字面量（L1 已命中）或预编译谓词。
    Prim(PreparedPrim),
}

/// 预编译 prim 节点。
#[derive(Debug, Clone)]
pub(crate) enum PreparedPrim {
    /// 括号分组。
    Paren(Box<PreparedExpr>),
    /// STRING / REGEX / BARE 字面量：L1 阶段已做必选组粗筛。
    ///
    /// **v0.42 修正**：原设计为"unit variant"（无字段），L2 视为永真
    /// （`eval_prepared_prim_with_captures` 直接返回 `EvalResult::True`）——在非 Not
    /// 上下文正确（L1 已 filter），但 **Not(Literal) 上下文错误**：
    /// L1 不下推 Not 内部字面量（`compile.rs:258` "保守不漏"），L1 无
    /// 必选组 → match-all，L2 视 Literal 为永真 → `Not(Literal)=False`
    /// 对所有行（包括不含字面量的行）。修复后 Literal 携带编译后的正则，
    /// L2 实际跑 `is_match` 校验：
    /// - 非 Not 上下文：L1 已 filter，L2 校验冗余但正确（不漏不增）
    /// - Not 上下文：L1 未 filter，L2 校验是必需的（否则 Not 语义破裂）
    ///
    /// **v0.49 Opt A（`l1_verified`，Rust 方案 §7.9）**：当该字面量字符串是
    /// `CompiledL1::verified_literals` 成员（单元素纯字面量必选组）时，
    /// L1 幸存行构造性保证子串存在，L2 豁免 `regex.is_match` 重校验直接 True。
    /// 同串跨子树（含 Not 内）语义一致：行必含该子串 ⇒ 任意位置同串节点恒 True。
    /// Regex 字面量永不 verified（L1 不保证正则命中——aho 只有字面量）。
    Literal {
        /// 编译后的正则（String/Bare 用 `regex::escape` 转义，Regex 用原文）。
        regex: regex::bytes::Regex,
        /// L1 已构造性保证该字面量存在（`verified_literals` 成员）时为 true，
        /// eval 豁免重校验。where 等无 L1 保证的上下文构造时传空集 → false。
        l1_verified: bool,
    },
    /// 预编译字段谓词。
    Pred(PreparedPred),
    /// `ast(NAME, STRING)` 结构模式匹配（设计文档 §2.2 + §8.2）。
    ///
    /// L1 不下推（视为可能命中），L2 求值时查 `ast_lines` 行号集合（由 `l3::structural::run_all_ast`
    /// 预先对整个文件跑 ast-grep find_all 得到）。
    ///
    /// feature `l3-structural` 关闭时，lang/pattern 字段被构造但从不读取
    /// （L2 求值时 `eval_prepared_prim_with_captures` 视 Ast 为永真），故加 `allow(dead_code)`。
    #[allow(dead_code)]
    Ast {
        /// 语言名（如 `"ts"`）。
        lang: String,
        /// ast-grep pattern（如 `useQuery($A)`）。
        pattern: String,
    },
    /// `capture(NAME, REGEX)` 正则捕获组提取（设计文档 §2.2 EBNF `capture` 规则）。
    ///
    /// L1 不下推（视为可能命中），L2 求值时对每行跑正则 find 提取字段值。
    /// 正则在 `prepare_prim` 阶段编译一次（`regex::bytes::Regex`），避免每行重复编译。
    Capture {
        /// 提取结果字段名（如 `trace_id`）。
        #[allow(dead_code)]
        name: String,
        /// 编译后的正则（用于每行 find）。
        regex: regex::bytes::Regex,
    },
}

/// 把 AST `Expr` 编译为 `PreparedExpr`：递归 walk，Pred 节点预解析阈值/边界。
///
/// `verified`：L1 已构造性保证存在的字面量集合（`CompiledL1::verified_literals`），
/// String/Bare 字面量命中时置 `l1_verified=true`（v0.49 Opt A）。
/// 无 L1 保证的上下文（apply_where 等）传 `&[]`。
///
/// 错误（fail-loud）：
/// - `Compare` 阈值不是合法数值（11 维契约拒绝）→ `GxpError::Runtime`
/// - `Between` 端点不是合法数值 → `GxpError::Runtime`
pub(crate) fn prepare_expr(expr: &Expr, verified: &[Vec<u8>]) -> crate::Result<PreparedExpr> {
    match expr {
        Expr::Or(l, r) => Ok(PreparedExpr::Or(
            Box::new(prepare_expr(l, verified)?),
            Box::new(prepare_expr(r, verified)?),
        )),
        Expr::And(l, r) => Ok(PreparedExpr::And(
            Box::new(prepare_expr(l, verified)?),
            Box::new(prepare_expr(r, verified)?),
        )),
        Expr::Not(e) => Ok(PreparedExpr::Not(Box::new(prepare_expr(e, verified)?))),
        Expr::Prim(prim) => Ok(PreparedExpr::Prim(prepare_prim(prim, verified)?)),
    }
}

/// 编译 `Prim` 节点。
pub(crate) fn prepare_prim(prim: &Prim, verified: &[Vec<u8>]) -> crate::Result<PreparedPrim> {
    match prim {
        Prim::Paren(e) => Ok(PreparedPrim::Paren(Box::new(prepare_expr(e, verified)?))),
        // v0.42：Literal 携带编译后的正则，供 L2 实际校验（修复 Not(Literal) bug）
        Prim::String(s) | Prim::Bare(s) => {
            // 字面量子串匹配：regex::escape 转义后作为 pattern
            let pattern = regex::escape(s);
            let re = regex::bytes::Regex::new(&pattern).map_err(|e| crate::GxpError::Runtime {
                detail: format!("字面量正则编译失败: {e}"),
            })?;
            // v0.49 Opt A：verified 成员 → L2 豁免重校验（Rust 方案 §7.9）
            let l1_verified = verified.iter().any(|v| v.as_slice() == s.as_bytes());
            Ok(PreparedPrim::Literal {
                regex: re,
                l1_verified,
            })
        }
        Prim::Regex(r) => {
            let re = regex::bytes::Regex::new(r).map_err(|e| crate::GxpError::Runtime {
                detail: format!("REGEX 字面量编译失败: {e}"),
            })?;
            // Regex 永不 l1_verified：L1 aho 只含字面量，不保证正则命中
            Ok(PreparedPrim::Literal {
                regex: re,
                l1_verified: false,
            })
        }
        Prim::Pred(p) => Ok(PreparedPrim::Pred(prepare_pred(p)?)),
        Prim::Ast { lang, pattern } => Ok(PreparedPrim::Ast {
            lang: lang.clone(),
            pattern: pattern.clone(),
        }),
        Prim::Capture { name, pattern } => {
            let re = regex::bytes::Regex::new(pattern).map_err(|e| crate::GxpError::Runtime {
                detail: format!("capture 正则编译失败: {e}"),
            })?;
            Ok(PreparedPrim::Capture {
                name: name.clone(),
                regex: re,
            })
        }
    }
}

/// 编译 `Pred` 节点：阈值/边界按 11 维契约解析。
pub(crate) fn prepare_pred(p: &Pred) -> crate::Result<PreparedPred> {
    match p {
        Pred::Compare { name, op, value } => {
            let threshold = crate::l2::parse_numeric_value(value.as_bytes()).ok_or_else(|| {
                crate::GxpError::Runtime {
                    detail: format!("阈值 '{value}' 不是合法数值（11 维契约拒绝）"),
                }
            })?;
            Ok(PreparedPred::Compare {
                field: name.clone(),
                op: *op,
                threshold,
            })
        }
        Pred::In { name, values } => Ok(PreparedPred::In {
            field: name.clone(),
            values: values.clone(),
        }),
        Pred::Exists { name } => Ok(PreparedPred::Exists {
            field: name.clone(),
        }),
        Pred::Between { name, lo, hi } => {
            // _ts 伪字段特例——时间戳 between 走 speedate 解析（D5 不适用）
            if name == "_ts" {
                let lo_ts = parse_timestamp(lo).ok_or_else(|| crate::GxpError::Runtime {
                    detail: format!("_ts between 下界 '{lo}' 不是合法 ISO 8601 时间戳"),
                })?;
                let hi_ts = parse_timestamp(hi).ok_or_else(|| crate::GxpError::Runtime {
                    detail: format!("_ts between 上界 '{hi}' 不是合法 ISO 8601 时间戳"),
                })?;
                Ok(PreparedPred::TimeBetween {
                    field: name.clone(),
                    lo: lo_ts,
                    hi: hi_ts,
                })
            } else {
                let (lo_f, hi_f) =
                    build_between_bounds(lo, hi).map_err(|e| crate::GxpError::Runtime {
                        detail: format!("{e}"),
                    })?;
                Ok(PreparedPred::Between {
                    field: name.clone(),
                    lo: lo_f,
                    hi: hi_f,
                })
            }
        }
    }
}
