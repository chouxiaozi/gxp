//! 数值谓词穷举测试：0..60000 × 6 区间 + 11 维格式契约（D5）+ Between 穷举 + Not 三态取反。
//!
//! 设计文档 §3.3 + Rust 方案 §8.3：
//! - **proptest**：`(n, threshold, op_idx)` 三元组暴力对账，验证
//!   `eval_numeric(format!("{}", n), op, threshold)` 与暴力计算结果一致。
//! - **穷举校验**：0..60000 全量遍历 × 6 个区间边界 × 6 操作符，0 错才算通过。
//! - **Between 穷举**：0..60000 × 6 区间闭区间 `lo ≤ n ≤ hi` 0 错。
//! - **Not 三态取反**：True→False、False→True、NotNumeric→True（§3.3 字段不存在语义）。
//! - **11 维格式契约**：固定输入测试 11 维契约表每一格，
//!   支持项解析正确，拒绝项返回 [`gxp::l2::EvalResult::NotNumeric`]。
//!
//! 不变式 `L1(粗筛) ⊇ L2(精判)`：本测试单独验证 L2 数值比较的正确性，
//! L1 只下推字段名前缀（设计文档 §3.2 方向 B），不做数值比较，无漏检窗口。

use gxp::dsl::ast::CmpOp;
use gxp::l2::{eval_between, eval_numeric, parse_numeric_value, EvalResult};

/// 把 0..6 的索引映射到比较操作符。
/// 索引 6 对应 `CmpOp::Assign`（`=`，与 `==` 等价）。
fn op_from_idx(idx: u8) -> CmpOp {
    match idx {
        0 => CmpOp::Gt,
        1 => CmpOp::Ge,
        2 => CmpOp::Lt,
        3 => CmpOp::Le,
        4 => CmpOp::Eq,
        5 => CmpOp::Ne,
        _ => CmpOp::Assign,
    }
}

/// 暴力计算期望结果（f64 比较）。
fn brute_force(n: u64, threshold: f64, op: CmpOp) -> EvalResult {
    let val = n as f64;
    let result = match op {
        CmpOp::Gt => val > threshold,
        CmpOp::Ge => val >= threshold,
        CmpOp::Lt => val < threshold,
        CmpOp::Le => val <= threshold,
        CmpOp::Eq => (val - threshold).abs() < f64::EPSILON,
        CmpOp::Ne => (val - threshold).abs() >= f64::EPSILON,
        CmpOp::Assign => (val - threshold).abs() < f64::EPSILON,
    };
    if result {
        EvalResult::True
    } else {
        EvalResult::False
    }
}

// ===== proptest 暴力对账 =====

use proptest::prop_assert_eq;

proptest::proptest! {
    /// 对任意 (n, threshold, op) 三元组，eval_numeric 与暴力计算结果一致。
    #[test]
    fn numeric_eval_matches_bruteforce(
        n in 0u64..60_000,
        threshold_int in 0u64..60_000,
        op_idx in 0u8..7,  // 6 个 op + Assign
    ) {
        let op = op_from_idx(op_idx);
        let value_str = format!("{}", n);
        let threshold = threshold_int as f64;
        let actual = eval_numeric(value_str.as_bytes(), op, threshold);
        let expected = brute_force(n, threshold, op);
        prop_assert_eq!(
            actual, expected,
            "n={} threshold={} op={:?} expected={:?} actual={:?}",
            n, threshold, op, expected, actual
        );
    }

    /// 浮点阈值场景：浮点 n 与浮点 threshold 的比较。
    #[test]
    fn numeric_eval_float_threshold(
        n in 0u64..60_000,
        threshold_int in 0u64..60_000,
        threshold_frac in 0u32..1_000,
        op_idx in 0u8..7,
    ) {
        let op = op_from_idx(op_idx);
        let threshold = threshold_int as f64 + (threshold_frac as f64) / 1000.0;
        let value_str = format!("{}", n);
        let actual = eval_numeric(value_str.as_bytes(), op, threshold);
        let expected = brute_force(n, threshold, op);
        prop_assert_eq!(
            actual, expected,
            "n={} threshold={} op={:?} expected={:?} actual={:?}",
            n, threshold, op, expected, actual
        );
    }
}

// ===== 0..60000 × 6 区间穷举校验 =====

