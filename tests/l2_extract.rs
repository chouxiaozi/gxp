//! L2 字段抽取集成测试：4 种格式 × L2 精判联动。
//!
//! 设计文档 §3.3 + Rust 方案 §8：
//! - logfmt（`k=v` / `k:v`）
//! - JSON line（`{"k": v}` / `[{...}]`）
//! - [tag] 方括号标签（伪字段 `_tag`）
//! - RFC 3339 时间戳（伪字段 `_ts`）
//!
//! 验证：
//! 1. `ExtractedFields::extract` 在 4 种格式下都能正确抽取字段
//! 2. `eval_numeric` 与抽取结果联动：logfmt/JSON 的数值字段都走 11 维契约
//! 3. L1→L2 不变式：L1 命中集 ⊇ L2 精判命中集

use gxp::dsl::ast::CmpOp;
use gxp::l2::{eval_numeric, EvalResult, ExtractedFields};

// ===== logfmt =====

#[test]
fn test_logfmt_extract_and_eval() {
    let line = b"level=info cost_ms=900 user=alice latency=50";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"level"), Some(&b"info"[..]));
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    assert_eq!(f.get(b"latency"), Some(&b"50"[..]));

    // L2 精判联动
    assert_eq!(
        eval_numeric(f.get(b"cost_ms").unwrap(), CmpOp::Gt, 800.0),
        EvalResult::True
    );
    assert_eq!(
        eval_numeric(f.get(b"latency").unwrap(), CmpOp::Lt, 100.0),
        EvalResult::True
    );
}

#[test]
fn test_logfmt_colon_separator() {
    let line = b"level:info cost_ms:900";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"level"), Some(&b"info"[..]));
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    assert_eq!(
        eval_numeric(f.get(b"cost_ms").unwrap(), CmpOp::Gt, 800.0),
        EvalResult::True
    );
}

#[test]
fn test_logfmt_11_dim_format_in_field_value() {
    // 字段值是 11 维契约的各维度
    let cases = [
        (b"a=950".as_ref(), b"a", 950.0),   // 整数
        (b"a=-5".as_ref(), b"a", -5.0),     // 负数
        (b"a=+950".as_ref(), b"a", 950.0),  // 正号
        (b"a=0950".as_ref(), b"a", 950.0),  // 前导零
        (b"a=950.5".as_ref(), b"a", 950.5), // 浮点
        (b"a=9.5e2".as_ref(), b"a", 950.0), // 科学计数
    ];
    for (line, field, expected_val) in cases {
        let f = ExtractedFields::extract(line);
        let v = f.get(field).expect("字段应存在");
        let parsed = gxp::l2::parse_numeric_value(v);
        assert_eq!(
            parsed,
            Some(expected_val),
            "line={:?} field={:?} 应解析为 {}",
            std::str::from_utf8(line).unwrap_or("<non-utf8>"),
            std::str::from_utf8(field).unwrap_or("<non-utf8>"),
            expected_val
        );
    }
}

#[test]
fn test_logfmt_rejected_formats_return_none_or_not_numeric() {
    // 千分位/单位/空白/空值/布尔/NaN：parse 返回 None，eval 返回 NotNumeric
    let cases: &[(&[u8], &[u8])] = &[
        (b"a=1,000", b"a"), // 千分位
        (b"a=900ms", b"a"), // 单位
        (b"a= 800", b"a"),  // 空白
        (b"a=", b"a"),      // 空值
        (b"a=null", b"a"),  // null
        (b"a=true", b"a"),  // 布尔
        (b"a=NaN", b"a"),   // NaN
        (b"a=inf", b"a"),   // Inf
    ];
    for (line, field) in cases {
        let f = ExtractedFields::extract(line);
        let v_opt = f.get(field);
        // 空值（a=）和前导空白（a= 800）字段都不存在：
        // - a= ：value 为空，logfmt 状态机跳过
        // - a= 800：value 起点是空白，memchr 立刻命中，value 切为空 → 跳过
        // 这两种场景与 11 维契约的"空白 trim 拒绝"语义一致：
        // 带前导空白的值被认为是切分错误，不视为字段。
        if line == b"a=" || line == b"a= 800" {
            assert_eq!(v_opt, None, "空值/前导空白字段应不存在: {:?}", line);
        } else {
            let v = v_opt.expect("非空值字段应存在");
            assert_eq!(
                gxp::l2::parse_numeric_value(v),
                None,
                "拒绝格式应返回 None: line={:?}",
                std::str::from_utf8(line).unwrap_or("<non-utf8>")
            );
            assert_eq!(
                eval_numeric(v, CmpOp::Gt, 0.0),
                EvalResult::NotNumeric,
                "拒绝格式 eval 应返回 NotNumeric: line={:?}",
                std::str::from_utf8(line).unwrap_or("<non-utf8>")
            );
        }
    }
}

