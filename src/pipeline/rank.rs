//! `rank top K by FIELD [desc|asc]`：Top-K 排序（gxp 独有，非 grep 等价）。
//!
//! 设计文档 §2.2 EBNF `stage` 规则 + §2.4 示例 2：
//! `| rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms`
//!
//! 语义（v1，对齐 L4 rank 层）：
//! - 按字段值排序取前 K，默认降序（`desc`），`asc` 升序
//! - 排序键：字段可解析为数值（11 维契约）→ 按数值；否则按字节序
//! - 数值用 [`parse_numeric_value`] 解析（与 L2 谓词同一契约）
//! - 非数值/缺失字段的行：按字节序排（降序时数值在前，升序时数值在后）
//! - 稳定输出（相等键保持原行顺序）
//!
//! # 算法（性能优化）
//!
//! 当 `top < matched.len()` 时使用 **partial-sort**：
//! 1. 预计算所有行的排序键（一次 `ExtractedFields::extract` + `parse_numeric_value`）
//! 2. `select_nth_unstable_by` 按 `top` 分区（O(N) 期望，不稳定）
//! 3. 对前 `top` 个元素做 `sort_by`（O(K log K)，稳定）
//!
//! 当 `top >= matched.len()` 时全量稳定排序。
//!
//! 对于 N=103k、K=20 的 §7.1 基准场景：O(N) + O(K log K) ≈ 443k 次比较，
//! 对比全量 O(N log N) ≈ 1.75M 次比较，理论 4× 加速。

use std::cmp::Ordering;

use super::MatchedLine;
use crate::l2::parse_numeric_value;

/// 排序键：数值优先（11 维契约），否则字节序。
///
/// v0.42 修复：`Bytes` 从"沉底标记"改为"按字节序比较的字符串值"。
/// 原 bug：所有 `Bytes` 视为相等（cmp_keys 返回 Equal），导致 `sort by _match asc` /
/// `sort by _line asc` / `sort by _file asc` 即使伪字段值不同也保持原序——
/// LLM 写 `sort by _line asc` 期望按整行字节序排序，但 Bytes 同键稳定排序失效。
/// 修复方案：`Bytes(Vec<u8>)` 携带实际字节，cmp_keys 对两个 Bytes 按字节序比较
/// （desc 可逆）。仅 `_match`/`_line`/`_file` 伪字段走 Bytes 分支（pseudo_field_value
/// 返回 Some(Vec)），普通字段值非数值仍走 Bytes 但携带实际字节，按字节序排。
#[derive(Debug, Clone)]
pub(super) enum RankKey {
    /// 可解析为数值（11 维契约）。
    Num(f64),
    /// 非数值：按字节序比较（携带实际字节）。
    Bytes(Vec<u8>),
}