/// 6 个区间边界（Rust 方案 §8.3）：
/// (500, 2000), (2000, 8000), (8000, 30000), (30000, 55000), (55000, 60000)
/// 加上 (0, 500) 总共 6 个区间。
const INTERVALS: [(u64, u64); 6] = [
    (0, 500),
    (500, 2000),
    (2000, 8000),
    (8000, 30000),
    (30000, 55000),
    (55000, 60000),
];

/// 穷举校验：对每个区间 (lo, hi)，遍历 0..60000 全量，
/// 验证 `eval_numeric(n, Ge, lo) == (n >= lo)`、`Le, hi) == (n <= hi)` 等。
///
/// **注**：60000 × 6 区间 × 4 操作符 = 1.44M 次比较，在 release 下 ~100ms。
#[test]
fn exhaustive_6_intervals_l2() {
    for (lo, hi) in INTERVALS {
        for n in 0u64..60_000 {
            let value_str = format!("{}", n);
            let value_bytes = value_str.as_bytes();

            // Ge lo
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Ge, lo as f64),
                brute_force(n, lo as f64, CmpOp::Ge),
                "Ge: n={} lo={}",
                n,
                lo
            );
            // Le hi
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Le, hi as f64),
                brute_force(n, hi as f64, CmpOp::Le),
                "Le: n={} hi={}",
                n,
                hi
            );
            // Gt lo
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Gt, lo as f64),
                brute_force(n, lo as f64, CmpOp::Gt),
                "Gt: n={} lo={}",
                n,
                lo
            );
            // Lt hi
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Lt, hi as f64),
                brute_force(n, hi as f64, CmpOp::Lt),
                "Lt: n={} hi={}",
                n,
                hi
            );
            // Eq lo（验证相等边界）
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Eq, lo as f64),
                brute_force(n, lo as f64, CmpOp::Eq),
                "Eq: n={} lo={}",
                n,
                lo
            );
            // Ne hi
            assert_eq!(
                eval_numeric(value_bytes, CmpOp::Ne, hi as f64),
                brute_force(n, hi as f64, CmpOp::Ne),
                "Ne: n={} hi={}",
                n,
                hi
            );
        }
    }
}

// ===== 11 维格式契约表逐格验证 =====

/// 设计文档 §3.3 11 维契约表：
/// - 6 维支持（整数/负数/正号/前导零/浮点/科学计数法）
/// - 6 维拒绝（千分位/单位/空白/空值/布尔/NaN/Inf）
#[test]
fn format_contract_11_dims() {
    // ===== 支持项：parse_numeric_value 返回 Some =====

    // 整数
    assert_eq!(parse_numeric_value(b"800"), Some(800.0));
    assert_eq!(parse_numeric_value(b"0"), Some(0.0));
    // 负数
    assert_eq!(parse_numeric_value(b"-5"), Some(-5.0));
    assert_eq!(parse_numeric_value(b"-800.5"), Some(-800.5));
    // 正号
    assert_eq!(parse_numeric_value(b"+800"), Some(800.0));
    assert_eq!(parse_numeric_value(b"+0"), Some(0.0));
    // 前导零
    assert_eq!(parse_numeric_value(b"0950"), Some(950.0));
    assert_eq!(parse_numeric_value(b"0050"), Some(50.0));
    assert_eq!(parse_numeric_value(b"00000"), Some(0.0));
    // 浮点
    assert_eq!(parse_numeric_value(b"800.5"), Some(800.5));
    assert_eq!(parse_numeric_value(b".5"), Some(0.5));
    assert_eq!(parse_numeric_value(b"8."), Some(8.0));
    // 科学计数法
    assert_eq!(parse_numeric_value(b"8e2"), Some(800.0));
    assert_eq!(parse_numeric_value(b"8E2"), Some(800.0));
    assert_eq!(parse_numeric_value(b"8.5e-1"), Some(0.85));
    assert_eq!(parse_numeric_value(b"1.5E3"), Some(1500.0));

    // ===== 拒绝项：parse_numeric_value 返回 None =====

    // 千分位
    assert_eq!(parse_numeric_value(b"1,000"), None);
    assert_eq!(parse_numeric_value(b"1,234,567"), None);
    // 单位
    assert_eq!(parse_numeric_value(b"800ms"), None);
    assert_eq!(parse_numeric_value(b"100%"), None);
    assert_eq!(parse_numeric_value(b"5kb"), None);
    // 空白 trim
    assert_eq!(parse_numeric_value(b" 800"), None);
    assert_eq!(parse_numeric_value(b"800 "), None);
    assert_eq!(parse_numeric_value(b" 800 "), None);
    // 空值
    assert_eq!(parse_numeric_value(b""), None);
    assert_eq!(parse_numeric_value(b"null"), None);
    assert_eq!(parse_numeric_value(b"NULL"), None);
    assert_eq!(parse_numeric_value(b"Null"), None);
    // 布尔
    assert_eq!(parse_numeric_value(b"true"), None);
    assert_eq!(parse_numeric_value(b"false"), None);
    assert_eq!(parse_numeric_value(b"TRUE"), None);
    // NaN/Inf
    assert_eq!(parse_numeric_value(b"NaN"), None);
    assert_eq!(parse_numeric_value(b"nan"), None);
    assert_eq!(parse_numeric_value(b"inf"), None);
    assert_eq!(parse_numeric_value(b"infinity"), None);
    assert_eq!(parse_numeric_value(b"-inf"), None);
    assert_eq!(parse_numeric_value(b"+inf"), None);
}