// ===== JSON line =====

#[test]
fn test_json_line_string_values() {
    let line = br#"{"level":"info","user":"alice"}"#;
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"level"), Some(&b"info"[..]));
    assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
}

#[test]
fn test_json_line_numeric_values() {
    let line = br#"{"cost_ms":900,"count":42.5}"#;
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    assert_eq!(f.get(b"count"), Some(&b"42.5"[..]));
    // L2 精判联动
    assert_eq!(
        eval_numeric(f.get(b"cost_ms").unwrap(), CmpOp::Gt, 800.0),
        EvalResult::True
    );
    assert_eq!(
        eval_numeric(f.get(b"count").unwrap(), CmpOp::Le, 50.0),
        EvalResult::True
    );
}

#[test]
fn test_json_line_null_skipped() {
    let line = br#"{"cost_ms":null,"user":"alice"}"#;
    let f = ExtractedFields::extract(line);
    // null 视为字段不存在（设计文档 §3.3 空值维度）
    assert_eq!(f.get(b"cost_ms"), None);
    assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
}

#[test]
fn test_json_line_array_first_object() {
    let line = br#"[{"k":"v1"},{"k":"v2"}]"#;
    let f = ExtractedFields::extract(line);
    // 数组取第一个对象的字段（保守，v1 不取 v2）
    assert_eq!(f.get(b"k"), Some(&b"v1"[..]));
}

// ===== [tag] =====

#[test]
fn test_bracket_tag_leading() {
    let line = b"[INFO] started cost_ms=900";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
}

#[test]
fn test_bracket_tag_inline() {
    let line = b"INFO [auth] user=alice";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_tag"), Some(&b"auth"[..]));
}

#[test]
fn test_bracket_tag_multiple_returns_first() {
    let line = b"[INFO] [auth] msg=ok";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
    // tags 列表保留全部
    assert_eq!(f.tags.len(), 2);
}

#[test]
fn test_bracket_tag_numeric_not_tag() {
    // `[0]` 数组索引不是 tag（设计文档：tag 必须字母开头）
    let line = b"items[0] value=x";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_tag"), None);
}

// ===== ISO timestamp =====

#[test]
fn test_iso_timestamp_basic() {
    let line = b"2026-01-02T03:04:05Z level=info";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05Z"[..]));
    assert_eq!(f.get(b"level"), Some(&b"info"[..]));
}

#[test]
fn test_iso_timestamp_with_millis() {
    let line = b"2026-01-02T03:04:05.123Z started";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05.123Z"[..]));
}

#[test]
fn test_iso_timestamp_with_timezone_offset() {
    let line = b"2026-01-02T03:04:05+08:00 started";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05+08:00"[..]));
}

#[test]
fn test_iso_timestamp_not_extracted_from_short_dates() {
    // `2026-01-02` 没有 `T` 不是 ISO 时间戳
    let line = b"2026-01-02 plain date";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_ts"), None);
}

// ===== 混合格式 =====

#[test]
fn test_mixed_iso_logfmt_bracket() {
    // ISO + [tag] + logfmt
    let line = b"2026-01-02T03:04:05Z [INFO] level=info cost_ms=900 user=alice";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05Z"[..]));
    assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
    assert_eq!(f.get(b"level"), Some(&b"info"[..]));
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    // L2 精判联动
    assert_eq!(
        eval_numeric(f.get(b"cost_ms").unwrap(), CmpOp::Gt, 800.0),
        EvalResult::True
    );
}

// ===== Late Materialization（单行内全扫）=====

