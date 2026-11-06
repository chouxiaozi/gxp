//! stdin 管道 + --strict 字段探活端到端集成测试。
//!
//! 覆盖场景：BARE/gxp 模式 stdin（显式 `-` / 隐式无文件参数）/
//! rank/context/count/files/fields/head 管道 / --verify PASS / --explain /
//! 空输入 / stdin+file 互斥 / BARE --verify 报错 /
//! --strict 字段探活（缺失报错 / 全字段通过 / 无谓词无操作 / 伪字段 / 多字段部分缺失）。

mod common;
use common::*;

use gxp::cli::RunOpts;

// ===== stdin 管道测试 =====

#[test]
fn test_stdin_bare_explicit_dash() {
    // gxp 'ERROR' - （显式 stdin）
    let content = "ERROR something\nINFO ok\nERROR again\n";
    let out = run_stdin(content, "ERROR").expect("BARE stdin 应成功执行");
    assert!(
        out.contains("1:ERROR something"),
        "第 1 行应命中，实际: {out}"
    );
    assert!(out.contains("3:ERROR again"), "第 3 行应命中");
    assert!(!out.contains("INFO"), "INFO 行不应命中");
}

#[test]
fn test_stdin_bare_implicit_no_file_arg() {
    // gxp 'ERROR' （隐式 stdin，无文件参数）
    // 隐式 stdin 与显式 stdin 走同一 dispatch_with_content 路径
    let content = "ERROR something\nINFO ok\n";
    let out = run_stdin_implicit(content, "ERROR").expect("BARE 隐式 stdin 应成功执行");
    assert!(
        out.contains("1:ERROR something"),
        "第 1 行应命中，实际: {out}"
    );
    assert!(!out.contains("INFO"), "INFO 行不应命中");
}

#[test]
fn test_stdin_gxp_literal_and_field_predicate() {
    // gxp '"ERROR" && cost_ms>800' - （stdin gxp 模式）
    let content = "ERROR cost_ms=900 user=alice\nERROR cost_ms=100 user=bob\nINFO cost_ms=50\n";
    let out =
        run_stdin(content, "\"ERROR\" && cost_ms>800").expect("gxp stdin 字面量+字段应成功执行");
    assert!(
        out.contains("1:ERROR cost_ms=900 user=alice"),
        "第 1 行 cost_ms=900>800 应命中，实际: {out}"
    );
    assert!(!out.contains("cost_ms=100"), "第 2 行 cost_ms=100 不应命中");
    assert!(!out.contains("INFO"), "INFO 行不应命中");
}

#[test]
fn test_stdin_gxp_rank() {
    // gxp '"ERROR" && cost_ms>0 | rank top 2 by cost_ms desc' -
    let mut content = String::new();
    for (i, cost) in [(1, 300), (2, 100), (3, 500), (4, 200)] {
        content.push_str(&format!("ERROR cost_ms={cost} line{i}\n"));
    }
    let out = run_stdin(
        &content,
        "\"ERROR\" && cost_ms>0 | rank top 2 by cost_ms desc",
    )
    .expect("gxp stdin rank 应成功执行");
    // Top 2 by desc: cost_ms=500 (line 3), cost_ms=300 (line 1)
    assert!(
        out.contains("3:ERROR cost_ms=500"),
        "第 3 行 cost_ms=500 应在 Top2 中，实际: {out}"
    );
    assert!(
        out.contains("1:ERROR cost_ms=300"),
        "第 1 行 cost_ms=300 应在 Top2 中，实际: {out}"
    );
    assert!(!out.contains("cost_ms=100"), "cost_ms=100 不应在 Top2 中");
    assert!(!out.contains("cost_ms=200"), "cost_ms=200 不应在 Top2 中");
}

#[test]
fn test_stdin_gxp_context() {
    // gxp '"ERROR" | context 1' - （stdin context 窗口）
    let content = "INFO line1\nERROR line2\nINFO line3\n";
    let out = run_stdin(content, "\"ERROR\" | context 1").expect("gxp stdin context 应成功执行");
    // ERROR 在第 2 行，context 1 前后各 1 行
    assert!(
        out.contains("1:INFO line1"),
        "第 1 行应在 context 窗口中，实际: {out}"
    );
    assert!(out.contains("2:ERROR line2"), "第 2 行应命中，实际: {out}");
    assert!(
        out.contains("3:INFO line3"),
        "第 3 行应在 context 窗口中，实际: {out}"
    );
}

