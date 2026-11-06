//! L2 数值解析与比较：11 维格式契约（设计文档 §3.3，D5）。
//!
//! ## 支持的格式（v1）
//!
//! | 维度 | 支持 | 示例 |
//! |---|---|---|
//! | 整数 | ✅ | `cost_ms=800` |
//! | 负数 | ✅ | `cost_ms=-5` |
//! | 正号 | ✅ | `cost_ms=+800` |
//! | 前导零 | ✅ | `cost_ms=0950` |
//! | 浮点 | ✅ | `cost_ms=800.5` / `.5` / `8.` |
//! | 科学计数 | ✅ | `cost_ms=8e2` / `8E2` / `8.5e-1` |
//!
//! ## 拒绝的格式（v1）
//!
//! | 维度 | 拒绝 | 示例 | 备注 |
//! |---|---|---|---|
//! | 千分位 | ❌ | `cost_ms=1,000` | 逗号视为字段分隔符 |
//! | 单位 | ❌ | `cost_ms=800ms` | 单位破坏数值解析 |
//! | 空白 trim | ❌ | `cost_ms= 800 ` | 不 trim（抽取时按空白切分，含空白说明切分错误） |
//! | 空值 | ❌ | `cost_ms=` / `cost_ms=null` | 视为字段不存在 |
//! | 布尔 | ❌ | `cost_ms=true` | 不支持布尔数值比较 |
//! | NaN/Inf | ❌ | `cost_ms=NaN` / `cost_ms=inf` | 阈值解析时已拒绝 |
//!
//! ## L1→L2 不变式（设计文档 §3.2）
//!
//! L1 只下推字段名前缀（`cost_ms=`）做粗筛，L2 抽字段值后调
//! [`eval_numeric`] 做精确比较。L1 命中集 ⊇ L2 精判命中集，无漏检窗口。

use crate::dsl::ast::CmpOp;
use std::str::FromStr;

/// L2 数值比较结果。失败的字段值（非数值、空值、拒绝格式）返回 [`EvalResult::NotNumeric`]，
/// 调用方按"谓词为假"处理（fail-safe，对应设计文档 §3.3"字段不存在的语义"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalResult {
    /// 谓词成立。
    True,
    /// 谓词不成立（值是合法数值，但不满足比较条件）。
    False,
    /// 值不是合法数值（千分位/单位/空白/空值/布尔/NaN/Inf）。
    /// 调用方按"谓词为假"处理，但语义层区分"字段是数值但不满足"与"字段不是数值"。
    NotNumeric,
}

