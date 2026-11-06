//! `head N`：限制输出行数（等价 `head -n`）。
//!
//! 设计文档 §2.2 EBNF `stage` 规则：`head N` 取前 N 条命中行。

/// 执行 `head N`：截断命中行到前 `n` 条。
///
/// 语义：保留前 `n` 条（原顺序），`n` 大于总数时保留全部。
pub fn apply_head(matched: Vec<super::MatchedLine>, n: usize) -> Vec<super::MatchedLine> {
    let mut out = matched;
    out.truncate(n);
    out
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

    #[test]
    fn test_head_takes_first_n() {
        let m = lines(&["a", "b", "c", "d"]);
        let out = apply_head(m, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(&*out[0].line, b"a");
        assert_eq!(&*out[1].line, b"b");
    }

    #[test]
    fn test_head_n_larger_than_total_keeps_all() {
        let m = lines(&["a", "b"]);
        let out = apply_head(m, 5);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_head_zero_returns_empty() {
        let m = lines(&["a", "b"]);
        let out = apply_head(m, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn test_head_empty_input() {
        let out = apply_head(Vec::new(), 3);
        assert!(out.is_empty());
    }
}
