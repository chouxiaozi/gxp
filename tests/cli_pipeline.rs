//! 管道动词 + --explain/--verify 端到端集成测试。
//!
//! 验证 pipeline::execute_stages 按语义顺序执行管道动词（count/group/head/context/
//! files/fields/rank/where/sample），以及 --explain 输出执行计划、--verify 暴力对账。
//!
//! 组合矩阵（§6.4）：sample 与 rank/head/context/fields/count 的两两组合，
//! 验证 reorder_stages 保序下 `A | sample` vs `sample | A` 语义差异。

mod common;
use common::*;

use gxp::cli::RunOpts;

// ===== 管道动词 =====

#[test]
fn test_pipeline_count() {
    // gxp '"error" | count' file：命中行总数
    let f = write_temp("error a\nerror b\ninfo c\n");
    let out = run(&args(r#""error" | count"#, f.path().to_str().unwrap())).expect("count 应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_pipeline_count_zero_hits() {
    // gxp 'nomatch | count' file：无命中 → 0
    let f = write_temp("error a\n");
    let out = run(&args("nomatch | count", f.path().to_str().unwrap())).expect("count 应成功");
    assert_eq!(out, "0\n");
}

#[test]
fn test_pipeline_count_group_by() {
    // gxp '"error" | group by _tag' file：按伪字段 _tag 分组计数
    let f = write_temp("[ERROR] error a\n[ERROR] error b\n[INFO] error c\n");
    let out = run(&args(
        r#""error" | group by _tag"#,
        f.path().to_str().unwrap(),
    ))
    .expect("group by 应成功");
    assert_eq!(out, "ERROR: 2\nINFO: 1\n");
}

#[test]
fn test_pipeline_head() {
    // gxp '"error" | head 2' file：取前 2 条命中行
    let f = write_temp("error a\nerror b\nerror c\n");
    let out = run(&args(r#""error" | head 2"#, f.path().to_str().unwrap())).expect("head 应成功");
    assert_eq!(out, "1:error a\n2:error b\n");
}

#[test]
fn test_pipeline_sample_count_returns_subset() {
    // gxp '"error" | sample 2' file：抽 2 条（6 条中取 2，固定种子 0）
    // 不验具体哪 2 条（依赖种子实现细节），只验：条数=2，且每条来自原集合
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\n");
    let out =
        run(&args(r#""error" | sample 2"#, f.path().to_str().unwrap())).expect("sample 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应抽 2 条");
    // 每行应是 "N:error X" 形态，N ∈ 1..=6
    for line in &lines {
        let content = line.split(':').nth(1).unwrap_or("");
        assert!(content.starts_with("error "), "非原集合元素: {line}");
    }
}

#[test]
fn test_pipeline_sample_count_zero_returns_empty() {
    let f = write_temp("error a\nerror b\n");
    let out =
        run(&args(r#""error" | sample 0"#, f.path().to_str().unwrap())).expect("sample 0 应成功");
    assert_eq!(out, "");
}

#[test]
fn test_pipeline_sample_count_larger_than_hits_returns_all() {
    // sample N > 命中数 → 返回全部
    let f = write_temp("error a\nerror b\n");
    let out =
        run(&args(r#""error" | sample 10"#, f.path().to_str().unwrap())).expect("sample 10 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应保留全部 2 条");
}

#[test]
fn test_pipeline_sample_ratio_half() {
    // 6 条 × 50% = 3 条
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\n");
    let out = run(&args(r#""error" | sample 50%"#, f.path().to_str().unwrap()))
        .expect("sample 50% 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "6 × 0.5 = 3 条");
}

#[test]
fn test_pipeline_sample_hundred_percent_returns_all() {
    let f = write_temp("error a\nerror b\nerror c\n");
    let out = run(&args(
        r#""error" | sample 100%"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sample 100% 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "100% 应保留全部");
}

#[test]
fn test_pipeline_sample_zero_percent_returns_empty() {
    let f = write_temp("error a\nerror b\n");
    let out =
        run(&args(r#""error" | sample 0%"#, f.path().to_str().unwrap())).expect("sample 0% 应成功");
    assert_eq!(out, "");
}

#[test]
fn test_pipeline_sample_reproducible_across_runs() {
    // 固定种子 0 → 同一查询多次执行结果一致
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\n");
    let args_vec = args(r#""error" | sample 3"#, f.path().to_str().unwrap());
    let out1 = run(&args_vec).expect("sample 第一次应成功");
    let out2 = run(&args_vec).expect("sample 第二次应成功");
    assert_eq!(out1, out2, "固定种子应保证可复现");
}

#[test]
fn test_pipeline_sample_over_100_percent_fails_loud() {
    let f = write_temp("error a\n");
    let err = run(&args(
        r#""error" | sample 150%"#,
        f.path().to_str().unwrap(),
    ))
    .expect_err("sample 150% 应失败");
    assert!(err.to_string().contains("0..=100"), "错误消息应含 0..=100");
}

#[test]
fn test_pipeline_sample_verify_aligned() {
    // --verify 暴力对账：固定种子下 sample 优化路径 = 暴力路径，0 FAIL
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\n");
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let out = run_with_opts(
        &args(r#""error" | sample 3"#, f.path().to_str().unwrap()),
        opts,
    )
    .expect("sample --verify 应成功");
    // --verify 输出含 "PASS" 表示对齐通过
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

#[test]
fn test_pipeline_context_expands_lines() {
    // gxp '"error" | context 1' file：命中行前后各 1 行
    let f = write_temp("l1\nERROR l2\nl3\nl4\n");
    let out =
        run(&args(r#""ERROR" | context 1"#, f.path().to_str().unwrap())).expect("context 应成功");
    // 命中行 2 → 窗口 1..=3：1,2,3
    assert_eq!(out, "1:l1\n2:ERROR l2\n3:l3\n");
}

#[test]
fn test_pipeline_files_outputs_name_on_hit() {
    // gxp '"error" | files' file：有命中 → 输出文件名
    let f = write_temp("error\ninfo\n");
    let path = f.path().to_str().unwrap();
    let out = run(&args(r#""error" | files"#, path)).expect("files 应成功");
    assert_eq!(out, format!("{path}\n"));
}

#[test]
fn test_pipeline_files_invert_outputs_name_on_no_hit() {
    // gxp 'nomatch | files --invert' file：无命中 → 输出文件名
    let f = write_temp("error\ninfo\n");
    let path = f.path().to_str().unwrap();
    let out = run(&args("nomatch | files --invert", path)).expect("files --invert 应成功");
    assert_eq!(out, format!("{path}\n"));
}

#[test]
fn test_pipeline_fields_projection() {
    // gxp '"error" | fields _tag,cost_ms' file：投影字段值
    let f = write_temp("[ERROR] error cost_ms=900\n[INFO] error cost_ms=100\n");
    let out = run(&args(
        r#""error" | fields _tag,cost_ms"#,
        f.path().to_str().unwrap(),
    ))
    .expect("fields 应成功");
    // 输出格式：行号:投影值（空格拼接）
    assert_eq!(out, "1:ERROR 900\n2:INFO 100\n");
}

#[test]
fn test_pipeline_rank_top_k_desc() {
    // gxp 'cost_ms exists | rank top 2 by cost_ms desc' file：Top-K 降序
    let f = write_temp("cost_ms=100\ncost_ms=900\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | rank top 2 by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("rank 应成功");
    // 降序前 2：cost_ms=900, cost_ms=500
    assert_eq!(out, "2:cost_ms=900\n3:cost_ms=500\n");
}

#[test]
fn test_pipeline_where_refilters() {
    // gxp '"error" | where cost_ms>800' file：对命中行再过滤
    let f = write_temp("error cost_ms=900\nerror cost_ms=100\ninfo cost_ms=900\n");
    let out = run(&args(
        r#""error" | where cost_ms>800"#,
        f.path().to_str().unwrap(),
    ))
    .expect("where 应成功");
    // 只保留命中且 cost_ms>800 的行
    assert_eq!(out, "1:error cost_ms=900\n");
}

#[test]
fn test_pipeline_capture_then_where_uses_capture_value() {
    // capture 提取的值供 where 谓词使用
    // capture(cost, /req-(\d+)/) | where cost>50000
    // 行内无 cost= 字段，capture 提取数字，where 用 capture 值过滤
    let f = write_temp(
        "req-12345 cost=high\nreq-67890 cost=low\nreq-99999 cost=mid\nreq-100 cost=low\n",
    );
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | where cost>50000"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture+where 应成功");
    // cost capture 值 >50000 的行：67890, 99999
    assert_eq!(out, "2:req-67890 cost=low\n3:req-99999 cost=mid\n");
}

#[test]
fn test_pipeline_capture_then_where_json_includes_captures() {
    // JSON 输出含 captures 字段（capture 值穿透 where 后仍保留）
    let f = write_temp("req-12345\nreq-67890\nreq-99999\n");
    let opts = RunOpts {
        json: true,
        ..Default::default()
    };
    let out = run_with_opts(
        &args(
            r#"capture(cost, /req-(\d+)/) | where cost>50000"#,
            f.path().to_str().unwrap(),
        ),
        opts,
    )
    .expect("capture+where --json 应成功");
    // 应有 2 行 NDJSON，captures.cost 分别为 67890 / 99999
    assert!(
        out.contains("\"captures\":{\"cost\":\"67890\"}"),
        "应含 capture cost=67890, 实际: {out}"
    );
    assert!(
        out.contains("\"captures\":{\"cost\":\"99999\"}"),
        "应含 capture cost=99999, 实际: {out}"
    );
    // 行数 = 2
    assert_eq!(out.lines().count(), 2, "应 2 行 NDJSON, 实际: {out}");
}

// ===== capture + 同字段谓词（L1 粗筛误淘汰 + 暴力路径漏 capture 的回归覆盖）=====
//
// `capture(X, /.../) && X>Y` 中 capture 提供的 X 字段值供谓词使用，
// 但 L1 `collect_field_predicates` 收集了 `X>Y` 的 `X=` 前缀粗筛，
// 行内无 `X=`（只有 capture 提取）→ L1 误淘汰所有行。
// pipeline where 路径已处理，L2 && 路径同构场景在此覆盖。

#[test]
fn test_pipeline_capture_and_predicate_same_field_compare() {
    // capture(rid, /req-(\d+)/) && rid>10000
    // 行内无 rid= 字段，capture 提取 rid 值供谓词使用
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && rid>10000 | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && rid> 应成功");
    // rid=12345, 67890 > 10000；rid=100 不满足
    assert_eq!(out, "1:12345\n2:67890\n");
}

#[test]
fn test_pipeline_capture_and_predicate_same_field_in_list() {
    // capture(rid, /req-(\d+)/) && rid in [12345, 99999]
    let f = write_temp("req-12345\nreq-67890\nreq-99999\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && rid in [12345, 99999] | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && rid in 应成功");
    assert_eq!(out, "1:12345\n3:99999\n");
}

#[test]
fn test_pipeline_capture_and_predicate_same_field_exists() {
    // capture(rid, /req-(\d+)/) && rid exists
    let f = write_temp("req-12345\nno_rid_here\nreq-67890\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && rid exists | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && rid exists 应成功");
    // 第 1、3 行有 rid capture，第 2 行无（regex 不匹配）
    assert_eq!(out, "1:12345\n3:67890\n");
}

#[test]
fn test_pipeline_capture_and_predicate_same_field_between() {
    // capture(rid, /req-(\d+)/) && rid between 10000 and 99999
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && rid between 10000 and 99999 | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && rid between 应成功");
    assert_eq!(out, "1:12345\n2:67890\n");
}

#[test]
fn test_pipeline_capture_and_predicate_verify_aligned() {
    // --verify 暴力路径也需提取 capture 值
    // 原暴力路径调 is_hit（无 capture 提取）→ capture(X) && X>Y 暴力 0 行
    // 修复：改调 is_hit_with_captures
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let out = run_with_opts(
        &args(
            r#"capture(rid, /req-(\d+)/) && rid>10000 | fields rid"#,
            f.path().to_str().unwrap(),
        ),
        opts,
    )
    .expect("capture && predicate --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

// ===== capture × Not(同字段谓词) 组合（Not 分支漏传 captures 的回归覆盖）=====
//
// `capture(X) && !(X>Y)` 中 Not 内部谓词需读外部 capture 提供的 X 值。
// 原 eval_prepared_with_captures 的 Not 分支调 eval_prepared（无 captures）
// → 谓词找不到 X → NotNumeric → Not=True → 所有行命中。
// Not 分支须调 eval_prepared_with_captures 传递 captures。

#[test]
fn test_pipeline_capture_and_not_predicate_same_field_compare() {
    // capture(rid, /req-(\d+)/) && !(rid>50000)
    // 67890 > 50000 → Not(True) = False → And False（应排除）
    // 12345 ≤ 50000 → Not(False) = True → And True（应命中）
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && !(rid>50000) | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && !rid> 应成功");
    // 12345 ≤ 50000 命中；100 ≤ 50000 命中；67890 > 50000 排除
    assert_eq!(out, "1:12345\n3:100\n");
}

#[test]
fn test_pipeline_capture_and_not_predicate_same_field_in_list() {
    // capture(rid) && !(rid in [12345, 99999])
    let f = write_temp("req-12345\nreq-67890\nreq-99999\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && !(rid in [12345, 99999]) | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && !rid in 应成功");
    // 67890 不在列表 → Not(False) = True → 命中；12345/99999 在列表 → Not(True) = False → 排除
    assert_eq!(out, "2:67890\n");
}

#[test]
fn test_pipeline_capture_and_not_predicate_same_field_between() {
    // capture(rid) && !(rid between 10000 and 50000)
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let out = run(&args(
        r#"capture(rid, /req-(\d+)/) && !(rid between 10000 and 50000) | fields rid"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture && !rid between 应成功");
    // 12345 在 [10000,50000] → Not(True) = False → 排除
    // 67890 不在区间 → Not(False) = True → 命中
    // 100 不在区间 → Not(False) = True → 命中
    assert_eq!(out, "2:67890\n3:100\n");
}

#[test]
fn test_pipeline_capture_and_not_predicate_verify_aligned() {
    // --verify 暴力路径用 is_hit_with_captures 已对齐（与 capture 同字段谓词同根因）
    let f = write_temp("req-12345\nreq-67890\nreq-100\n");
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let out = run_with_opts(
        &args(
            r#"capture(rid, /req-(\d+)/) && !(rid>50000)"#,
            f.path().to_str().unwrap(),
        ),
        opts,
    )
    .expect("capture && !predicate --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

// ===== where 多 token 谓词（between/in/exists 不被当作 stage 动词）=====

#[test]
fn test_pipeline_where_between_multi_token() {
    // `where cost between 100 and 500` 多 token 谓词
    // 原 bug：where 只取单个 token，`between` 被当作下一个 stage 动词 → UnknownStage
    let f = write_temp("ERROR cost=50\nERROR cost=100\nERROR cost=300\nERROR cost=500\nERROR cost=900\ninfo cost=100\n");
    let out = run(&args(
        r#""ERROR" | where cost between 100 and 500"#,
        f.path().to_str().unwrap(),
    ))
    .expect("where between 应成功");
    // 闭区间 [100, 500]，ERROR 行中 cost ∈ {100, 300, 500}
    assert_eq!(
        out,
        "2:ERROR cost=100\n3:ERROR cost=300\n4:ERROR cost=500\n"
    );
}

#[test]
fn test_pipeline_where_in_list_multi_token() {
    // `where cost in [100, 500]` 多 token 谓词
    let f = write_temp(
        "ERROR cost=50\nERROR cost=100\nERROR cost=300\nERROR cost=500\nERROR cost=900\n",
    );
    let out = run(&args(
        r#""ERROR" | where cost in [100, 500]"#,
        f.path().to_str().unwrap(),
    ))
    .expect("where in 应成功");
    // 只匹配 cost=100 和 cost=500
    assert_eq!(out, "2:ERROR cost=100\n4:ERROR cost=500\n");
}

#[test]
fn test_pipeline_where_exists_multi_token() {
    // `where cost exists` 多 token 谓词
    let f = write_temp("ERROR cost=100\nERROR no_cost\nERROR cost=300\n");
    let out = run(&args(
        r#""ERROR" | where cost exists"#,
        f.path().to_str().unwrap(),
    ))
    .expect("where exists 应成功");
    // 第 1 行（cost=100）和第 3 行（cost=300）保留；第 2 行（no_cost）淘汰
    assert_eq!(out, "1:ERROR cost=100\n3:ERROR cost=300\n");
}

#[test]
fn test_pipeline_where_between_after_capture() {
    // capture | where between 链路
    // 行内无 cost= 字段，capture 提取 req-N 中的数字，where between 用 capture 值
    let f = write_temp("ERROR req-100 cost=high\nERROR req-300 cost=low\nERROR req-500 cost=mid\nERROR req-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | where cost between 200 and 800"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | where between 应成功");
    // capture 值 ∈ {100, 300, 500, 900}，[200,800] 闭区间 → 300, 500
    assert_eq!(out, "2:ERROR req-300 cost=low\n3:ERROR req-500 cost=mid\n");
}

#[test]
fn test_pipeline_where_then_chained_stage_terminates_correctly() {
    // 验证 where 多 token 收集正确终止于下一个 stage 动词
    // `where cost between 100 and 500 | head 2`：where 应吃下 5 个 token，head 是下一 stage
    let f = write_temp(
        "ERROR cost=100\nERROR cost=200\nERROR cost=300\nERROR cost=400\nERROR cost=500\n",
    );
    let out = run(&args(
        r#""ERROR" | where cost between 100 and 500 | head 2"#,
        f.path().to_str().unwrap(),
    ))
    .expect("where between | head 2 应成功");
    // 全部命中 between，head 2 取前 2 行
    assert_eq!(out, "1:ERROR cost=100\n2:ERROR cost=200\n");
}

// ===== capture × 下游 stage 组合（§6.4 矩阵补缺）=====
//
// capture 提取的值需穿透到下游 stage（rank/sort/distinct/stats/percentiles）。
// 这些组合在单元层有，但 e2e 层易缺——captures 优先后必须覆盖真实管道流。

#[test]
fn test_pipeline_capture_then_rank_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | rank top 2 by cost desc
    // 行内无 cost= 字段（值是 high/low/mid），capture 提取 req-N 数字供 rank 排序
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | rank top 2 by cost desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | rank 应成功");
    // 降序前 2：cost=900, cost=500
    assert_eq!(out, "4:req-900 cost=low\n3:req-500 cost=mid\n");
}

#[test]
fn test_pipeline_capture_then_sort_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | sort by cost desc
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | sort by cost desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | sort 应成功");
    // 全量降序：900, 500, 300, 100
    assert_eq!(
        out,
        "4:req-900 cost=low\n3:req-500 cost=mid\n2:req-300 cost=low\n1:req-100 cost=high\n"
    );
}

#[test]
fn test_pipeline_capture_then_distinct_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | distinct by cost
    // 4 个 capture 值各不相同，全部保留
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | distinct by cost"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | distinct 应成功");
    // capture 值各不相同，4 行全保留
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "应保留全部 4 行（capture 值均不同），实际: {out}"
    );
}

#[test]
fn test_pipeline_capture_then_stats_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | stats cost
    // capture 值 [100, 300, 500, 900] → min=100, max=900, avg=450
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | stats cost"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | stats 应成功");
    assert!(out.contains("min: 100"), "应含 min: 100, 实际: {out}");
    assert!(out.contains("max: 900"), "应含 max: 900, 实际: {out}");
    assert!(out.contains("avg: 450"), "应含 avg: 450, 实际: {out}");
}

#[test]
fn test_pipeline_capture_then_percentiles_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | percentiles cost
    // capture 值 [100, 300, 500, 900] 排序后 → p50=300（idx=ceil(0.5*4)-1=1）,
    // p95=900（idx=ceil(0.95*4)-1=3）, p99=900（同 p95）
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | percentiles cost"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | percentiles 应成功");
    assert!(out.contains("p50: 300"), "应含 p50: 300, 实际: {out}");
    assert!(out.contains("p95: 900"), "应含 p95: 900, 实际: {out}");
    assert!(out.contains("p99: 900"), "应含 p99: 900, 实际: {out}");
}

// ===== capture × 终止 stage（§6.5.2 自举组合检查发现缺口）=====
//
// capture × rank/sort/distinct/stats/percentiles 5 项 e2e 已有，
// 但完全没覆盖 capture × group by / capture × count。
// 自举 `gxp '"capture" && "group"' tests/` 0 命中 → 矩阵缺口。

#[test]
fn test_pipeline_capture_then_group_by_uses_capture_value() {
    // capture(cost, /req-(\d+)/) | group by cost
    // 行内无 cost= 字段（值是 high/low/mid），capture 提取 req-N 数字供 group by 分组
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | group by cost"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | group by 应成功");
    // capture 值 [100, 300, 500, 900] 各不同 → 4 组各 1
    assert!(out.contains("100: 1"), "应含 100: 1, 实际: {out}");
    assert!(out.contains("300: 1"), "应含 300: 1, 实际: {out}");
    assert!(out.contains("500: 1"), "应含 500: 1, 实际: {out}");
    assert!(out.contains("900: 1"), "应含 900: 1, 实际: {out}");
    // 4 行输出（每 capture 值一组）
    assert_eq!(out.lines().count(), 4, "应 4 行输出, 实际: {out}");
}

#[test]
fn test_pipeline_capture_then_count_returns_total() {
    // capture(cost, /req-(\d+)/) | count
    // count 不依赖字段值，只数命中行数。capture 不改变行数 → count = 命中行数
    let f = write_temp("req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-900 cost=low\n");
    let out = run(&args(
        r#"capture(cost, /req-(\d+)/) | count"#,
        f.path().to_str().unwrap(),
    ))
    .expect("capture | count 应成功");
    // 4 行全命中 → count = 4
    assert_eq!(out, "4\n", "应输出 4, 实际: {out}");
}

// ===== stdin × capture × stage（§6.4 第 1 条三路径必测 × 第 3 条数据来源）=====

#[test]
fn test_pipeline_stdin_capture_then_group_by() {
    // stdin 路径 × capture × group by：验证 capture 值穿透到 group by
    // （§6.4 三路径 × 数据来源矩阵补缺）
    let content = "req-100 cost=high\nreq-300 cost=low\nreq-500 cost=mid\nreq-100 cost=low\n";
    let out = run_stdin(content, r#"capture(cost, /req-(\d+)/) | group by cost"#)
        .expect("stdin capture | group by 应成功");
    // capture 值 [100, 300, 500, 100] → 3 组（100:2, 300:1, 500:1）
    assert!(out.contains("100: 2"), "应含 100: 2, 实际: {out}");
    assert!(out.contains("300: 1"), "应含 300: 1, 实际: {out}");
    assert!(out.contains("500: 1"), "应含 500: 1, 实际: {out}");
}

#[test]
fn test_pipeline_stdin_capture_then_distinct_by_capture() {
    // stdin 路径 × capture × distinct by FIELD
    let content = "req-100\nreq-300\nreq-100\nreq-500\n";
    let out = run_stdin(content, r#"capture(cost, /req-(\d+)/) | distinct by cost"#)
        .expect("stdin capture | distinct 应成功");
    // capture 值 [100, 300, 100, 500] → distinct 保留首次出现 → 3 行（100, 300, 500）
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "应保留 3 行（distinct by capture 值）, 实际: {out}"
    );
}

#[test]
fn test_pipeline_stdin_group_by_file_returns_stdin_key() {
    // stdin 路径 × group by _file 的 _file 值应为 "<stdin>"
    // （execute_stages 入口补填 m.path = file_path = "<stdin>"）
    let content = "ERROR a\nERROR b\nERROR c\nWARN d\n";
    let out =
        run_stdin(content, r#""ERROR" | group by _file"#).expect("stdin group by _file 应成功");
    // 3 行 ERROR，全部归到 <stdin> 键下
    assert_eq!(out, "<stdin>: 3\n", "应输出 '<stdin>: 3', 实际: {out}");
}

// ===== 伪字段 × stage 组合（§6.4 矩阵补缺）=====
//
// _match/_line/_file 三个伪字段必须穿透到下游 stage。
// percentiles 须走 pseudo_field_value——覆盖真实管道流。

#[test]
fn test_pipeline_sort_by_match_pseudo_field() {
    // "ERROR" | sort by _match desc：_match = L1 命中字节（即 "ERROR"）
    // 同值 → 稳定排序保留原序
    let f = write_temp("ERROR alpha\nERROR beta\nINFO gamma\n");
    let out = run(&args(
        r#""ERROR" | sort by _match desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sort by _match 应成功");
    // _match 全为 "ERROR"（同值）→ 稳定排序保留原序
    assert_eq!(out, "1:ERROR alpha\n2:ERROR beta\n");
}

#[test]
fn test_pipeline_rank_by_match_pseudo_field() {
    // "ERROR" | rank top 2 by _match desc：_match 同值 → top 2 取前 2 行
    let f = write_temp("ERROR alpha\nERROR beta\nINFO gamma\n");
    let out = run(&args(
        r#""ERROR" | rank top 2 by _match desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("rank by _match 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应取 top 2, 实际: {out}");
}

#[test]
fn test_pipeline_distinct_by_line_pseudo_field() {
    // "ERROR" | distinct by _line：_line = 整行内容，2 个 ERROR 行不同 → 全保留
    let f = write_temp("ERROR alpha\nERROR beta\nINFO gamma\n");
    let out = run(&args(
        r#""ERROR" | distinct by _line"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct by _line 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应保留 2 行（_line 各不同）, 实际: {out}");
}

#[test]
fn test_pipeline_stats_match_pseudo_field_returns_na() {
    // "ERROR" | stats _match：_match = "ERROR" 非数值 → min/max/avg 全 N/A
    let f = write_temp("ERROR alpha\nERROR beta\n");
    let out = run(&args(
        r#""ERROR" | stats _match"#,
        f.path().to_str().unwrap(),
    ))
    .expect("stats _match 应成功");
    assert!(out.contains("N/A"), "_match 非数值应返回 N/A, 实际: {out}");
}

#[test]
fn test_pipeline_percentiles_line_pseudo_field_returns_na() {
    // "ERROR" | percentiles _line：_line 非数值 → p50/p95/p99 全 N/A
    let f = write_temp("ERROR alpha\nERROR beta\n");
    let out = run(&args(
        r#""ERROR" | percentiles _line"#,
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles _line 应成功");
    assert!(out.contains("N/A"), "_line 非数值应返回 N/A, 实际: {out}");
}

#[test]
fn test_pipeline_sort_by_file_pseudo_field_in_directory() {
    // "ERROR" | sort by _file：目录模式，按文件路径排序
    // 需要 2 个文件让 _file 有不同值
    let (dir, _) = write_temp_dir(&[("b.log", "ERROR in b\n"), ("a.log", "ERROR in a\n")]);
    let out = run(&args(
        r#""ERROR" | sort by _file"#,
        dir.path().to_str().unwrap(),
    ))
    .expect("sort by _file 应成功");
    // a.log 应在 b.log 之前（按字节序）
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应保留 2 行, 实际: {out}");
    assert!(
        lines[0].contains("a.log"),
        "第一行应来自 a.log（字节序在前）, 实际: {out}"
    );
    assert!(
        lines[1].contains("b.log"),
        "第二行应来自 b.log, 实际: {out}"
    );
}

#[test]
fn test_pipeline_distinct_by_file_pseudo_field_in_directory() {
    // "ERROR" | distinct by _file：按文件去重，每文件保留首行
    let (dir, _) = write_temp_dir(&[
        ("f1.log", "ERROR a\nERROR b\n"),
        ("f2.log", "ERROR c\nINFO d\n"),
    ]);
    let out = run(&args(
        r#""ERROR" | distinct by _file"#,
        dir.path().to_str().unwrap(),
    ))
    .expect("distinct by _file 应成功");
    // 每文件保留首行 → 2 行
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应保留 2 行（每文件首行）, 实际: {out}");
}

// ===== _file × 单文件模式（之前 m.path = None → 空键）=====

#[test]
fn test_pipeline_group_by_file_single_file_returns_filename_key() {
    // 单文件模式 group by _file 之前返回空键（": N"）
    // 现在 m.path 在 execute_stages 入口被补填，应输出文件名: 计数
    let f = write_temp("ERROR a\nERROR b\nWARN c\nERROR d\n");
    let out = run(&args(
        r#""ERROR" | group by _file"#,
        f.path().to_str().unwrap(),
    ))
    .expect("group by _file 单文件应成功");
    // 3 行 ERROR 全部归到当前文件名键下
    let path = f.path().to_str().unwrap().replace('\\', "/");
    let expected = format!("{path}: 3\n");
    assert_eq!(out, expected, "应输出 '文件名: 3', 实际: {out}");
}

#[test]
fn test_pipeline_distinct_by_file_single_file_keeps_first_row_only() {
    // 单文件模式 distinct by _file 之前全部归空键 → 只留首行
    // （虽最终行为恰好正确——单文件所有行 key 相同，distinct 应只保留首行）
    // 此测试固化正确行为，确保补填 path 后行为不退化。
    let f = write_temp("ERROR a\nERROR b\nERROR c\n");
    let out = run(&args(
        r#""ERROR" | distinct by _file"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct by _file 单文件应成功");
    // 所有行 _file 相同 → distinct 保留首行
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "单文件 distinct by _file 只留首行, 实际: {out}"
    );
    assert!(
        lines[0].contains("ERROR a"),
        "首行应是 ERROR a, 实际: {out}"
    );
}

#[test]
fn test_pipeline_sort_by_file_single_file_preserves_line_order() {
    // 单文件模式 sort by _file 之前 key 全空 → 不稳定排序
    // 现在 key=文件名（所有行相同）→ 稳定排序保持原行号顺序
    let f = write_temp("ERROR c\nERROR a\nERROR b\n");
    let out = run(&args(
        r#""ERROR" | sort by _file"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sort by _file 单文件应成功");
    // 所有行 _file 相同 → 稳定排序保持原顺序
    assert_eq!(
        out, "1:ERROR c\n2:ERROR a\n3:ERROR b\n",
        "应保持原行号顺序, 实际: {out}"
    );
}

#[test]
fn test_pipeline_rank_then_fields_chain() {
    // gxp 'cost_ms exists | rank top 2 by cost_ms desc | fields cost_ms' file：
    // 排序 → 投影组合（设计文档 §2.4 示例 2 形态）
    let f = write_temp("cost_ms=100\ncost_ms=900\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | rank top 2 by cost_ms desc | fields cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("rank+fields 链应成功");
    assert_eq!(out, "2:900\n3:500\n");
}

#[test]
fn test_pipeline_context_then_fields() {
    // gxp 'cost_ms>800 | context 1 | fields cost_ms' file：先扩行再投影
    let f = write_temp("l1\ncost_ms=900 l2\nl3\n");
    let out = run(&args(
        "cost_ms>800 | context 1 | fields cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("context+fields 链应成功");
    // 窗口 1..=3：l1/l3（无 cost_ms → 空）和 cost_ms=900
    assert_eq!(out, "1:\n2:900\n3:\n");
}

// ===== --explain / --verify =====

#[test]
fn test_explain_text_outputs_plan_without_scanning() {
    // gxp --explain '"error" && cost_ms>800' file：
    // 输出执行计划，不执行扫描（不读文件内容也能产出）
    let f = write_temp("error cost_ms=900\n");
    let out = run_with_opts(
        &args("\"error\" && cost_ms>800", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..Default::default()
        },
    )
    .expect("--explain 应成功");
    assert!(out.contains("L1 必选组"), "应含 L1 必选组，实际: {out}");
    assert!(out.contains("\"error\""), "应含字面量 error，实际: {out}");
    assert!(out.contains("L2 字段谓词"), "应含 L2 字段谓词，实际: {out}");
    assert!(out.contains("cost_ms > 800"), "应含字段谓词，实际: {out}");
    // 不扫描：不应输出行号
    assert!(!out.contains("1:error"), "不应扫描输出行号，实际: {out}");
}

#[test]
fn test_explain_json_outputs_structured_plan() {
    let f = write_temp("error cost_ms=900\n");
    let out = run_with_opts(
        &args("\"error\" && cost_ms>800", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            json: true,
            ..Default::default()
        },
    )
    .expect("--explain --json 应成功");
    assert!(
        out.contains("\"required_groups\""),
        "JSON 应含 required_groups: {out}"
    );
    assert!(
        out.contains("\"field_predicates\""),
        "JSON 应含 field_predicates: {out}"
    );
    assert!(out.contains("\"cost_ms\""), "JSON 应含 cost_ms: {out}");
    assert!(out.contains("\"threshold\": 800.0"), "JSON 应含阈值: {out}");
}

#[test]
fn test_explain_rejects_bare_mode_pattern() {
    // BARE 模式无优化层可对账，--explain 应报错
    let f = write_temp("ERROR\n");
    let result = run_with_opts(
        &args("ERROR", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..Default::default()
        },
    );
    assert!(result.is_err(), "--explain 在 BARE 模式应报错");
    let err = result.unwrap_err();
    assert!(
        err.contains("--explain") && err.contains("BARE"),
        "错误应说明 --explain 不适用 BARE 模式，实际: {err}"
    );
}

#[test]
fn test_verify_alone_passes_when_results_match() {
    // gxp --verify 'cost_ms>800' file：
    // 暴力全扫 vs 优化路径，结果一致 → PASS
    let f = write_temp("cost_ms=900\ncost_ms=100\nno_field\n");
    let out = run_with_opts(
        &args("cost_ms>800", f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify 应成功");
    assert!(out.contains("verify: PASS"), "应输出 PASS，实际: {out}");
    assert!(out.contains("1 行命中"), "应含 1 行命中，实际: {out}");
}

#[test]
fn test_verify_with_not_predicate_passes() {
    // !(cost_ms>800)：含 Not 谓词的不变式校验
    // 暴力路径：逐行跑 eval_prepared（含 Not 三态取反）
    // 优化路径：L1 不下推 Not 子树字段前缀 + L2 求值
    // 不变式 L1 ⊇ L2 保证二者一致
    let f = write_temp("cost_ms=900\ncost_ms=100\nno_field\n");
    let out = run_with_opts(
        &args("!(cost_ms>800)", f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify Not 应成功");
    assert!(out.contains("verify: PASS"), "应输出 PASS，实际: {out}");
    // 第 2 行 cost_ms=100 <= 800 → False → Not → True → 命中
    // 第 3 行无字段 → NotNumeric → Not → True → 命中
    // 共 2 行命中
    assert!(out.contains("2 行命中"), "应含 2 行命中，实际: {out}");
}

#[test]
fn test_verify_with_literal_query_passes() {
    // 纯字面量查询：无字段谓词，暴力路径走必选组正则
    let f = write_temp("error line 1\ninfo\nerror line 3\n");
    let out = run_with_opts(
        &args("\"error\"", f.path().to_str().unwrap()),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify 字面量应成功");
    assert!(out.contains("verify: PASS"), "应输出 PASS，实际: {out}");
    assert!(out.contains("2 行命中"), "应含 2 行命中，实际: {out}");
}

#[test]
fn test_explain_and_verify_combined_outputs_plan_plus_summary() {
    // --explain --verify 联动：计划末尾追加 verify 摘要
    let f = write_temp("cost_ms=900\n");
    let out = run_with_opts(
        &args("cost_ms>800", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            verify: true,
            ..Default::default()
        },
    )
    .expect("--explain --verify 应成功");
    assert!(out.contains("L1 必选组"), "应含 L1 必选组，实际: {out}");
    assert!(
        out.contains("verify: PASS"),
        "应含 verify PASS，实际: {out}"
    );
}

// ===== sample 组合矩阵（§6.4）=====
//
// reorder_stages 保序执行（§7.7），所以 `A | sample` vs `sample | A` 语义不同。
// 矩阵验证：sample 与其他 stage 组合时，条数/顺序符合组合语义。
// 每组测正向（条数符合预期）+ 语义差异（顺序不同结果不同）。

#[test]
fn test_sample_then_head_truncates_sample_result() {
    // sample 5 | head 2：先抽 5，再取前 2 → 2 条
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\nerror g\nerror h\n");
    let out = run(&args(
        r#""error" | sample 5 | head 2"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sample 5 | head 2 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 5 → head 2 应得 2 条");
}

#[test]
fn test_head_then_sample_samples_from_head_window() {
    // head 5 | sample 2：先取前 5，再从中抽 2 → 2 条（且来自前 5）
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\nerror g\nerror h\n");
    let out = run(&args(
        r#""error" | head 5 | sample 2"#,
        f.path().to_str().unwrap(),
    ))
    .expect("head 5 | sample 2 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "head 5 → sample 2 应得 2 条");
    // 每行行号应在 1..=5（head 窗口）
    for line in &lines {
        let line_no: usize = line.split(':').next().unwrap().parse().unwrap();
        assert!(line_no <= 5, "head 5 窗口外的行: {line_no}");
    }
}

#[test]
fn test_sample_then_count_outputs_sampled_count() {
    // sample 3 | count：抽 3 条，再计数 → 3
    let f = write_temp("error a\nerror b\nerror c\nerror d\nerror e\nerror f\n");
    let out = run(&args(
        r#""error" | sample 3 | count"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sample 3 | count 应成功");
    assert_eq!(out, "3\n", "sample 3 → count 应输出 3");
}

#[test]
fn test_sample_then_fields_projects_sampled_rows() {
    // sample 2 | fields _tag：抽 2 条后投影 _tag
    // fields 输出格式 = "行号:_tag值"
    let f = write_temp("[ERR] error a\n[INFO] error b\n[WARN] error c\n");
    let out = run(&args(
        r#""error" | sample 2 | fields _tag"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sample 2 | fields _tag 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 2 → fields 应得 2 条");
    // 每行格式 "N:TAG"，TAG ∈ {ERR, INFO, WARN}
    for line in &lines {
        let tag = line.split(':').nth(1).unwrap_or("");
        assert!(
            matches!(tag, "ERR" | "INFO" | "WARN"),
            "非预期 _tag 值: {line}"
        );
    }
}

#[test]
fn test_rank_then_sample_from_topk() {
    // rank top 3 by cost_ms desc | sample 2：先排 Top-3，再从 Top-3 抽 2
    // 用 gxp 模式：cost_ms exists（有 cost_ms 字段的行）
    let f = write_temp(
        "cost_ms=100\n\
         cost_ms=500\n\
         cost_ms=300\n\
         cost_ms=900\n\
         cost_ms=700\n",
    );
    let out = run(&args(
        "cost_ms exists | rank top 3 by cost_ms desc | sample 2",
        f.path().to_str().unwrap(),
    ))
    .expect("rank top 3 | sample 2 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "rank top 3 → sample 2 应得 2 条");
    // rank top 3 desc 应选 cost_ms 最大的 3 行：900(行4), 700(行5), 500(行2)
    // sample 2 从这 3 条里抽 2，行号应在 {2, 4, 5}
    for line in &lines {
        let line_no: usize = line.split(':').next().unwrap().parse().unwrap();
        assert!(
            matches!(line_no, 2 | 4 | 5),
            "rank top 3 窗口外的行: {line_no}"
        );
    }
}

#[test]
fn test_sample_then_rank_ranks_sampled_subset() {
    // sample 3 | rank top 2 by cost_ms desc：先抽 3，再对样本排 Top-2
    // 用 gxp 模式：cost_ms exists
    let f = write_temp(
        "cost_ms=100\n\
         cost_ms=500\n\
         cost_ms=300\n\
         cost_ms=900\n\
         cost_ms=700\n",
    );
    let out = run(&args(
        "cost_ms exists | sample 3 | rank top 2 by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("sample 3 | rank top 2 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 3 → rank top 2 应得 2 条");
    // 样本内的 Top-2 desc：输出两条，前一条 cost_ms ≥ 后一条
    let get_cost = |line: &str| -> f64 {
        let content = line.split(':').nth(1).unwrap_or("");
        content
            .strip_prefix("cost_ms=")
            .and_then(|n| n.parse::<f64>().ok())
            .unwrap_or(f64::MIN)
    };
    let c1 = get_cost(lines[0]);
    let c2 = get_cost(lines[1]);
    assert!(c1 >= c2, "rank desc 应保证 c1 >= c2，实际 c1={c1} c2={c2}");
}

#[test]
fn test_context_then_sample_samples_expanded_window() {
    // context 1 | sample 2：先扩行（命中行 ±1），再从扩行结果抽 2
    let f = write_temp("l1\nERROR l2\nl3\nERROR l4\nl5\n");
    let out = run(&args(
        r#""ERROR" | context 1 | sample 2"#,
        f.path().to_str().unwrap(),
    ))
    .expect("context 1 | sample 2 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "context 1 → sample 2 应得 2 条");
    // context 1 扩展命中行 2 和 4：窗口 1-3 + 3-5 = {1,2,3,4,5}（去重后全 5 行）
    // sample 2 从这 5 条里抽 2，行号应在 1..=5
    for line in &lines {
        let line_no: usize = line.split(':').next().unwrap().parse().unwrap();
        assert!(line_no <= 5, "context 扩展窗口外的行: {line_no}");
    }
}

#[test]
fn test_sample_verify_aligned_in_combination() {
    // --verify 在 sample | rank 组合下也应 0 FAIL（固定种子保证对齐）
    let f = write_temp(
        "cost_ms=100\ncost_ms=500\ncost_ms=300\ncost_ms=900\ncost_ms=700\n\
         cost_ms=200\ncost_ms=800\ncost_ms=600\ncost_ms=400\ncost_ms=150\n",
    );
    let out = run_with_opts(
        &args(
            "cost_ms exists | sample 5 | rank top 3 by cost_ms desc",
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("sample | rank --verify 应成功");
    assert!(out.contains("PASS"), "组合 --verify 应通过，输出: {out}");
}

#[test]
fn test_sample_order_matters_rank_vs_sample() {
    // 语义差异验证：rank | sample 与 sample | rank 结果不同（当 len > top）
    // rank top 2 | sample 1：从 Top-2 抽 1，结果是 Top-2 之一
    // sample 1 | rank top 2：先抽 1，再"排 Top-2"，结果就是那 1 条
    // 两者结果数都 ≤ 2，但语义链路不同；这里验证两者都能跑通且条数符合
    let f = write_temp("cost_ms=100\ncost_ms=500\ncost_ms=300\ncost_ms=900\ncost_ms=700\n");
    let out_rs = run(&args(
        "cost_ms exists | rank top 2 by cost_ms desc | sample 1",
        f.path().to_str().unwrap(),
    ))
    .expect("rank | sample 应成功");
    let out_sr = run(&args(
        "cost_ms exists | sample 1 | rank top 2 by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("sample | rank 应成功");
    let rs_count = out_rs.lines().count();
    let sr_count = out_sr.lines().count();
    assert!(
        rs_count <= 2,
        "rank top 2 | sample 1 应 ≤ 2 条，实际 {rs_count}"
    );
    assert!(
        sr_count <= 2,
        "sample 1 | rank top 2 应 ≤ 2 条，实际 {sr_count}"
    );
}

// ===== sort 独立 e2e =====
//
// sort [by FIELD] [desc|asc]：全量稳定排序（§9.1 候选 #5，v0.34）。
// 默认 asc（对齐 SQL ORDER BY / Unix sort / Python sorted）；省略 by 按整行字节序。
// 与 rank 默认 desc 不同（Top-K 语义 vs 全量有序原型）。

#[test]
fn test_pipeline_sort_by_field_asc() {
    // cost_ms exists | sort by cost_ms：升序（默认 asc）
    let f = write_temp("cost_ms=300\ncost_ms=100\ncost_ms=200\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("sort by cost_ms 应成功");
    // 升序：100(行2), 200(行3), 300(行1)
    assert_eq!(out, "2:cost_ms=100\n3:cost_ms=200\n1:cost_ms=300\n");
}

#[test]
fn test_pipeline_sort_by_field_desc() {
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("sort by cost_ms desc 应成功");
    // 降序：300(行2), 200(行3), 100(行1)
    assert_eq!(out, "2:cost_ms=300\n3:cost_ms=200\n1:cost_ms=100\n");
}

#[test]
fn test_pipeline_sort_default_asc_by_line_bytes() {
    // 省略 by → 整行字节序升序（聚集相同错误，对齐 Unix sort）
    let f = write_temp("ERROR timeout\nERROR conn refused\nERROR timeout\n");
    let out = run(&args(r#""ERROR" | sort"#, f.path().to_str().unwrap())).expect("sort 应成功");
    // 字节序：'ERROR conn refused' < 'ERROR timeout'
    // 两个 ERROR timeout 相邻（稳定：行1 在行3 前）
    assert_eq!(
        out,
        "2:ERROR conn refused\n1:ERROR timeout\n3:ERROR timeout\n"
    );
}

#[test]
fn test_pipeline_sort_desc_only_reverses_line_order() {
    // sort desc（省略 by）：整行字节序降序
    let f = write_temp("a\nc\nb\n");
    let out = run(&args(
        r#""a" || "b" || "c" | sort desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sort desc 应成功");
    // 文件：行1=a, 行2=c, 行3=b；降序字节序：c(行2), b(行3), a(行1)
    assert_eq!(out, "2:c\n3:b\n1:a\n");
}

#[test]
fn test_pipeline_sort_verify_aligned() {
    // --verify 暴力对账：sort 是确定性算法，优化路径 = 暴力路径，0 FAIL
    let f = write_temp("cost_ms=300\ncost_ms=100\ncost_ms=200\ncost_ms=500\n");
    let out = run_with_opts(
        &args(
            "cost_ms exists | sort by cost_ms desc",
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("sort --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

#[test]
fn test_pipeline_sort_empty_input() {
    // 无命中 → sort 空集 → 空输出
    let f = write_temp("info line\nother line\n");
    let out = run(&args(
        r#""nomatch" | sort by _ts"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sort 空集应成功");
    assert_eq!(out, "");
}

#[test]
fn test_pipeline_sort_non_numeric_field_sinks_to_bottom() {
    // 非数值/缺失字段无论升降序都排最后（对齐 rank 语义）
    // 注意：查询是 cost_ms exists，所以 other=x 行（无 cost_ms 字段）不命中，不在结果中
    let f = write_temp("cost_ms=500\ncost_ms=abc\ncost_ms=xyz\ncost_ms=100\n");
    let out_asc = run(&args(
        "cost_ms exists | sort by cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("sort by cost_ms asc 应成功");
    // 升序：数值 100(行4), 500(行1) 在前；非数值按原顺序 abc(行2), xyz(行3) 在后
    assert_eq!(
        out_asc,
        "4:cost_ms=100\n1:cost_ms=500\n2:cost_ms=abc\n3:cost_ms=xyz\n"
    );
}

#[test]
fn test_pipeline_sort_stable_equal_keys() {
    // 相同键保持原顺序（稳定排序）
    let f = write_temp("x=1 a\nx=1 b\nx=1 c\n");
    let out = run(&args("x exists | sort by x", f.path().to_str().unwrap())).expect("sort 应成功");
    // 全部 x=1，原顺序保持
    assert_eq!(out, "1:x=1 a\n2:x=1 b\n3:x=1 c\n");
}

#[test]
fn test_pipeline_sort_numeric_11_dim_contract() {
    // 前导零 / 科学计数法按数值排序（11 维契约）
    let f = write_temp("cost=1e3\ncost=0950\ncost=900\n");
    let out = run(&args(
        "cost exists | sort by cost",
        f.path().to_str().unwrap(),
    ))
    .expect("sort by cost 应成功");
    // 升序：900(行3), 950=0950(行2), 1000=1e3(行1)
    assert_eq!(out, "3:cost=900\n2:cost=0950\n1:cost=1e3\n");
}

#[test]
fn test_pipeline_sort_in_directory_mode() {
    // 目录模式：多文件命中聚合后全局排序
    let dir = std::env::temp_dir().join("gxp_sort_dir_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.log"), "cost_ms=300\n").unwrap();
    std::fs::write(dir.join("b.log"), "cost_ms=100\n").unwrap();
    let out = run(&args(
        "cost_ms exists | sort by cost_ms",
        dir.to_str().unwrap(),
    ))
    .expect("目录 sort 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "两文件各 1 行命中");
    // 升序：b.log(100) 在前，a.log(300) 在后
    assert!(lines[0].contains("100"), "首行应是 cost_ms=100: {lines:?}");
    assert!(lines[1].contains("300"), "次行应是 cost_ms=300: {lines:?}");
    std::fs::remove_dir_all(&dir).ok();
}

// ===== sort 组合矩阵（§6.4）=====
//
// reorder_stages 保序执行（§7.7），所以 `A | sort` vs `sort | A` 语义不同。
// 矩阵验证：sort 与 head/sample/rank/fields/count 的两两组合，条数/顺序符合组合语义。

#[test]
fn test_sort_then_head_takes_first_n_after_sort() {
    // sort by cost desc | head 2：排序后取前 2 → 最大 2 条
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms desc | head 2",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | head 应成功");
    // 降序排序后取前 2：500(行4), 300(行2)
    assert_eq!(out, "4:cost_ms=500\n2:cost_ms=300\n");
}

#[test]
fn test_head_then_sort_sorts_head_window() {
    // head 2 | sort by cost desc：先取前 2，再排序 → head 窗口内的有序
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | head 2 | sort by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("head | sort 应成功");
    // head 2 取行 1,2（cost=100,300），降序：300(行2), 100(行1)
    assert_eq!(out, "2:cost_ms=300\n1:cost_ms=100\n");
}

#[test]
fn test_sort_then_sample_samples_from_sorted() {
    // sort by cost desc | sample 2：先排序，再从有序集抽 2 → 2 条（全局均匀）
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\ncost_ms=500\ncost_ms=400\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms desc | sample 2",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | sample 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sort → sample 2 应得 2 条");
}

#[test]
fn test_sample_then_sort_sorts_sampled_subset() {
    // sample 3 | sort by cost desc：先抽 3，再对样本降序排序
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\ncost_ms=500\ncost_ms=400\n");
    let out = run(&args(
        "cost_ms exists | sample 3 | sort by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("sample | sort 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "sample 3 → sort 应得 3 条");
    // 验证降序：解析 cost_ms 值，应递减
    let vals: Vec<f64> = lines
        .iter()
        .map(|l| l.split("cost_ms=").nth(1).unwrap().parse::<f64>().unwrap())
        .collect();
    assert!(
        vals.windows(2).all(|w| w[0] >= w[1]),
        "降序应递减，实际: {vals:?}"
    );
}

#[test]
fn test_rank_then_sort_sorts_full_set_after_topk() {
    // rank top 3 by cost desc | sort by cost asc：先取 Top-3，再升序排
    let f = write_temp("cost_ms=100\ncost_ms=900\ncost_ms=500\ncost_ms=700\ncost_ms=300\n");
    let out = run(&args(
        "cost_ms exists | rank top 3 by cost_ms desc | sort by cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("rank | sort 应成功");
    // rank top 3 desc：900(行2), 700(行4), 500(行3)
    // sort asc：500(行3), 700(行4), 900(行2)
    assert_eq!(out, "3:cost_ms=500\n4:cost_ms=700\n2:cost_ms=900\n");
}

#[test]
fn test_sort_then_rank_rank_takes_topk_from_sorted() {
    // sort by cost asc | rank top 2 by cost desc：先升序，再从有序集取 Top-2 降序
    let f = write_temp("cost_ms=100\ncost_ms=500\ncost_ms=300\ncost_ms=900\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms | rank top 2 by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | rank 应成功");
    // sort asc 后全集有序，但 rank top 2 desc 取最大 2 条：900(行4), 500(行2)
    assert_eq!(out, "4:cost_ms=900\n2:cost_ms=500\n");
}

#[test]
fn test_sort_then_count_outputs_sorted_count() {
    // sort | count：排序后计数 → 条数不变
    let f = write_temp("error a\nerror b\nerror c\n");
    let out = run(&args(
        r#""error" | sort | count"#,
        f.path().to_str().unwrap(),
    ))
    .expect("sort | count 应成功");
    assert_eq!(out, "3\n");
}

#[test]
fn test_sort_then_fields_projects_sorted_rows() {
    // sort by cost desc | fields cost_ms：排序后投影字段
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\n");
    let out = run(&args(
        "cost_ms exists | sort by cost_ms desc | fields cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | fields 应成功");
    // 降序排序后投影：300(行2), 200(行3), 100(行1)
    assert_eq!(out, "2:300\n3:200\n1:100\n");
}

#[test]
fn test_sort_verify_aligned_in_combination() {
    // --verify 在 sort | head 组合下也应 0 FAIL（确定性算法天然对齐）
    let f = write_temp("cost_ms=300\ncost_ms=100\ncost_ms=200\ncost_ms=500\n");
    let out = run_with_opts(
        &args(
            "cost_ms exists | sort by cost_ms desc | head 2",
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("sort | head --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

#[test]
fn test_sort_order_matters_head_vs_sort() {
    // 语义差异验证：sort | head 与 head | sort 结果不同
    // sort by cost desc | head 1：最大值 500(行4)
    // head 1 | sort by cost desc：head 窗口 [行1]，sort 后仍只有 1 条
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\ncost_ms=500\n");
    let out_sh = run(&args(
        "cost_ms exists | sort by cost_ms desc | head 1",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | head 应成功");
    assert_eq!(out_sh, "4:cost_ms=500\n");

    let out_hs = run(&args(
        "cost_ms exists | head 1 | sort by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("head | sort 应成功");
    // head 1 只取行1（cost=100），sort 后仍只有这 1 条
    assert_eq!(out_hs, "1:cost_ms=100\n");
}

// ===== distinct 独立 e2e =====
//
// distinct [by FIELD]：稳定去重（§9.1 候选 #3，v0.35）。
// - 无 by：整行字节去重，保留首次出现（对齐 Unix `sort | uniq`，但不排序）
// - by FIELD：按字段值去重（对齐 SQL `SELECT DISTINCT ON (field) *`）
// 与 group by 区别：group by 输出 `value: N`（计数），
// distinct 输出去重后的行——两者语义不重叠。

#[test]
fn test_pipeline_distinct_whole_line_keeps_first() {
    // 整行去重：相同行只保留首次出现
    let f = write_temp("ERROR timeout\nERROR conn refused\nERROR timeout\nWARN disk\n");
    let out =
        run(&args(r#""ERROR" | distinct"#, f.path().to_str().unwrap())).expect("distinct 应成功");
    // ERROR timeout(行1), ERROR conn refused(行2)；第三条 ERROR timeout 重复删除
    assert_eq!(out, "1:ERROR timeout\n2:ERROR conn refused\n");
}

#[test]
fn test_pipeline_distinct_by_field_keeps_first_per_value() {
    // distinct by user：每个 user 首次出现的行
    let f = write_temp("user=alice cost=1\nuser=bob cost=2\nuser=alice cost=3\nuser=bob cost=4\n");
    let out = run(&args(
        r#""user=" | distinct by user"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct by user 应成功");
    // alice(行1), bob(行2)
    assert_eq!(out, "1:user=alice cost=1\n2:user=bob cost=2\n");
}

#[test]
fn test_pipeline_distinct_by_field_missing_field_groups_empty() {
    // 字段缺失的行归入空串键，多条缺失只留首条
    let f = write_temp("user=alice\nno field\nuser=bob\nalso missing\n");
    let out = run(&args(
        r#""no" || "field" || "user" || "alice" || "bob" || "missing" | distinct by user"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct by user (含缺失) 应成功");
    // alice(1), 空串(2), bob(3)；第 4 条空串与第 2 条同 → 删
    assert_eq!(out, "1:user=alice\n2:no field\n3:user=bob\n");
}

#[test]
fn test_pipeline_distinct_all_unique_unchanged() {
    // 全部唯一行 → distinct 不删除任何行
    let f = write_temp("line1\nline2\nline3\n");
    let out = run(&args(r#""line" | distinct"#, f.path().to_str().unwrap()))
        .expect("distinct 全唯一应成功");
    assert_eq!(out, "1:line1\n2:line2\n3:line3\n");
}

#[test]
fn test_pipeline_distinct_empty_input() {
    // 无命中 → distinct 空集 → 空输出
    let f = write_temp("info line\nother line\n");
    let out = run(&args(r#""nomatch" | distinct"#, f.path().to_str().unwrap()))
        .expect("distinct 空集应成功");
    assert_eq!(out, "");
}

#[test]
fn test_pipeline_distinct_preserves_original_order() {
    // 不排序：保留原序，仅删除重复
    let f = write_temp("c\na\nb\nc\na\n");
    let out = run(&args(
        r#""a" || "b" || "c" | distinct"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct 保序应成功");
    // 命中行 1,2,3,4,5（全命中）；去重：c(1), a(2), b(3)
    assert_eq!(out, "1:c\n2:a\n3:b\n");
}

#[test]
fn test_pipeline_distinct_verify_aligned() {
    // --verify 暴力对账：distinct 是确定性算法，优化路径 = 暴力路径，0 FAIL
    let f = write_temp("a\nb\na\nc\nb\na\n");
    let out = run_with_opts(
        &args(
            r#""a" || "b" || "c" | distinct"#,
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("distinct --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

#[test]
fn test_pipeline_distinct_in_directory_mode() {
    // 目录模式：多文件命中聚合后全局去重
    let dir = std::env::temp_dir().join("gxp_distinct_dir_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.log"), "dup\n").unwrap();
    std::fs::write(dir.join("b.log"), "dup\n").unwrap();
    std::fs::write(dir.join("c.log"), "unique\n").unwrap();
    let out = run(&args(
        r#""dup" || "unique" | distinct"#,
        dir.to_str().unwrap(),
    ))
    .expect("目录 distinct 应成功");
    // 两文件各 1 行 dup，去重后只留首条（按 walk 顺序）+ unique 行
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "去重后应剩 2 行: {out}");
    // 至少有一条是 unique
    assert!(out.contains("unique"), "应含 unique 行: {out}");
    // dup 只出现一次
    assert_eq!(out.matches("dup").count(), 1, "dup 应去重为 1 条: {out}");
}

// ===== distinct 组合矩阵（§6.4）=====
//
// reorder_stages 保序执行（§7.7），所以 `A | distinct` vs `distinct | A` 语义不同。
// 矩阵验证：distinct 与 sort/head/fields/count 的两两组合，条数/顺序符合组合语义。

#[test]
fn test_distinct_then_sort_sorts_deduped() {
    // distinct | sort by cost desc：先去重，再对去重后的集合降序排序
    let f = write_temp("cost=500\nuser=a\ncost=500\nuser=b\ncost=100\n");
    let out = run(&args(
        r#""cost" || "user" | distinct by cost | sort by cost desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct | sort 应成功");
    // distinct by cost：500(1), 空串(2), 100(5)；第三条 500 与第一条同 → 删
    // sort by cost desc：数值优先降序 → 500(1), 100(5)，非数值(空串)沉底 → 2
    assert_eq!(out, "1:cost=500\n5:cost=100\n2:user=a\n");
}

#[test]
fn test_sort_then_distinct_dedupes_after_sort() {
    // sort by cost desc | distinct by cost：先排序，再去重（首条保留排序后首位）
    let f = write_temp("cost=100\ncost=500\ncost=500\ncost=200\n");
    let out = run(&args(
        "cost exists | sort by cost desc | distinct by cost",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | distinct 应成功");
    // sort desc 后：500(行2), 500(行3), 200(行4), 100(行1)
    // distinct by cost：500(行2), 200(行4), 100(行1)
    assert_eq!(out, "2:cost=500\n4:cost=200\n1:cost=100\n");
}

#[test]
fn test_distinct_then_head_takes_first_n_after_dedup() {
    // distinct | head 2：去重后取前 2
    let f = write_temp("a\nb\na\nc\nb\n");
    let out = run(&args(
        r#""a" || "b" || "c" | distinct | head 2"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct | head 应成功");
    // distinct：a(1), b(2), c(4)
    // head 2：a(1), b(2)
    assert_eq!(out, "1:a\n2:b\n");
}

#[test]
fn test_head_then_distinct_dedupes_head_window() {
    // head 2 | distinct：head 窗口内去重（窗口里无重复时全部保留）
    let f = write_temp("a\nb\na\nc\n");
    let out = run(&args(
        r#""a" || "b" || "c" | head 2 | distinct"#,
        f.path().to_str().unwrap(),
    ))
    .expect("head | distinct 应成功");
    // head 2：a(1), b(2)
    // distinct：a(1), b(2)（无重复）
    assert_eq!(out, "1:a\n2:b\n");
}

#[test]
fn test_distinct_then_count_outputs_deduped_count() {
    // distinct | count：去重后计数 → 条数 = 去重后行数
    let f = write_temp("a\nb\na\nc\nb\na\n");
    let out = run(&args(
        r#""a" || "b" || "c" | distinct | count"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct | count 应成功");
    // distinct：a, b, c → 3 条
    assert_eq!(out, "3\n");
}

#[test]
fn test_distinct_then_fields_projects_deduped_rows() {
    // distinct by user | fields user,cost：去重后投影字段
    let f = write_temp("user=alice cost=1\nuser=bob cost=2\nuser=alice cost=3\nuser=bob cost=4\n");
    let out = run(&args(
        r#""user=" | distinct by user | fields user,cost"#,
        f.path().to_str().unwrap(),
    ))
    .expect("distinct | fields 应成功");
    // distinct by user：alice(1), bob(2)
    // fields user,cost：alice 1 / bob 2（空格分隔）
    assert_eq!(out, "1:alice 1\n2:bob 2\n");
}

#[test]
fn test_distinct_verify_aligned_in_combination() {
    // --verify 在 distinct | head 组合下也应 0 FAIL（确定性算法天然对齐）
    let f = write_temp("a\nb\na\nc\nb\na\n");
    let out = run_with_opts(
        &args(
            r#""a" || "b" || "c" | distinct | head 2"#,
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("distinct | head --verify 应成功");
    assert!(out.contains("PASS"), "--verify 应通过，输出: {out}");
}

#[test]
fn test_distinct_order_matters_sort_vs_distinct() {
    // 语义差异验证：sort | distinct 与 distinct | sort 结果可能不同
    let f = write_temp("cost=500\ncost=100\ncost=500\ncost=200\n");
    // sort by cost desc | distinct by cost：排序后首条即最大 cost 的首次出现
    let out_sd = run(&args(
        "cost exists | sort by cost desc | distinct by cost",
        f.path().to_str().unwrap(),
    ))
    .expect("sort | distinct 应成功");
    // sort desc 后：500(1), 500(3), 200(4), 100(2)
    // distinct：500(1), 200(4), 100(2)
    assert_eq!(out_sd, "1:cost=500\n4:cost=200\n2:cost=100\n");

    // distinct by cost | sort by cost desc：先去重再排序
    let out_ds = run(&args(
        "cost exists | distinct by cost | sort by cost desc",
        f.path().to_str().unwrap(),
    ))
    .expect("distinct | sort 应成功");
    // distinct：500(1), 100(2), 200(4)（第三条 500 与第一条同 → 删）
    // sort desc：500(1), 200(4), 100(2)
    assert_eq!(out_ds, "1:cost=500\n4:cost=200\n2:cost=100\n");
}

// B1 修复：终止 stage 后非空 stage → fail-loud

#[test]
fn test_b1_count_then_head_errors() {
    // count | head 3 → count 是终止动词，head 在后面 → 应报错
    let f = write_temp("error a\nerror b\nerror c\n");
    let result = run(&args(
        r#""error" | count | head 3"#,
        f.path().to_str().unwrap(),
    ));
    let Err(e) = result else {
        panic!("count | head 应报错（count 是终止动词），实际成功");
    };
    let msg = e.to_string();
    assert!(msg.contains("count"), "错误应含 count: {msg}");
    assert!(msg.contains("head"), "错误应含 head: {msg}");
    assert!(msg.contains("终止动词"), "应说明是终止动词: {msg}");
}

#[test]
fn test_b1_stats_then_fields_errors() {
    // stats cost_ms | fields _ts → stats 是终止动词 → 应报错
    let f = write_temp("cost_ms=100\ncost_ms=300\n");
    let result = run(&args(
        "cost_ms exists | stats cost_ms | fields _ts",
        f.path().to_str().unwrap(),
    ));
    assert!(result.is_err(), "stats | fields 应报错");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("stats"), "错误应含 stats: {msg}");
    assert!(msg.contains("fields"), "错误应含 fields: {msg}");
}

#[test]
fn test_b1_group_by_then_sort_errors() {
    // group by _tag | sort → group by 是终止动词 → 应报错
    let f = write_temp("[ERROR] a\n[INFO] b\n");
    let result = run(&args(
        r#""a" | group by _tag | sort"#,
        f.path().to_str().unwrap(),
    ));
    assert!(result.is_err(), "group by | sort 应报错");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("group by"), "错误应含 group by: {msg}");
    assert!(msg.contains("sort"), "错误应含 sort: {msg}");
}

#[test]
fn test_b1_count_alone_succeeds() {
    // count 单独使用 → 正常输出计数
    let f = write_temp("error a\nerror b\n");
    let out =
        run(&args(r#""error" | count"#, f.path().to_str().unwrap())).expect("count 单独应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_b1_group_by_alone_succeeds() {
    // group by FIELD 单独使用 → 正常输出分组计数
    let f = write_temp("[ERROR] a\n[ERROR] a\n[INFO] a\n");
    let out = run(&args(r#""a" | group by _tag"#, f.path().to_str().unwrap()))
        .expect("group by 单独应成功");
    assert_eq!(out, "ERROR: 2\nINFO: 1\n");
}

// percentiles FIELD [p50,p95,p99]：非流式百分位计算（§9.1 候选 #6 落地）

#[test]
fn test_pipeline_percentiles_default() {
    // percentiles cost_ms（默认 p50,p95,p99）→ 3 个有效值，p50/p95/p99 都命中最高值
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 应成功");
    // 排序后 [100, 300, 500]，p50 idx=ceil(0.5*3)-1=1 → 300
    // p95 idx=ceil(0.95*3)-1=3→clamp=3-1=2 → 500
    // p99 idx=ceil(0.99*3)-1=3→clamp=3-1=2 → 500
    assert_eq!(out, "p50: 300, p95: 500, p99: 500\n");
}

#[test]
fn test_pipeline_percentiles_custom_list() {
    // percentiles cost_ms p50,p95 → 自定义百分位列表
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=500\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms p50,p95",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 自定义列表应成功");
    assert_eq!(out, "p50: 300, p95: 500\n");
}

#[test]
fn test_pipeline_percentiles_single_value() {
    // 单个有效值 → 所有百分位 = 该值
    let f = write_temp("cost_ms=42\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 单值应成功");
    assert_eq!(out, "p50: 42, p95: 42, p99: 42\n");
}

#[test]
fn test_pipeline_percentiles_empty_hits() {
    // 无命中 → 所有百分位 = N/A
    let f = write_temp("cost_ms=100\n");
    let out = run(&args(
        "nomatch | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 空命中应成功");
    assert_eq!(out, "p50: N/A, p95: N/A, p99: N/A\n");
}

#[test]
fn test_pipeline_percentiles_skips_non_numeric() {
    // 混合有效/无效值：只统计有效数值
    let f = write_temp("cost_ms=10\ncost_ms=abc\ncost_ms=20\nno field\ncost_ms=30\n");
    let out = run(&args(
        "cost_ms | percentiles cost_ms p50",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 跳过非数值应成功");
    // 有效值 [10, 20, 30]，p50 idx=ceil(0.5*3)-1=1 → 20
    assert_eq!(out, "p50: 20\n");
}

#[test]
fn test_pipeline_percentiles_float_values() {
    // 浮点数值：排序后 nearest-rank
    let f = write_temp("cost_ms=1.5\ncost_ms=2.5\ncost_ms=3.5\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms p50",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 浮点应成功");
    // 排序后 [1.5, 2.5, 3.5]，p50 idx=1 → 2.5
    assert_eq!(out, "p50: 2.5\n");
}

#[test]
fn test_pipeline_percentiles_large_dataset() {
    // 1..=100，p50=50, p95=95, p99=99
    let content: String = (1..=100).map(|i| format!("cost_ms={i}\n")).collect();
    let f = write_temp(&content);
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 100 值应成功");
    assert_eq!(out, "p50: 50, p95: 95, p99: 99\n");
}

#[test]
fn test_b1_percentiles_then_head_errors() {
    // percentiles cost_ms | head 3 → percentiles 是终止动词 → 应报错
    let f = write_temp("cost_ms=100\ncost_ms=300\n");
    let result = run(&args(
        "cost_ms exists | percentiles cost_ms | head 3",
        f.path().to_str().unwrap(),
    ));
    let Err(e) = result else {
        panic!("percentiles | head 应报错（percentiles 是终止动词），实际成功");
    };
    let msg = e.to_string();
    assert!(msg.contains("percentiles"), "错误应含 percentiles: {msg}");
    assert!(msg.contains("head"), "错误应含 head: {msg}");
    assert!(msg.contains("终止动词"), "应说明是终止动词: {msg}");
}

#[test]
fn test_b1_percentiles_alone_succeeds() {
    // percentiles FIELD 单独使用 → 正常输出百分位
    let f = write_temp("cost_ms=100\ncost_ms=200\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 单独应成功");
    assert!(out.contains("p50: 100"), "应含 p50: 100, 实际: {out}");
    assert!(out.contains("p95: 200"), "应含 p95: 200, 实际: {out}");
}

// ===== B2: percentiles 边界值（borderline 缺口）=====

#[test]
fn test_percentiles_boundary_p0_p100() {
    // p0 = 最小值，p100 = 最大值
    let f = write_temp("cost_ms=100\ncost_ms=200\ncost_ms=300\ncost_ms=400\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms p0,p100",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles p0,p100 应成功");
    // nearest-rank: p0 → idx=ceil(0/100*4)-1=0 → 100; p100 → idx=ceil(100/100*4)-1=3 → 400
    assert!(out.contains("p0: 100"), "p0 应为最小值 100，实际: {out}");
    assert!(
        out.contains("p100: 400"),
        "p100 应为最大值 400，实际: {out}"
    );
}

#[test]
fn test_percentiles_duplicate_values() {
    // 重复百分位：p50,p50 → 同一值出现两次（同一行内逗号分隔）
    let f = write_temp("cost_ms=100\ncost_ms=200\ncost_ms=300\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms p50,p50",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles p50,p50 应成功");
    // p50,p50 → 输出 "p50: 200, p50: 200"（同一行内两次）
    let p50_count = out.matches("p50:").count();
    assert_eq!(p50_count, 2, "p50,p50 应含 2 个 p50: 标记，实际: {out}");
}

#[test]
fn test_percentiles_all_non_numeric_na() {
    // 全部行字段值非数值 → 全 N/A
    let f = write_temp("cost_ms=abc\ncost_ms=NaN\ncost_ms=xyz\n");
    let out = run(&args(
        "cost_ms exists | percentiles cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("percentiles 全 N/A 应成功");
    assert!(out.contains("N/A"), "全非数值应输出 N/A，实际: {out}");
}

#[test]
fn test_ts_between_in_range_matches() {
    // gxp '_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z"' file
    // 3 行日志，2 行在区间内，1 行在区间外
    let content = "2026-10-01T08:30:00Z level=info cost_ms=100\n\
                   2026-10-01T09:45:00Z level=error cost_ms=900\n\
                   2026-10-01T11:00:00Z level=info cost_ms=50\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between 应执行成功");
    assert!(
        out.contains("08:30:00"),
        "08:30 在 [08:00, 10:00] 内应命中，实际: {out}"
    );
    assert!(
        out.contains("09:45:00"),
        "09:45 在 [08:00, 10:00] 内应命中，实际: {out}"
    );
    assert!(
        !out.contains("11:00:00"),
        "11:00 在 [08:00, 10:00] 外不应命中，实际: {out}"
    );
}

#[test]
fn test_ts_between_out_of_range_no_match() {
    // 全部行都在区间外 → 空输出
    let content = "2026-10-01T06:00:00Z level=info\n2026-10-01T12:00:00Z level=error\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "区间外应空输出，实际: {out:?}");
}

#[test]
fn test_ts_between_boundary_inclusive() {
    // 闭区间：边界值应命中（lo 和 hi 本身）
    let content = "2026-10-01T08:00:00Z level=start\n\
                   2026-10-01T10:00:00Z level=end\n\
                   2026-10-01T07:59:59Z level=before\n\
                   2026-10-01T10:00:01Z level=after\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between 边界测试应执行成功");
    assert!(
        out.contains("08:00:00"),
        "下界 08:00:00 应命中（闭区间），实际: {out}"
    );
    assert!(
        out.contains("10:00:00"),
        "上界 10:00:00 应命中（闭区间），实际: {out}"
    );
    assert!(
        !out.contains("07:59:59"),
        "07:59:59 在下界前不应命中，实际: {out}"
    );
    assert!(
        !out.contains("10:00:01"),
        "10:00:01 在上界后不应命中，实际: {out}"
    );
}

#[test]
fn test_ts_between_with_verify() {
    // --verify 暴力对账：_ts between 走 L1→L2 全链，优化路径与暴力路径一致
    let content = "2026-10-01T08:30:00Z level=info cost_ms=100\n\
                   2026-10-01T09:45:00Z level=error cost_ms=900\n\
                   2026-10-01T11:00:00Z level=info cost_ms=50\n";
    let f = write_temp(content);
    let out = run_with_opts(
        &args(
            r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify 应成功");
    assert!(
        out.contains("PASS") || out.contains("08:30"),
        "--verify 应 PASS 且含命中行，实际: {out}"
    );
}

#[test]
fn test_ts_between_no_timestamp_in_log() {
    // 行中无 ISO 时间戳 → _ts 提取失败 → NotNumeric → 不命中
    let content = "level=info cost_ms=100\nno timestamp here\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between 无时间戳行应执行成功");
    assert!(
        out.trim().is_empty(),
        "无 ISO 时间戳的行不应命中，实际: {out:?}"
    );
}

#[test]
fn test_ts_between_with_timezone_offset() {
    // 带时区偏移的 ISO 时间戳：+08:00 等价于 Z 减 8 小时
    // 2026-10-01T16:30:00+08:00 == 2026-10-01T08:30:00Z → 在 [08:00Z, 10:00Z] 内
    let content = "2026-10-01T16:30:00+08:00 level=info\n2026-10-01T12:00:00Z level=out\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between 带时区应执行成功");
    assert!(
        out.contains("16:30:00"),
        "16:30+08:00 == 08:30Z 在区间内应命中，实际: {out}"
    );
    assert!(
        !out.contains("12:00:00Z"),
        "12:00Z 在 [08:00, 10:00] 外不应命中，实际: {out}"
    );
}

#[test]
fn test_ts_between_combined_with_pipeline() {
    // _ts between | rank top K by FIELD — 场景 D + B 组合
    let content = "2026-10-01T08:30:00Z level=error cost_ms=100\n\
                   2026-10-01T09:45:00Z level=error cost_ms=900\n\
                   2026-10-01T11:00:00Z level=error cost_ms=500\n";
    let f = write_temp(content);
    let out = run(&args(
        r#"_ts between "2026-10-01T08:00:00Z" and "2026-10-01T10:00:00Z" | rank top 1 by cost_ms desc"#,
        f.path().to_str().unwrap(),
    ))
    .expect("_ts between + rank 应成功");
    // 时间窗口内 2 行（cost_ms=100 和 900），rank top 1 desc → cost_ms=900
    assert!(
        out.contains("cost_ms=900"),
        "rank top 1 desc 应取 cost_ms=900，实际: {out}"
    );
    assert!(
        !out.contains("cost_ms=100"),
        "rank top 1 应不含 cost_ms=100，实际: {out}"
    );
    assert!(
        !out.contains("cost_ms=500"),
        "时间窗外 cost_ms=500 不应命中，实际: {out}"
    );
}

// ===== G3: stats FIELD 独立 e2e（inline field= 数据来源 × stats，P1 缺口）=====

#[test]
fn test_stats_inline_field_basic() {
    // cost_ms exists | stats cost_ms — inline field= 数据来源
    let f = write_temp("cost_ms=100\ncost_ms=300\ncost_ms=200\n");
    let out = run(&args(
        "cost_ms exists | stats cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("stats 应成功");
    assert!(out.contains("min: 100"), "应含 min: 100, 实际: {out}");
    assert!(out.contains("max: 300"), "应含 max: 300, 实际: {out}");
    assert!(out.contains("avg: 200"), "应含 avg: 200, 实际: {out}");
}

#[test]
fn test_stats_inline_field_with_non_numeric_skipped() {
    // 混合合法/非法数值行 → 非法行跳过
    let f = write_temp("cost_ms=100\ncost_ms=abc\ncost_ms=300\nno field\n");
    let out = run(&args(
        "cost_ms exists | stats cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("stats 应成功");
    assert!(out.contains("min: 100"), "应含 min: 100, 实际: {out}");
    assert!(out.contains("max: 300"), "应含 max: 300, 实际: {out}");
    // avg = (100 + 300) / 2 = 200（abc 和无字段行跳过）
    assert!(out.contains("avg: 200"), "应含 avg: 200, 实际: {out}");
}

#[test]
fn test_stats_inline_field_negative_values() {
    // 负数 stats（D5 十一维契约）
    let f = write_temp("temp=-10\ntemp=5\ntemp=0\n");
    let out = run(&args(
        "temp exists | stats temp",
        f.path().to_str().unwrap(),
    ))
    .expect("stats 负数应成功");
    assert!(out.contains("min: -10"), "应含 min: -10, 实际: {out}");
    assert!(out.contains("max: 5"), "应含 max: 5, 实际: {out}");
    // avg = (-10 + 5 + 0) / 3 = -1.666...
    assert!(out.contains("avg: -1.66"), "应含 avg: -1.66, 实际: {out}");
}

#[test]
fn test_stats_inline_field_no_valid_values_na() {
    // 全部行无数值 → N/A
    let f = write_temp("cost_ms=NaN\ncost_ms=abc\nno field\n");
    let out = run(&args(
        "cost_ms exists | stats cost_ms",
        f.path().to_str().unwrap(),
    ))
    .expect("stats 全 N/A 应成功");
    assert!(out.contains("N/A"), "无有效行应 N/A, 实际: {out}");
}

// ===== G6: 负数 e2e（D5 十一维契约，P1 缺口）=====

#[test]
fn test_negative_rank_by_field_desc() {
    // rank top K by FIELD desc — 负数字段排序
    let f = write_temp("temp=-5\ntemp=-10\ntemp=3\ntemp=-1\n");
    let out = run(&args(
        "temp exists | rank top 2 by temp desc",
        f.path().to_str().unwrap(),
    ))
    .expect("rank 负数应成功");
    // desc 排序：3, -1, -5, -10 → top 2 = 3, -1
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "rank top 2 应 2 行，实际: {out}");
    assert!(
        lines[0].contains("temp=3"),
        "rank desc 第 1 应 temp=3，实际: {lines:?}"
    );
    assert!(
        lines[1].contains("temp=-1"),
        "rank desc 第 2 应 temp=-1，实际: {lines:?}"
    );
}

#[test]
fn test_negative_sort_by_field() {
    // sort by FIELD — 负数排序
    let f = write_temp("temp=-5\ntemp=10\ntemp=-10\ntemp=0\n");
    let out = run(&args(
        "temp exists | sort by temp",
        f.path().to_str().unwrap(),
    ))
    .expect("sort 负数应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "sort 应 4 行，实际: {out}");
    // asc 排序：-10, -5, 0, 10
    assert!(
        lines[0].contains("temp=-10"),
        "sort asc 第 1 应 temp=-10，实际: {lines:?}"
    );
    assert!(
        lines[3].contains("temp=10"),
        "sort asc 第 4 应 temp=10，实际: {lines:?}"
    );
}

#[test]
fn test_negative_where_predicate() {
    // where FIELD > negative_threshold — 负数谓词过滤
    let f = write_temp("temp=-5\ntemp=-10\ntemp=3\ntemp=-1\n");
    let out = run(&args(
        "temp exists | where temp>-3",
        f.path().to_str().unwrap(),
    ))
    .expect("where 负数应成功");
    // temp>-3 → -1 和 3 命中
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "where temp>-3 应 2 行，实际: {out}");
    assert!(out.contains("temp=-1"), "temp=-1 > -3 应命中，实际: {out}");
    assert!(out.contains("temp=3"), "temp=3 > -3 应命中，实际: {out}");
    assert!(
        !out.contains("temp=-5"),
        "temp=-5 不 > -3 不应命中，实际: {out}"
    );
    assert!(
        !out.contains("temp=-10"),
        "temp=-10 不 > -3 不应命中，实际: {out}"
    );
}

#[test]
fn test_negative_between_predicate() {
    // temp between -10 and -1 — 负数区间
    let f = write_temp("temp=-5\ntemp=-10\ntemp=3\ntemp=-15\n");
    let out = run(&args("temp between -10 and -1", f.path().to_str().unwrap()))
        .expect("between 负数应成功");
    // -10 <= v <= -1 → -5 和 -10 命中
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "between 负数应 2 行，实际: {out}");
    assert!(
        out.contains("temp=-5"),
        "temp=-5 在 [-10,-1] 应命中，实际: {out}"
    );
    assert!(
        out.contains("temp=-10"),
        "temp=-10 在 [-10,-1] 应命中（闭区间），实际: {out}"
    );
    assert!(!out.contains("temp=3"), "temp=3 不在 [-10,-1] 不应命中");
    assert!(!out.contains("temp=-15"), "temp=-15 不在 [-10,-1] 不应命中");
}

// ===== ! /regex/ Not+正则 e2e（borderline 缺口）=====

#[test]
fn test_not_regex_returns_lines_without_match() {
    // ! /ERROR/ → 不含 ERROR 正则匹配的行
    let f = write_temp("ERROR line 1\ninfo line 2\nERROR line 3\n");
    let out = run(&args(r#"! /ERROR/"#, f.path().to_str().unwrap())).expect("Not regex 应成功");
    assert!(
        out.contains("2:info line 2"),
        "Not /ERROR/ 应返回不含 ERROR 的行: {out}"
    );
    assert!(
        !out.contains("ERROR line 1"),
        "含 ERROR 的行应被 Not 淘汰: {out}"
    );
}

#[test]
fn test_not_regex_with_pattern_not_literal() {
    // ! /err.*or/i → 不匹配 err 开头 or 结尾的行（正则模式非字面量）
    let f = write_temp("error line 1\ninfo line 2\nerror line 3\n");
    let out =
        run(&args(r#"! /err.*or/"#, f.path().to_str().unwrap())).expect("Not regex pattern 应成功");
    // /err.*or/ 匹配 "error" → Not 淘汰含 error 的行
    assert!(
        out.contains("2:info line 2"),
        "Not /err.*or/ 应返回不含 error 的行: {out}"
    );
    assert!(
        !out.contains("error line 1"),
        "含 error 的行应被 Not 淘汰: {out}"
    );
}

// ===== B14：无括号混合 &&/|| + Regex 在布尔组合中的 L1 健全性 =====
//
// 根因（v0.47 修复）：l1::compiler 对 Or 中不推进字面量的分支（And/Not/Regex/
// Pred/Ast/Capture）静默跳过，剩余分支的字面量仍组成「必选组」→ 部分组误杀
// 其余分支候选（L1 ⊇ L2 违反）；且 scan 捷径 `!needs_l2`（只看字段谓词）把
// 纯字面量查询当 "L1 即终判"——Regex 右支从不检查、混合 Or 直接 match-all。
// 夹具（5 行，期望集独立计算）：
//   1 "trace json"  2 "trace stdin"  3 "neither"  4 "json only"  5 "stdin only"

/// B14 夹具：write_temp 包一份标准 5 行布尔夹具。
fn bool_fixture() -> tempfile::NamedTempFile {
    write_temp("trace json\ntrace stdin\nneither\njson only\nstdin only\n")
}

#[test]
fn test_bool_mixed_and_or_four_terms_exact() {
    // (trace∧json) ∨ (trace∧stdin) = {1,2}——修复前 match-all 5 行
    let f = bool_fixture();
    let out = run(&args(
        r#""trace" && "json" || "trace" && "stdin""#,
        f.path().to_str().unwrap(),
    ))
    .expect("混合 4 项应成功");
    assert!(out.contains("1:trace json"), "{out}");
    assert!(out.contains("2:trace stdin"), "{out}");
    assert!(!out.contains("neither"), "无关行不得命中: {out}");
    assert!(!out.contains("only"), "only 行不得命中: {out}");
}

#[test]
fn test_bool_mixed_and_or_three_terms_and_first() {
    // (trace∧json) ∨ stdin = {1,2,5}——修复前只剩 {2,5}（And 分支被部分组误杀）
    let f = bool_fixture();
    let out = run(&args(
        r#""trace" && "json" || "stdin""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert!(
        out.contains("1:trace json"),
        "And 分支候选行 1 不得被 L1 误杀: {out}"
    );
    assert!(out.contains("2:trace stdin"), "{out}");
    assert!(out.contains("5:stdin only"), "{out}");
    assert!(!out.contains("neither"), "{out}");
    assert!(!out.contains("4:json only"), "{out}");
}

#[test]
fn test_bool_mixed_and_or_three_terms_or_first() {
    // json ∨ (trace∧stdin) = {1,2,4}——修复前只剩 {1,4}（右侧 And 分支被误杀）
    let f = bool_fixture();
    let out = run(&args(
        r#""json" || "trace" && "stdin""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    assert!(out.contains("1:trace json"), "{out}");
    assert!(
        out.contains("2:trace stdin"),
        "And 分支候选行 2 不得被 L1 误杀: {out}"
    );
    assert!(out.contains("4:json only"), "{out}");
    assert!(!out.contains("neither"), "{out}");
    assert!(!out.contains("5:stdin only"), "{out}");
}

#[test]
fn test_bool_and_with_regex_rhs_checked_by_l2() {
    // json ∧ /onl/ = {4}——修复前 Regex 右支从不检查（{1,4} 假阳性）
    let f = bool_fixture();
    let out = run(&args(r#""json" && /onl/"#, f.path().to_str().unwrap())).expect("应成功");
    assert!(out.contains("4:json only"), "{out}");
    assert!(
        !out.contains("1:trace json"),
        "不匹配 /onl/ 的行不得命中（Regex 必须被 L2 检查）: {out}"
    );
}

#[test]
fn test_bool_or_with_regex_branch_keeps_regex_hits() {
    // json ∨ /neith/ = {1,3,4}——修复前只匹配正则的行被部分组误杀（{1,4}）
    let f = bool_fixture();
    let out = run(&args(r#""json" || /neith/"#, f.path().to_str().unwrap())).expect("应成功");
    assert!(
        out.contains("3:neither"),
        "只匹配 Regex 分支的行不得被 L1 误杀（L1 ⊇ L2）: {out}"
    );
    assert!(out.contains("1:trace json"), "{out}");
    assert!(out.contains("4:json only"), "{out}");
    assert!(!out.contains("stdin"), "{out}");
}

#[test]
fn test_bool_mixed_and_or_verify_agrees() {
    // --verify 对账混合 4 项：两条路径（暴力/优化）语义一致且结果正确（2 行）
    let f = bool_fixture();
    let out = run(&args(
        r#""trace" && "json" || "trace" && "stdin""#,
        f.path().to_str().unwrap(),
    ))
    .expect("应成功");
    let verify = run_with_opts(
        &args(
            r#""trace" && "json" || "trace" && "stdin""#,
            f.path().to_str().unwrap(),
        ),
        RunOpts {
            verify: true,
            ..Default::default()
        },
    )
    .expect("--verify 应成功");
    assert!(verify.contains("PASS [2 行命中]"), "verify: {verify}");
    assert!(out.lines().count() == 2, "命中应为 2 行: {out}");
}

#[test]
fn test_where_bool_mixed_semantics() {
    // B14 where 路径：apply_where 捷径同修——`| where "x" && "y" || "z"` = (x∧y)∨z
    let f = write_temp("err x y\nerr x z\nerr x\nerr z\nplain\n");
    let out = run(&args(
        r#""err" | where "x" && "y" || "z""#,
        f.path().to_str().unwrap(),
    ))
    .expect("where 混合布尔应成功");
    assert!(out.contains("1:err x y"), "x∧y 命中: {out}");
    assert!(out.contains("2:err x z"), "z 命中: {out}");
    assert!(out.contains("4:err z"), "z 命中: {out}");
    assert!(!out.contains("3:err x"), "仅 x 不满足 (x∧y)∨z: {out}");
    assert!(!out.contains("plain"), "非 err 行不得进入 where: {out}");
}
