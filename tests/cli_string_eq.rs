//! 字符串等值谓词 `NAME="STRING"` e2e（v0.48，设计文档 §3.2 字符串等值脱糖）。
//!
//! **独立期望集**（B14 教训：期望值手算，不从被测路径推导；`--verify` 与优化路径
//! 共用 L1 编译，不能作为唯一对账）。`="` / `=="` / `in [单元素]` 三元同集是
//! 构造性等价（解析期脱糖为同一 In 节点）的运行时证明。
//!
//! GT 基线（v0.47 二进制实测锁定，2026-11-09）：
//! - `level in [ERROR]` 命中 JSON 行（`"level":"ERROR"` 去引号提取）
//! - JSON 行数值谓词正常（`"cost_ms":950` ⊨ cost_ms>850）
//! - BARE 文本输出带 `N:` 行号前缀（输出自包含，与 gxp 模式一致）
//! - `--verify` 输出 `verify: PASS [N 行命中]`（替代正常输出）

mod common;
use common::*;

use gxp::cli::RunOpts;

/// 固定 fixture：7 行，覆盖 logfmt / 无字段 / BARE 字面 / JSON / 伪字段 / 多值。
const FIXTURE: &str = concat!(
    "2026-11-09T10:00:00Z [ERROR] level=ERROR msg=timeout cost_ms=900\n",
    "2026-11-09T10:00:01Z [INFO] level=INFO msg=ok cost_ms=100\n",
    "2026-11-09T10:00:02Z [ERROR] msg=retry cost_ms=800\n",
    "plain line no fields\n",
    "xa=b y\n",
    "{\"level\":\"ERROR\",\"msg\":\"json err\",\"cost_ms\":950}\n",
    "2026-11-09T10:00:03Z [WARN] level=WARN msg=slow cost_ms=850\n",
);

/// 按手算行号（1-based）拼期望输出（`N:line` 格式，机械格式化）。
fn expected_lines(idx: &[usize]) -> String {
    let lines: Vec<&str> = FIXTURE.lines().collect();
    idx.iter()
        .map(|&i| format!("{i}:{}\n", lines[i - 1]))
        .collect()
}

#[test]
fn test_string_eq_hits_logfmt_and_json() {
    // level="ERROR"：logfmt 行 1 + JSON 行 6（去引号提取，GT 见文件头）
    let f = write_temp(FIXTURE);
    let out = run(&args(r#"level="ERROR""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[1, 6]));
}

#[test]
fn test_string_eq_triple_equivalence() {
    // 构造性等价的运行时证明：=" / ==" / in [单元素] 三元同集
    let f = write_temp(FIXTURE);
    let p = f.path().to_str().unwrap();
    let eq1 = run(&args(r#"level="ERROR""#, p)).expect("= 应成功");
    let eq2 = run(&args(r#"level=="ERROR""#, p)).expect("== 应成功");
    let via_in = run(&args("level in [ERROR]", p)).expect("in 应成功");
    assert_eq!(eq1, expected_lines(&[1, 6]));
    assert_eq!(eq2, eq1, "== 与 = 应同集");
    assert_eq!(via_in, eq1, "in [单元素] 应与 = 同集");
}

#[test]
fn test_string_eq_not_includes_field_missing_lines() {
    // !(level="ERROR")（D6 强制括号）：补集含无 level 字段行（3,4,5）与 INFO/WARN（2,7）
    let f = write_temp(FIXTURE);
    let out = run(&args(r#"!(level="ERROR")"#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[2, 3, 4, 5, 7]));
}

#[test]
fn test_string_eq_and_numeric_pred() {
    // level="ERROR" && cost_ms>850：行 1（900）+ 行 6（JSON 950）
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"level="ERROR" && cost_ms>850"#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 6]));
}

#[test]
fn test_string_eq_or_literal_group_b14() {
    // Or 组混字面量 + 脱糖谓词（B14 Or 组完整性：L1 不得因字面量分支淘汰谓词候选）
    // 该测试在修复前暴露 B15（v0.47 前装：Or 内谓词前缀被当必选，plain-only 行被杀）
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"level="ERROR" || "plain""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 4, 6]));
}

#[test]
fn test_or_pred_literal_b15_regression_in_form() {
    // B15 回归（in 形态，v0.47 前装丢行）：脱糖只是入口，不变式修在 L1
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"level in [ERROR] || "plain""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 4, 6]));
}

#[test]
fn test_or_numeric_pred_literal_b15() {
    // B15 同构镜像：数值谓词 Or 字面量（cost_ms>800 → 1,6,7；"plain" → 4）
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"cost_ms>800 || "plain""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 4, 6, 7]));
}

#[test]
fn test_or_literal_pred_flipped_order_b15() {
    // 顺序对调同修复（"plain" || level="ERROR"）
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#""plain" || level="ERROR""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1, 4, 6]));
}

