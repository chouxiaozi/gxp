//! 端到端测试：从 detect.rs 内联 `#[cfg(test)]` 块外迁（AGENTS.md §6.6 门禁）。
//!
//! 这 14 项测试调用 `run_with_writer`（公开入口）+ tempfile，属于集成测试，
//! 不应在 `src/` 内联。CI 门禁 `scripts/check_no_e2e_inline.sh` 拒绝
//! `src/` `#[cfg(test)]` 块中出现 `run_with_writer` 调用。
//!
//! 纯函数单元测试（`is_gxp_mode`/`has_any_predicate` 等）仍保留在 detect.rs 内联。

use gxp::cli::RunOpts;
use gxp::mode::detect::run_with_writer;

#[test]
fn test_run_gxp_mode_compare_predicate_executes() {
    // cost_ms>800：L1 粗筛 + L2 精判
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "cost_ms=900 user=alice\ncost_ms=100 user=bob\nlatency=50 user=carol\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["cost_ms>800".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Compare 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    // 只命中第 1 行（cost_ms=900 > 800）
    assert!(out_str.contains("1:cost_ms=900 user=alice"));
    assert!(!out_str.contains("cost_ms=100"));
    assert!(!out_str.contains("latency=50"));
}

#[test]
fn test_run_gxp_mode_compare_with_logfmt_and_literal() {
    // "timeout" && cost_ms>800：L1 必选组 + L2 精判
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "timeout cost_ms=900 user=alice\ntimeout cost_ms=100 user=bob\nerror cost_ms=900 user=carol\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    // gxp '"timeout" && cost_ms>800'
    run_with_writer(
        &["\"timeout\" && cost_ms>800".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("组合查询应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    // 只命中第 1 行
    assert!(out_str.contains("1:timeout cost_ms=900 user=alice"));
    assert!(!out_str.contains("cost_ms=100"));
    assert!(!out_str.contains("error cost_ms"));
}

// --- 端到端：L2 接通 Not/In/Between/Exists 谓词 ---

#[test]
fn test_run_gxp_mode_not_predicate_hits_lines_without_field() {
    // !(cost_ms>800)：字段不存在 → 谓词为假 → 取反为真 → 命中
    // 设计文档 §3.3 "字段不存在语义"
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "cost_ms=900 user=alice\nno cost field here\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["!(cost_ms>800)".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Not 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    // 第 2 行无 cost_ms 字段 → NotNumeric → Not → True → 命中
    assert!(out_str.contains("2:no cost field here"));
    // 第 1 行 cost_ms=900 > 800 → True → Not → False → 不命中
    assert!(!out_str.contains("cost_ms=900"));
}

#[test]
fn test_run_gxp_mode_not_predicate_filters_matching_lines() {
    // !(cost_ms>800)：cost_ms 存在但 <= 800 → False → Not → True → 命中
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "cost_ms=900 user=alice\ncost_ms=100 user=bob\nno_field\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["!(cost_ms>800)".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Not 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    // 第 2 行 cost_ms=100 <= 800 → False → Not → True → 命中
    assert!(out_str.contains("2:cost_ms=100"));
    // 第 3 行无字段 → NotNumeric → Not → True → 命中
    assert!(out_str.contains("3:no_field"));
    // 第 1 行 cost_ms=900 > 800 → True → Not → False → 不命中
    assert!(!out_str.contains("900 user=alice"));
}

#[test]
fn test_run_gxp_mode_in_predicate_matches_string_values() {
    // tag in [info, error]：字节字面量集合判定
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "tag=info msg=a\ntag=error msg=b\ntag=warn msg=c\nno_tag msg=d\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["tag in [info, error]".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("In 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("1:tag=info"));
    assert!(out_str.contains("2:tag=error"));
    assert!(!out_str.contains("warn"));
    assert!(!out_str.contains("no_tag"));
}

#[test]
fn test_run_gxp_mode_in_predicate_field_absent() {
    // tag in [info]：字段不存在 → False
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "tag=info\nother=val\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["tag in [info]".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("In 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("1:tag=info"));
    assert!(!out_str.contains("other=val"));
}

#[test]
fn test_run_gxp_mode_between_predicate_inclusive_bounds() {
    // cost between 100 and 200：闭区间
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "cost=150\ncost=100\ncost=200\ncost=99\ncost=201\nno_cost\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["cost between 100 and 200".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Between 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    // 闭区间：100 和 200 都命中
    assert!(out_str.contains("1:cost=150"));
    assert!(out_str.contains("2:cost=100"));
    assert!(out_str.contains("3:cost=200"));
    assert!(!out_str.contains("cost=99"));
    assert!(!out_str.contains("cost=201"));
    assert!(!out_str.contains("no_cost"));
}

#[test]
fn test_run_gxp_mode_between_predicate_not_numeric_field() {
    // cost 字段不是数值（NotNumeric → False，不命中）
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "cost=abc\ncost=150\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["cost between 100 and 200".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Between 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(!out_str.contains("abc"));
    assert!(out_str.contains("2:cost=150"));
}

#[test]
fn test_run_gxp_mode_between_rejects_invalid_bound() {
    // between 端点不是合法数值 → 编译期 fail-loud
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "cost=150\n").unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    // 下界 "abc" 不是数值 → 11 维契约拒绝
    let result = run_with_writer(
        &["cost between abc and 200".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    );
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("下界"));
    assert!(err.contains("abc"));
}

