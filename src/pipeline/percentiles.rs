//! `percentiles FIELD [p50,p95,p99]`：非流式百分位计算（§9.1 候选 #6 落地）。
//!
//! # 语义
//!
//! - `percentiles FIELD`：默认 p50/p95/p99
//! - `percentiles FIELD p50,p95,p99`：指定百分位列表
//!
//! # 与 `stats` 区别
//!
//! - `stats`：流式 min/max/avg（O(1) 内存，逐行更新）
//! - `percentiles`：非流式 p50/p95/p99（O(N) 内存缓冲全量，与 sort 一致）
//!
//! avg 掩盖长尾分布——SLO 报告需 p95/p99 衡量尾部延迟。
//!
//! # 算法
//!
//! 1. 收集所有有效数值（11 维契约 D5 解析）
//! 2. 排序（`sort_by` 稳定排序）
//! 3. 对每个百分位 p，索引 = `ceil(p/100 * len) - 1`（nearest-rank 方法）
//!
//! 输出格式：`p50: X, p95: Y, p99: Z`（与 stats 的 `min: X, max: Y, avg: Z` 格式一致）。
//! 无有效行时输出 `p50: N/A, p95: N/A, p99: N/A`。
//!
//! # --verify 对齐
//!
//! 确定性算法（排序 + nearest-rank），暴力路径与优化路径走同一逻辑，100% 对齐。

use crate::l2::parse_numeric_value;
use crate::pipeline::MatchedLine;

/// 执行 `percentiles FIELD [p50,p95,p99]`：非流式百分位计算。
///
/// - `field`：聚合字段名
/// - `percentiles`：百分位列表（如 `[50.0, 95.0, 99.0]`）
///
/// 返回格式化字符串：`p50: X, p95: Y, p99: Z`。
/// 字段可见性与 `fields`/`group by` 一致：captures 优先 + 行内 fallback。
pub fn apply_percentiles(lines: &[MatchedLine], field: &[u8], percentiles: &[f64]) -> String {
    let field_str = String::from_utf8_lossy(field).into_owned();

    // 收集所有有效数值
    let mut values: Vec<f64> = Vec::with_capacity(lines.len());
    for m in lines {
        // captures 优先 + 行内 fallback（对齐 stats/fields/group by）
        // 用 pseudo_field_value 替代直接 find_value，支持
        // _match/_line/_file 伪字段（与 stats.rs / count.rs / sort.rs 对齐）
        let val = m
            .captures
            .get(&field_str)
            .cloned()
            .or_else(|| crate::pipeline::pseudo_field_value(m, field))
            .unwrap_or_default();
        if let Some(v) = parse_numeric_value(&val) {
            values.push(v);
        }
    }

    if values.is_empty() {
        let n_a: Vec<String> = percentiles
            .iter()
            .map(|p| format!("p{}: N/A", *p as u64))
            .collect();
        return n_a.join(", ");
    }

    // 排序
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // 对每个百分位计算 nearest-rank
    let results: Vec<String> = percentiles
        .iter()
        .map(|p| {
            let idx = nearest_rank_index(*p, values.len());
            let val = values[idx];
            format!("p{}: {}", *p as u64, fmt_num(val))
        })
        .collect();

    results.join(", ")
}

/// nearest-rank 方法：索引 = ceil(p/100 * len) - 1，clamp 到 [0, len-1]。
///
/// - p=50, len=100 → idx = ceil(0.5*100)-1 = 50-1 = 49（第 50 个值，中位数）
/// - p=95, len=100 → idx = ceil(0.95*100)-1 = 95-1 = 94
/// - p=99, len=100 → idx = ceil(0.99*100)-1 = 99-1 = 98
/// - p=100, len=100 → idx = ceil(1.0*100)-1 = 100-1 = 99（最大值）
fn nearest_rank_index(p: f64, len: usize) -> usize {
    let rank = (p / 100.0 * len as f64).ceil() as usize;
    // clamp 到 [1, len] 然后减 1 → [0, len-1]
    (rank.max(1).min(len)) - 1
}