/// 解析字段值字节为 f64。按 11 维契约（设计文档 §3.3）拒绝不支持格式。
///
/// 返回 `None` 表示不是数值字段（千分位/单位/空白/空值/布尔/NaN/Inf）。
/// 返回 `Some(f64)` 表示合法数值，可参与比较。
///
/// # 11 维契约实现
///
/// - **整数/负数/正号/前导零/浮点/科学计数**：交给 `f64::from_str`（Rust 标准库覆盖）
/// - **千分位**：含 `,` 直接拒绝
/// - **单位**：末位不是 `[0-9]` 且不是 `.eE+-` 的中间位 → 拒绝
///   （简化：先尝试 `f64::from_str`，失败即拒绝；这覆盖了所有单位场景）
/// - **空白 trim**：前导/尾随空白直接拒绝（不 trim）
/// - **空值**：空字节或 `"null"` 拒绝
/// - **布尔**：`"true"` / `"false"` 拒绝
/// - **NaN/Inf**：`f64::from_str` 接受这些字符串，需要显式拒绝
///
/// # 性能优化
///
/// 用 `eq_ignore_ascii_case` 做大小写不敏感比较，避免 `to_ascii_lowercase()`
/// 的 String 分配。对于 N=103k 行的 §7.1 基准场景，消除 103k 次堆分配。
pub fn parse_numeric_value(v: &[u8]) -> Option<f64> {
    // 快速路径：空值
    if v.is_empty() {
        return None;
    }

    // 拒绝千分位（含 `,`）
    // 注：设计文档 §3.3 "千分位会与字段分隔歧义"，故直接拒绝
    if memchr::memchr(b',', v).is_some() {
        return None;
    }

    // 拒绝前导/尾随空白（不 trim）
    if v[0] == b' ' || v[v.len() - 1] == b' ' {
        return None;
    }

    // 转 str（非 ASCII 字节直接拒绝）
    let s = std::str::from_utf8(v).ok()?;

    // 用 eq_ignore_ascii_case 避免分配 String
    // 空值 "null" 拒绝（大小写不敏感）
    if s.eq_ignore_ascii_case("null") {
        return None;
    }
    // 布尔拒绝
    if s.eq_ignore_ascii_case("true") || s.eq_ignore_ascii_case("false") {
        return None;
    }
    // NaN/Inf 拒绝（f64::from_str 会接受这些字符串）
    if s.eq_ignore_ascii_case("nan")
        || s.eq_ignore_ascii_case("inf")
        || s.eq_ignore_ascii_case("infinity")
        || s.eq_ignore_ascii_case("-nan")
        || s.eq_ignore_ascii_case("-inf")
        || s.eq_ignore_ascii_case("-infinity")
        || s.eq_ignore_ascii_case("+nan")
        || s.eq_ignore_ascii_case("+inf")
        || s.eq_ignore_ascii_case("+infinity")
    {
        return None;
    }

    // 标准库 from_str 覆盖：整数/负数/正号/前导零/浮点/科学计数
    // 注：from_str 接受 "100" "-5" "+800" "0950" "8.5" ".5" "8." "8e2" "8E2" "8.5e-1"
    // 拒绝：单位 "800ms"（末位 m 不是数字）等所有非标准格式
    f64::from_str(s).ok()
}

/// L2 精判：给定字段值字节、操作符、阈值，判定谓词是否成立。
///
/// 数值字段按 11 维契约解析（[`parse_numeric_value`]）；
/// 非数值字段（千分位/单位/空白/空值/布尔/NaN/Inf）返回 [`EvalResult::NotNumeric`]，
/// 调用方按"谓词为假"处理（fail-safe，设计文档 §3.3 "字段不存在的语义"）。
///
/// `Eq`/`Ne` 用 `f64::EPSILON` 容差比较（避免浮点表示误差）。
pub fn eval_numeric(value_bytes: &[u8], op: CmpOp, threshold: f64) -> EvalResult {
    let Some(val) = parse_numeric_value(value_bytes) else {
        return EvalResult::NotNumeric;
    };
    let result = match op {
        CmpOp::Gt => val > threshold,
        CmpOp::Ge => val >= threshold,
        CmpOp::Lt => val < threshold,
        CmpOp::Le => val <= threshold,
        // Eq/Ne 用 EPSILON 容差比较
        CmpOp::Eq => (val - threshold).abs() < f64::EPSILON,
        CmpOp::Ne => (val - threshold).abs() >= f64::EPSILON,
        // `=` 与 `==` 等价（AST CmpOp::Assign）
        CmpOp::Assign => (val - threshold).abs() < f64::EPSILON,
    };
    if result {
        EvalResult::True
    } else {
        EvalResult::False
    }
}

/// L2 `In` 谓词：字段值字节与候选值列表任一严格相等。
///
/// 设计文档 §2.2 EBNF `NAME in [v1, v2, ...]`：候选值是字节字面量，不做数值解析。
/// 字段不存在 → [`EvalResult::False`]（设计文档 §3.3 "字段不存在语义"：谓词为假）。
/// 字段存在但与所有候选值都不等 → [`EvalResult::False`]。
/// 字段存在且等于某个候选值 → [`EvalResult::True`]。
///
/// `NotNumeric` 不会出现在 `In` 谓词中——In 是字面量集合判定，
/// 不涉及数值解析（即便字段值是 "abc" 或候选值是 "900"，都按字节比较）。
pub fn eval_in(value: Option<&[u8]>, candidates: &[String]) -> EvalResult {
    let Some(v) = value else {
        return EvalResult::False;
    };
    for cand in candidates {
        if v == cand.as_bytes() {
            return EvalResult::True;
        }
    }
    EvalResult::False
}

