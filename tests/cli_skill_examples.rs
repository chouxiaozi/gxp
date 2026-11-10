//! skill 分发示例可执行守护（e2e）。
//!
//! 与 tests/cli_readme_examples.rs 同构：README 之后的第二远端交付物是
//! skills/gxp/（plugin / skills.sh / release zip 三出口同源），skill 里的
//! 查询模板错了最晚被发现。本文件把 SKILL.md 场景表 + 谓词先行教学 +
//! references/gxp-examples.md D 节逐条锚定到 e2e：
//! **改 skills/gxp/ 示例必须同步本文件，反之亦然。**

mod common;

use common::{args, run, write_temp};

/// JSONL fixture：字段 _ts/level/trace_id/cost_ms（SKILL.md D 行同日期）。
/// 行号（1-based）：1=INFO(100)、2=ERROR(900)、3=ERROR(2000)、4=ERROR(3000)、5=INFO(50)。
/// 刻意不含 "count" 子串（供谓词先行负向测试用：裸 `count` 当 ERE 搜 0 命中）。
const JSONL: &str = r#"{"_ts":"2026-10-27T09:59:00Z","level":"INFO","trace_id":"req-001","cost_ms":100}
{"_ts":"2026-10-27T10:05:00Z","level":"ERROR","trace_id":"req-123","cost_ms":900}
{"_ts":"2026-10-27T10:40:00Z","level":"ERROR","trace_id":"req-456","cost_ms":2000}
{"_ts":"2026-10-27T11:05:00Z","level":"ERROR","trace_id":"req-789","cost_ms":3000}
{"_ts":"2026-10-27T11:06:00Z","level":"INFO","trace_id":"req-999","cost_ms":50}
"#;

#[test]
fn test_skill_scene_a_error_context() {
    // SKILL.md 场景表 A 行：`ERROR | context 3`。ERROR 行应全命中，且 context
    // 带回邻近非 ERROR 行（首条 ERROR 的前文 INFO、行间 INFO）。
    let f = write_temp(
        "2026-10-27T10:00:00Z INFO [auth] ok\n\
         2026-10-27T10:05:00Z ERROR [billing] late cost_ms=2000\n\
         2026-10-27T10:06:00Z INFO [auth] recovered\n\
         2026-10-27T10:40:00Z ERROR [checkout] boom cost_ms=900\n",
    );
    let out = run(&args("ERROR | context 3", f.path().to_str().unwrap())).expect("context 应成功");
    assert!(
        out.contains("late") && out.contains("boom"),
        "ERROR 行应全命中: {out}"
    );
    assert!(
        out.contains("recovered"),
        "context 3 应带回 ERROR 行之间的 INFO 行: {out}"
    );
    assert!(
        out.contains("[auth] ok"),
        "context 3 应带回首条 ERROR 的前文行: {out}"
    );
}

#[test]
fn test_skill_scene_b_rank_top20() {
    // SKILL.md 场景表 B 行：`cost_ms>800 | rank top 20 by cost_ms desc`。
    // 手算：>800 → 第 4 行(3000) / 第 3 行(2000) / 第 2 行(900)，降序 4 → 3 → 2。
    let f = write_temp(JSONL);
    let out = run(&args(
        "cost_ms>800 | rank top 20 by cost_ms desc",
        f.path().to_str().unwrap(),
    ))
    .expect("rank 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "应恰好 3 行命中: {out}");
    assert!(
        lines[0].starts_with("4:") && lines[0].contains("3000"),
        "第一名第 4 行: {out}"
    );
    assert!(
        lines[1].starts_with("3:") && lines[1].contains("2000"),
        "第二名第 3 行: {out}"
    );
    assert!(
        lines[2].starts_with("2:") && lines[2].contains("900"),
        "第三名第 2 行: {out}"
    );
}

#[test]
fn test_skill_scene_d_time_window_and_capture() {
    // SKILL.md 场景表 D 行（between，含无 Z 与带 Z 两种写法均须可执行）
    // + references/gxp-examples.md D 节 capture 模板。手算：[10:00,11:00] 闭区间
    // 命中第 2/3 行（10:05、10:40）；capture trace_id 全 5 行命中。
    let f = write_temp(JSONL);
    let path = f.path().to_str().unwrap();

    let count_noz = run(&args(
        r#"_ts between "2026-10-27T10:00:00" and "2026-10-27T11:00:00" | count"#,
        path,
    ))
    .expect("between（无 Z）应成功");
    assert_eq!(count_noz, "2\n");

    let count_z = run(&args(
        r#"_ts between "2026-10-27T10:00:00Z" and "2026-10-27T11:00:00Z" | count"#,
        path,
    ))
    .expect("between（带 Z）应成功");
    assert_eq!(count_z, "2\n");

    let cap = run(&args(
        r#"capture(trace_id, /req-\d+/) | fields trace_id"#,
        path,
    ))
    .expect("capture + fields 应成功");
    let cap_lines: Vec<&str> = cap.lines().collect();
    assert_eq!(cap_lines.len(), 5, "5 行全命中: {cap}");
    assert!(
        cap.contains("2:req-123") && cap.contains("5:req-999"),
        "字段值应为 req-XXX: {cap}"
    );
}

#[test]
fn test_skill_predicate_first_teaching() {
    // SKILL.md「谓词先行」教学：纯 stage 查询被 BARE 吞。正例两条 +
    // 负例一条（裸 `count` 在无 "count" 字面量的 fixture 上 0 命中退出码 1）。
    let f = write_temp(JSONL);
    let path = f.path().to_str().unwrap();

    let count = run(&args(r#""ERROR" | count"#, path)).expect("谓词先行 count 应成功");
    assert_eq!(count, "3\n");

    let distinct = run(&args("trace_id exists | distinct by trace_id", path))
        .expect("谓词先行 distinct 应成功");
    assert_eq!(distinct.lines().count(), 5, "5 个唯一 trace_id: {distinct}");

    // 吞没实证：含字面 "count" 的行会被裸 `count` 当 ERE 命中（输出该行本身，
    // 而非行数统计）。CLI 层面 0 命中另表现为退出码 1，属 main.rs 行为，
    // 不在 run_with_writer 库入口范围内，故用"命中错位"作为库层证明。
    let f2 = write_temp("INFO counter overflow\nERROR boom\n");
    let swallowed = run(&args("count", f2.path().to_str().unwrap()))
        .expect("BARE 执行本身应 Ok（0 命中也是成功执行）");
    assert_eq!(
        swallowed, "1:INFO counter overflow\n",
        "裸 count 被当 ERE 字面量搜文件（吞没实证）：应命中含 count 的行而非统计行数"
    );
}

#[test]
fn test_skill_scene_f_sample_fixed_size() {
    // SKILL.md 场景表 F 行：`ERROR | sample 2`。ERROR 共 3 行，sample 固定数量
    // 恰取 2（种子 0 可复现，但只断言数量与成员归属，不断言具体选中哪两条）。
    let f = write_temp(JSONL);
    let out = run(&args("ERROR | sample 2", f.path().to_str().unwrap())).expect("sample 应成功");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "sample 2 应恰取 2 行: {out}");
    for l in &lines {
        assert!(
            l.contains("\"level\":\"ERROR\""),
            "样本成员必须是 ERROR 行: {out}"
        );
    }
}
