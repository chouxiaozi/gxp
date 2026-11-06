//! gxp L1 编译 + 扫描集成测试。
//!
//! 端到端：DSL 字符串 → `parse_query` → `compile` → `scan_lines` → 命中行号。
//!
//! 不变式校验（设计文档 §3.2）：L1(粗筛) ⊇ L2(精判)。
//! 每个数值谓词用例都构造暴力 L2 逻辑验证 L1 命中集是超集。
use gxp::dsl::ast::*;
use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines;

/// 构造日志字节串：用 `\n` 连接多行，末尾不加 `\n`（测试末尾无换行也工作）。
fn make_log(lines: &[&str]) -> Vec<u8> {
    lines.join("\n").into_bytes()
}

/// 暴力 L2 数值比较：在行里找 `field=` 或 `field:` 后的数字，转 f64 与阈值比较。
fn l2_numeric_match(line: &[u8], field: &str, op: CmpOp, threshold: f64) -> bool {
    let key_eq = format!("{field}=");
    let key_colon = format!("{field}:");
    let key_eq_b = key_eq.as_bytes();
    let key_colon_b = key_colon.as_bytes();
    let val_start = line
        .windows(key_eq_b.len())
        .position(|w| w == key_eq_b)
        .map(|p| p + key_eq_b.len())
        .or_else(|| {
            line.windows(key_colon_b.len())
                .position(|w| w == key_colon_b)
                .map(|p| p + key_colon_b.len())
        });
    let Some(start) = val_start else {
        return false;
    };
    let end = line[start..]
        .iter()
        .position(|&b| !b.is_ascii_digit() && b != b'.' && b != b'-')
        .map(|p| start + p)
        .unwrap_or(line.len());
    let val_str = match std::str::from_utf8(&line[start..end]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let val: f64 = match val_str.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    match op {
        CmpOp::Gt => val > threshold,
        CmpOp::Ge => val >= threshold,
        CmpOp::Lt => val < threshold,
        CmpOp::Le => val <= threshold,
        CmpOp::Eq => (val - threshold).abs() < f64::EPSILON,
        CmpOp::Ne => (val - threshold).abs() >= f64::EPSILON,
        CmpOp::Assign => (val - threshold).abs() < f64::EPSILON,
    }
}

// ===== 单字面量查询 =====

#[test]
fn test_l1_single_string_via_dsl() {
    // "error" → 匹配含 error 子串的行
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&["info line", "error line", "warn line", "another error"]);
    let hits = scan_lines(&compiled, &log);
    assert_eq!(hits, vec![2, 4]);
}

#[test]
fn test_l1_bare_via_dsl() {
    // BARE: ERROR（ERE 兼容，等价子串匹配）
    let q = parse_query("ERROR").expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&["ERROR line", "info", "error lowercase", "ERROR again"]);
    let hits = scan_lines(&compiled, &log);
    // BARE 是大小写敏感的 ERE：只匹配大写 ERROR
    assert_eq!(hits, vec![1, 4]);
}

// ===== AND 查询 =====

#[test]
fn test_l1_and_via_dsl() {
    // "error" && "timeout"
    let q = parse_query(r#""error" && "timeout""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&[
        "error timeout line",
        "error only",
        "timeout only",
        "both error and timeout here",
    ]);
    let hits = scan_lines(&compiled, &log);
    // 行 1 和行 4 同时含两者
    assert_eq!(hits, vec![1, 4]);
}

// ===== OR 查询 =====

#[test]
fn test_l1_or_via_dsl() {
    // "error" || "timeout"
    let q = parse_query(r#""error" || "timeout""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&["error line", "info", "timeout line", "both error timeout"]);
    let hits = scan_lines(&compiled, &log);
    // 行 1、3、4 命中
    assert_eq!(hits, vec![1, 3, 4]);
}

// ===== 字段谓词：L1 只下推字段名前缀 =====

#[test]
fn test_l1_compare_field_prefix_via_dsl() {
    // cost_ms > 800
    let q = parse_query("cost_ms>800").expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&[
        "cost_ms=900 user=a",
        "latency=100 user=b",
        "cost_ms=50 user=c",
        "cost_ms:1000 user=d",
        "no fields here",
    ]);
    let hits = scan_lines(&compiled, &log);
    // L1 命中：含 cost_ms= 或 cost_ms: 前缀的行（1, 3, 4）
    assert_eq!(hits, vec![1, 3, 4]);
}