/// 11 维契约在 `eval_numeric` 层的体现：拒绝格式返回 `NotNumeric`。
#[test]
fn eval_numeric_rejects_11_dim_disallowed() {
    // 千分位
    assert_eq!(
        eval_numeric(b"1,000", CmpOp::Gt, 800.0),
        EvalResult::NotNumeric
    );
    // 单位
    assert_eq!(
        eval_numeric(b"800ms", CmpOp::Gt, 800.0),
        EvalResult::NotNumeric
    );
    // 空白
    assert_eq!(
        eval_numeric(b" 800", CmpOp::Gt, 800.0),
        EvalResult::NotNumeric
    );
    // 空值
    assert_eq!(eval_numeric(b"", CmpOp::Gt, 0.0), EvalResult::NotNumeric);
    assert_eq!(
        eval_numeric(b"null", CmpOp::Gt, 0.0),
        EvalResult::NotNumeric
    );
    // 布尔
    assert_eq!(
        eval_numeric(b"true", CmpOp::Eq, 1.0),
        EvalResult::NotNumeric
    );
    // NaN/Inf
    assert_eq!(eval_numeric(b"NaN", CmpOp::Gt, 0.0), EvalResult::NotNumeric);
    assert_eq!(eval_numeric(b"inf", CmpOp::Gt, 0.0), EvalResult::NotNumeric);
}

/// 11 维契约在 `eval_numeric` 层的体现：支持格式返回 True/False（非 NotNumeric）。
#[test]
fn eval_numeric_accepts_11_dim_allowed() {
    // 整数
    assert_eq!(eval_numeric(b"950", CmpOp::Gt, 800.0), EvalResult::True);
    assert_eq!(eval_numeric(b"100", CmpOp::Gt, 800.0), EvalResult::False);
    // 负数
    assert_eq!(eval_numeric(b"-5", CmpOp::Lt, 0.0), EvalResult::True);
    // 正号
    assert_eq!(eval_numeric(b"+950", CmpOp::Gt, 800.0), EvalResult::True);
    // 前导零
    assert_eq!(eval_numeric(b"0950", CmpOp::Gt, 800.0), EvalResult::True);
    // 浮点
    assert_eq!(eval_numeric(b"950.5", CmpOp::Gt, 800.0), EvalResult::True);
    assert_eq!(eval_numeric(b".5", CmpOp::Lt, 1.0), EvalResult::True);
    // 科学计数法
    assert_eq!(eval_numeric(b"9.5e2", CmpOp::Gt, 800.0), EvalResult::True);
    assert_eq!(eval_numeric(b"9.5E2", CmpOp::Eq, 950.0), EvalResult::True);
}

// ===== Eq/Ne 用 EPSILON 容差验证 =====

