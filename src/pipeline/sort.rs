//! `sort [by FIELD] [desc|asc]`：全量稳定排序。
//!
//! 设计文档 §2.2 EBNF `stage` 规则；§9.1 候选 #5（L2 等级，落地 v0.34）。
//!
//! # 语义
//!
//! - `sort`：按整行字节序升序（聚集相同错误行，对齐 Unix `sort` 默认）
//! - `sort by FIELD`：按字段值排序（数值优先 11 维契约，非数值沉底）
//! - `sort by FIELD desc`：降序；`asc` 升序（默认）
//!
//! # 与 `rank top K by FIELD` 区别
//!
//! - rank 只取 Top-K（partial-sort + select_nth），默认 `desc`（"最大的 K 个"语义）
//! - sort 是全量有序（O(N log N) 稳定排序），默认 `asc`（"排成有序"原型）
//! - 两者默认方向不同：rank 的 desc 是 Top-K 语义；sort 的 asc 是对齐 SQL ORDER BY /
//!   Unix sort / Python sorted 的 LLM 预训练强先验
//!
//! # --verify 对齐
//!
//! 确定性算法（无随机种子），暴力路径与优化路径走同一 `sort_by`，100% 对齐。
//! 比 `sample` 更简单——sample 靠固定种子保证对齐，sort 天然对齐。

use std::cmp::Ordering;

use super::pseudo_field_value;
use super::rank::{cmp_keys, RankKey};
use super::MatchedLine;
use crate::l2::parse_numeric_value;

