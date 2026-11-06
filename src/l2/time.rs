//! ISO 8601 时间戳解析与比较（设计文档 §2.2 EBNF `_ts between`）。
//!
//! `_ts` 伪字段特例：D5（11 维数值字段格式契约）不适用。
//! 使用 `speedate` crate 解析 RFC 3339 / ISO 8601 时间戳为 epoch seconds。

use speedate::DateTime;

/// 解析 ISO 8601 时间戳字符串为 epoch seconds。
///
/// 支持 RFC 3339 全格式：`2026-10-20T08:00:00Z`、`2026-10-20T08:00:00+08:00` 等。
/// speedate 的 `timestamp_tz()` 返回正确的 Unix epoch seconds（已应用时区偏移）。
///
/// 返回 `None` 表示解析失败（调用方 fail-loud）。
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let dt = DateTime::parse_str(s).ok()?;
    Some(dt.timestamp_tz())
}

/// 从行字节中提取 `_ts` 时间戳并比较是否在 `[lo, hi]` 闭区间内。
///
/// 用于 `PreparedPred::TimeBetween` 的 L2 求值。
/// `_ts` 字段值从 `ExtractedFields::find_value(line, "_ts")` 获取。
///
/// 返回三态（与 `eval_between` 一致）：
/// - `EvalResult::True`：时间戳在区间内
/// - `EvalResult::False`：时间戳不在区间内（字段存在但值在区间外）
/// - `EvalResult::NotNumeric`：字段不存在或时间戳解析失败
pub fn eval_time_between(value_bytes: &[u8], lo: i64, hi: i64) -> crate::l2::EvalResult {
    // 从字节切片解析时间戳字符串
    let s = std::str::from_utf8(value_bytes).ok();
    let Some(s) = s else {
        return crate::l2::EvalResult::NotNumeric;
    };
    let Some(ts) = parse_timestamp(s) else {
        return crate::l2::EvalResult::NotNumeric;
    };
    if ts >= lo && ts <= hi {
        crate::l2::EvalResult::True
    } else {
        crate::l2::EvalResult::False
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l2::EvalResult;

    #[test]
    fn test_parse_timestamp_z_suffix() {
        let ts = parse_timestamp("2026-10-20T08:00:00Z");
        assert!(ts.is_some());
        // epoch seconds for 2026-10-20T08:00:00Z
        assert_eq!(ts.unwrap(), 1792483200);
    }

    #[test]
    fn test_parse_timestamp_offset() {
        let ts = parse_timestamp("2026-10-20T16:00:00+08:00");
        assert!(ts.is_some());
        // same instant as 08:00:00Z → same epoch
        assert_eq!(ts.unwrap(), 1792483200);
    }

    #[test]
    fn test_parse_timestamp_invalid() {
        assert!(parse_timestamp("not a timestamp").is_none());
        assert!(parse_timestamp("").is_none());
        assert!(parse_timestamp("2026-10-20").is_none()); // date only, no time
    }

    #[test]
    fn test_eval_time_between_in_range() {
        let lo = parse_timestamp("2026-10-20T08:00:00Z").unwrap();
        let hi = parse_timestamp("2026-10-20T09:00:00Z").unwrap();
        let line = b"2026-10-20T08:30:00Z";
        assert_eq!(eval_time_between(line, lo, hi), EvalResult::True);
    }

    #[test]
    fn test_eval_time_between_out_of_range() {
        let lo = parse_timestamp("2026-10-20T08:00:00Z").unwrap();
        let hi = parse_timestamp("2026-10-20T09:00:00Z").unwrap();
        let line = b"2026-10-20T10:00:00Z";
        assert_eq!(eval_time_between(line, lo, hi), EvalResult::False);
    }

    #[test]
    fn test_eval_time_between_invalid_timestamp() {
        let lo = parse_timestamp("2026-10-20T08:00:00Z").unwrap();
        let hi = parse_timestamp("2026-10-20T09:00:00Z").unwrap();
        let line = b"not a timestamp";
        assert_eq!(eval_time_between(line, lo, hi), EvalResult::NotNumeric);
    }

    #[test]
    fn test_eval_time_between_boundary() {
        let lo = parse_timestamp("2026-10-20T08:00:00Z").unwrap();
        let hi = parse_timestamp("2026-10-20T09:00:00Z").unwrap();
        assert_eq!(
            eval_time_between(b"2026-10-20T08:00:00Z", lo, hi),
            EvalResult::True
        );
        assert_eq!(
            eval_time_between(b"2026-10-20T09:00:00Z", lo, hi),
            EvalResult::True
        );
        assert_eq!(
            eval_time_between(b"2026-10-20T07:59:59Z", lo, hi),
            EvalResult::False
        );
        assert_eq!(
            eval_time_between(b"2026-10-20T09:00:01Z", lo, hi),
            EvalResult::False
        );
    }
}