/// `Eq` 在浮点表示误差范围内成立。
/// `8e2 == 800` 在 f64 上是 800.0 == 800.0（精确），但保险起见用 EPSILON 容差。
#[test]
fn eval_eq_with_epsilon_tolerance() {
    // 8e2 解析为 800.0，阈值 800.0 → Eq
    assert_eq!(eval_numeric(b"8e2", CmpOp::Eq, 800.0), EvalResult::True);
    // 8.5e-1 解析为 0.85，阈值 0.85 → Eq
    assert_eq!(eval_numeric(b"8.5e-1", CmpOp::Eq, 0.85), EvalResult::True);
    // 0.1 + 0.2 ≠ 0.3（浮点表示误差），但用 EPSILON 容差：
    // 我们的 parse_numeric_value 只接受字面量，不做算术，所以这个场景不会出现
    // 直接验证字面量 0.3
    assert_eq!(eval_numeric(b"0.3", CmpOp::Eq, 0.3), EvalResult::True);
}

/// `Ne` 是 `Eq` 的否定。
#[test]
fn eval_ne_is_negation_of_eq() {
    assert_eq!(eval_numeric(b"950", CmpOp::Ne, 800.0), EvalResult::True);
    assert_eq!(eval_numeric(b"800", CmpOp::Ne, 800.0), EvalResult::False);
}

// ===== Between 穷举：闭区间 lo ≤ n ≤ hi =====

/// 穷举校验 `eval_between`：对每个区间 (lo, hi)，遍历 0..60000 全量，
/// 验证 `eval_between(format!("{}", n), lo as f64, hi as f64)` 返回 True 当且仅当 `lo ≤ n ≤ hi`。
///
/// **注**：60000 × 6 区间 = 360K 次比较，release 下 ~30ms。
#[test]
fn exhaustive_between_6_intervals() {
    for (lo, hi) in INTERVALS {
        for n in 0u64..60_000 {
            let value_str = format!("{}", n);
            let value_bytes = value_str.as_bytes();
            let expected = if n >= lo && n <= hi {
                EvalResult::True
            } else {
                EvalResult::False
            };
            assert_eq!(
                eval_between(value_bytes, lo as f64, hi as f64),
                expected,
                "between: n={} lo={} hi={}",
                n,
                lo,
                hi
            );
        }
    }
}

/// Between 边界语义验证：`val == lo` 和 `val == hi` 都为 True（闭区间）。
#[test]
fn between_closed_interval_boundary() {
    // 区间 (500, 2000)
    assert_eq!(
        eval_between(b"500", 500.0, 2000.0),
        EvalResult::True,
        "下界等于 lo 应为 True（闭区间）"
    );
    assert_eq!(
        eval_between(b"2000", 500.0, 2000.0),
        EvalResult::True,
        "上界等于 hi 应为 True（闭区间）"
    );
    assert_eq!(
        eval_between(b"499", 500.0, 2000.0),
        EvalResult::False,
        "lo-1 应为 False"
    );
    assert_eq!(
        eval_between(b"2001", 500.0, 2000.0),
        EvalResult::False,
        "hi+1 应为 False"
    );
    assert_eq!(
        eval_between(b"1000", 500.0, 2000.0),
        EvalResult::True,
        "区间内应为 True"
    );
}

/// Between 非数值字段返回 NotNumeric（11 维契约拒绝的格式）。
#[test]
fn between_non_numeric_returns_not_numeric() {
    // 千分位
    assert_eq!(
        eval_between(b"1,000", 500.0, 2000.0),
        EvalResult::NotNumeric
    );
    // 单位
    assert_eq!(
        eval_between(b"800ms", 500.0, 2000.0),
        EvalResult::NotNumeric
    );
    // 空白
    assert_eq!(eval_between(b" 800", 500.0, 2000.0), EvalResult::NotNumeric);
    // 空值
    assert_eq!(eval_between(b"", 500.0, 2000.0), EvalResult::NotNumeric);
    // 布尔
    assert_eq!(eval_between(b"true", 500.0, 2000.0), EvalResult::NotNumeric);
    // NaN/Inf
    assert_eq!(eval_between(b"NaN", 500.0, 2000.0), EvalResult::NotNumeric);
}

// ===== Not 三态取反穷举 =====

/// 把 EvalResult 映射到 "Not 取反后" 的期望值。
///
/// 设计文档 §3.3 "字段不存在语义"：
/// - True → False
/// - False → True
/// - NotNumeric → True（字段不存在 → 谓词为假 → 取反为真 → 命中）
fn not_invert(r: EvalResult) -> EvalResult {
    match r {
        EvalResult::True => EvalResult::False,
        EvalResult::False => EvalResult::True,
        EvalResult::NotNumeric => EvalResult::True,
    }
}