/// 执行 `sort [by FIELD] [desc|asc]`：全量稳定排序。
///
/// - `by=None`：按整行字节序
/// - `by=Some(field)`：按字段值（数值优先，非数值/缺失沉底）
/// - `desc=true` 降序，`false` 升序（默认 asc）
///
/// 稳定排序：相等键保持原行顺序（与 SQL ORDER BY 一致）。
/// 非数值字段（含缺失）无论升降序都排最后（`Bytes` 键，对齐 rank 语义）。
///
/// # 算法
///
/// `slice::sort_by` 稳定排序，O(N log N)。
/// - `by=Some(field)`：每行预提取 `RankKey`（数值优先/Bytes 兜底），同键靠原下标稳定
/// - `by=None`：直接按整行字节序比较（聚集相同错误，对齐 Unix sort 默认）
pub fn apply_sort(matched: Vec<MatchedLine>, by: Option<&[u8]>, desc: bool) -> Vec<MatchedLine> {
    if matched.len() <= 1 {
        return matched;
    }

    match by {
        None => {
            // 整行字节序：直接按行字节比较，同行靠原下标稳定
            let mut keyed: Vec<(usize, MatchedLine)> = matched.into_iter().enumerate().collect();
            keyed.sort_by(|a, b| {
                let ord = a.1.line.cmp(&b.1.line);
                let ord = if desc { ord.reverse() } else { ord };
                if ord == Ordering::Equal {
                    a.0.cmp(&b.0) // 稳定
                } else {
                    ord
                }
            });
            keyed.into_iter().map(|(_, m)| m).collect()
        }
        Some(field) => {
            // 字段值排序：数值优先，非数值沉底
            // v0.42 修复：用 pseudo_field_value 替代直接 find_value，
            // 支持 _match/_line/_file 伪字段（与 project.rs 语义对齐）。
            // 原 bug：find_value 只特判 _ts/_tag，对 _match 走 logfmt 找 `_match=`
            // 显然找不到 → 返回 None → RankKey::Bytes → 同键稳定排序保持原序。
            //
            // v0.42 修复（续）：captures 优先——capture(NAME, /REGEX/) 提取的值
            // 应优先于行内同名字段（与 project.rs / count.rs / percentiles.rs 对齐）。
            // 原 bug：`capture(id, /(\w+)/) | sort by id` 不排序——capture 值被忽略，
            // pseudo_field_value 走 find_value 在行内找 `id=` 找不到 → None → Bytes。
            let field_str = String::from_utf8_lossy(field).into_owned();
            let mut keyed: Vec<(RankKey, usize, MatchedLine)> = matched
                .into_iter()
                .enumerate()
                .map(|(i, m)| {
                    let val = m
                        .captures
                        .get(&field_str)
                        .cloned()
                        .or_else(|| pseudo_field_value(&m, field));
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
            keyed.sort_by(|a, b| {
                let key_ord = cmp_keys(&a.0, &b.0, desc);
                if key_ord == Ordering::Equal {
                    a.1.cmp(&b.1)
                } else {
                    key_ord
                }
            });
            keyed.into_iter().map(|(_, _, m)| m).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- by FIELD ---

    #[test]
    fn test_sort_by_field_asc_numeric() {
        let m = lines(&["cost=100", "cost=900", "cost=500", "cost=700", "cost=300"]);
        let out = apply_sort(m, Some(b"cost"), false);
        // 升序：100, 300, 500, 700, 900
        assert_eq!(
            texts(&out),
            vec!["cost=100", "cost=300", "cost=500", "cost=700", "cost=900"]
        );
    }

    #[test]
    fn test_sort_by_field_desc_numeric() {
        let m = lines(&["cost=100", "cost=900", "cost=500", "cost=700", "cost=300"]);
        let out = apply_sort(m, Some(b"cost"), true);
        // 降序：900, 700, 500, 300, 100
        assert_eq!(
            texts(&out),
            vec!["cost=900", "cost=700", "cost=500", "cost=300", "cost=100"]
        );
    }

    #[test]
    fn test_sort_numeric_parsing_follows_11_dim_contract() {
        // 前导零 / 正号 / 科学计数法都按数值排序
        let m = lines(&["cost=1e3", "cost=0950", "cost=900"]);
        let out = apply_sort(m, Some(b"cost"), false);
        // 升序：900, 950(0950), 1000(1e3)
        assert_eq!(texts(&out), vec!["cost=900", "cost=0950", "cost=1e3"]);
    }

    // --- 整行字节序（by=None）---

    #[test]
    fn test_sort_by_line_bytes_asc_default() {
        let m = lines(&["c", "a", "b", "a", "b"]);
        let out = apply_sort(m, None, false);
        // 字节序升序：a, a, b, b, c（稳定：相同字符保持原顺序）
        assert_eq!(texts(&out), vec!["a", "a", "b", "b", "c"]);
    }

    #[test]
    fn test_sort_by_line_bytes_desc() {
        let m = lines(&["a", "c", "b"]);
        let out = apply_sort(m, None, true);
        assert_eq!(texts(&out), vec!["c", "b", "a"]);
    }

    #[test]
    fn test_sort_aggregates_identical_errors() {
        // 场景 2：聚集相同错误行（Unix sort | uniq -c 经典模式）
        let m = lines(&[
            "ERROR connection refused",
            "ERROR timeout",
            "ERROR connection refused",
            "WARN disk full",
            "ERROR connection refused",
        ]);
        let out = apply_sort(m, None, false);
        // 字节序：相同错误消息相邻
        assert_eq!(
            texts(&out),
            vec![
                "ERROR connection refused",
                "ERROR connection refused",
                "ERROR connection refused",
                "ERROR timeout",
                "WARN disk full"
            ]
        );
    }

    // --- 非数值/缺失字段沉底 ---

    #[test]
    fn test_sort_non_numeric_falls_to_bottom_regardless_of_direction() {
        // v0.42 语义修正：非数值字段按字节序比较（原"全部同键"是 bug）
        // 非数值（含缺失）无论升降序都排最后（数值优先），但非数值间按字节序排序
        let m = lines(&["cost=500", "cost=abc", "other=x", "cost=100", "cost=xyz"]);
        // 升序：数值 100, 500 在前；非数值按字节序升序：""(other) < abc < xyz
        let out_asc = apply_sort(m.clone(), Some(b"cost"), false);
        assert_eq!(
            texts(&out_asc),
            vec!["cost=100", "cost=500", "other=x", "cost=abc", "cost=xyz"]
        );
        // 降序：数值 500, 100 在前；非数值按字节序降序：xyz > abc > ""
        let out_desc = apply_sort(m, Some(b"cost"), true);
        assert_eq!(
            texts(&out_desc),
            vec!["cost=500", "cost=100", "cost=xyz", "cost=abc", "other=x"]
        );
    }

    // --- 稳定性 ---

    #[test]
    fn test_sort_stable_for_equal_numeric_keys() {
        // 相同数值保持原顺序（稳定排序）
        let m = lines(&["cost=900 a", "cost=900 b", "cost=100 c"]);
        let out = apply_sort(m, Some(b"cost"), false);
        assert_eq!(texts(&out), vec!["cost=100 c", "cost=900 a", "cost=900 b"]);
    }

    // --- 边界 ---

    #[test]
    fn test_sort_empty_input() {
        let out = apply_sort(Vec::new(), Some(b"cost"), false);
        assert!(out.is_empty());
    }

    #[test]
    fn test_sort_single_element_unchanged() {
        let m = lines(&["cost=100"]);
        let out = apply_sort(m, Some(b"cost"), false);
        assert_eq!(texts(&out), vec!["cost=100"]);
    }

    #[test]
    fn test_sort_all_equal_keys_preserves_original_order() {
        let m = lines(&["x=1 a", "x=1 b", "x=1 c"]);
        let out = apply_sort(m, Some(b"x"), false);
        assert_eq!(texts(&out), vec!["x=1 a", "x=1 b", "x=1 c"]);
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

    fn lines_with_path(rows: &[(&str, &str)]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, (s, p))| {
                let mut m = MatchedLine::new(i + 1, s.as_bytes());
                m.path = Some(p.to_string());
                m
            })
            .collect()
    }

    #[test]
    fn test_sort_by_match_pseudo_field_asc() {
        // _match = L1 命中字节（等价 grep -o），按字节序升序
        let m = lines_with_match(&[
            ("error b", Some(b"error")),
            ("error a", Some(b"error")),
            ("error c", Some(b"error")),
        ]);
        // _match 都是 "error" → 同键稳定保持原序
        let out = apply_sort(m, Some(b"_match"), false);
        assert_eq!(texts(&out), vec!["error b", "error a", "error c"]);
    }

    #[test]
    fn test_sort_by_match_pseudo_field_different_values() {
        // OR 查询：每行 _match 不同（"banana"/"apple"/"cherry"）
        let m = lines_with_match(&[
            ("banana", Some(b"banana")),
            ("apple", Some(b"apple")),
            ("cherry", Some(b"cherry")),
        ]);
        let out = apply_sort(m, Some(b"_match"), false);
        // 字节序升序：apple < banana < cherry
        assert_eq!(texts(&out), vec!["apple", "banana", "cherry"]);
        let m = lines_with_match(&[
            ("banana", Some(b"banana")),
            ("apple", Some(b"apple")),
            ("cherry", Some(b"cherry")),
        ]);
        let out = apply_sort(m, Some(b"_match"), true);
        // 字节序降序：cherry > banana > apple
        assert_eq!(texts(&out), vec!["cherry", "banana", "apple"]);
    }

    #[test]
    fn test_sort_by_line_pseudo_field_asc() {
        // _line = 整行内容，按字节序升序
        let m = lines(&["banana", "apple", "cherry"]);
        let out = apply_sort(m, Some(b"_line"), false);
        assert_eq!(texts(&out), vec!["apple", "banana", "cherry"]);
    }

    #[test]
    fn test_sort_by_line_pseudo_field_desc() {
        let m = lines(&["banana", "apple", "cherry"]);
        let out = apply_sort(m, Some(b"_line"), true);
        assert_eq!(texts(&out), vec!["cherry", "banana", "apple"]);
    }

    #[test]
    fn test_sort_by_file_pseudo_field_asc() {
        let m = lines_with_path(&[("x", "src/b.rs"), ("x", "src/a.rs"), ("x", "src/c.rs")]);
        let out = apply_sort(m, Some(b"_file"), false);
        // 按 path 字节序升序：a < b < c
        let paths: Vec<&str> = out.iter().map(|m| m.path.as_deref().unwrap()).collect();
        assert_eq!(paths, vec!["src/a.rs", "src/b.rs", "src/c.rs"]);
    }

    #[test]
    fn test_sort_by_file_pseudo_field_desc() {
        let m = lines_with_path(&[("x", "src/b.rs"), ("x", "src/a.rs"), ("x", "src/c.rs")]);
        let out = apply_sort(m, Some(b"_file"), true);
        let paths: Vec<&str> = out.iter().map(|m| m.path.as_deref().unwrap()).collect();
        assert_eq!(paths, vec!["src/c.rs", "src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn test_sort_by_match_when_match_bytes_none() {
        // 无 match_bytes（纯字段谓词查询）→ 空 Vec → 全部同键稳定
        let m = lines(&["a=100", "a=200"]);
        let out = apply_sort(m, Some(b"_match"), false);
        // 全部空 Vec → 同键稳定保持原序
        assert_eq!(texts(&out), vec!["a=100", "a=200"]);
    }

    // --- captures 优先（v0.42 修复：与 project/count/percentiles 对齐）---

    #[test]
    fn test_sort_by_field_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段
        let mut m1 = MatchedLine::new(1, b"id=zzz");
        m1.captures.insert("id".to_string(), b"abc".to_vec());
        let mut m2 = MatchedLine::new(2, b"id=xxx");
        m2.captures.insert("id".to_string(), b"def".to_vec());
        let m = vec![m1, m2];
        let out = apply_sort(m, Some(b"id"), false);
        // 按 capture 值字节序升序：abc < def
        assert_eq!(texts(&out), vec!["id=zzz", "id=xxx"]);
        // line_no 应反映排序后的顺序
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn test_sort_by_field_capture_when_no_line_field() {
        // 行内无 id 字段，capture 提取的值仍可用
        let mut m1 = MatchedLine::new(1, b"ERROR abc");
        m1.captures.insert("id".to_string(), b"def".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR def");
        m2.captures.insert("id".to_string(), b"abc".to_vec());
        let m = vec![m1, m2];
        let out = apply_sort(m, Some(b"id"), false);
        // 按 capture 值字节序升序：abc < def → 第 2 行在前
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn test_sort_by_field_capture_mixed_with_line_field() {
        // 混合：有 capture 的按 capture 排，无 capture 的按行内字段排
        let mut m1 = MatchedLine::new(1, b"id=zzz");
        m1.captures.insert("id".to_string(), b"abc".to_vec());
        let m2 = MatchedLine::new(2, b"id=mmm"); // 无 capture，按行内 id=mmm
        let m = vec![m1, m2];
        let out = apply_sort(m, Some(b"id"), false);
        // m1 capture=abc, m2 line=mmm → 升序：abc < mmm
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![1, 2]
        );
        // 反向：desc → mmm > abc
        let mut m1 = MatchedLine::new(1, b"id=zzz");
        m1.captures.insert("id".to_string(), b"abc".to_vec());
        let m2 = MatchedLine::new(2, b"id=mmm");
        let m = vec![m1, m2];
        let out = apply_sort(m, Some(b"id"), true);
        assert_eq!(
            out.iter().map(|m| m.line_no).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }
}