#[test]
fn test_string_eq_pseudo_field_tag() {
    // _tag="ERROR"：伪字段字符串等值（[ERROR] 行 1,3；JSON 行 6 无方括号 tag）
    let f = write_temp(FIXTURE);
    let out = run(&args(r#"_tag="ERROR""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[1, 3]));
}

#[test]
fn test_string_eq_on_capture_field() {
    // capture × 字符串等值（§6.5.2 数据来源矩阵）：capture(lvl,...) 只在 logfmt 行
    // 1,2,7 提取（JSON 行无 `level=` 子串）；lvl="ERROR" → 行 1
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"capture(lvl, /level=(\w+)/) && lvl="ERROR""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, expected_lines(&[1]));
}

#[test]
fn test_string_eq_value_with_space_json() {
    // STRING RHS 允许空格（单一记号，in 裸词列表无法表达）：msg="json err" → 行 6
    let f = write_temp(FIXTURE);
    let out = run(&args(r#"msg="json err""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[6]));
}

#[test]
fn test_string_eq_pipeline_count() {
    let f = write_temp(FIXTURE);
    let out = run(&args(
        r#"level="ERROR" | count"#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_string_eq_stdin_path() {
    // §6.4 三路径之 stdin
    let out = run_stdin(FIXTURE, r#"level="ERROR""#).expect("应成功");
    assert_eq!(out, expected_lines(&[1, 6]));
}

#[test]
fn test_string_eq_directory_path() {
    // §6.4 三路径之目录递归：a.log 2 条 ERROR + b.log 1 条 → count 3
    let (dir, _) = write_temp_dir(&[
        ("a.log", "level=ERROR a1\nlevel=INFO a2\nlevel=ERROR a3\n"),
        ("b.log", "level=WARN b1\nlevel=ERROR b2\n"),
    ]);
    let out = run(&args_dir(
        r#"level="ERROR" | count"#,
        dir.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "3\n");
}

#[test]
fn test_string_eq_verify_passes() {
    // --verify 对账 PASS（独立性由本文件其余手算期望测试承担，此处锁 verify 通路）
    let f = write_temp(FIXTURE);
    let out = run_with_opts(
        &args(r#"level="ERROR" || "plain""#, f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("verify 应 PASS");
    assert_eq!(out, "verify: PASS [3 行命中]\n");
}

#[test]
fn test_bare_single_eq_is_ere_literal() {
    // v0.48：`=` 单等号非数值 RHS → BARE ERE 字面量（≡ grep -E 'a=b'，设计文档 §2.2
    // 分歧操作符列表不含 `=`）。v0.47 该查询报 runtime 数值错误（文档-实现不一致，
    // 本测试锁定修正后行为）
    let f = write_temp(FIXTURE);
    let out = run(&args("a=b", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[5]));
}

#[test]
fn test_bare_numeric_eq_still_predicate() {
    // 对照：无引号数值 RHS 仍是数值等值谓词（v0.47 行为不变）：cost_ms=850 → 行 7
    let f = write_temp(FIXTURE);
    let out = run(&args("cost_ms=850", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, expected_lines(&[7]));
}

#[test]
fn test_string_eq_wrong_op_error_hint() {
    // level>"ERROR"：fail-loud + 定向 hint（§6.3 fail-loud 路径测试）
    let f = write_temp(FIXTURE);
    let err = run(&args(r#"level>"ERROR""#, f.path().to_str().unwrap()))
        .expect_err("带引号 RHS + 非等值操作符应报错");
    assert!(err.contains("只支持等值操作符"), "hint 缺失：{err}");
    assert!(err.contains("level=\"ERROR\""), "等值写法缺失：{err}");
}

#[test]
fn test_string_eq_ne_wrong_op_error_hint() {
    // level!="ERROR"：hint 指向 !(level="ERROR")（§3.4 词法下 `!` 前是字母按字面，
    // != 无法成为操作符——不是实现偷懒）
    let f = write_temp(FIXTURE);
    let err = run(&args(r#"level!="ERROR""#, f.path().to_str().unwrap()))
        .expect_err("!= 带引号 RHS 应报错");
    assert!(
        err.contains("!(level=\"ERROR\")"),
        "否定写法 hint 缺失：{err}"
    );
}

#[test]
fn test_double_eq_non_numeric_runtime_reject_unchanged() {
    // a==b：`==` 有意分歧操作符，非数值 RHS 保持 D5 runtime 拒绝（v0.47 行为不变）
    let f = write_temp(FIXTURE);
    let err = run(&args("a==b", f.path().to_str().unwrap())).expect_err("== 非数值应报错");
    assert!(err.contains("不是合法数值"), "应含 D5 拒绝：{err}");
}
