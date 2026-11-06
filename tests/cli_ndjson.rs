//! `--json` NDJSON 流式输出测试。
//!
//! 验证 `--json` 在非 `--explain` 场景下逐行输出 NDJSON
//! （`{"line_no":N,"line":"..."}` 每行一个 JSON 对象）。
//! 覆盖 BARE 模式 + gxp 模式 + 目录模式。

mod common;
use common::*;

use gxp::cli::RunOpts;

fn json_opts() -> RunOpts {
    RunOpts {
        json: true,
        ..Default::default()
    }
}

// ===== BARE 模式 --json =====

#[test]
fn test_bare_json_outputs_ndjson() {
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let out =
        run_with_opts(&args("ERROR", f.path().to_str().unwrap()), json_opts()).expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 2 行 NDJSON，实际: {out}");

    let obj1: serde_json::Value = serde_json::from_str(lines[0]).expect("第 1 行应是合法 JSON");
    assert_eq!(obj1["line_no"], 1);
    assert_eq!(obj1["line"], "ERROR line 1");
    // #7：单文件 --json 也输出 path（与目录模式 schema 一致）
    assert!(
        !obj1["path"].as_str().unwrap().is_empty(),
        "path 不应为空，实际: {}",
        obj1["path"]
    );

    let obj2: serde_json::Value = serde_json::from_str(lines[1]).expect("第 2 行应是合法 JSON");
    assert_eq!(obj2["line_no"], 3);
    assert_eq!(obj2["line"], "ERROR line 3");
}

#[test]
fn test_bare_json_no_match_empty_output() {
    let f = write_temp("info\nwarn\n");
    let out =
        run_with_opts(&args("ERROR", f.path().to_str().unwrap()), json_opts()).expect("应成功");
    assert!(out.is_empty(), "无命中时应输出空，实际: {out}");
}

// ===== gxp 模式 --json =====

#[test]
fn test_gxp_json_predicate_outputs_ndjson() {
    let f = write_temp("cost_ms=100 ok\nERROR cost_ms=900 fail\nINFO cost_ms=50\n");
    let out = run_with_opts(
        &args("cost_ms>800", f.path().to_str().unwrap()),
        json_opts(),
    )
    .expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "应输出 1 行 NDJSON，实际: {out}");

    let obj: serde_json::Value = serde_json::from_str(lines[0]).expect("应是合法 JSON");
    assert_eq!(obj["line_no"], 2);
    assert_eq!(obj["line"], "ERROR cost_ms=900 fail");
    // #7：gxp 模式单文件 --json 也输出 path
    assert!(
        !obj["path"].as_str().unwrap().is_empty(),
        "path 不应为空，实际: {}",
        obj["path"]
    );
}