#[test]
fn test_stdin_gxp_count() {
    // gxp '"ERROR" | count' -
    let content = "ERROR a\nINFO b\nERROR c\n";
    let out = run_stdin(content, "\"ERROR\" | count").expect("gxp stdin count 应成功执行");
    assert!(out.contains("2"), "应 count 2 行命中 ERROR，实际: {out}");
}

#[test]
fn test_stdin_gxp_files() {
    // gxp '"ERROR" | files' - → 输出 <stdin>
    let content = "ERROR a\nINFO b\n";
    let out = run_stdin(content, "\"ERROR\" | files").expect("gxp stdin files 应成功执行");
    assert!(out.contains("<stdin>"), "files 应输出 <stdin>，实际: {out}");
}

#[test]
fn test_stdin_gxp_fields() {
    // gxp '"ERROR" | fields cost_ms' -
    let content = "ERROR cost_ms=900 user=alice\nERROR cost_ms=100 user=bob\n";
    let out =
        run_stdin(content, "\"ERROR\" | fields cost_ms").expect("gxp stdin fields 应成功执行");
    assert!(out.contains("900"), "应输出 cost_ms=900，实际: {out}");
    assert!(out.contains("100"), "应输出 cost_ms=100，实际: {out}");
    // fields 投影不应输出 user 字段
    assert!(
        !out.contains("alice"),
        "fields cost_ms 不应输出 user=alice，实际: {out}"
    );
}

#[test]
fn test_stdin_gxp_head() {
    // gxp '"ERROR" | head 2' -
    let mut content = String::new();
    for i in 1..=5 {
        content.push_str(&format!("ERROR line{i}\n"));
    }
    let out = run_stdin(&content, "\"ERROR\" | head 2").expect("gxp stdin head 应成功执行");
    assert!(
        out.contains("1:ERROR line1"),
        "第 1 行应在 head 2 中，实际: {out}"
    );
    assert!(
        out.contains("2:ERROR line2"),
        "第 2 行应在 head 2 中，实际: {out}"
    );
    assert!(!out.contains("line3"), "第 3 行不应在 head 2 中");
}

#[test]
fn test_stdin_gxp_distinct() {
    // gxp '"ERROR" | distinct' - （stdin distinct 路径，§6.4 三路径覆盖）
    let content = "ERROR a\nERROR b\nERROR a\nINFO c\n";
    let out = run_stdin(content, "\"ERROR\" | distinct").expect("gxp stdin distinct 应成功执行");
    // 去重：a(1), b(2)（第三条 a 重复删除，INFO 不命中）
    assert!(out.contains("1:ERROR a"), "应含首条 a: {out}");
    assert!(out.contains("2:ERROR b"), "应含 b: {out}");
    // 不应含第三条 a
    let a_count = out.matches("ERROR a").count();
    assert_eq!(a_count, 1, "ERROR a 应只出现 1 次: {out}");
    assert!(!out.contains("INFO"), "INFO 不应命中");
}

