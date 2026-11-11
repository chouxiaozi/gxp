//! logfmt 带引号值（v0.50，设计文档 §3.3 带引号值契约）强制 e2e（§6.6）。
//!
//! 根因：v0.49 及之前 `extract_logfmt_kv` / `find_value` 取值到空白为止、
//! 不剥引号——数据行 `level="ERROR"` 提取值为带引号 `"ERROR"`，string
//! equality `level="ERROR"` 永不命中（静默 0 结果），且与 JSON 通路
//! （已剥引号）语义分叉。修复：[`logfmt_value_span`] 共享切分。
//!
//! 覆盖（§6.4 第 3 条数据来源 × stage 矩阵 + §6.6 强制 e2e）：
//! - 谓词全家 × quoted 数据：string equality / 数值比较 / in / between /
//!   exists / not
//! - stage 受益面：fields 投影 / sort / distinct（提取层单一事实源）
//! - 三路径：单文件 / 目录 / stdin
//! - fail-soft：未闭合引号回退（v0.49 行为保持）
//! - `--verify` 0 FAIL 兜底（L1 ⊇ L2）

mod common;
use common::*;
use gxp::cli::RunOpts;

/// 混合形态数据：quoted 值 / 裸值并存（独立期望集按行计算）。
const DATA: &str = "2026-11-11T10:00:00 level=\"ERROR\" cost_ms=\"900\" msg=\"boom bad\"\n\
                    2026-11-11T10:00:01 level=\"INFO\" cost_ms=\"12\" msg=ok\n\
                    2026-11-11T10:00:02 level=ERROR cost_ms=800 msg=logfmt\n";

// ===== 谓词全家 × quoted 数据（P5b 核心场景）=====

#[test]
fn test_quoted_string_equality_hits_quoted_and_bare() {
    // level="ERROR"：行 1（quoted 值剥引号）+ 行 3（裸值）→ 2
    let f = write_temp(DATA);
    let out = run(&args("level=\"ERROR\" | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_quoted_numeric_compare() {
    // cost_ms>500：行 1 quoted 900 + 行 3 裸 800 → 2（quoted 数值剥引号后可比较）
    let f = write_temp(DATA);
    let out = run(&args("cost_ms>500 | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_quoted_numeric_compare_exclusive_upper() {
    // cost_ms>800：只命中行 1（quoted 900）→ 1
    let f = write_temp(DATA);
    let out = run(&args("cost_ms>800 | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1\n");
}

#[test]
fn test_quoted_in_predicate() {
    // level in [ERROR, WARN]：行 1 + 行 3 → 2
    let f = write_temp(DATA);
    let out = run(&args(
        "level in [ERROR, WARN] | count",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_quoted_between_predicate() {
    // cost_ms between 100 and 1000：900 + 800 → 2
    let f = write_temp(DATA);
    let out = run(&args(
        "cost_ms between 100 and 1000 | count",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_quoted_exists_predicate() {
    // msg exists：3 行全有 msg（含带空格的 quoted 值）→ 3
    let f = write_temp(DATA);
    let out = run(&args("msg exists | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "3\n");
}

#[test]
fn test_quoted_not_string_equality() {
    // !(level="ERROR")：行 2（INFO）→ 1
    let f = write_temp(DATA);
    let out = run(&args(
        "!(level=\"ERROR\") | count",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "1\n");
}

// ===== stage 受益面（提取层单一事实源）=====

#[test]
fn test_quoted_fields_projection() {
    // fields cost_ms：投影出剥引号后的数值（行号:值 格式，对齐 cli_pipeline 断言口径）
    let f = write_temp(DATA);
    let out = run(&args(
        "cost_ms>500 | fields cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out, "1:900\n3:800\n");
}

#[test]
fn test_quoted_value_with_space_survives_sort() {
    // sort by cost_ms desc：行 1（900）在前——quoted 含空格值不破坏排序键提取
    let f = write_temp(DATA);
    let out = run(&args(
        "cost_ms>500 | sort by cost_ms desc | head 1",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(
        out,
        "1:2026-11-11T10:00:00 level=\"ERROR\" cost_ms=\"900\" msg=\"boom bad\"\n"
    );
}

#[test]
fn test_quoted_distinct_by_field() {
    // distinct by level：ERROR 两行去重留 1 + INFO 1 → 2 行
    let f = write_temp(DATA);
    let out = run(&args(
        "level exists | distinct by level",
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out.lines().count(), 2, "实际: {out}");
}

// ===== 三路径 =====

#[test]
fn test_quoted_directory_path() {
    let (dir, _) = write_temp_dir(&[
        ("a.log", "level=\"ERROR\" cost_ms=1\n"),
        ("b.log", "level=\"INFO\" cost_ms=2\n"),
        ("c.log", "level=\"ERROR\" cost_ms=3\n"),
    ]);
    let out = run(&args(
        "level=\"ERROR\" | count",
        dir.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert_eq!(out.trim(), "2");
}

#[test]
fn test_quoted_stdin_path() {
    // run_stdin 在测试进程内构造 argv——无 shell 引号层，双引号原样进解析器
    let out = run_stdin(DATA, "level=\"ERROR\" | count").expect("应成功");
    assert_eq!(out, "2\n");
}

// ===== fail-soft：未闭合引号回退（v0.49 行为保持）=====

#[test]
fn test_quoted_unclosed_quote_fallback_semantics() {
    // 未闭合 `k="abc`：值=`"abc`（带引号字面）——string eq 不命中（0），exists 命中（1）
    let f = write_temp("x=1 k=\"abc\n");
    let out = run(&args("k=\"abc\" | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "0\n");
    let out = run(&args("k exists | count", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1\n");
}

// ===== --verify 兜底（L1 ⊇ L2）=====

#[test]
fn test_quoted_string_equality_verify_pass() {
    let f = write_temp(DATA);
    let out = run_with_opts(
        &args("level=\"ERROR\"", f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..RunOpts::default()
        },
    )
    .expect("应成功");
    assert!(out.contains("PASS"), "应 PASS: {out}");
    assert!(out.contains("2 行命中"), "应命中 2 行: {out}");
}

#[test]
fn test_quoted_numeric_verify_pass() {
    let f = write_temp(DATA);
    let out = run_with_opts(
        &args("cost_ms>500", f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..RunOpts::default()
        },
    )
    .expect("应成功");
    assert!(out.contains("PASS"), "应 PASS: {out}");
}
