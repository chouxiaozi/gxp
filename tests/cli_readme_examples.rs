//! README 示例可执行守护（e2e）。
//!
//! 根因（2026-11-10 审计发现）：README 曾展示 `_ts between "12:00:00Z" and "12:30:00Z"`——
//! time-only 下界 speedate 从不支持，任何版本都直接报错，而测试无感知（违反 AGENTS §3.1
//! "示例可执行"）。本文件把 README 工作流表格中的查询逐条锚定到 e2e：
//! **改 README 示例必须同步本文件，反之亦然。**

mod common;

use common::{args, run, write_temp};

/// JSONL fixture：字段 _ts/level/service/msg/cost_ms（与 A/B 实验 911mb.log 同构）。
/// 行号（1-based）：1=11:59:59 INFO、2=12:00:00 ERROR(900)、3=12:10 ERROR(2000)、
/// 4=12:20 WARN(850)、5=12:30:00 INFO、6=12:30:01 ERROR(3000)。
const JSONL: &str = r#"{"_ts":"2026-10-20T11:59:59Z","level":"INFO","service":"checkout","msg":"ok","cost_ms":100}
{"_ts":"2026-10-20T12:00:00Z","level":"ERROR","service":"checkout","msg":"boom","cost_ms":900}
{"_ts":"2026-10-20T12:10:00Z","level":"ERROR","service":"billing","msg":"late","cost_ms":2000}
{"_ts":"2026-10-20T12:20:00Z","level":"WARN","service":"checkout","msg":"slow","cost_ms":850}
{"_ts":"2026-10-20T12:30:00Z","level":"INFO","service":"auth","msg":"edge-in","cost_ms":50}
{"_ts":"2026-10-20T12:30:01Z","level":"ERROR","service":"checkout","msg":"edge-out","cost_ms":3000}
"#;

#[test]
fn test_readme_time_window_between_full_timestamp() {
    // README "Time-window tracing" 行。含端语义手算：[12:00:00Z, 12:30:00Z] 闭区间
    // 命中第 2/3/4/5 行（12:00:00 / 12:10 / 12:20 / 12:30:00）；11:59:59 与 12:30:01 不命中。
    let f = write_temp(JSONL);
    let path = f.path().to_str().unwrap();

    let count = run(&args(
        r#"_ts between "2026-10-20T12:00:00Z" and "2026-10-20T12:30:00Z" | count"#,
        path,
    ))
    .expect("between | count 应成功");
    assert_eq!(count, "4\n");

    let out = run(&args(
        r#"_ts between "2026-10-20T12:00:00Z" and "2026-10-20T12:30:00Z""#,
        path,
    ))
    .expect("between 应成功");
    assert!(out.contains("\"msg\":\"boom\""), "窗口起点应命中: {out}");
    assert!(
        out.contains("\"msg\":\"edge-in\""),
        "上界含端 12:30:00Z 应命中: {out}"
    );
    assert!(!out.contains("11:59:59Z"), "窗口前不应命中: {out}");
    assert!(!out.contains("12:30:01Z"), "窗口后不应命中: {out}");
}

#[test]
fn test_readme_slow_query_rank_top20() {
    // README "Slow-query Top 20" 行。手算：checkout ∧ cost_ms>800 →
    // 第 6 行(3000) / 第 2 行(900) / 第 4 行(850)，降序即 6 → 2 → 4。
    let f = write_temp(JSONL);
    let out = run(&args(
        r#""checkout" && cost_ms>800 | rank top 20 by cost_ms desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("rank 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "应恰好 3 行命中: {out}");
    assert!(
        lines[0].starts_with("6:") && lines[0].contains("3000"),
        "第一名应为第 6 行 cost_ms=3000: {out}"
    );
    assert!(
        lines[1].starts_with("2:") && lines[1].contains("900"),
        "第二名应为第 2 行: {out}"
    );
    assert!(
        lines[2].starts_with("4:") && lines[2].contains("850"),
        "第三名应为第 4 行: {out}"
    );
}

#[test]
fn test_readme_error_triage_context() {
    // README "Error triage" 行（level in [ERROR] | context 3）。ERROR 行 = 第 2/3/6 行；
    // context 3 应带回邻近非 ERROR 行：第 1 行（11:59:59）与第 5 行（edge-in）。
    let f = write_temp(JSONL);
    let out = run(&args(
        "level in [ERROR] | context 3",
        f.path().to_str().unwrap(),
    ))
    .expect("context 应成功");
    for hit in ["boom", "late", "edge-out"] {
        assert!(out.contains(hit), "ERROR 行 {hit} 应在输出: {out}");
    }
    assert!(
        out.contains("11:59:59Z"),
        "context 应带回首条 ERROR 的前文行: {out}"
    );
    assert!(
        out.contains("edge-in"),
        "context 应带回末条 ERROR 的邻近 INFO 行: {out}"
    );
}

#[test]
fn test_readme_group_by_tag() {
    // README "Exception grouping" 行（ERROR | group by _tag）。_tag 取自 [tag] 字面量；
    // 手算：billing 1 条 + checkout 2 条，INFO [auth] 不参与。
    // 标签刻意选为 billing < checkout（插入序与字典序一致，断言对两种实现顺序均稳定）。
    let f = write_temp(
        "2026-10-20T12:01:00Z ERROR [billing] late cost_ms=2000\n\
         2026-10-20T12:02:00Z ERROR [checkout] boom cost_ms=900\n\
         2026-10-20T12:03:00Z ERROR [checkout] again cost_ms=100\n\
         2026-10-20T12:04:00Z INFO [auth] ok cost_ms=10\n",
    );
    let out =
        run(&args("ERROR | group by _tag", f.path().to_str().unwrap())).expect("group by 应成功");
    assert_eq!(out, "billing: 1\ncheckout: 2\n");
}
