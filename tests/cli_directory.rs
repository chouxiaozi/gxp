//! 目录参数集成测试（§11.2.7）。
//!
//! 验证 `gxp 'pattern' dir/` 完整链路：
//! PathPlan::walk → 逐文件 read_for_scan → L1+L2 扫描 → execute_stages_directory
//!
//! 覆盖场景：BARE 递归 / gxp 字面量+字段 / rank 全局归并 / context 不跨文件 /
//! count+group / files --invert / stdin 互斥 / --verify 逐文件 / --explain walk

mod common;
use common::*;

use gxp::cli::RunOpts;

#[test]
fn test_directory_bare_recursive_scans_all_files() {
    // gxp 'ERROR' dir/ ≡ grep -Er 'ERROR' dir/
    // 3 个文件，2 个含 ERROR
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR line 1\ninfo\nERROR line 3\n"),
        ("b.log", "info only\n"),
        ("c.log", "ERROR in c\n"),
    ]);
    let out = run(&args_dir("ERROR", dir.path().to_str().unwrap())).expect("BARE 目录扫描应成功");
    // 应命中 3 行，跨 2 个文件
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "应命中 3 行，实际: {out}");
    // 每行应含文件路径前缀
    assert!(
        out.contains("a.log:1:ERROR line 1"),
        "应含 a.log:1，实际: {out}"
    );
    assert!(
        out.contains("a.log:3:ERROR line 3"),
        "应含 a.log:3，实际: {out}"
    );
    assert!(
        out.contains("c.log:1:ERROR in c"),
        "应含 c.log:1，实际: {out}"
    );
    // b.log 无命中，不应出现
    assert!(!out.contains("b.log"), "b.log 无命中不应出现，实际: {out}");
}

#[test]
fn test_directory_gxp_mode_literal_and_field_predicate() {
    // gxp '"ERROR" && cost_ms>800' dir/
    // L1 粗筛 + L2 精判，跨文件
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR cost_ms=900\nERROR cost_ms=100\ninfo\n"),
        ("b.log", "ERROR cost_ms=1200\ninfo\n"),
    ]);
    let out = run(&args_dir(
        "\"ERROR\" && cost_ms>800",
        dir.path().to_str().unwrap(),
    ))
    .expect("gxp 目录扫描应成功");
    // 应命中 2 行：a.log:1 (900) + b.log:1 (1200)
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应命中 2 行，实际: {out}");
    assert!(out.contains("900"), "应含 900，实际: {out}");
    assert!(out.contains("1200"), "应含 1200，实际: {out}");
    // a.log:2 (cost_ms=100) 不应命中（100 <= 800）
    assert!(
        !out.contains("cost_ms=100"),
        "cost_ms=100 不应命中，实际: {out}"
    );
}

#[test]
fn test_directory_rank_global_merge_top_k() {
    // gxp '"ERROR" | rank top 2 by cost_ms desc' dir/
    // 全局归并：所有命中文件的命中行进同一 Vec → partial-sort Top-2
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR cost_ms=300\nERROR cost_ms=900\n"),
        ("b.log", "ERROR cost_ms=500\nERROR cost_ms=1200\n"),
    ]);
    let out = run(&args_dir(
        "\"ERROR\" | rank top 2 by cost_ms desc",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 rank 应成功");
    // 全局 Top-2：1200 + 900（跨文件归并）
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 Top-2，实际: {out}");
    // 1200 应在 900 之前（desc 排序）
    assert!(
        out.find("1200").unwrap() < out.find("900").unwrap(),
        "1200 应在 900 之前，实际: {out}"
    );
    // 300 和 500 不应在 Top-2 中
    assert!(
        !out.contains("cost_ms=300"),
        "300 不应在 Top-2 中，实际: {out}"
    );
    assert!(
        !out.contains("cost_ms=500"),
        "500 不应在 Top-2 中，实际: {out}"
    );
}

#[test]
fn test_directory_count_aggregates_across_files() {
    // gxp '"ERROR" | count' dir/
    // 全目录命中行计数（全局聚合）
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR line 1\ninfo\nERROR line 3\n"),
        ("b.log", "ERROR line 1\ninfo\n"),
    ]);
    let out = run(&args_dir("\"ERROR\" | count", dir.path().to_str().unwrap()))
        .expect("目录 count 应成功");
    // 应输出 3（2 + 1）
    let trimmed = out.trim();
    assert_eq!(trimmed, "3", "应输出 3，实际: {out}");
}