#[test]
fn test_l1_exists_field_prefix_via_dsl() {
    // user_id exists
    let q = parse_query("user_id exists").expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&[
        "user_id=abc cost=1",
        "cost=2 no user",
        "user_id:def cost=3",
        "empty",
    ]);
    let hits = scan_lines(&compiled, &log);
    assert_eq!(hits, vec![1, 3]);
}

// ===== 字面量 + 字段谓词组合 =====

#[test]
fn test_l1_combined_literal_and_field_via_dsl() {
    // "timeout" && cost_ms > 800
    let q = parse_query(r#""timeout" && cost_ms>800"#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = make_log(&[
        "timeout cost_ms=900",
        "timeout cost_ms=100", // 字面量命中但数值不满足（L1 仍命中，交给 L2 淘汰）
        "cost_ms=900 no match", // 字段命中但缺字面量（"no match" 不含 timeout）
        "timeout cost_ms:1500",
    ]);
    let hits = scan_lines(&compiled, &log);
    // L1 命中：行 1、2（timeout + cost_ms 前缀都在）、行 4
    assert_eq!(hits, vec![1, 2, 4]);
}

// ===== 不变式 L1 ⊇ L2（核心校验）=====

#[test]
fn test_l1_superset_invariant_with_numeric_predicate() {
    // cost_ms > 800：L1 命中集 ⊇ L2 精判命中集
    let q = parse_query("cost_ms>800").expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log_lines = [
        "cost_ms=900 user=a",
        "latency=100 user=b",
        "cost_ms=50 user=c",
        "cost_ms:1000 user=d",
        "cost_ms=800 user=e", // 边界：= 800，L2 不命中（> 严格大于）
        "no fields",
    ];
    let log = make_log(&log_lines);
    let l1_hits = scan_lines(&compiled, &log);

    // L1 命中：所有含 cost_ms= 或 cost_ms: 的行（1, 3, 4, 5）
    assert_eq!(l1_hits, vec![1, 3, 4, 5]);

    // 暴力 L2 精判：只保留 cost_ms 值 > 800 的行
    let l2_hits: Vec<usize> = l1_hits
        .iter()
        .copied()
        .filter(|&i| {
            let line = log_lines[i - 1].as_bytes();
            l2_numeric_match(line, "cost_ms", CmpOp::Gt, 800.0)
        })
        .collect();

    // L2 命中：行 1（900）、行 4（1000）
    assert_eq!(l2_hits, vec![1, 4]);
    // 不变式：L2 ⊆ L1
    for l2 in &l2_hits {
        assert!(l1_hits.contains(l2), "L2 命中 {} 不在 L1 命中集中", l2);
    }
}

#[test]
fn test_l1_superset_invariant_with_combined_query() {
    // "timeout" && cost_ms > 800：L1 命中 ⊇ L2 精判
    let q = parse_query(r#""timeout" && cost_ms>800"#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log_lines = [
        "timeout cost_ms=900",  // L1 ✓ L2 ✓
        "timeout cost_ms=100",  // L1 ✓（字面量+前缀都在）L2 ✗（100 <= 800）
        "timeout no field",     // L1 ✗（无 cost_ms 前缀）
        "cost_ms=900 no match", // L1 ✗（无 timeout 字面量）
        "timeout cost_ms:1500", // L1 ✓ L2 ✓
    ];
    let log = make_log(&log_lines);
    let l1_hits = scan_lines(&compiled, &log);

    // L1 命中：行 1、2、5
    assert_eq!(l1_hits, vec![1, 2, 5]);

    // L2 精判：字面量 timeout 子串 + cost_ms 值 > 800
    let l2_hits: Vec<usize> = l1_hits
        .iter()
        .copied()
        .filter(|&i| {
            let line = log_lines[i - 1].as_bytes();
            line.windows(b"timeout".len()).any(|w| w == b"timeout")
                && l2_numeric_match(line, "cost_ms", CmpOp::Gt, 800.0)
        })
        .collect();

    // L2 命中：行 1、5
    assert_eq!(l2_hits, vec![1, 5]);
    for l2 in &l2_hits {
        assert!(l1_hits.contains(l2), "L2 命中 {} 不在 L1 命中集中", l2);
    }
}

// ===== 边界场景 =====

#[test]
fn test_l1_empty_log() {
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let hits = scan_lines(&compiled, b"");
    assert!(hits.is_empty());
}

#[test]
fn test_l1_single_line_no_match() {
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = b"just a warning";
    let hits = scan_lines(&compiled, log);
    assert!(hits.is_empty());
}

#[test]
fn test_l1_trailing_newline() {
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");
    let log = b"error\ninfo\n";
    let hits = scan_lines(&compiled, log);
    assert_eq!(hits, vec![1]);
}
