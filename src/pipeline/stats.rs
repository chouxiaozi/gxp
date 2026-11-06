//! `stats FIELD`：流式聚合 min/max/avg（设计文档 §2.2 EBNF `stage` 规则）。
//!
//! 只做 min/max/avg（可流式）；p50/p95/p99 由 [`crate::pipeline::percentiles`] 承担。
//! 逐行更新 min/max/sum/count，不缓冲全量数据。
//! 输出格式：`min: X, max: Y, avg: Z`（一行摘要）。
//!
//! 数值字段按 11 维契约（D5）解析；不可解析的行跳过（不计入统计）。

use crate::l2::parse_numeric_value;
use crate::pipeline::MatchedLine;

/// 流式统计状态：逐行更新 min/max/sum/count。
#[derive(Debug, Default)]
pub struct StatsState {
    /// 最小值
    pub min: f64,
    /// 最大值
    pub max: f64,
    /// 数值和（用于计算 avg = sum / count）
    pub sum: f64,
    /// 有效行数（数值成功解析的行）
    pub count: usize,
}

impl StatsState {
    /// 更新一条命中行的数值。
    /// 如果字段不存在或数值不可解析（11 维契约拒绝），跳过（不更新 count）。
    ///
    /// # 字段可见性（见下）
    ///
    /// 接受 `Option<&[u8]>` 预提取的字段值，调用方负责 captures 优先 + 行内 fallback。
    /// 原签名 `update(&mut self, line: &[u8], field: &[u8])` 直接调 `find_value`，
    /// 漏 captures 优先 → `capture(cost, /(\d+)/) | stats cost` 全输出 N/A。
    /// 改签名让 `apply_stats` 统一走 captures 优先链路，与 percentiles.rs 对齐。
    pub fn update(&mut self, value: Option<&[u8]>) {
        let Some(value) = value else {
            return;
        };
        let Some(v) = parse_numeric_value(value) else {
            return;
        };
        if self.count == 0 {
            self.min = v;
            self.max = v;
        } else {
            if v < self.min {
                self.min = v;
            }
            if v > self.max {
                self.max = v;
            }
        }
        self.sum += v;
        self.count += 1;
    }

    /// 计算平均值（sum / count）。无有效行时返回 None。
    pub fn avg(&self) -> Option<f64> {
        if self.count == 0 {
            None
        } else {
            Some(self.sum / self.count as f64)
        }
    }

    /// 格式化输出：`min: X, max: Y, avg: Z`（无有效行时输出 `min: N/A, max: N/A, avg: N/A`）。
    pub fn format_summary(&self) -> String {
        match self.avg() {
            Some(avg) => {
                let fmt = |v: f64| -> String {
                    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
                        format!("{}", v as i64)
                    } else {
                        format!("{v}")
                    }
                };
                format!(
                    "min: {}, max: {}, avg: {}",
                    fmt(self.min),
                    fmt(self.max),
                    fmt(avg)
                )
            }
            None => "min: N/A, max: N/A, avg: N/A".to_string(),
        }
    }
}