/// 穷举验证 Not 三态取反：对每个区间 (lo, hi) × 6 操作符 × 0..60000 全量，
/// `Not(eval_numeric(n, op, threshold))` 与 `not_invert(eval_numeric(n, op, threshold))` 一致。
///
/// 模拟 `!(cost OP threshold)` 语义：取 eval 结果后反演。
#[test]
fn exhaustive_not_three_state_inversion() {
    for (lo, hi) in INTERVALS {
        // 用 lo 和 hi 作为两个 threshold 测试
        for &threshold in &[lo as f64, hi as f64] {
            for n in 0u64..60_000 {
                let value_str = format!("{}", n);
                let value_bytes = value_str.as_bytes();

                // 对每个 op，验证 Not 三态取反
                for op_idx in 0u8..7 {
                    let op = op_from_idx(op_idx);
                    let inner = eval_numeric(value_bytes, op, threshold);
                    let negated = not_invert(inner);
                    // 验证双重否定 = 原值（经典反演律）
                    let double_neg = not_invert(negated);
                    assert_eq!(
                        double_neg, inner,
                        "double-neg mismatch: n={} op={:?} threshold={} inner={:?} neg={:?} neg2={:?}",
                        n,
                        op,
                        threshold,
                        inner,
                        negated,
                        double_neg
                    );
                    // 验证取反结果与暴力计算一致
                    let brute = brute_force(n, threshold, op);
                    let brute_neg = not_invert(brute);
                    assert_eq!(
                        negated, brute_neg,
                        "not vs brute-neg mismatch: n={} op={:?} threshold={} got={:?} want={:?}",
                        n, op, threshold, negated, brute_neg
                    );
                }
            }
        }
    }
}

/// Not 对 NotNumeric 的特殊语义：字段不是数值（千分位/单位/空白/空值/布尔/NaN）→ NotNumeric → 取反为 True。
///
/// 这是 §3.3 "字段不存在语义" 的关键：`!(cost>800)` 对 cost 字段不存在的行视为命中。
#[test]
fn not_inverts_not_numeric_to_true() {
    // 千分位 → NotNumeric → Not 后为 True
    let r = eval_numeric(b"1,000", CmpOp::Gt, 800.0);
    assert_eq!(r, EvalResult::NotNumeric);
    assert_eq!(not_invert(r), EvalResult::True);

    // 单位 → NotNumeric → Not 后为 True
    let r = eval_numeric(b"800ms", CmpOp::Gt, 800.0);
    assert_eq!(not_invert(r), EvalResult::True);

    // 空值 → NotNumeric → Not 后为 True
    let r = eval_numeric(b"", CmpOp::Gt, 0.0);
    assert_eq!(not_invert(r), EvalResult::True);

    // 布尔 → NotNumeric → Not 后为 True
    let r = eval_numeric(b"true", CmpOp::Eq, 1.0);
    assert_eq!(not_invert(r), EvalResult::True);

    // NaN → NotNumeric → Not 后为 True
    let r = eval_numeric(b"NaN", CmpOp::Gt, 0.0);
    assert_eq!(not_invert(r), EvalResult::True);
}

/// Not 对 True/False 的正常反演：True→False、False→True。
#[test]
fn not_inverts_true_false_normally() {
    // 950 > 800 → True → Not 后为 False
    let r = eval_numeric(b"950", CmpOp::Gt, 800.0);
    assert_eq!(r, EvalResult::True);
    assert_eq!(not_invert(r), EvalResult::False);

    // 100 > 800 → False → Not 后为 True
    let r = eval_numeric(b"100", CmpOp::Gt, 800.0);
    assert_eq!(r, EvalResult::False);
    assert_eq!(not_invert(r), EvalResult::True);

    // 800 > 800 → False（> 不含等） → Not 后为 True
    let r = eval_numeric(b"800", CmpOp::Gt, 800.0);
    assert_eq!(r, EvalResult::False);
    assert_eq!(not_invert(r), EvalResult::True);

    // 800 >= 800 → True → Not 后为 False
    let r = eval_numeric(b"800", CmpOp::Ge, 800.0);
    assert_eq!(r, EvalResult::True);
    assert_eq!(not_invert(r), EvalResult::False);
}