/// L2 `Between` 谓词：`lo ≤ val ≤ hi`（闭区间）。
///
/// 设计文档 §2.2 EBNF `NAME between LO and HI`：LO/HI 是数值字面量，按 11 维契约解析。
/// 字段值不是合法数值 → [`EvalResult::NotNumeric`]（谓词为假，调用方按假处理）。
/// 字段值是合法数值但不在 `[lo, hi]` 闭区间内 → [`EvalResult::False`]。
/// `lo` / `hi` 自身不是合法数值 → 调用方应在编译期拒绝（[`build_between_bounds`] 预解析），
/// 运行时不再处理。
///
/// 边界含端点：`val == lo` 或 `val == hi` 都为 `True`。
pub fn eval_between(value_bytes: &[u8], lo: f64, hi: f64) -> EvalResult {
    let Some(val) = parse_numeric_value(value_bytes) else {
        return EvalResult::NotNumeric;
    };
    if val >= lo && val <= hi {
        EvalResult::True
    } else {
        EvalResult::False
    }
}

/// L2 `Exists` 谓词：字段是否出现在抽取结果中。
///
/// 设计文档 §2.2 EBNF `NAME exists`：判定字段是否在行中存在。
/// 伪字段 `_ts` / `_tag` 同样适用（通过 [`ExtractedFields::get`]）。
/// 字段存在（含空值——空值在抽取阶段已被剔除，不会出现） → [`EvalResult::True`]；
/// 字段不存在 → [`EvalResult::False`]。
///
/// [`ExtractedFields::get`]: crate::l2::ExtractedFields::get
pub fn eval_exists(value: Option<&[u8]>) -> EvalResult {
    if value.is_some() {
        EvalResult::True
    } else {
        EvalResult::False
    }
}

/// 把 Between 谓词的 lo / hi 字节字面量预解析为 f64。
///
/// 在 `build_l2_predicates` 阶段调用：解析失败（非合法数值，11 维契约拒绝）
/// 时立即报错（fail-loud），避免运行时静默把 Between 视为永假。
///
/// 返回 `(lo_f64, hi_f64)`；任一失败返回 `Err` 含具体哪个边界非法。
pub fn build_between_bounds(lo: &str, hi: &str) -> Result<(f64, f64), BetweenBoundError> {
    let lo_f =
        parse_numeric_value(lo.as_bytes()).ok_or_else(|| BetweenBoundError::LowerInvalid {
            raw: lo.to_string(),
        })?;
    let hi_f =
        parse_numeric_value(hi.as_bytes()).ok_or_else(|| BetweenBoundError::UpperInvalid {
            raw: hi.to_string(),
        })?;
    Ok((lo_f, hi_f))
}