/// 执行 `rank top K by FIELD [desc|asc]`：按字段值排序取前 K。
///
/// - `by`：排序字段名
/// - `top`：取前 K 条
/// - `desc`：`true` 降序（默认），`false` 升序
///
/// 非数值字段（含缺失）降序时排最后（`Bytes` 键），升序时排最前。
///
/// # 算法
///
/// - `top >= len`：全量稳定排序（保持原行顺序的相等元素）
/// - `top < len`：partial-sort——`select_nth_unstable_by` 分区 + 前 `top` 稳定排序
///
/// partial-sort 保证输出前 `top` 行按指定顺序排列，且相等键间保持原顺序。
pub fn apply_rank(matched: &[MatchedLine], by: &[u8], top: usize, desc: bool) -> Vec<MatchedLine> {
    if matched.is_empty() || top == 0 {
        return Vec::new();
    }

    // 每行提取排序键（一次抽取，避免排序闭包内重复 extract）
    // 性能优化：用 `ExtractedFields::find_value` 跳过全字段扫描
    // （单字段定位，memchr 直接定位 `name=` / `name:`，2-3× 加速）
    // 附带原下标用于稳定排序：相同键时按原顺序输出
    //
    // v0.42 修复：用 pseudo_field_value 替代直接 find_value，
    // 支持 _match/_line/_file 伪字段（与 project.rs 语义对齐）。
    //
    // v0.42 修复（续）：captures 优先——capture(NAME, /REGEX/) 提取的值
    // 应优先于行内同名字段（与 project.rs / count.rs / percentiles.rs 对齐）。
    let field_str = String::from_utf8_lossy(by).into_owned();
    let mut keyed: Vec<(RankKey, usize, &MatchedLine)> = matched
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let val = m
                .captures
                .get(&field_str)
                .cloned()
                .or_else(|| super::pseudo_field_value(m, by));
            let key = match val {
                Some(v) => match parse_numeric_value(&v) {
                    Some(n) => RankKey::Num(n),
                    None => RankKey::Bytes(v),
                },
                None => RankKey::Bytes(Vec::new()),
            };
            (key, i, m)
        })
        .collect();

    // 比较闭包：先按 RankKey（desc 可逆），同键时按原下标升序（稳定）
    let cmp = |a: &(RankKey, usize, &MatchedLine), b: &(RankKey, usize, &MatchedLine)| {
        let key_ord = cmp_keys(&a.0, &b.0, desc);
        if key_ord == Ordering::Equal {
            // 稳定：相同键按原下标升序
            a.1.cmp(&b.1)
        } else {
            key_ord
        }
    };

    if top >= keyed.len() {
        // 全量稳定排序
        keyed.sort_by(cmp);
    } else {
        // partial-sort：分区 + 前 top 稳定排序
        // select_nth_unstable_by 是 O(N) 期望，将前 top 小的元素放到切片前段（顺序无保证）
        let (front, _, _) = keyed.select_nth_unstable_by(top, cmp);
        // 对前 top 稳定排序（K=20 时 O(20 * log20) ≈ 86 次比较）
        front.sort_by(cmp);
    }

    keyed
        .into_iter()
        .take(top)
        .map(|(_, _, m)| m.clone())
        .collect()
}