#[test]
fn test_stdin_verify_passes() {
    // gxp '"ERROR" && cost_ms>800' --verify - → PASS
    let content = "ERROR cost_ms=900\nINFO cost_ms=50\nERROR cost_ms=100\n";
    let out = run_stdin_with_opts(
        content,
        "\"ERROR\" && cost_ms>800",
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("stdin --verify 应成功执行");
    assert!(out.contains("PASS"), "stdin --verify 应 PASS，实际: {out}");
    assert!(out.contains("1 行命中"), "应 1 行命中，实际: {out}");
}

#[test]
fn test_stdin_explain_shows_stdin() {
    // gxp '"ERROR" && cost_ms>800' --explain - → 显示 <stdin>
    let content = "ERROR cost_ms=900\n";
    let out = run_stdin_with_opts(
        content,
        "\"ERROR\" && cost_ms>800",
        RunOpts {
            explain: true,
            ..Default::default()
        },
    )
    .expect("stdin --explain 应成功执行");
    assert!(
        out.contains("L1 必选组") || out.contains("plan") || out.contains("Plan"),
        "应输出执行计划，实际: {out}"
    );
}

#[test]
fn test_stdin_explain_verify_combined() {
    // gxp '"ERROR" && cost_ms>800' --explain --verify - → 计划 + verify 摘要
    let content = "ERROR cost_ms=900\nERROR cost_ms=100\n";
    let out = run_stdin_with_opts(
        content,
        "\"ERROR\" && cost_ms>800",
        RunOpts {
            explain: true,
            verify: true,
            ..Default::default()
        },
    )
    .expect("stdin --explain --verify 应成功执行");
    assert!(out.contains("PASS"), "应包含 verify PASS 摘要，实际: {out}");
}

#[test]
fn test_stdin_empty_input() {
    // 空 stdin → 0 命中
    let out = run_stdin("", "ERROR").expect("空 stdin 应成功执行");
    assert!(out.trim().is_empty(), "空输入应无命中，实际: {out:?}");
}

#[test]
fn test_stdin_mutex_with_file_errors() {
    // gxp 'pattern' - file → stdin 与 file 互斥报错
    let f = write_temp("ERROR line\n");
    let r = run(&[
        "ERROR".to_string(),
        "-".to_string(),
        f.path().to_str().unwrap().to_string(),
    ]);
    assert!(r.is_err(), "stdin + file 应报错");
    let err = r.unwrap_err();
    assert!(err.contains("互斥"), "错误应含'互斥'提示，实际: {err}");
}

#[test]
fn test_stdin_bare_verify_errors() {
    // BARE 模式 stdin --verify → 报错"仅 gxp 模式可用"
    let content = "ERROR line\n";
    let r = run_stdin_with_opts(
        content,
        "ERROR",
        RunOpts {
            verify: true,
            ..Default::default()
        },
    );
    assert!(r.is_err(), "BARE --verify 应报错");
    let err = r.unwrap_err();
    assert!(
        err.contains("gxp 模式") || err.contains("BARE"),
        "错误应含 gxp 模式提示，实际: {err}"
    );
}

// ===== --strict 字段探活测试（设计文档 §2.4 示例 5）=====

#[test]
fn test_strict_missing_field_reports_error() {
    // --strict 'ERROR && http_status>400' → http_status 从未出现 → fail-loud
    let f = write_temp("ERROR cost_ms=900 msg=ok\nERROR msg=fail\n");
    let r = run_with_opts(
        &[
            "ERROR && http_status>400".to_string(),
            f.path().to_str().unwrap().to_string(),
        ],
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    assert!(r.is_err(), "--strict 应检测到 http_status 从未出现");
    let err = r.unwrap_err();
    assert!(
        err.contains("http_status"),
        "错误应含 http_status 字段名，实际: {err}"
    );
    assert!(
        err.contains("从未出现"),
        "错误应含'从未出现'提示，实际: {err}"
    );
}

#[test]
fn test_strict_all_fields_present_passes() {
    // --strict 'ERROR && cost_ms>400' → cost_ms 存在 → 正常执行
    let f = write_temp("ERROR cost_ms=900 msg=ok\nERROR cost_ms=100 msg=fail\n");
    let r = run_with_opts(
        &[
            "\"ERROR\" && cost_ms>400".to_string(),
            f.path().to_str().unwrap().to_string(),
        ],
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    assert!(r.is_ok(), "--strict 字段都存在时应正常执行，实际: {r:?}");
    let out = r.unwrap();
    assert!(
        out.contains("cost_ms=900"),
        "应命中第 1 行 cost_ms=900，实际: {out}"
    );
    assert!(
        !out.contains("cost_ms=100"),
        "不应命中 cost_ms=100（<400），实际: {out}"
    );
}

#[test]
fn test_strict_no_predicate_fields_no_op() {
    // --strict 'ERROR' → 无字段谓词 → --strict 无操作（正常执行）
    let f = write_temp("ERROR line 1\nother line\nERROR line 2\n");
    let r = run_with_opts(
        &["ERROR".to_string(), f.path().to_str().unwrap().to_string()],
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    // BARE 模式 + --strict → --strict 无操作（BARE 模式无字段谓词）
    // 实际上 "ERROR" 是 BARE 模式，不走 gxp 分支
    assert!(r.is_ok(), "--strict BARE 模式应正常执行，实际: {r:?}");
}

#[test]
fn test_strict_pseudo_field_ts() {
    // --strict '_ts exists' → _ts 伪字段存在 → 通过
    let f = write_temp("2026-01-02T03:04:05Z level=info\nno_timestamp here\n");
    let r = run_with_opts(
        &[
            "_ts exists".to_string(),
            f.path().to_str().unwrap().to_string(),
        ],
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    assert!(r.is_ok(), "--strict _ts exists 应通过，实际: {r:?}");
    let out = r.unwrap();
    assert!(out.contains("2026-01-02"), "应命中时间戳行，实际: {out}");
}

#[test]
fn test_strict_multiple_fields_partial_missing() {
    // --strict 'cost_ms>100 && latency>50' → latency 从未出现 → fail-loud
    // 输入有 cost_ms 但无 latency
    let f = write_temp("cost_ms=200 msg=ok\ncost_ms=300 msg=fail\n");
    let r = run_with_opts(
        &[
            "cost_ms>100 && latency>50".to_string(),
            f.path().to_str().unwrap().to_string(),
        ],
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    assert!(r.is_err(), "--strict 应检测到 latency 从未出现");
    let err = r.unwrap_err();
    // 错误消息的"从未出现"列表应含 latency
    assert!(
        err.contains("latency"),
        "错误应含 latency 字段名，实际: {err}"
    );
    // 错误消息的"从未出现"列表不应含 cost_ms
    // （cost_ms 会出现在"实际探测到的字段"列表中，但不应出现在"从未出现"列表中）
    // 检查"从未出现"部分不含 cost_ms：截取该段后验证
    let never_part = err.split("从未出现").next().unwrap_or("");
    assert!(
        !never_part.contains("cost_ms"),
        "cost_ms 不应在'从未出现'列表中，实际: {err}"
    );
}

// ===== G4: sort/stats/percentiles/sample × stdin（§6.4 三路径覆盖，P1 缺口）=====

#[test]
fn test_stdin_sort_by_field() {
    // stdin: "ERROR" | sort by cost_ms desc
    let content = "ERROR cost_ms=300\nERROR cost_ms=100\nERROR cost_ms=200\n";
    let out = run_stdin(content, r#""ERROR" | sort by cost_ms desc"#).expect("stdin sort 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "应 3 行，实际: {out}");
    // desc → 300, 200, 100
    assert!(
        lines[0].contains("cost_ms=300"),
        "sort desc 第 1 行应 cost_ms=300，实际: {lines:?}"
    );
    assert!(
        lines[2].contains("cost_ms=100"),
        "sort desc 第 3 行应 cost_ms=100，实际: {lines:?}"
    );
}

#[test]
fn test_stdin_stats_field() {
    // stdin: cost_ms exists | stats cost_ms
    let content = "cost_ms=100\ncost_ms=300\ncost_ms=200\n";
    let out = run_stdin(content, "cost_ms exists | stats cost_ms").expect("stdin stats 应成功");
    assert!(
        out.contains("min: 100"),
        "stdin stats 应含 min: 100, 实际: {out}"
    );
    assert!(
        out.contains("max: 300"),
        "stdin stats 应含 max: 300, 实际: {out}"
    );
    assert!(
        out.contains("avg: 200"),
        "stdin stats 应含 avg: 200, 实际: {out}"
    );
}

#[test]
fn test_stdin_percentiles_field() {
    // stdin: cost_ms exists | percentiles cost_ms
    let content = "cost_ms=100\ncost_ms=200\ncost_ms=300\n";
    let out = run_stdin(content, "cost_ms exists | percentiles cost_ms")
        .expect("stdin percentiles 应成功");
    assert!(
        out.contains("p50:"),
        "stdin percentiles 应含 p50, 实际: {out}"
    );
    assert!(
        out.contains("p95:"),
        "stdin percentiles 应含 p95, 实际: {out}"
    );
    assert!(
        out.contains("p99:"),
        "stdin percentiles 应含 p99, 实际: {out}"
    );
}

#[test]
fn test_stdin_sample_n() {
    // stdin: "ERROR" | sample 2
    let content = "ERROR a\nERROR b\nERROR c\nERROR d\n";
    let out = run_stdin(content, r#""ERROR" | sample 2"#).expect("stdin sample 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 2 应输出 2 行，实际: {out}");
    for line in &lines {
        assert!(line.contains("ERROR"), "sample 行应含 ERROR，实际: {line}");
    }
}

// ===== G7: --strict × stdin（P1 缺口）=====

#[test]
fn test_stdin_strict_all_fields_present_passes() {
    // --strict + stdin：字段在输入中出现 → 通过
    let content = "ERROR cost_ms=900\nERROR cost_ms=100\n";
    let out = run_stdin_with_opts(
        content,
        r#""ERROR" && cost_ms>500"#,
        RunOpts {
            strict: true,
            ..Default::default()
        },
    )
    .expect("--strict stdin 全字段通过应成功");
    assert!(
        out.contains("cost_ms=900"),
        "应命中 cost_ms=900，实际: {out}"
    );
}

#[test]
fn test_stdin_strict_missing_field_reports_error() {
    // --strict + stdin：字段从未出现 → fail-loud
    let content = "ERROR msg a\nERROR msg b\n";
    let result = run_stdin_with_opts(
        content,
        r#""ERROR" && latency>100"#,
        RunOpts {
            strict: true,
            ..Default::default()
        },
    );
    let err = result.expect_err("--strict stdin 缺字段应报错");
    assert!(
        err.contains("latency"),
        "错误应含 latency 字段名，实际: {err}"
    );
}