#[test]
fn test_gxp_json_string_literal_outputs_ndjson() {
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let out = run_with_opts(&args(r#""ERROR""#, f.path().to_str().unwrap()), json_opts())
        .expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 2 行 NDJSON，实际: {out}");

    let obj1: serde_json::Value = serde_json::from_str(lines[0]).expect("第 1 行应是合法 JSON");
    assert_eq!(obj1["line_no"], 1);
    assert_eq!(obj1["line"], "ERROR line 1");
    // #7：gxp 模式单文件 --json 也输出 path
    assert!(
        !obj1["path"].as_str().unwrap().is_empty(),
        "path 不应为空，实际: {}",
        obj1["path"]
    );

    let obj2: serde_json::Value = serde_json::from_str(lines[1]).expect("第 2 行应是合法 JSON");
    assert_eq!(obj2["line_no"], 3);
    assert_eq!(obj2["line"], "ERROR line 3");
}

// ===== gxp 模式 --json + pipeline stage =====

#[test]
fn test_gxp_json_with_count_stage_outputs_number() {
    // count stage 终止链，输出纯数字（非 NDJSON）
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let args = vec![
        r#""ERROR" | count"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    assert_eq!(out.trim(), "2", "count stage 应输出命中数，实际: {out}");
}

// ===== BARE 目录模式 --json =====

#[test]
fn test_bare_json_directory_outputs_ndjson_with_path() {
    let (dir, paths) = write_temp_dir(&[
        ("a.log", "ERROR line 1\ninfo\n"),
        ("b.log", "info\nERROR line 2\n"),
    ]);
    let dir_str = dir.path().to_str().unwrap();
    let out = run_with_opts(&args_dir("ERROR", dir_str), json_opts()).expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应输出 2 行 NDJSON，实际: {out}");

    let obj1: serde_json::Value = serde_json::from_str(lines[0]).expect("第 1 行应是合法 JSON");
    assert_eq!(obj1["line_no"], 1);
    assert_eq!(obj1["line"], "ERROR line 1");
    assert!(
        obj1["path"].as_str().unwrap().ends_with("a.log"),
        "path 应含 a.log，实际: {}",
        obj1["path"]
    );

    let obj2: serde_json::Value = serde_json::from_str(lines[1]).expect("第 2 行应是合法 JSON");
    assert_eq!(obj2["line_no"], 2);
    assert_eq!(obj2["line"], "ERROR line 2");
    assert!(
        obj2["path"].as_str().unwrap().ends_with("b.log"),
        "path 应含 b.log，实际: {}",
        obj2["path"]
    );

    let _ = paths; // suppress unused warning
}

// ===== 默认（非 --json）输出不受影响 =====

#[test]
fn test_default_output_not_json() {
    let f = write_temp("ERROR line 1\ninfo\n");
    let out = run(&args("ERROR", f.path().to_str().unwrap())).expect("应成功");
    // 非 --json 时输出格式：行号:行内容
    assert_eq!(out, "1:ERROR line 1\n");
}

// ===== G2: --json × 终止 stage（P0 缺口）=====
// 终止 stage（count/group by/files/stats/percentiles）直接输出纯文本，
// --json 不影响终止 stage 的输出格式（只影响命中行的 NDJSON 序列化）。

#[test]
fn test_json_with_group_by_outputs_text_not_ndjson() {
    // --json + group by _tag → 应输出 "TAG: N" 纯文本（非 NDJSON）
    let f = write_temp("[ERROR] error a\n[ERROR] error b\n[INFO] error c\n");
    let args = vec![
        r#""error" | group by _tag"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    assert!(
        out.contains("ERROR: 2"),
        "group by 应输出纯文本 'ERROR: 2'，实际: {out}"
    );
    assert!(
        out.contains("INFO: 1"),
        "group by 应输出纯文本 'INFO: 1'，实际: {out}"
    );
    // 不应是 NDJSON 格式（不以 { 开头）
    for line in out.lines() {
        assert!(
            !line.starts_with('{'),
            "终止 stage 输出不应是 NDJSON，实际: {line}"
        );
    }
}

#[test]
fn test_json_with_files_outputs_text_not_ndjson() {
    // --json + files → 应输出文件路径纯文本
    let (dir, _) = write_temp_dir(&[("a.log", "ERROR line 1\n"), ("b.log", "info only\n")]);
    let args = vec![
        r#""ERROR" | files"#.to_string(),
        dir.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    assert!(
        out.contains("a.log"),
        "files 应输出含命中的文件名，实际: {out}"
    );
    assert!(
        !out.contains("b.log"),
        "files 不应输出无命中的文件名，实际: {out}"
    );
    for line in out.lines() {
        assert!(
            !line.starts_with('{'),
            "终止 stage 输出不应是 NDJSON，实际: {line}"
        );
    }
}

#[test]
fn test_json_with_stats_outputs_text_not_ndjson() {
    // --json + stats FIELD → 应输出 "min: X, max: Y, avg: Z" 纯文本
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\n");
    let args = vec![
        "cost_ms exists | stats cost_ms".to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    assert!(
        out.contains("min: 100"),
        "stats 应输出 min: 100，实际: {out}"
    );
    assert!(
        out.contains("max: 300"),
        "stats 应输出 max: 300，实际: {out}"
    );
    assert!(
        out.contains("avg: 200"),
        "stats 应输出 avg: 200，实际: {out}"
    );
    for line in out.lines() {
        assert!(
            !line.starts_with('{'),
            "终止 stage 输出不应是 NDJSON，实际: {line}"
        );
    }
}

#[test]
fn test_json_with_percentiles_outputs_text_not_ndjson() {
    // --json + percentiles FIELD → 应输出 "p50: X, p95: Y, p99: Z" 纯文本
    let f = write_temp("cost_ms=100\ncost_ms=200\ncost_ms=300\n");
    let args = vec![
        "cost_ms exists | percentiles cost_ms".to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    assert!(out.contains("p50:"), "percentiles 应含 p50，实际: {out}");
    assert!(out.contains("p95:"), "percentiles 应含 p95，实际: {out}");
    assert!(out.contains("p99:"), "percentiles 应含 p99，实际: {out}");
    for line in out.lines() {
        assert!(
            !line.starts_with('{'),
            "终止 stage 输出不应是 NDJSON，实际: {line}"
        );
    }
}

#[test]
fn test_json_with_distinct_outputs_ndjson() {
    // --json + distinct（非终止）→ 应输出 NDJSON（distinct 返回行列表，非聚合摘要）
    let f = write_temp("ERROR a\nERROR a\nERROR b\n");
    let args = vec![
        r#""ERROR" | distinct"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    // distinct 去重后 2 行（"ERROR a" 和 "ERROR b"）
    assert_eq!(lines.len(), 2, "distinct 去重后应 2 行，实际: {out}");
    // 每行应为 NDJSON
    for line in &lines {
        let obj: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("应是 NDJSON，实际: {line}"));
        assert!(
            obj["line"].as_str().unwrap().contains("ERROR"),
            "NDJSON line 字段应含 ERROR，实际: {line}"
        );
    }
}

// ===== G10: context × --json（P2 缺口）=====

#[test]
fn test_json_with_context_outputs_ndjson_with_expanded_lines() {
    // --json + context 1 → 命中行 + 上下文行都以 NDJSON 输出
    let f = write_temp("line before\nERROR target\nline after\n");
    let args = vec![
        r#""ERROR" | context 1"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, json_opts()).expect("应成功");
    let lines: Vec<&str> = out.lines().collect();
    // context 1 → 3 行（前 1 + 命中 + 后 1）
    assert_eq!(lines.len(), 3, "context 1 应 3 行 NDJSON，实际: {out}");
    // 每行应为合法 NDJSON
    for line in &lines {
        let obj: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("应是 NDJSON，实际: {line}"));
        assert!(
            obj["line_no"].as_i64().is_some(),
            "NDJSON 应含 line_no，实际: {line}"
        );
        assert!(
            !obj["path"].as_str().unwrap().is_empty(),
            "NDJSON 应含 path，实际: {line}"
        );
    }
}

// ===== G11: --explain --json × 目录（P2 缺口）=====

#[test]
fn test_explain_json_directory_outputs_plan() {
    // --explain --json + 目录 → 文本执行计划 + plan_json 段
    let (dir, _) = write_temp_dir(&[("a.log", "ERROR line 1\n"), ("b.log", "info\n")]);
    let opts = RunOpts {
        explain: true,
        json: true,
        ..Default::default()
    };
    let out = run_with_opts(&args_dir(r#""ERROR""#, dir.path().to_str().unwrap()), opts)
        .expect("--explain --json 目录应成功");
    // 应含文本计划前缀 + plan_json 段
    assert!(
        out.contains("execution plan"),
        "应含 execution plan 文本，实际: {out}"
    );
    assert!(out.contains("plan_json"), "应含 plan_json 段，实际: {out}");
    assert!(
        out.contains("required_groups"),
        "plan_json 应含 required_groups，实际: {out}"
    );
}

// ===== --verify --json（borderline 缺口）=====

#[test]
fn test_verify_json_outputs_results() {
    // --verify --json → 暴力对账 + NDJSON 输出命中行
    // --verify 需要 gxp 模式（非 BARE）
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let opts = RunOpts {
        verify: true,
        json: true,
        ..Default::default()
    };
    let args = vec![
        r#""ERROR""#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, opts).expect("--verify --json 应成功");
    // --verify 应通过（PASS 或含命中行 NDJSON）
    assert!(
        out.contains("PASS") || out.contains("ERROR line 1"),
        "--verify --json 应 PASS 或含命中行，实际: {out}"
    );
}