/// 对命中行集合跑 stats 聚合，返回格式化摘要字符串。
///
/// 字段可见性（见下）：captures 优先 + 行内 fallback（pseudo_field_value），
/// 与 percentiles.rs / count.rs / sort.rs / rank.rs / project.rs 对齐。
/// 原 apply_stats 直接调 `ExtractedFields::find_value`，漏 captures 优先 →
/// `capture(cost, /(\d+)/) | stats cost` 全输出 N/A（capture 值被忽略）。
pub fn apply_stats(lines: &[MatchedLine], field: &[u8]) -> String {
    let field_str = String::from_utf8_lossy(field).into_owned();
    let mut state = StatsState::default();
    for m in lines {
        // captures 优先（clone 一份避免借用冲突），fallback 到 pseudo_field_value
        //（pseudo_field_value 已返回 owned Vec<u8>，无需再 clone）
        let val: Option<Vec<u8>> = m
            .captures
            .get(&field_str)
            .cloned()
            .or_else(|| crate::pipeline::pseudo_field_value(m, field));
        state.update(val.as_deref());
    }
    state.format_summary()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MatchedLine;

    fn make_line(line_no: usize, content: &str) -> MatchedLine {
        MatchedLine::new(line_no, content.as_bytes())
    }

    #[test]
    fn test_stats_basic_aggregation() {
        let lines = vec![
            make_line(1, "cost_ms=100"),
            make_line(2, "cost_ms=200"),
            make_line(3, "cost_ms=300"),
        ];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(
            summary.contains("min: 100"),
            "应含 min: 100, 实际: {summary}"
        );
        assert!(
            summary.contains("max: 300"),
            "应含 max: 300, 实际: {summary}"
        );
        assert!(
            summary.contains("avg: 200"),
            "应含 avg: 200, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_skips_non_numeric_values() {
        let lines = vec![
            make_line(1, "cost_ms=100"),
            make_line(2, "cost_ms=abc"), // 非法数值 → 跳过
            make_line(3, "cost_ms=300"),
            make_line(4, "no field here"), // 无字段 → 跳过
        ];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(
            summary.contains("min: 100"),
            "应含 min: 100, 实际: {summary}"
        );
        assert!(
            summary.contains("max: 300"),
            "应含 max: 300, 实际: {summary}"
        );
        // avg = (100 + 300) / 2 = 200
        assert!(
            summary.contains("avg: 200"),
            "应含 avg: 200, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_no_valid_lines() {
        let lines = vec![
            make_line(1, "cost_ms=NaN"), // 拒绝
            make_line(2, "no field"),    // 无字段
        ];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(
            summary.contains("N/A"),
            "无有效行应输出 N/A, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_float_values() {
        let lines = vec![
            make_line(1, "cost_ms=1.5"),
            make_line(2, "cost_ms=2.5"),
            make_line(3, "cost_ms=3.0"),
        ];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(
            summary.contains("min: 1.5"),
            "应含 min: 1.5, 实际: {summary}"
        );
        assert!(summary.contains("max: 3"), "应含 max: 3, 实际: {summary}");
        // avg = (1.5 + 2.5 + 3.0) / 3 = 2.333...
        assert!(
            summary.contains("avg: 2.33"),
            "应含 avg: 2.33, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_negative_values() {
        let lines = vec![
            make_line(1, "temp=-10"),
            make_line(2, "temp=5"),
            make_line(3, "temp=0"),
        ];
        let summary = apply_stats(&lines, b"temp");
        assert!(
            summary.contains("min: -10"),
            "应含 min: -10, 实际: {summary}"
        );
        assert!(summary.contains("max: 5"), "应含 max: 5, 实际: {summary}");
        // avg = (-10 + 5 + 0) / 3 = -1.666...
        assert!(
            summary.contains("avg: -1.66"),
            "应含 avg: -1.66, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_single_value() {
        let lines = vec![make_line(1, "cost_ms=42")];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(summary.contains("min: 42"), "应含 min: 42, 实际: {summary}");
        assert!(summary.contains("max: 42"), "应含 max: 42, 实际: {summary}");
        assert!(summary.contains("avg: 42"), "应含 avg: 42, 实际: {summary}");
    }

    #[test]
    fn test_stats_empty_lines() {
        let lines: Vec<MatchedLine> = vec![];
        let summary = apply_stats(&lines, b"cost_ms");
        assert!(summary.contains("N/A"), "空行集应输出 N/A, 实际: {summary}");
    }

    // --- captures 优先（与 percentiles/count/sort/rank/project 对齐）---

    #[test]
    fn test_stats_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段
        // 行 1: cost=line_100, capture=200；行 2: cost=line_300, capture=400
        // stats cost 应按 capture 值聚合 → min=200, max=400, avg=300
        let mut m1 = MatchedLine::new(1, b"cost=line_100");
        m1.captures.insert("cost".to_string(), b"200".to_vec());
        let mut m2 = MatchedLine::new(2, b"cost=line_300");
        m2.captures.insert("cost".to_string(), b"400".to_vec());
        let summary = apply_stats(&[m1, m2], b"cost");
        assert!(
            summary.contains("min: 200"),
            "应按 capture 值聚合 min=200, 实际: {summary}"
        );
        assert!(
            summary.contains("max: 400"),
            "应按 capture 值聚合 max=400, 实际: {summary}"
        );
        assert!(
            summary.contains("avg: 300"),
            "应按 capture 值聚合 avg=300, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_capture_when_no_line_field() {
        // 行内无 cost 字段，capture 提取的值仍可用作 stats 输入
        let mut m1 = MatchedLine::new(1, b"ERROR abc");
        m1.captures.insert("cost".to_string(), b"100".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR def");
        m2.captures.insert("cost".to_string(), b"300".to_vec());
        let summary = apply_stats(&[m1, m2], b"cost");
        assert!(
            summary.contains("min: 100"),
            "应含 min: 100, 实际: {summary}"
        );
        assert!(
            summary.contains("max: 300"),
            "应含 max: 300, 实际: {summary}"
        );
        assert!(
            summary.contains("avg: 200"),
            "应含 avg: 200, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_capture_mixed_with_line_field() {
        // 混合：有 capture 的按 capture 聚合，无 capture 的按行内字段聚合
        let mut m1 = MatchedLine::new(1, b"cost=1000");
        m1.captures.insert("cost".to_string(), b"50".to_vec());
        // m2 无 capture，按行内 cost=200
        let m2 = MatchedLine::new(2, b"cost=200");
        let summary = apply_stats(&[m1, m2], b"cost");
        // m1 capture=50, m2 line=200 → min=50, max=200, avg=125
        assert!(
            summary.contains("min: 50"),
            "m1 capture=50 应优先, 实际: {summary}"
        );
        assert!(summary.contains("max: 200"), "m2 line=200, 实际: {summary}");
        // avg = (50 + 200) / 2 = 125
        assert!(
            summary.contains("avg: 125"),
            "应含 avg: 125, 实际: {summary}"
        );
    }

    #[test]
    fn test_stats_capture_pseudo_field_still_skipped() {
        // 伪字段 _match/_line/_file 的值通常是字符串，无法数值解析 → 跳过 → N/A
        // 验证：伪字段仍走 pseudo_field_value 路径，但 parse_numeric_value 拒绝非数字
        let mut m1 = MatchedLine::new(1, b"ERROR abc");
        m1.match_bytes = Some(b"ERROR".to_vec());
        let summary = apply_stats(&[m1], b"_match");
        // _match = "ERROR"，parse_numeric_value 返回 None → 跳过 → N/A
        assert!(
            summary.contains("N/A"),
            "_match 字符串应跳过, 实际: {summary}"
        );
    }
}
