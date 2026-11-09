//! L2 Literal 重校验豁免（v0.49 Opt A，Rust 方案 §7.9）e2e。
//!
//! **独立期望集**（B14/B15 双教训）：期望行号手算，不从被测路径推导。
//! `--verify` 暴力与优化路径共用 `PreparedScan::build`（verify.rs）——豁免标志
//! 不健全时两侧同错对齐，`--verify` 不可发现，唯一防线是本文件的独立期望集。
//!
//! 覆盖的健全性边界（与 compiler.rs verified_literals 单元测试一一对应）：
//! - And 侧单元素字面量：豁免后结果与手算一致（正常路径）
//! - Paren(And) 陷阱：`"a" && ("b" && "c")` 的 b/c **无** L1 组（B14 整组放弃），
//!   不得豁免——只含 a 的行必须被 L2 正确淘汰
//! - Or 侧字面量：组成员 ≥2，不得豁免——含 errC 的行不得命中
//! - Not 侧字面量：Not 子树不下推，语义依赖 L2 实际校验
//! - 同串跨子树：`"err" && !("err")` 恒空（L1 保证 err 存在 → Not 恒 False）

mod common;
use common::*;

use gxp::cli::RunOpts;

/// 固定 fixture：12 行，行号 1-based（期望集手算依据，逐行标注于注释）。
const FIXTURE: &str = concat!(
    // 1: timeout + [checkout] + cost 900 → And 命中
    "2026-11-09T10:00:00Z [checkout] timeout cost_ms=900 req=1\n",
    // 2: timeout + [auth] → tag 谓词淘汰
    "2026-11-09T10:00:01Z [auth] timeout cost_ms=900 req=2\n",
    // 3: retry（无 timeout）→ 字面量淘汰
    "2026-11-09T10:00:02Z [checkout] retry cost_ms=950 req=3\n",
    // 4: 纯文本无字段
    "plain line no fields\n",
    // 5: alpha+beta+gamma → Paren(And) 命中
    "alpha beta gamma\n",
    // 6: 只有 alpha → Paren(And) 陷阱行：b/c 未豁免必须被 L2 淘汰
    "alpha only\n",
    // 7: failA + cost 50 → Or 字面量命中
    "failA happened cost_ms=50\n",
    // 8: failC（非 failA/failB）+ cost 50 → Or 侧未豁免必须被淘汰
    "failC happened cost_ms=50\n",
    // 9: warning+error（含 warn 子串）→ Not 淘汰
    "warning and error together\n",
    // 10: error standalone（err 子串、无 warn）→ Not 命中
    "error occurred standalone\n",
    // 11: timeout + checkout + 801（边界 >800）
    "2026-11-09T10:00:03Z [checkout] timeout cost_ms=801 req=11\n",
    // 12: timeout + checkout + 800（边界不 >800）
    "2026-11-09T10:00:04Z [checkout] timeout cost_ms=800 req=12\n",
);

/// 按手算行号（1-based）拼期望输出（`N:line` 格式）。
fn expected_lines(idx: &[usize]) -> String {
    let lines: Vec<&str> = FIXTURE.lines().collect();
    idx.iter()
        .map(|&i| format!("{i}:{}\n", lines[i - 1]))
        .collect()
}

#[test]
fn test_and_literal_verified_normal_path() {
    // And 侧单元素豁免的正常路径：结果与手算一致（行 1 + 边界行 11；行 12 的
    // 800>800 为假、行 2 tag 不符、行 3 无字面量、行 4 无字段）
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#""timeout" && _tag in [checkout] && cost_ms>800"#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 11]));
}

#[test]
fn test_paren_and_trap_literals_still_checked() {
    // Paren(And) 陷阱：`"alpha" && ("beta" && "gamma")` 的 beta/gamma 无 L1 组
    // （B14 完整性整组放弃）→ 不豁免 → 行 6（只有 alpha）必须被 L2 淘汰。
    // 若误豁免：行 6 alpha verified→True + beta/gamma 恒 True → 假阳性，本测试抓。
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#""alpha" && ("beta" && "gamma")"#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[5]));
}

#[test]
fn test_or_side_literal_not_verified() {
    // Or 组 [failA, failB] ≥2 元素 → 不豁免。行 8 含 failC：若 Or 侧字面量
    // 误豁免（恒 True）→ cost_ms=50>10 通过 → 假阳性，本测试抓。
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"("failA" || "failB") && cost_ms>10"#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[7]));
}

#[test]
fn test_not_literal_semantics_preserved() {
    // Not 侧字面量不豁免：行 10（error 子串、无 warn 子串）命中，
    // 行 9（warning 含 warn 子串）淘汰。若 warn 误豁免 → Not(True)=False
    // → 行 10 也被淘汰，本测试抓。（字面量是子串语义：err ⊂ error ✓）
    let f = write_temp(FIXTURE);
    let out = run(&args(r#"!("warn") && "err""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[10]));
}

#[test]
fn test_same_string_across_subtrees_always_empty() {
    // 同串跨子树：L1 保证 err 存在（And 侧单元素组）→ Not(err) 恒 False →
    // 整体恒空。两种书写顺序同集（And 交换律）。
    // 注意行 9/10 含 err 子串会进 L1，但 Not 恒 False 淘汰 → 恒空。
    let f = write_temp(FIXTURE);
    let p = f.path().to_str().unwrap();
    let out1 = run(&args(r#""err" && !("err")"#, p)).expect("应成功");
    assert_eq!(out1, "", "err 已保证存在 → Not(err)=False → 恒空");
    let out2 = run(&args(r#"!("err") && "err""#, p)).expect("应成功");
    assert_eq!(out2, "", "And 交换律：同集");
}

#[test]
fn test_verified_exempt_consistent_with_verify() {
    // --verify 交叉：豁免开启下暴力与优化路径对账应 PASS（两者共用豁免标志，
    // 本测试不构成健全性证明——健全性由上面独立期望集保证，此处只确认
    // 豁免没有引入两路径的语义分歧）
    let f = write_temp(FIXTURE);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let out = run_with_opts(
        &args(
            r#""timeout" && _tag in [checkout] && cost_ms>800"#,
            f.path().to_str().unwrap(),
        ),
        opts,
    )
    .expect("应成功");
    assert!(
        out.contains("PASS"),
        "--verify 应 PASS（豁免未引入两路径分歧），实际: {out}"
    );
}
