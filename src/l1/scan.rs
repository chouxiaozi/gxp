//! L1 行扫描入口：方向 B 两遍链（设计文档 §3.2）。
//!
//! 大文件流式按行扫描（`memchr` 定位 `\n`），每行调用 `CompiledL1::l1_match`
//! 做粗筛。命中行号收集返回，供 L2 精判消费。
//!
//! 不变式：`L1(粗筛) ⊇ L2(精判)`——`scan_lines` 返回的行号集合是 L2 精判的超集，
//! 绝不漏答案，可能多送候选。

use std::io::{BufRead, BufReader};
use std::path::Path;

use memchr::memchr;

use super::compiler::CompiledL1;

/// L1 行扫描：返回所有命中 L1 粗筛的行号（1-based）。
///
/// 行号从 1 开始（与 grep -n 一致）。空行（只有 `\n`）也计入行号。
/// 末尾无 `\n` 的最后一行也作为完整行处理。
///
/// 不变式：返回的行号集合 ⊇ L2 精判命中集合。
pub fn scan_lines(compiled: &CompiledL1, haystack: &[u8]) -> Vec<usize> {
    let mut hits: Vec<usize> = Vec::new();
    let mut line_no: usize = 0;
    let mut start: usize = 0;
    while start < haystack.len() {
        let end = memchr(b'\n', &haystack[start..])
            .map(|i| start + i)
            .unwrap_or(haystack.len());
        line_no += 1;
        let line = &haystack[start..end];
        if compiled.l1_match(line) {
            hits.push(line_no);
        }
        // 跳过 `\n`；末尾无 `\n` 时 end == haystack.len()，循环结束
        start = end + 1;
        if end == haystack.len() {
            break;
        }
    }
    hits
}

/// L1 行扫描（回调版）：对每个命中 L1 粗筛的行调用 `on_hit(line_no, line)`。
///
/// `on_hit` 返回 `false` 时提前终止扫描（用于 `head N` 场景：收集够 N 条后停止）。
/// 返回扫描的行数（用于统计/进度报告）。
pub fn scan_lines_cb<F: FnMut(usize, &[u8]) -> bool>(
    compiled: &CompiledL1,
    haystack: &[u8],
    mut on_hit: F,
) -> usize {
    let mut scanned: usize = 0;
    let mut line_no: usize = 0;
    let mut start: usize = 0;
    while start < haystack.len() {
        let end = memchr(b'\n', &haystack[start..])
            .map(|i| start + i)
            .unwrap_or(haystack.len());
        line_no += 1;
        scanned += 1;
        let line = &haystack[start..end];
        if compiled.l1_match(line) && !on_hit(line_no, line) {
            return scanned;
        }
        start = end + 1;
        if end == haystack.len() {
            break;
        }
    }
    scanned
}

/// 流式 L1 行扫描（回调版）：从文件 `BufReader` 逐行读取，不读入全量内容。
///
/// 流式 fallback：文件 > 512MB 时使用此函数替代 `scan_lines_cb`。
/// 用 `BufReader::read_until(b'\n')` 逐行读取，每行调用 `compiled.l1_match` 粗筛。
/// 命中后调用 `on_hit(line_no, line)`，`on_hit` 返回 `false` 时提前终止。
///
/// 不变式：与 `scan_lines_cb` 相同——`L1(粗筛) ⊇ L2(精判)`。
/// 行号语义：1-based，与 grep -n 一致。末尾无 `\n` 的最后一行也处理。
///
/// **注意**：`on_hit` 接收的 `line` 不含末尾 `\n`（已 trim），
/// 与 `scan_lines_cb` 中 `memchr` 切行语义一致。
pub fn scan_stream_lines_cb<F: FnMut(usize, &[u8]) -> bool>(
    compiled: &CompiledL1,
    path: &Path,
    mut on_hit: F,
) -> std::io::Result<usize> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    let mut line_no: usize = 0;
    let mut scanned: usize = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);

    loop {
        buf.clear();
        let bytes_read = reader.read_until(b'\n', &mut buf)?;
        if bytes_read == 0 {
            break;
        }
        line_no += 1;
        scanned += 1;
        // 去掉末尾 \n（与 scan_lines_cb 的 memchr 切行语义一致）
        let line = strip_trailing_newline(&buf);
        if compiled.l1_match(line) && !on_hit(line_no, line) {
            return Ok(scanned);
        }
    }
    Ok(scanned)
}