/// 格式化数值：整数显示无小数点，浮点保留原样。
fn fmt_num(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MatchedLine;

    fn make_line(line_no: usize, content: &str) -> MatchedLine {
        MatchedLine::new(line_no, content.as_bytes())
    }

    fn lines(rows: &[&str]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, s)| MatchedLine::new(i + 1, s.as_bytes()))
            .collect()
    }

    #[test]
    fn test_percentiles_basic_100_values() {
        // 1..=100，p50=50, p95=95, p99=99
        let l: Vec<MatchedLine> = (1..=100)
            .map(|i| make_line(i as usize, &format!("v={i}")))
            .collect();
        let out = apply_percentiles(&l, b"v", &[50.0, 95.0, 99.0]);
        assert!(out.contains("p50: 50"), "p50 应为 50, 实际: {out}");
        assert!(out.contains("p95: 95"), "p95 应为 95, 实际: {out}");
        assert!(out.contains("p99: 99"), "p99 应为 99, 实际: {out}");
    }

    #[test]
    fn test_percentiles_single_value() {
        let l = lines(&["v=42"]);
        let out = apply_percentiles(&l, b"v", &[50.0, 95.0, 99.0]);
        // 单值 → 所有百分位 = 42
        assert!(out.contains("p50: 42"), "p50 应为 42, 实际: {out}");
        assert!(out.contains("p95: 42"), "p95 应为 42, 实际: {out}");
        assert!(out.contains("p99: 42"), "p99 应为 42, 实际: {out}");
    }

    #[test]
    fn test_percentiles_no_valid_lines() {
        let l = lines(&["v=NaN", "no field"]);
        let out = apply_percentiles(&l, b"v", &[50.0, 95.0]);
        assert!(out.contains("N/A"), "无有效行应输出 N/A, 实际: {out}");
    }

    #[test]
    fn test_percentiles_skips_non_numeric() {
        // 混合有效/无效值：只统计有效的
        let l = lines(&["v=10", "v=abc", "v=20", "no field", "v=30"]);
        let out = apply_percentiles(&l, b"v", &[50.0]);
        // 有效值 [10, 20, 30]，p50 → idx = ceil(0.5*3)-1 = 2-1 = 1 → 20
        assert!(out.contains("p50: 20"), "p50 应为 20, 实际: {out}");
    }

    #[test]
    fn test_percentiles_custom_list() {
        // 自定义百分位列表 p90,p99.9（验证非默认百分位 + 小数十分位截断）
        let l: Vec<MatchedLine> = (1..=1000)
            .map(|i| make_line(i as usize, &format!("v={i}")))
            .collect();
        let out = apply_percentiles(&l, b"v", &[90.0, 99.9]);
        // p90: idx = ceil(0.9*1000)-1 = 900-1 = 899 → 900
        assert!(out.contains("p90: 900"), "p90 应为 900, 实际: {out}");
        // p99.9 as u64 截断为 99；
        // 浮点 0.999*1000 = 999.0000001 → ceil = 1000 → idx 999 → value 1000
        assert!(
            out.contains("p99: 1000"),
            "p99.9 应格式化为 p99 且值为 1000（浮点 ceil 上取整），实际: {out}"
        );
    }

    #[test]
    fn test_percentiles_empty_input() {
        let l: Vec<MatchedLine> = vec![];
        let out = apply_percentiles(&l, b"v", &[50.0, 95.0]);
        assert!(out.contains("N/A"), "空输入应输出 N/A, 实际: {out}");
    }

    #[test]
    fn test_percentiles_captures_priority() {
        // captures 优先于行内字段（对齐 fields/group by）
        let mut m = MatchedLine::new(1, b"v=10");
        m.captures.insert("v".to_string(), b"50".to_vec());
        let out = apply_percentiles(&[m], b"v", &[50.0]);
        assert!(out.contains("p50: 50"), "capture 值 50 应优先, 实际: {out}");
    }

    #[test]
    fn test_percentiles_float_values() {
        let l = lines(&["v=1.5", "v=2.5", "v=3.5"]);
        let out = apply_percentiles(&l, b"v", &[50.0]);
        // 排序后 [1.5, 2.5, 3.5]，p50 → idx=ceil(0.5*3)-1=2-1=1 → 2.5
        assert!(out.contains("p50: 2.5"), "p50 应为 2.5, 实际: {out}");
    }

    #[test]
    fn test_nearest_rank_index() {
        // p=50, len=100 → 49
        assert_eq!(nearest_rank_index(50.0, 100), 49);
        // p=95, len=100 → 94
        assert_eq!(nearest_rank_index(95.0, 100), 94);
        // p=99, len=100 → 98
        assert_eq!(nearest_rank_index(99.0, 100), 98);
        // p=100, len=100 → 99
        assert_eq!(nearest_rank_index(100.0, 100), 99);
        // p=0, len=100 → 0（clamp）
        assert_eq!(nearest_rank_index(0.0, 100), 0);
        // p=50, len=1 → 0
        assert_eq!(nearest_rank_index(50.0, 1), 0);
        // p=50, len=3 → 1
        assert_eq!(nearest_rank_index(50.0, 3), 1);
    }

    // --- 伪字段 _match / _line / _file（对齐 stats/sort/rank/count）---

    #[test]
    fn test_percentiles_match_pseudo_field_skipped_as_non_numeric() {
        // _match = L1 命中字节，非数值 → parse_numeric_value 拒绝 → N/A
        // 验证 pseudo_field_value 路径接通（虽非数值仍走该路径，但被 parse 拒绝）
        let mut m1 = MatchedLine::new(1, b"ERROR foo");
        m1.match_bytes = Some(std::sync::Arc::from(b"ERROR".as_ref()));
        let mut m2 = MatchedLine::new(2, b"ERROR bar");
        m2.match_bytes = Some(std::sync::Arc::from(b"ERROR".as_ref()));
        let out = apply_percentiles(&[m1, m2], b"_match", &[50.0]);
        assert!(out.contains("N/A"), "_match 非数值应输出 N/A, 实际: {out}");
    }

    #[test]
    fn test_percentiles_line_pseudo_field_skipped_as_non_numeric() {
        // _line = 整行内容，非数值 → 跳过
        let l = lines(&["abc", "def", "ghi"]);
        let out = apply_percentiles(&l, b"_line", &[50.0]);
        assert!(out.contains("N/A"), "_line 非数值应输出 N/A, 实际: {out}");
    }

    #[test]
    fn test_percentiles_file_pseudo_field_skipped_as_non_numeric() {
        // _file = 文件路径，非数值 → 跳过
        let mut m1 = MatchedLine::new(1, b"x");
        m1.path = Some(std::sync::Arc::from("src/a.rs"));
        let mut m2 = MatchedLine::new(2, b"x");
        m2.path = Some(std::sync::Arc::from("src/b.rs"));
        let out = apply_percentiles(&[m1, m2], b"_file", &[50.0]);
        assert!(out.contains("N/A"), "_file 非数值应输出 N/A, 实际: {out}");
    }
}
