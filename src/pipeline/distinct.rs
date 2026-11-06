//! `distinct [by FIELD]`：稳定去重投影（§9.1 候选 #3 落地）。
//!
//! 设计文档 §2.2 EBNF `stage` 规则；§2.4 示例。
//!
//! # 语义
//!
//! - `distinct`：整行字节去重，保留首次出现的行（对齐 Unix `sort | uniq`，但不排序）
//! - `distinct by FIELD`：按字段值去重，每个字段值只留首次出现的行
//!   （对齐 SQL `SELECT DISTINCT ON (field) *`，字段缺失的行归入空串键）
//!
//! # 与其他动词区别
//!
//! - `group by FIELD`：输出 `value: N`（计数），不输出原始行
//! - `distinct by FIELD`：输出去重后的行（保留原始字段/整行内容）
//! - `sort`：改变顺序（聚集相同键相邻），`distinct` 删除重复（保持原序）
//!
//! 稳定去重：相同键只保留首次出现，后续删除（对齐 SQL DISTINCT 语义）。
//!
//! # --verify 对齐
//!
//! 确定性算法（基于 HashSet 的顺序扫描），暴力路径与优化路径走同一逻辑，100% 对齐。

use std::collections::HashSet;

use super::pseudo_field_value;
use super::MatchedLine;