#[test]
fn test_run_gxp_mode_exists_predicate_hits_when_field_present() {
    // user_id exists：字段存在 → True
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "user_id=alice msg=ok\nno_user msg=fail\nuser_id=bob msg=ok\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["user_id exists".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Exists 谓词应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("1:user_id=alice"));
    assert!(!out_str.contains("no_user msg=fail"));
    assert!(out_str.contains("3:user_id=bob"));
}

#[test]
fn test_run_gxp_mode_exists_pseudo_fields_ts() {
    // _ts exists：伪字段 _ts（ISO 时间戳）
    // 注：`_ts` 不以字母开头（`_` 开头），但含 ` exists` 子串 → is_gxp_mode 返回 true
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "2026-01-02T03:04:05Z level=info\nno_timestamp here\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["_ts exists".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Exists _ts 应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("1:2026-01-02T03:04:05Z"));
    assert!(!out_str.contains("no_timestamp"));
}

#[test]
fn test_run_gxp_mode_exists_pseudo_fields_tag() {
    // _tag exists：伪字段 _tag（方括号标签）
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "[INFO] msg=a\nno_bracket msg=b\n[ERROR] msg=c\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    run_with_writer(
        &["_tag exists".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("Exists _tag 应执行成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(out_str.contains("1:[INFO]"));
    assert!(!out_str.contains("no_bracket"));
    assert!(out_str.contains("3:[ERROR]"));
}

#[test]
fn test_run_gxp_mode_rejects_invalid_threshold() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "cost_ms=900\n").unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    let mut out = Vec::new();
    // 阈值是 NaN（11 维契约拒绝）
    let result = run_with_writer(
        &["cost_ms>NaN".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    );
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("不是合法数值"));
}

#[test]
fn test_run_gxp_mode_compare_11_dim_format_contract() {
    // 11 维契约：支持整数/负数/正号/前导零/浮点/科学计数法
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let content = "a=950 b=-5 c=+950 d=0950 e=950.5 f=9.5e2 g=1,000 h=900ms\n";
    std::fs::write(tmp.path(), content).unwrap();
    let path = tmp.path().to_string_lossy().to_string();

    // 各格式都 > 900
    let cases = [
        ("a>900", "a=950"),   // 整数
        ("b>-10", "b=-5"),    // 负数
        ("c>900", "c=+950"),  // 正号
        ("d>900", "d=0950"),  // 前导零
        ("e>900", "e=950.5"), // 浮点
        ("f>900", "f=9.5e2"), // 科学计数
    ];
    for (query, expected_line) in cases {
        let mut out = Vec::new();
        run_with_writer(
            &[query.to_string(), path.clone()],
            RunOpts::default(),
            &mut out,
        )
        .expect("查询应成功");
        let out_str = String::from_utf8(out).unwrap();
        assert!(
            out_str.contains(expected_line),
            "查询 `{query}` 应命中 `{expected_line}`，实际输出: {out_str}"
        );
    }

    // 拒绝项：千分位 1,000 和单位 900ms 不应被数值谓词命中
    // cost_ms>800 不会匹配 "1,000"（千分位）或 "900ms"（单位）
    let mut out = Vec::new();
    run_with_writer(
        &["g>0".to_string(), path.clone()],
        RunOpts::default(),
        &mut out,
    )
    .expect("查询应成功");
    let out_str = String::from_utf8(out).unwrap();
    assert!(
        !out_str.contains("1,000"),
        "千分位 1,000 不应被数值谓词命中，实际: {out_str}"
    );
    assert!(
        !out_str.contains("900ms"),
        "单位 900ms 不应被数值谓词命中，实际: {out_str}"
    );
}
