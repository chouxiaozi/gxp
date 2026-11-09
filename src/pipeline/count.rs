//! `count` / `group by field`：行计数（等价 `-c`）。
//!
//! 设计文档 §2.4 示例 9：`gxp 'ERROR app.log | count'` ≡ `grep -c ERROR app.log`。
//!
//! - `count`：输出命中行总数
//! - `group by FIELD`：按字段值分组计数（设计文档 §2.2 EBNF `group by`，独立动词）

use super::MatchedLine;

/// 执行 `count`：返回命中行总数。
pub fn apply_count(matched: &[MatchedLine]) -> usize {
    matched.len()
}

/// 执行 `group by FIELD`：按字段值分组计数。
///
/// 返回 `(字段值字节, 计数)` 列表，按首次出现顺序。字段缺失的行归入 `(空, 计数)`。
///
/// 字段可见性与 `fields` 投影一致：captures 优先，fallback 到行内 `find_value`
/// （对齐 `project.rs` 的 captures 优先逻辑，B2 修复）。
pub fn apply_count_group_by(matched: &[MatchedLine], field: &[u8]) -> Vec<(Vec<u8>, usize)> {
    let field_str = String::from_utf8_lossy(field).into_owned();
    let mut order: Vec<Vec<u8>> = Vec::new();
    let mut counts: Vec<(Vec<u8>, usize)> = Vec::new();
    for m in matched {
        // v0.42 修复：伪字段 _match/_line/_file 走 pseudo_field_value
        //（原直接调 find_value 只特判 _ts/_tag，对 _match 走 logfmt 找不到 → 空串归组）
        // captures 优先（与 fields 投影一致），fallback 到 pseudo_field_value
        let val = m
            .captures
            .get(&field_str)
            .cloned()
            .or_else(|| super::pseudo_field_value(m, field))
            .unwrap_or_default();
        match counts.iter_mut().find(|(k, _)| *k == val) {
            Some((_, n)) => *n += 1,
            None => {
                order.push(val.clone());
                counts.push((val, 1));
            }
        }
    }
    // 按首次出现顺序重排（order 保证）
    let mut out = Vec::with_capacity(order.len());
    for key in order {
        if let Some((_, n)) = counts.iter().find(|(k, _)| *k == key) {
            out.push((key, *n));
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

    #[test]
    fn test_count_matches() {
        let m = lines(&["a", "b", "c"]);
        assert_eq!(apply_count(&m), 3);
    }

    #[test]
    fn test_count_zero() {
        assert_eq!(apply_count(&[]), 0);
    }

    #[test]
    fn test_count_group_by_single_field() {
        let m = lines(&[
            "svc=checkout cost=1",
            "svc=billing cost=2",
            "svc=checkout cost=3",
        ]);
        let g = apply_count_group_by(&m, b"svc");
        assert_eq!(g, vec![(b"checkout".to_vec(), 2), (b"billing".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_missing_field_goes_empty() {
        let m = lines(&["svc=checkout", "no field here", "svc=billing"]);
        let g = apply_count_group_by(&m, b"svc");
        // 缺失字段归入空串
        assert_eq!(
            g,
            vec![
                (b"checkout".to_vec(), 1),
                (b"".to_vec(), 1),
                (b"billing".to_vec(), 1)
            ]
        );
    }

    // B2 修复：captures 优先 + 行内 fallback（对齐 fields 投影）

    #[test]
    fn test_count_group_by_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段（与 fields 投影一致）
        let mut m = MatchedLine::new(1, b"svc=checkout");
        m.captures.insert("svc".to_string(), b"billing".to_vec());
        let g = apply_count_group_by(&[m], b"svc");
        // capture 值 billing 优先于行内 svc=checkout
        assert_eq!(g, vec![(b"billing".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_capture_when_no_line_field() {
        // 行内无 svc 字段，capture 提取的值仍可用
        let mut m = MatchedLine::new(1, b"ERROR no field");
        m.captures.insert("svc".to_string(), b"checkout".to_vec());
        let g = apply_count_group_by(&[m], b"svc");
        assert_eq!(g, vec![(b"checkout".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_capture_fallback_to_line_field() {
        // 无 capture 时 fallback 到行内字段
        let m = lines(&["svc=checkout", "svc=billing", "svc=checkout"]);
        let g = apply_count_group_by(&m, b"svc");
        assert_eq!(g, vec![(b"checkout".to_vec(), 2), (b"billing".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_capture_and_line_field_mixed() {
        // 混合：有 capture 的行按 capture 分组，无 capture 的按行内字段分组
        let mut m1 = MatchedLine::new(1, b"svc=checkout");
        m1.captures.insert("svc".to_string(), b"checkout".to_vec());
        let m2 = MatchedLine::new(2, b"svc=checkout");
        // m2 无 capture，fallback 到行内 svc=checkout
        let g = apply_count_group_by(&[m1, m2], b"svc");
        // 两者都归入 checkout 组
        assert_eq!(g, vec![(b"checkout".to_vec(), 2)]);
    }

    // --- 伪字段 _match / _line / _file（v0.42 修复）---

    fn lines_with_match(rows: &[(&str, Option<&[u8]>)]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, (s, mb))| {
                let mut m = MatchedLine::new(i + 1, s.as_bytes());
                m.match_bytes = mb.map(std::sync::Arc::from);
                m
            })
            .collect()
    }

    fn lines_with_path(rows: &[(&str, &str)]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, (s, p))| {
                let mut m = MatchedLine::new(i + 1, s.as_bytes());
                m.path = Some(std::sync::Arc::from(*p));
                m
            })
            .collect()
    }

    #[test]
    fn test_count_group_by_match_pseudo_field() {
        let m = lines_with_match(&[
            ("banana", Some(b"banana")),
            ("apple", Some(b"apple")),
            ("banana", Some(b"banana")),
        ]);
        let g = apply_count_group_by(&m, b"_match");
        // banana: 2, apple: 1（按首次出现顺序）
        assert_eq!(g, vec![(b"banana".to_vec(), 2), (b"apple".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_line_pseudo_field() {
        let m = lines(&["banana", "apple", "banana"]);
        let g = apply_count_group_by(&m, b"_line");
        assert_eq!(g, vec![(b"banana".to_vec(), 2), (b"apple".to_vec(), 1)]);
    }

    #[test]
    fn test_count_group_by_file_pseudo_field() {
        let m = lines_with_path(&[("x", "src/a.rs"), ("x", "src/b.rs"), ("x", "src/a.rs")]);
        let g = apply_count_group_by(&m, b"_file");
        // a.rs: 2, b.rs: 1
        assert_eq!(
            g,
            vec![(b"src/a.rs".to_vec(), 2), (b"src/b.rs".to_vec(), 1)]
        );
    }

    #[test]
    fn test_count_group_by_match_when_match_bytes_none() {
        // 无 match_bytes → 空 Vec 同组
        let m = lines(&["a=1", "a=2", "a=3"]);
        let g = apply_count_group_by(&m, b"_match");
        // 全部归入空串组
        assert_eq!(g, vec![(b"".to_vec(), 3)]);
    }
}