/// 执行 `distinct [by FIELD]`：稳定去重。
///
/// - `by=None`：整行字节去重
/// - `by=Some(field)`：按字段值去重（字段缺失归入空串键）
///
/// 稳定：相同键只保留首次出现的行，后续行删除。
/// 用 `HashSet<Vec<u8>>` 记录已见键，O(N) 平均时间复杂度。
///
/// v0.42 修复：用 pseudo_field_value 替代直接 find_value，
/// 支持 _match/_line/_file 伪字段（与 project/sort/rank 语义对齐）。
///
/// 补 captures 优先逻辑——原只调 pseudo_field_value，
/// `capture(NAME, /REGEX/) | distinct by NAME` 不去重 capture 值，
/// 退化成"按行内字段去重"（行内无 NAME 字段则全归空串键，只留首条）。
/// 与 count.rs / sort.rs / rank.rs / project.rs / percentiles.rs 对齐：
/// captures.get(field) 优先，fallback 到 pseudo_field_value。
pub fn apply_distinct(matched: Vec<MatchedLine>, by: Option<&[u8]>) -> Vec<MatchedLine> {
    let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(matched.len());
    let mut out: Vec<MatchedLine> = Vec::with_capacity(matched.len());
    for m in matched {
        let key: Vec<u8> = match by {
            None => m.line.as_ref().to_vec(),
            Some(field) => {
                let field_str = String::from_utf8_lossy(field).into_owned();
                m.captures
                    .get(&field_str)
                    .cloned()
                    .or_else(|| pseudo_field_value(&m, field))
                    .unwrap_or_default()
            }
        };
        if seen.insert(key) {
            out.push(m);
        }
    }
    out
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

    fn line_nos(lines: &[MatchedLine]) -> Vec<usize> {
        lines.iter().map(|m| m.line_no).collect()
    }

    // --- distinct（整行去重）---

    #[test]
    fn test_distinct_whole_line_keeps_first_occurrence() {
        let m = lines(&["a", "b", "a", "c", "b"]);
        let out = apply_distinct(m, None);
        // 保留首次出现：a(1), b(2), c(4)
        assert_eq!(texts(&out), vec!["a", "b", "c"]);
        assert_eq!(line_nos(&out), vec![1, 2, 4]);
    }

    #[test]
    fn test_distinct_whole_line_all_unique_unchanged() {
        let m = lines(&["x", "y", "z"]);
        let out = apply_distinct(m, None);
        assert_eq!(texts(&out), vec!["x", "y", "z"]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_distinct_whole_line_all_same_keeps_first() {
        let m = lines(&["dup", "dup", "dup"]);
        let out = apply_distinct(m, None);
        assert_eq!(texts(&out), vec!["dup"]);
        assert_eq!(line_nos(&out), vec![1]);
    }

    #[test]
    fn test_distinct_empty_input() {
        let out = apply_distinct(Vec::new(), None);
        assert!(out.is_empty());
    }

    #[test]
    fn test_distinct_single_element() {
        let m = lines(&["only"]);
        let out = apply_distinct(m, None);
        assert_eq!(texts(&out), vec!["only"]);
    }

    // --- distinct by FIELD（字段去重）---

    #[test]
    fn test_distinct_by_field_keeps_first_per_field_value() {
        let m = lines(&[
            "user=alice cost=1",
            "user=bob cost=2",
            "user=alice cost=3",
            "user=bob cost=4",
            "user=alice cost=5",
        ]);
        let out = apply_distinct(m, Some(b"user"));
        // 每个 user 首次出现的行
        assert_eq!(texts(&out), vec!["user=alice cost=1", "user=bob cost=2"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }

    #[test]
    fn test_distinct_by_field_missing_field_groups_into_empty_key() {
        // 字段缺失的行归入空串键，多条缺失只留首条
        let m = lines(&[
            "user=alice",
            "no field here",
            "user=bob",
            "also missing",
            "user=alice again",
        ]);
        let out = apply_distinct(m, Some(b"user"));
        // 首次：alice(1), 空串(2), bob(3)；后两条删
        assert_eq!(texts(&out), vec!["user=alice", "no field here", "user=bob"]);
        assert_eq!(line_nos(&out), vec![1, 2, 3]);
    }

    #[test]
    fn test_distinct_by_field_all_unique_values() {
        let m = lines(&["k=1", "k=2", "k=3"]);
        let out = apply_distinct(m, Some(b"k"));
        assert_eq!(texts(&out), vec!["k=1", "k=2", "k=3"]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_distinct_by_field_preserves_original_order() {
        // 保留原序：先 alice 后 bob 后 alice → alice, bob
        let m = lines(&["u=b x", "u=a y", "u=b z", "u=a w"]);
        let out = apply_distinct(m, Some(b"u"));
        // 首次 b(1), a(2)，后两条删
        assert_eq!(texts(&out), vec!["u=b x", "u=a y"]);
    }

    #[test]
    fn test_distinct_by_field_treats_values_as_bytes_not_numeric() {
        // 字段值按字节去重，不做数值归一（"0950" ≠ "950"）
        let m = lines(&["c=0950", "c=950", "c=0950"]);
        let out = apply_distinct(m, Some(b"c"));
        // 0950(1), 950(2) 是不同字节串 → 都保留；第三条 0950 与第一条同 → 删
        assert_eq!(texts(&out), vec!["c=0950", "c=950"]);
    }

    // --- 稳定性 ---

    #[test]
    fn test_distinct_stable_keeps_first_on_ties() {
        // 相同键时保留首次出现的行（稳定）
        let m = lines(&["v=1 first", "v=1 second", "v=1 third"]);
        let out = apply_distinct(m, Some(b"v"));
        // 只留首条
        assert_eq!(texts(&out), vec!["v=1 first"]);
        assert_eq!(line_nos(&out), vec![1]);
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
    fn test_distinct_by_match_pseudo_field() {
        // _match = L1 命中字节，相同 _match 只留首条
        let m = lines_with_match(&[
            ("banana line1", Some(b"banana")),
            ("apple line2", Some(b"apple")),
            ("banana line3", Some(b"banana")), // _match 同 banana → 删
        ]);
        let out = apply_distinct(m, Some(b"_match"));
        assert_eq!(texts(&out), vec!["banana line1", "apple line2"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }

    #[test]
    fn test_distinct_by_line_pseudo_field() {
        // _line = 整行内容，相同行只留首条
        let m = lines(&["dup", "unique", "dup", "other"]);
        let out = apply_distinct(m, Some(b"_line"));
        assert_eq!(texts(&out), vec!["dup", "unique", "other"]);
        assert_eq!(line_nos(&out), vec![1, 2, 4]);
    }

    #[test]
    fn test_distinct_by_file_pseudo_field() {
        // _file = 文件路径，相同路径只留首条
        let m = lines_with_path(&[
            ("x", "src/a.rs"),
            ("x", "src/b.rs"),
            ("x", "src/a.rs"), // 同 a.rs → 删
        ]);
        let out = apply_distinct(m, Some(b"_file"));
        assert_eq!(texts(&out), vec!["x", "x"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }

    #[test]
    fn test_distinct_by_match_when_match_bytes_none() {
        // 无 match_bytes（纯字段谓词）→ 空 Vec 同键 → 只留首条
        let m = lines(&["a=1", "a=2", "a=3"]);
        let out = apply_distinct(m, Some(b"_match"));
        // 全部空 Vec → 只留首条
        assert_eq!(texts(&out), vec!["a=1"]);
        assert_eq!(line_nos(&out), vec![1]);
    }

    // --- captures 优先（与 count/sort/rank/project/percentiles 对齐）---

    #[test]
    fn test_distinct_by_field_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段
        // 行 1: id=line_zzz, capture=aaa；行 2: id=line_yyy, capture=bbb
        // distinct by id 应按 capture 值去重 → 两条都保留（capture 值不同）
        let mut m1 = MatchedLine::new(1, b"id=line_zzz");
        m1.captures.insert("id".to_string(), b"aaa".to_vec());
        let mut m2 = MatchedLine::new(2, b"id=line_yyy");
        m2.captures.insert("id".to_string(), b"bbb".to_vec());
        let out = apply_distinct(vec![m1, m2], Some(b"id"));
        // capture 值 aaa, bbb 不同 → 两条都保留
        assert_eq!(texts(&out), vec!["id=line_zzz", "id=line_yyy"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }

    #[test]
    fn test_distinct_by_field_capture_dedups_when_capture_repeats() {
        // 3 行 capture 值相同 → 只留首条（capture 去重）
        let mut m1 = MatchedLine::new(1, b"ERROR first");
        m1.captures
            .insert("uid".to_string(), b"user=alice".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR second");
        m2.captures
            .insert("uid".to_string(), b"user=alice".to_vec());
        let mut m3 = MatchedLine::new(3, b"ERROR third");
        m3.captures
            .insert("uid".to_string(), b"user=alice".to_vec());
        let out = apply_distinct(vec![m1, m2, m3], Some(b"uid"));
        // 3 行 capture 同 user=alice → 只留首条
        assert_eq!(texts(&out), vec!["ERROR first"]);
        assert_eq!(line_nos(&out), vec![1]);
    }

    #[test]
    fn test_distinct_by_field_capture_when_no_line_field() {
        // 行内无 uid 字段，capture 提取的值仍可用作 distinct 键
        let mut m1 = MatchedLine::new(1, b"ERROR abc");
        m1.captures
            .insert("uid".to_string(), b"user=alice".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR def");
        m2.captures.insert("uid".to_string(), b"user=bob".to_vec());
        let out = apply_distinct(vec![m1, m2], Some(b"uid"));
        // 按 capture 值去重：alice, bob 不同 → 两条都保留
        assert_eq!(texts(&out), vec!["ERROR abc", "ERROR def"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }

    #[test]
    fn test_distinct_by_field_capture_mixed_with_line_field() {
        // 混合：有 capture 的按 capture 去重，无 capture 的按行内字段去重
        let mut m1 = MatchedLine::new(1, b"id=zzz");
        m1.captures.insert("id".to_string(), b"aaa".to_vec());
        // m2 无 capture，按行内 id=mmm
        let m2 = MatchedLine::new(2, b"id=mmm");
        // m3 行内 id=aaa，但行内字段去重（无 capture） → 与 m1 的 capture=aaa 字节相同
        // m1 capture=aaa，m3 行内 id=aaa → 应判为重复（同 key）→ 删 m3
        let m3 = MatchedLine::new(3, b"id=aaa");
        let out = apply_distinct(vec![m1, m2, m3], Some(b"id"));
        // m1: aaa（capture）, m2: mmm（行内）, m3: aaa（行内，与 m1 同键 → 删）
        assert_eq!(texts(&out), vec!["id=zzz", "id=mmm"]);
        assert_eq!(line_nos(&out), vec![1, 2]);
    }
}