/// `build_between_bounds` 错误：lo / hi 不是合法数值（11 维契约拒绝）。
#[derive(Debug, thiserror::Error)]
pub enum BetweenBoundError {
    /// 下界不是合法数值。
    #[error("between 下界 '{raw}' 不是合法数值（11 维契约拒绝）")]
    LowerInvalid {
        /// 原始字节字面量（未解析成功）。
        raw: String,
    },
    /// 上界不是合法数值。
    #[error("between 上界 '{raw}' 不是合法数值（11 维契约拒绝）")]
    UpperInvalid {
        /// 原始字节字面量（未解析成功）。
        raw: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_numeric_value 11 维契约表逐格验证 ---

    #[test]
    fn test_parse_integer() {
        assert_eq!(parse_numeric_value(b"800"), Some(800.0));
        assert_eq!(parse_numeric_value(b"0"), Some(0.0));
    }

    #[test]
    fn test_parse_negative() {
        assert_eq!(parse_numeric_value(b"-5"), Some(-5.0));
        assert_eq!(parse_numeric_value(b"-800.5"), Some(-800.5));
    }

    #[test]
    fn test_parse_positive_sign() {
        assert_eq!(parse_numeric_value(b"+800"), Some(800.0));
        assert_eq!(parse_numeric_value(b"+0"), Some(0.0));
    }

    #[test]
    fn test_parse_leading_zeros() {
        assert_eq!(parse_numeric_value(b"0950"), Some(950.0));
        assert_eq!(parse_numeric_value(b"0050"), Some(50.0));
        assert_eq!(parse_numeric_value(b"00000"), Some(0.0));
    }

    #[test]
    fn test_parse_float() {
        assert_eq!(parse_numeric_value(b"800.5"), Some(800.5));
        assert_eq!(parse_numeric_value(b".5"), Some(0.5));
        assert_eq!(parse_numeric_value(b"8."), Some(8.0));
    }

    #[test]
    fn test_parse_scientific() {
        assert_eq!(parse_numeric_value(b"8e2"), Some(800.0));
        assert_eq!(parse_numeric_value(b"8E2"), Some(800.0));
        assert_eq!(parse_numeric_value(b"8.5e-1"), Some(0.85));
        assert_eq!(parse_numeric_value(b"1.5E3"), Some(1500.0));
    }

    // --- 拒绝项 ---

    #[test]
    fn test_parse_reject_thousands() {
        assert_eq!(parse_numeric_value(b"1,000"), None);
        assert_eq!(parse_numeric_value(b"1,234,567"), None);
    }

    #[test]
    fn test_parse_reject_unit_suffix() {
        assert_eq!(parse_numeric_value(b"800ms"), None);
        assert_eq!(parse_numeric_value(b"100%"), None);
        assert_eq!(parse_numeric_value(b"5kb"), None);
    }

    #[test]
    fn test_parse_reject_whitespace() {
        assert_eq!(parse_numeric_value(b" 800"), None);
        assert_eq!(parse_numeric_value(b"800 "), None);
        assert_eq!(parse_numeric_value(b" 800 "), None);
    }

    #[test]
    fn test_parse_reject_empty() {
        assert_eq!(parse_numeric_value(b""), None);
        assert_eq!(parse_numeric_value(b"null"), None);
        assert_eq!(parse_numeric_value(b"NULL"), None);
        assert_eq!(parse_numeric_value(b"Null"), None);
    }

    #[test]
    fn test_parse_reject_boolean() {
        assert_eq!(parse_numeric_value(b"true"), None);
        assert_eq!(parse_numeric_value(b"false"), None);
        assert_eq!(parse_numeric_value(b"TRUE"), None);
    }

    #[test]
    fn test_parse_reject_nan_inf() {
        assert_eq!(parse_numeric_value(b"NaN"), None);
        assert_eq!(parse_numeric_value(b"nan"), None);
        assert_eq!(parse_numeric_value(b"inf"), None);
        assert_eq!(parse_numeric_value(b"infinity"), None);
        assert_eq!(parse_numeric_value(b"-inf"), None);
        assert_eq!(parse_numeric_value(b"+inf"), None);
    }

    // --- eval_numeric 正向比较 ---

    #[test]
    fn test_eval_gt_true() {
        assert_eq!(eval_numeric(b"900", CmpOp::Gt, 800.0), EvalResult::True);
    }

    #[test]
    fn test_eval_gt_false() {
        assert_eq!(eval_numeric(b"800", CmpOp::Gt, 800.0), EvalResult::False);
        assert_eq!(eval_numeric(b"100", CmpOp::Gt, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_ge_boundary() {
        assert_eq!(eval_numeric(b"800", CmpOp::Ge, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"799", CmpOp::Ge, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_lt() {
        assert_eq!(eval_numeric(b"100", CmpOp::Lt, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"800", CmpOp::Lt, 800.0), EvalResult::False);
        assert_eq!(eval_numeric(b"900", CmpOp::Lt, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_le_boundary() {
        assert_eq!(eval_numeric(b"800", CmpOp::Le, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"801", CmpOp::Le, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_eq() {
        assert_eq!(eval_numeric(b"800", CmpOp::Eq, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"800.0", CmpOp::Eq, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"900", CmpOp::Eq, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_ne() {
        assert_eq!(eval_numeric(b"900", CmpOp::Ne, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"800", CmpOp::Ne, 800.0), EvalResult::False);
    }

    #[test]
    fn test_eval_assign_equivalent_to_eq() {
        // `=` 与 `==` 等价
        assert_eq!(
            eval_numeric(b"800", CmpOp::Assign, 800.0),
            eval_numeric(b"800", CmpOp::Eq, 800.0)
        );
    }

    #[test]
    fn test_eval_negative_value_comparison() {
        // 负数与正数比较
        assert_eq!(eval_numeric(b"-5", CmpOp::Lt, 0.0), EvalResult::True);
        assert_eq!(eval_numeric(b"-5", CmpOp::Gt, -10.0), EvalResult::True);
        assert_eq!(eval_numeric(b"-5", CmpOp::Gt, 0.0), EvalResult::False);
    }

    #[test]
    fn test_eval_scientific_value_comparison() {
        // 8e2 = 800.0
        assert_eq!(eval_numeric(b"8e2", CmpOp::Gt, 700.0), EvalResult::True);
        assert_eq!(eval_numeric(b"8e2", CmpOp::Eq, 800.0), EvalResult::True);
    }

    #[test]
    fn test_eval_leading_zeros_comparison() {
        // 0950 = 950.0
        assert_eq!(eval_numeric(b"0950", CmpOp::Gt, 800.0), EvalResult::True);
        assert_eq!(eval_numeric(b"0950", CmpOp::Eq, 950.0), EvalResult::True);
    }

    // --- eval_numeric 非数值字段 ---

    #[test]
    fn test_eval_not_numeric_thousands() {
        assert_eq!(
            eval_numeric(b"1,000", CmpOp::Gt, 800.0),
            EvalResult::NotNumeric
        );
    }

    #[test]
    fn test_eval_not_numeric_unit() {
        assert_eq!(
            eval_numeric(b"800ms", CmpOp::Gt, 800.0),
            EvalResult::NotNumeric
        );
    }

    #[test]
    fn test_eval_not_numeric_boolean() {
        assert_eq!(
            eval_numeric(b"true", CmpOp::Eq, 1.0),
            EvalResult::NotNumeric
        );
    }

    #[test]
    fn test_eval_not_numeric_empty() {
        assert_eq!(eval_numeric(b"", CmpOp::Gt, 0.0), EvalResult::NotNumeric);
        assert_eq!(
            eval_numeric(b"null", CmpOp::Gt, 0.0),
            EvalResult::NotNumeric
        );
    }

    // --- eval_in 字符串字面量集合判定 ---

    #[test]
    fn test_eval_in_match_first_candidate() {
        let cands = vec!["info".to_string(), "error".to_string()];
        assert_eq!(eval_in(Some(b"info"), &cands), EvalResult::True);
    }

    #[test]
    fn test_eval_in_match_second_candidate() {
        let cands = vec!["info".to_string(), "error".to_string()];
        assert_eq!(eval_in(Some(b"error"), &cands), EvalResult::True);
    }

    #[test]
    fn test_eval_in_no_match() {
        let cands = vec!["info".to_string(), "error".to_string()];
        assert_eq!(eval_in(Some(b"warn"), &cands), EvalResult::False);
    }

    #[test]
    fn test_eval_in_field_absent() {
        // 字段不存在 → False（设计文档 §3.3 "字段不存在语义"）
        let cands = vec!["info".to_string()];
        assert_eq!(eval_in(None, &cands), EvalResult::False);
    }

    #[test]
    fn test_eval_in_empty_candidates() {
        // 空候选集 → False（无法匹配任何）
        assert_eq!(eval_in(Some(b"info"), &[]), EvalResult::False);
    }

    #[test]
    fn test_eval_in_numeric_value_as_string() {
        // 数值候选值按字节比较（不做数值解析）
        let cands = vec!["800".to_string(), "900".to_string()];
        assert_eq!(eval_in(Some(b"800"), &cands), EvalResult::True);
        assert_eq!(eval_in(Some(b"850"), &cands), EvalResult::False);
        // 前导零不等：0950 ≠ 950（字节比较）
        assert_eq!(eval_in(Some(b"0950"), &cands), EvalResult::False);
    }

    // --- eval_between 闭区间 ---

    #[test]
    fn test_eval_between_in_range() {
        assert_eq!(eval_between(b"150", 100.0, 200.0), EvalResult::True);
    }

    #[test]
    fn test_eval_between_at_lo_boundary() {
        // 闭区间：val == lo 为 True
        assert_eq!(eval_between(b"100", 100.0, 200.0), EvalResult::True);
    }

    #[test]
    fn test_eval_between_at_hi_boundary() {
        // 闭区间：val == hi 为 True
        assert_eq!(eval_between(b"200", 100.0, 200.0), EvalResult::True);
    }

    #[test]
    fn test_eval_between_below_lo() {
        assert_eq!(eval_between(b"99", 100.0, 200.0), EvalResult::False);
    }

    #[test]
    fn test_eval_between_above_hi() {
        assert_eq!(eval_between(b"201", 100.0, 200.0), EvalResult::False);
    }

    #[test]
    fn test_eval_between_not_numeric_field() {
        // 字段不是数值 → NotNumeric（谓词为假，但语义层区分 False / NotNumeric）
        assert_eq!(eval_between(b"abc", 100.0, 200.0), EvalResult::NotNumeric);
        assert_eq!(eval_between(b"1,000", 100.0, 200.0), EvalResult::NotNumeric);
        assert_eq!(eval_between(b"800ms", 100.0, 200.0), EvalResult::NotNumeric);
    }

    #[test]
    fn test_eval_between_negative_range() {
        // 负数区间
        assert_eq!(eval_between(b"-50", -100.0, -10.0), EvalResult::True);
        assert_eq!(eval_between(b"-5", -100.0, -10.0), EvalResult::False);
    }

    #[test]
    fn test_eval_between_scientific_bounds() {
        // 科学计数法字段值 + 浮点端点
        assert_eq!(eval_between(b"8e2", 700.0, 900.0), EvalResult::True);
        assert_eq!(eval_between(b"8.5e1", 80.0, 90.0), EvalResult::True);
    }

    // --- eval_exists 字段存在性 ---

    #[test]
    fn test_eval_exists_present() {
        assert_eq!(eval_exists(Some(b"900")), EvalResult::True);
        assert_eq!(eval_exists(Some(b"alice")), EvalResult::True);
    }

    #[test]
    fn test_eval_exists_absent() {
        assert_eq!(eval_exists(None), EvalResult::False);
    }

    // --- build_between_bounds 预解析 ---

    #[test]
    fn test_build_between_bounds_valid() {
        let (lo, hi) = build_between_bounds("100", "200").expect("合法端点应解析");
        assert_eq!(lo, 100.0);
        assert_eq!(hi, 200.0);
    }

    #[test]
    fn test_build_between_bounds_scientific() {
        let (lo, hi) = build_between_bounds("1e2", "1.5e3").expect("科学计数法应解析");
        assert_eq!(lo, 100.0);
        assert_eq!(hi, 1500.0);
    }

    #[test]
    fn test_build_between_bounds_lo_invalid() {
        let err = build_between_bounds("abc", "200").expect_err("非数值下界应失败");
        assert!(format!("{err}").contains("下界"));
        assert!(format!("{err}").contains("abc"));
    }

    #[test]
    fn test_build_between_bounds_hi_invalid() {
        let err =
            build_between_bounds("100", "1,000").expect_err("千分位上界应失败（11 维契约拒绝）");
        assert!(format!("{err}").contains("上界"));
        assert!(format!("{err}").contains("1,000"));
    }

    #[test]
    fn test_build_between_bounds_both_invalid_reports_lo_first() {
        // 两个端点都非法时，先报下界（实现顺序：lo 先 hi 后）
        let err = build_between_bounds("null", "NaN").expect_err("两端都非法应失败");
        assert!(format!("{err}").contains("下界"));
    }
}