#[test]
fn test_late_materialization_referenced_fields_only() {
    // 即使只查询一个字段，单行内所有字段都被抽取（v1 实现）
    // Late Materialization 体现在跨行/跨字段：L2 只对 L1 命中行做抽取
    let line = b"level=info cost_ms=900 user=alice latency=50";
    let f = ExtractedFields::extract(line);
    // 查询只引用 cost_ms，但其他字段也可访问（单行内全扫）
    assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    assert_eq!(f.get(b"latency"), Some(&b"50"[..]));
    assert_eq!(f.kv.len(), 4); // 单行内全扫了 4 个 kv 对
}

// ===== 边界 =====

#[test]
fn test_empty_line() {
    let f = ExtractedFields::extract(b"");
    assert!(f.kv.is_empty());
    assert!(f.tags.is_empty());
    assert_eq!(f.get(b"anything"), None);
}

#[test]
fn test_plain_text_no_fields() {
    let line = b"the quick brown fox";
    let f = ExtractedFields::extract(line);
    assert!(f.kv.is_empty());
    assert_eq!(f.get(b"anything"), None);
}

#[test]
fn test_field_not_present_returns_none() {
    let line = b"level=info user=alice";
    let f = ExtractedFields::extract(line);
    assert_eq!(f.get(b"missing"), None);
}

// ===== L1→L2 不变式（模拟）=====

/// 不变式 `L1(粗筛) ⊇ L2(精判)`：
/// L1 命中集 = 所有含 `cost_ms=` 的行（粗筛）
/// L2 命中集 = cost_ms 值 > 800 的行（精判，子集）
#[test]
fn test_l1_superset_l2_invariant_via_extract() {
    let lines: &[&[u8]] = &[
        b"cost_ms=900 user=alice", // L2 命中（900 > 800）
        b"cost_ms=100 user=bob",   // L2 不命中（100 <= 800），L1 命中（有 cost_ms=）
        b"latency=50 user=carol",  // L1 不命中（无 cost_ms=）
        b"no fields here",         // L1 不命中
    ];

    // 模拟 L1 命中：含 cost_ms= 或 cost_ms: 的行
    let l1_hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            // 简化的 L1 粗筛：含 cost_ms= 或 cost_ms:
            line.windows(b"cost_ms=".len()).any(|w| w == b"cost_ms=")
                || line.windows(b"cost_ms:".len()).any(|w| w == b"cost_ms:")
        })
        .map(|(i, _)| i)
        .collect();
    // L1 命中行 0 和 1
    assert_eq!(l1_hits, vec![0, 1]);

    // L2 精判：抽字段值 + eval_numeric
    let l2_hits: Vec<usize> = l1_hits
        .iter()
        .copied()
        .filter(|&i| {
            let f = ExtractedFields::extract(lines[i]);
            let v = f.get(b"cost_ms");
            matches!(
                eval_numeric(v.unwrap_or(b""), CmpOp::Gt, 800.0),
                EvalResult::True
            )
        })
        .collect();
    // L2 只命中行 0
    assert_eq!(l2_hits, vec![0]);

    // 不变式：L2 ⊆ L1
    for l2 in &l2_hits {
        assert!(l1_hits.contains(l2), "L2 命中 {} 不在 L1 命中集中", l2);
    }
}

/// L1→L2 不变式在 JSON 格式下也成立。
#[test]
fn test_l1_superset_l2_invariant_via_json() {
    let lines: &[&[u8]] = &[
        br#"{"cost_ms":900,"user":"alice"}"#.as_ref(), // L2 命中
        br#"{"cost_ms":100,"user":"bob"}"#.as_ref(),   // L1 命中，L2 不命中
        br#"{"latency":50,"user":"carol"}"#.as_ref(),  // L1 不命中
        br#"{"user":"dave"}"#.as_ref(),                // L1 不命中
    ];

    // L1 粗筛：含 cost_ms 字段名前缀（b"cost_ms" 在 JSON 里出现）
    let l1_hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.windows(7).any(|w| w == b"cost_ms"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(l1_hits, vec![0, 1]);

    let l2_hits: Vec<usize> = l1_hits
        .iter()
        .copied()
        .filter(|&i| {
            let f = ExtractedFields::extract(lines[i]);
            let v = f.get(b"cost_ms").unwrap_or(b"");
            matches!(eval_numeric(v, CmpOp::Gt, 800.0), EvalResult::True)
        })
        .collect();
    assert_eq!(l2_hits, vec![0]);

    for l2 in &l2_hits {
        assert!(l1_hits.contains(l2));
    }
}