/// 排序键比较：`desc=true` 降序（大值在前），`desc=false` 升序。
///
/// 数值与字节序的跨类比较：数值始终排在非数值之前（两类排序都如此），
/// 因为数值是 Top-K 排序的主要关注对象，非数值/缺失字段沉底。
///
/// v0.42 修复：`Bytes(Bytes)` → `Bytes(Vec<u8>)`，两个 Bytes 间按字节序比较
/// （原 Equal 导致 sort by _line/_match/_file 静默失效）。
pub(super) fn cmp_keys(a: &RankKey, b: &RankKey, desc: bool) -> Ordering {
    match (a, b) {
        (RankKey::Num(x), RankKey::Num(y)) => {
            let ord = x.partial_cmp(y).unwrap_or(Ordering::Equal);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }
        // 跨类：数值始终在非数值之前
        (RankKey::Num(_), RankKey::Bytes(_)) => Ordering::Less,
        (RankKey::Bytes(_), RankKey::Num(_)) => Ordering::Greater,
        (RankKey::Bytes(a), RankKey::Bytes(b)) => {
            let ord = a.cmp(b);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MatchedLine;

    fn lines(rows: &[&str]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, s)| MatchedLine::new(i + 1, s.as_bytes()))
            .collect()
    }

    fn texts(lines: &[MatchedLine]) -> Vec<String> {
        lines
            .iter()
            .map(|m| String::from_utf8_lossy(&m.line).to_string())
            .collect()
    }

    #[test]
    fn test_rank_desc_takes_top_k_numeric() {
        let m = lines(&["cost=100", "cost=900", "cost=500", "cost=700", "cost=300"]);
        let out = apply_rank(&m, b"cost", 3, true);
        // 降序前 3：900, 700, 500
        assert_eq!(texts(&out), vec!["cost=900", "cost=700", "cost=500"]);
    }

    #[test]
    fn test_rank_asc_takes_top_k_numeric() {
        let m = lines(&["cost=100", "cost=900", "cost=500"]);
        let out = apply_rank(&m, b"cost", 2, false);
        // 升序前 2：100, 500
        assert_eq!(texts(&out), vec!["cost=100", "cost=500"]);
    }

    #[test]
    fn test_rank_numeric_parsing_follows_11_dim_contract() {
        // 前导零 / 正号 / 科学计数法都按数值排序
        let m = lines(&["cost=0950", "cost=1e3", "cost=900"]);
        let out = apply_rank(&m, b"cost", 3, true);
        // 1e3=1000, 0950=950, 900
        assert_eq!(texts(&out), vec!["cost=1e3", "cost=0950", "cost=900"]);
    }

    #[test]
    fn test_rank_non_numeric_bytes_fallback() {
        // v0.42 语义修正：非数值字段按字节序比较（原"全部同键"是 bug）
        // desc=true：字节序大的在前
        let m = lines(&["cost=900", "cost=abc", "other=x"]);
        let out = apply_rank(&m, b"cost", 3, true);
        // 数值 900 在前；非数值 abc 字节序 > ""（other=x 缺 cost 字段 → 空 Vec）
        // 降序：abc > "" → abc 在前，other 在最后
        assert_eq!(texts(&out), vec!["cost=900", "cost=abc", "other=x"]);
    }

    #[test]
    fn test_rank_stable_for_equal_keys() {
        // 相同数值保持原顺序（稳定排序）
        let m = lines(&["cost=900 a", "cost=900 b", "cost=100 c"]);
        let out = apply_rank(&m, b"cost", 3, true);
        assert_eq!(texts(&out), vec!["cost=900 a", "cost=900 b", "cost=100 c"]);
    }

    #[test]
    fn test_rank_top_larger_than_total_keeps_all() {
        let m = lines(&["cost=100", "cost=900"]);
        let out = apply_rank(&m, b"cost", 10, true);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_rank_empty_input() {
        let out = apply_rank(&[], b"cost", 5, true);
        assert!(out.is_empty());
    }

    // --- partial-sort 正确性校验 ---

    #[test]
    fn test_rank_partial_sort_matches_full_sort_desc() {
        // 构造 100 行，确保 N > top，触发 partial-sort 路径
        let rows: Vec<String> = (0..100).map(|i| format!("cost={}", 100 - i)).collect();
        let rows_ref: Vec<&str> = rows.iter().map(|s| s.as_str()).collect();
        let m = lines(&rows_ref);
        let partial = apply_rank(&m, b"cost", 10, true);
        // partial-sort 结果应与全量 sort + take 完全一致
        assert_eq!(partial.len(), 10);
        // 降序前 10：cost=100, cost=99, ..., cost=91
        let expected: Vec<String> = (0..10).map(|i| format!("cost={}", 100 - i)).collect();
        assert_eq!(texts(&partial), expected);
    }

    #[test]
    fn test_rank_partial_sort_matches_full_sort_asc() {
        let rows: Vec<String> = (0..50).map(|i| format!("cost={}", i + 100)).collect();
        let rows_ref: Vec<&str> = rows.iter().map(|s| s.as_str()).collect();
        let m = lines(&rows_ref);
        let partial = apply_rank(&m, b"cost", 5, false);
        // 升序前 5：cost=100, cost=101, ..., cost=104
        let expected: Vec<String> = (0..5).map(|i| format!("cost={}", i + 100)).collect();
        assert_eq!(texts(&partial), expected);
    }

    #[test]
    fn test_rank_partial_sort_stability_with_ties() {
        // 多个相同值：partial-sort 应保持原顺序
        let rows = vec![
            "cost=500 a",
            "cost=500 b",
            "cost=100 c",
            "cost=500 d",
            "cost=100 e",
            "cost=500 f",
            "cost=100 g",
            "cost=500 h",
        ];
        let m = lines(&rows);
        // 取 top 5 降序：500 优先，同值按原顺序
        let out = apply_rank(&m, b"cost", 5, true);
        // 500 组共 5 条（a,b,d,f,h），全部入榜
        assert_eq!(
            texts(&out),
            vec![
                "cost=500 a",
                "cost=500 b",
                "cost=500 d",
                "cost=500 f",
                "cost=500 h"
            ]
        );
        // 取 top 6：第 6 条应从 100 组取首条（c）
        let out6 = apply_rank(&m, b"cost", 6, true);
        assert_eq!(
            texts(&out6),
            vec![
                "cost=500 a",
                "cost=500 b",
                "cost=500 d",
                "cost=500 f",
                "cost=500 h",
                "cost=100 c"
            ]
        );
    }

    #[test]
    fn test_rank_partial_sort_top_zero() {
        let m = lines(&["cost=100", "cost=200"]);
        let out = apply_rank(&m, b"cost", 0, true);
        assert!(out.is_empty());
    }

    #[test]
    fn test_rank_partial_sort_top_one() {
        let m = lines(&["cost=100", "cost=500", "cost=200"]);
        let out = apply_rank(&m, b"cost", 1, true);
        assert_eq!(texts(&out), vec!["cost=500"]);
    }

    #[test]
    fn test_rank_partial_sort_top_equals_len() {
        // top == len：走全量排序路径
        let m = lines(&["cost=300", "cost=100", "cost=200"]);
        let out = apply_rank(&m, b"cost", 3, true);
        assert_eq!(texts(&out), vec!["cost=300", "cost=200", "cost=100"]);
    }

    #[test]
    fn test_rank_partial_sort_non_numeric_desc_fallback() {
        // v0.42 语义修正：非数值字段按字节序比较（原"全部同键"是 bug）
        // 非数值 + 缺失：降序时数值在前，非数值按字节序降序在后
        let m = lines(&["cost=500", "cost=abc", "other=x", "cost=100", "cost=xyz"]);
        let out = apply_rank(&m, b"cost", 3, true);
        // 数值 500, 100 在前（降序）；非数值 xyz > abc > ""（other=x 缺 cost）
        // 降序取 top 3 第 3 条：xyz
        assert_eq!(texts(&out), vec!["cost=500", "cost=100", "cost=xyz"]);
    }

    // --- 伪字段 _match / _line / _file（v0.42 修复）---

    fn lines_with_match(rows: &[(&str, Option<&[u8]>)]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, (s, mb))| {
                let mut m = MatchedLine::new(i + 1, s.as_bytes());
                m.match_bytes = mb.map(|b| b.to_vec());
                m
            })
            .collect()
    }

    #[test]
    fn test_rank_by_match_pseudo_field_desc() {
        // OR 查询：rank top 2 by _match desc → 字节序降序前 2
        let m = lines_with_match(&[
            ("banana", Some(b"banana")),
            ("apple", Some(b"apple")),
            ("cherry", Some(b"cherry")),
        ]);
        let out = apply_rank(&m, b"_match", 2, true);
        // 降序前 2：cherry, banana
        assert_eq!(texts(&out), vec!["cherry", "banana"]);
    }

    #[test]
    fn test_rank_by_line_pseudo_field_asc() {
        let m = lines(&["banana", "apple", "cherry"]);
        let out = apply_rank(&m, b"_line", 2, false);
        // 升序前 2：apple, banana
        assert_eq!(texts(&out), vec!["apple", "banana"]);
    }

    #[test]
    fn test_rank_by_match_when_match_bytes_none_desc() {
        // 无 match_bytes（纯字段谓词）→ 空 Vec 同键稳定
        let m = lines(&["a=100", "a=200", "a=150"]);
        let out = apply_rank(&m, b"_match", 2, true);
        // 全部空 Vec → 同键稳定保持原序，取前 2
        assert_eq!(texts(&out), vec!["a=100", "a=200"]);
    }

    // --- captures 优先（v0.42 修复：与 project/count/percentiles 对齐）---

    #[test]
    fn test_rank_by_field_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段
        let mut m1 = MatchedLine::new(1, b"id=zzz");
        m1.captures.insert("id".to_string(), b"abc".to_vec());
        let mut m2 = MatchedLine::new(2, b"id=xxx");
        m2.captures.insert("id".to_string(), b"def".to_vec());
        let m = vec![m1, m2];
        let out = apply_rank(&m, b"id", 2, false);
        // 按 capture 值字节序升序前 2：abc, def
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![1, 2]
        );
        // desc 前 2：def, abc
        let out = apply_rank(&m, b"id", 2, true);
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn test_rank_by_field_capture_when_no_line_field() {
        // 行内无 id 字段，capture 提取的值仍可用
        let mut m1 = MatchedLine::new(1, b"ERROR abc");
        m1.captures.insert("id".to_string(), b"def".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR def");
        m2.captures.insert("id".to_string(), b"abc".to_vec());
        let m = vec![m1, m2];
        let out = apply_rank(&m, b"id", 1, false);
        // 升序 top 1：abc → 第 2 行
        assert_eq!(out.iter().map(|m| m.line_no).collect::<Vec<_>>(), vec![2]);
    }
}