#[test]
fn test_directory_files_outputs_matched_file_paths() {
    // gxp '"ERROR" | files' dir/
    // 输出命中文件名列表
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR line 1\ninfo\n"),
        ("b.log", "info only\n"),
        ("c.log", "ERROR in c\n"),
    ]);
    let out = run(&args_dir("\"ERROR\" | files", dir.path().to_str().unwrap()))
        .expect("目录 files 应成功");
    // 应输出 a.log + c.log 路径，不含 b.log
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 2 个命中文件，实际: {out}");
    assert!(out.contains("a.log"), "应含 a.log，实际: {out}");
    assert!(out.contains("c.log"), "应含 c.log，实际: {out}");
    assert!(!out.contains("b.log"), "b.log 无命中不应出现，实际: {out}");
}

#[test]
fn test_directory_files_invert_outputs_unmatched_files() {
    // gxp '"ERROR" | files --invert' dir/
    // 输出未命中文件名列表
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR line 1\n"),
        ("b.log", "info only\n"),
        ("c.log", "info only 2\n"),
    ]);
    let out = run(&args_dir(
        "\"ERROR\" | files --invert",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 files --invert 应成功");
    // 应输出 b.log + c.log（未命中），不含 a.log（命中）
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 2 个未命中文件，实际: {out}");
    assert!(out.contains("b.log"), "应含 b.log，实际: {out}");
    assert!(out.contains("c.log"), "应含 c.log，实际: {out}");
    assert!(
        !out.contains("a.log"),
        "a.log 命中不应在 --invert 中出现，实际: {out}"
    );
}

#[test]
fn test_directory_stdin_exclusivity_with_dir() {
    // gxp 'pattern' - dir/ → 报错（stdin 与 dir 互斥，§11.5.1）
    let (dir, _) = write_temp_dir(&[("a.log", "ERROR\n")]);
    let result = run(&args_dir("ERROR", dir.path().to_str().unwrap()));
    // 这个测试验证 stdin 与 dir 互斥：args 含 "-" 和 dir 路径
    let mut args_mixed = args_dir("ERROR", dir.path().to_str().unwrap());
    args_mixed.push("-".to_string());
    let result_mixed = run(&args_mixed);
    let err = result_mixed.expect_err("stdin 与 dir 混用应报错");
    assert!(err.contains("互斥"), "错误消息应含'互斥'，实际: {err}");
    // 单独 dir（无 stdin）应成功
    result.expect("单独目录应成功");
}

#[test]
fn test_directory_verify_per_file_summary() {
    // gxp --verify '"ERROR" && cost_ms>800' dir/
    // 逐文件对账 + 末尾汇总（§11.5.2）
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR cost_ms=900\nERROR cost_ms=100\n"),
        ("b.log", "ERROR cost_ms=1200\ninfo\n"),
    ]);
    let out = run_with_opts(
        &args_dir("\"ERROR\" && cost_ms>800", dir.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify 目录场景应成功");
    // 应含每文件 PASS + 末尾汇总
    assert!(out.contains("PASS"), "应含 PASS，实际: {out}");
    assert!(
        out.contains("files checked"),
        "应含 'files checked' 汇总，实际: {out}"
    );
    assert!(out.contains("0 failed"), "应含 '0 failed'，实际: {out}");
    // 应含两个文件路径
    assert!(out.contains("a.log"), "应含 a.log，实际: {out}");
    assert!(out.contains("b.log"), "应含 b.log，实际: {out}");
}

#[test]
fn test_directory_explain_outputs_walk_plan() {
    // gxp --explain '"ERROR"' dir/
    // 输出 walk 计划（§11.5.5）
    let (dir, _) = write_temp_dir(&[("a.log", "ERROR line 1\n"), ("b.log", "info\n")]);
    let out = run_with_opts(
        &args_dir("\"ERROR\"", dir.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..Default::default()
        },
    )
    .expect("--explain 目录场景应成功");
    // 应含 directory mode + PathPlan::walk + 文件数
    assert!(
        out.contains("directory mode"),
        "应含 'directory mode'，实际: {out}"
    );
    assert!(
        out.contains("PathPlan::walk"),
        "应含 'PathPlan::walk'，实际: {out}"
    );
    assert!(out.contains("2 files"), "应含 '2 files'，实际: {out}");
}

#[test]
fn test_directory_context_does_not_cross_file_boundary() {
    // gxp '"TARGET" | context 1' dir/
    // context 窗口不跨文件：文件 A 末行 + 文件 B 首行不拼成跨文件 context
    let (dir, _) = write_temp_dir(&[
        ("a.log", "line before\nTARGET line\nline after\n"),
        ("b.log", "TARGET in b\nunrelated\n"),
    ]);
    let out = run(&args_dir(
        "\"TARGET\" | context 1",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 context 应成功");
    // 文件 a.log: TARGET 行(行2) + 前1(行1) + 后1(行3) = 3 行
    // 文件 b.log: TARGET 行(行1) + 后1(行2) = 2 行（context 1 不跨文件取 a.log 末行）
    // 总计 5 行
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5, "应输出 5 行（3+2），实际: {out}");
    // 验证 a.log 的 context 含 "line before" 和 "line after"
    assert!(
        out.contains("line before"),
        "应含 'line before'，实际: {out}"
    );
    assert!(out.contains("line after"), "应含 'line after'，实际: {out}");
    // 验证 b.log 的 context 不含 a.log 的 "line after"（不跨文件）
    // b.log 的 context 应是 "TARGET in b" + "unrelated"
    assert!(
        out.contains("TARGET in b"),
        "应含 'TARGET in b'，实际: {out}"
    );
    assert!(out.contains("unrelated"), "应含 'unrelated'，实际: {out}");
}

// ===== G5: stats/percentiles/sample/group by × 目录（P1 缺口）=====

#[test]
fn test_directory_stats_aggregates_across_files() {
    // stats cost_ms 跨文件聚合：3 个文件的 cost_ms 全局 min/max/avg
    let (dir, _) = write_temp_dir(&[
        ("a.log", "cost_ms=100\ninfo\n"),
        ("b.log", "cost_ms=300\n"),
        ("c.log", "cost_ms=200\n"),
    ]);
    let out = run(&args_dir(
        "cost_ms exists | stats cost_ms",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 stats 应成功");
    assert!(
        out.contains("min: 100"),
        "目录 stats 应含 min: 100, 实际: {out}"
    );
    assert!(
        out.contains("max: 300"),
        "目录 stats 应含 max: 300, 实际: {out}"
    );
    // avg = (100 + 300 + 200) / 3 = 200
    assert!(
        out.contains("avg: 200"),
        "目录 stats 应含 avg: 200, 实际: {out}"
    );
}

#[test]
fn test_directory_percentiles_across_files() {
    // percentiles cost_ms 跨文件聚合
    let (dir, _) = write_temp_dir(&[
        ("a.log", "cost_ms=100\ncost_ms=200\n"),
        ("b.log", "cost_ms=300\ncost_ms=400\n"),
    ]);
    let out = run(&args_dir(
        "cost_ms exists | percentiles cost_ms",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 percentiles 应成功");
    assert!(
        out.contains("p50:"),
        "目录 percentiles 应含 p50, 实际: {out}"
    );
    assert!(
        out.contains("p95:"),
        "目录 percentiles 应含 p95, 实际: {out}"
    );
    assert!(
        out.contains("p99:"),
        "目录 percentiles 应含 p99, 实际: {out}"
    );
}

#[test]
fn test_directory_sample_across_files() {
    // sample N 跨文件采样
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR a1\nERROR a2\n"),
        ("b.log", "ERROR b1\nERROR b2\n"),
    ]);
    let out = run(&args_dir(
        r#""ERROR" | sample 2"#,
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 sample 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 2 应输出 2 行，实际: {out}");
    for line in &lines {
        assert!(line.contains("ERROR"), "sample 行应含 ERROR，实际: {line}");
    }
}

#[test]
fn test_directory_group_by_across_files() {
    // group by _tag 跨文件全局分组
    let (dir, _) = write_temp_dir(&[
        ("a.log", "[ERROR] msg a\n[INFO] msg b\n"),
        ("b.log", "[ERROR] msg c\n[ERROR] msg d\n"),
    ]);
    let out = run(&args_dir(
        r#""msg" | group by _tag"#,
        dir.path().to_str().unwrap(),
    ))
    .expect("目录 group by 应成功");
    // 全局：ERROR 3 条，INFO 1 条
    assert!(
        out.contains("ERROR: 3"),
        "目录 group by 应含 ERROR: 3, 实际: {out}"
    );
    assert!(
        out.contains("INFO: 1"),
        "目录 group by 应含 INFO: 1, 实际: {out}"
    );
}

// ===== G7: --strict × 目录（P1 缺口）=====

#[test]
fn test_directory_strict_all_fields_present_passes() {
    // --strict + 目录：字段在跨文件并集中存在 → 通过
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR cost_ms=100\n"),
        ("b.log", "ERROR cost_ms=200\n"),
    ]);
    let out = run_with_opts(
        &args_dir(r#""ERROR" && cost_ms>50"#, dir.path().to_str().unwrap()),
        RunOpts {
            strict: true,
            ..Default::default()
        },
    )
    .expect("--strict 目录全字段通过应成功");
    assert!(
        out.contains("cost_ms=100") || out.contains("cost_ms=200"),
        "应含命中行，实际: {out}"
    );
}

#[test]
fn test_directory_strict_missing_field_reports_error() {
    // --strict + 目录：字段在所有文件中都未出现 → fail-loud
    let (dir, _) = write_temp_dir(&[("a.log", "ERROR msg a\n"), ("b.log", "ERROR msg b\n")]);
    let result = run_with_opts(
        &args_dir(r#""ERROR" && latency>100"#, dir.path().to_str().unwrap()),
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    let err = result.expect_err("--strict 目录缺字段应报错");
    assert!(
        err.contains("latency"),
        "错误应含 latency 字段名，实际: {err}"
    );
}

#[test]
fn test_directory_strict_field_in_some_files_passes() {
    // --strict + 目录：字段只在部分文件出现 → 跨文件并集检测通过
    // a.log 有 cost_ms，b.log 没有 → 并集含 cost_ms → 通过
    let (dir, _) = write_temp_dir(&[
        ("a.log", "ERROR cost_ms=100\n"),
        ("b.log", "ERROR no field\n"),
    ]);
    let out = run_with_opts(
        &args_dir(r#""ERROR" && cost_ms>50"#, dir.path().to_str().unwrap()),
        RunOpts {
            strict: true,
            ..Default::default()
        },
    )
    .expect("--strict 字段在部分文件出现应通过（跨文件并集）");
    // 只有 a.log 的行满足 cost_ms>50
    assert!(
        out.contains("cost_ms=100"),
        "应含 a.log 命中行，实际: {out}"
    );
    assert!(
        !out.contains("no field"),
        "b.log 无 cost_ms 不应命中，实际: {out}"
    );
}