/// 去掉末尾 `\n`（和 `\r\n`），与 `scan_lines_cb` 的 `memchr` 切行语义一致。
///
/// 公共辅助函数：`scan_stream_lines_cb`、`bare_scan_stream_lines`、
/// `apply_context_streaming` 三处流式路径共用，保证行截断语义一致。
#[inline]
pub(crate) fn strip_trailing_newline(buf: &[u8]) -> &[u8] {
    let len = buf.len();
    if len == 0 {
        return buf;
    }
    if buf[len - 1] == b'\n' {
        let trim_len = if len >= 2 && buf[len - 2] == b'\r' {
            len - 2
        } else {
            len - 1
        };
        &buf[..trim_len]
    } else {
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::ast::{CmpOp, Expr, Prim};
    use crate::l1::compiler::compile;

    /// 构造 `Prim::String(s)` 表达式。
    fn expr_str(s: &str) -> Expr {
        Expr::Prim(Prim::String(s.to_string()))
    }

    /// 构造 `Prim::Pred(Compare)` 表达式。
    fn expr_compare(name: &str, op: CmpOp, value: &str) -> Expr {
        Expr::Prim(Prim::Pred(crate::dsl::ast::Pred::Compare {
            name: name.to_string(),
            op,
            value: value.to_string(),
        }))
    }

    // --- scan_lines 基本场景 ---

    #[test]
    fn test_scan_lines_returns_1based_line_numbers() {
        // 3 行，第 2 行命中 "error"
        let hay = b"info line\nerror line\nwarn line";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        // 行号从 1 开始，第 2 行命中
        assert_eq!(hits, vec![2]);
    }

    #[test]
    fn test_scan_lines_multiple_hits() {
        // 4 行，第 1 和 3 行命中 "log"
        let hay = b"log line 1\nno match\nlog line 3\nwarn";
        let expr = expr_str("log");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert_eq!(hits, vec![1, 3]);
    }

    #[test]
    fn test_scan_lines_no_hits() {
        let hay = b"info\nwarn\ndebug";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_scan_lines_empty_haystack() {
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, b"");
        assert!(hits.is_empty());
    }

    #[test]
    fn test_scan_lines_trailing_newline() {
        // 末尾有 \n：3 行（第 3 行为空行）
        let hay = b"error line\ninfo\n\n";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert_eq!(hits, vec![1]);
    }

    #[test]
    fn test_scan_lines_no_trailing_newline() {
        // 末尾无 \n：最后一行仍被处理
        let hay = b"info\nerror";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert_eq!(hits, vec![2]);
    }

    #[test]
    fn test_scan_lines_and_query() {
        // "error" && "timeout"：行必须同时含两者
        let hay = b"error timeout\nerror only\ntimeout only\nerror timeout again";
        let expr = Expr::And(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert_eq!(hits, vec![1, 4]);
    }

    #[test]
    fn test_scan_lines_or_query() {
        // "error" || "timeout"：行含任一即命中
        let hay = b"error line\ninfo\ntimeout line\nboth error timeout";
        let expr = Expr::Or(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        assert_eq!(hits, vec![1, 3, 4]);
    }

    #[test]
    fn test_scan_lines_field_prefix_filter() {
        // cost_ms > 800：L1 只看 cost_ms= 或 cost_ms: 前缀存在性
        let hay = b"cost_ms=900 user=a\nlatency=100 user=b\ncost_ms:50 user=c\nno fields";
        let expr = expr_compare("cost_ms", CmpOp::Gt, "800");
        let compiled = compile(&expr).expect("编译应成功");
        let hits = scan_lines(&compiled, hay);
        // 第 1 行（cost_ms=）和第 3 行（cost_ms:）命中 L1 粗筛
        assert_eq!(hits, vec![1, 3]);
    }

    // --- scan_lines_cb 回调版 ---

    #[test]
    fn test_scan_lines_cb_collects_hits() {
        let hay = b"error\ninfo\nerror";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut collected: Vec<(usize, Vec<u8>)> = Vec::new();
        let scanned = scan_lines_cb(&compiled, hay, |line_no, line| {
            collected.push((line_no, line.to_vec()));
            true
        });
        assert_eq!(scanned, 3);
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].0, 1);
        assert_eq!(collected[0].1, b"error".to_vec());
        assert_eq!(collected[1].0, 3);
        assert_eq!(collected[1].1, b"error".to_vec());
    }

    #[test]
    fn test_scan_lines_cb_early_termination() {
        // head 2 场景：收集够 2 条后提前终止
        let hay = b"error\nerror\nerror\nerror\ninfo";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut count: usize = 0;
        let mut hits: Vec<usize> = Vec::new();
        let scanned = scan_lines_cb(&compiled, hay, |line_no, _line| {
            hits.push(line_no);
            count += 1;
            count < 2 // 收够 2 条后返回 false 终止
        });
        // 提前终止：扫描到第 2 行就停（不会扫完所有 5 行）
        assert_eq!(hits, vec![1, 2]);
        assert_eq!(scanned, 2);
    }

    #[test]
    fn test_scan_lines_cb_no_hits_scans_all() {
        let hay = b"info\nwarn\ndebug";
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut hits: Vec<usize> = Vec::new();
        let scanned = scan_lines_cb(&compiled, hay, |line_no, _line| {
            hits.push(line_no);
            true
        });
        assert!(hits.is_empty());
        assert_eq!(scanned, 3);
    }

    // --- scan_stream_lines_cb 流式扫描测试 ---

    #[test]
    fn test_scan_stream_collects_hits() {
        let hay = b"error\ninfo\nerror";
        let f = write_temp(hay);
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut collected: Vec<(usize, Vec<u8>)> = Vec::new();
        let scanned = scan_stream_lines_cb(&compiled, f.path(), |line_no, line| {
            collected.push((line_no, line.to_vec()));
            true
        })
        .expect("流式扫描应成功");
        assert_eq!(scanned, 3);
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].0, 1);
        assert_eq!(collected[0].1, b"error".to_vec());
        assert_eq!(collected[1].0, 3);
        assert_eq!(collected[1].1, b"error".to_vec());
    }

    #[test]
    fn test_scan_stream_no_trailing_newline() {
        // 末尾无 \n 的最后一行也应处理
        let hay = b"info\nerror";
        let f = write_temp(hay);
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut hits: Vec<usize> = Vec::new();
        let scanned = scan_stream_lines_cb(&compiled, f.path(), |line_no, _line| {
            hits.push(line_no);
            true
        })
        .expect("流式扫描应成功");
        assert_eq!(hits, vec![2]);
        assert_eq!(scanned, 2);
    }

    #[test]
    fn test_scan_stream_empty_file() {
        let f = write_temp(b"");
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        let mut hits: Vec<usize> = Vec::new();
        let scanned = scan_stream_lines_cb(&compiled, f.path(), |line_no, _line| {
            hits.push(line_no);
            true
        })
        .expect("流式扫描应成功");
        assert!(hits.is_empty());
        assert_eq!(scanned, 0);
    }

    fn write_temp(content: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("创建临时文件失败");
        f.write_all(content).expect("写入失败");
        f.flush().expect("flush 失败");
        f
    }

    // --- 不变式 L1 ⊇ L2 ---

    #[test]
    fn test_scan_lines_l1_superset_l2() {
        // cost_ms > 800
        // L1 命中 = 所有含 cost_ms= 或 cost_ms: 的行
        // L2 命中 = cost_ms 值 > 800 的行（子集）
        let hay = b"cost_ms=900\nlatency=100\ncost_ms=50\ncost_ms:1000\nno field";
        let expr = expr_compare("cost_ms", CmpOp::Gt, "800");
        let compiled = compile(&expr).expect("编译应成功");
        let l1_hits = scan_lines(&compiled, hay);

        // L1 命中：行 1（cost_ms=900）、行 3（cost_ms=50）、行 4（cost_ms:1000）
        assert_eq!(l1_hits, vec![1, 3, 4]);

        // 模拟 L2 精判：抽取 cost_ms 值，> 800 才命中
        let lines: Vec<&[u8]> = hay.split(|&b| b == b'\n').collect();
        let l2_hits: Vec<usize> = l1_hits
            .iter()
            .copied()
            .filter(|&i| {
                let line = lines[i - 1];
                let key_eq = b"cost_ms=";
                let key_colon = b"cost_ms:";
                let val_start = line
                    .windows(key_eq.len())
                    .position(|w| w == key_eq)
                    .map(|p| p + key_eq.len())
                    .or_else(|| {
                        line.windows(key_colon.len())
                            .position(|w| w == key_colon)
                            .map(|p| p + key_colon.len())
                    });
                val_start
                    .map(|start| {
                        let end = line[start..]
                            .iter()
                            .position(|&b| !b.is_ascii_digit() && b != b'.')
                            .map(|p| start + p)
                            .unwrap_or(line.len());
                        std::str::from_utf8(&line[start..end])
                            .ok()
                            .and_then(|s| s.parse::<f64>().ok())
                            .map(|v| v > 800.0)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .collect();

        // L2 只命中行 1（900）和行 4（1000）
        assert_eq!(l2_hits, vec![1, 4]);
        // 不变式：L2 命中集 ⊆ L1 命中集
        for l2 in &l2_hits {
            assert!(l1_hits.contains(l2), "L2 命中 {} 不在 L1 命中集中", l2);
        }
    }
}
