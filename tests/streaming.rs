//! 流式扫描集成测试。
//!
//! 验证 `scan_stream_lines_cb`（BufReader 逐行扫描）与 `scan_lines_cb`
//! （memchr 全量切片扫描）结果一致——不变式 `L1(流式) == L1(全量)`。
//!
//! 同时验证 `apply_context_streaming`（二次遍历文件）与
//! `apply_context`（全量 content）结果一致。
//!
//! 注意：512MB 阈值在测试中不触发（无法创建 > 512MB 临时文件），
//! 此处直接调用流式 API 验证语义正确性。

use std::io::Write;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::{scan_lines_cb, scan_stream_lines_cb};
use gxp::pipeline::context::{apply_context, apply_context_streaming};
use gxp::pipeline::MatchedLine;

fn write_temp(content: &[u8]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("创建临时文件失败");
    f.write_all(content).expect("写入失败");
    f.flush().expect("flush 失败");
    f
}

/// 收集 scan_lines_cb 命中行（全量切片路径）
fn collect_memchr(compiled: &gxp::l1::compiler::CompiledL1, bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let mut hits = Vec::new();
    scan_lines_cb(compiled, bytes, |line_no, line| {
        hits.push((line_no, line.to_vec()));
        true
    });
    hits
}

/// 收集 scan_stream_lines_cb 命中行（流式 BufReader 路径）
fn collect_stream(
    compiled: &gxp::l1::compiler::CompiledL1,
    path: &std::path::Path,
) -> Vec<(usize, Vec<u8>)> {
    let mut hits = Vec::new();
    scan_stream_lines_cb(compiled, path, |line_no, line| {
        hits.push((line_no, line.to_vec()));
        true
    })
    .expect("流式扫描应成功");
    hits
}

#[test]
fn test_stream_scan_matches_memchr_simple() {
    let content = b"error line\ninfo line\nerror again\nwarn\n";
    let f = write_temp(content);
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    let memchr_hits = collect_memchr(&compiled, content);
    let stream_hits = collect_stream(&compiled, f.path());

    assert_eq!(memchr_hits.len(), stream_hits.len(), "命中数应一致");
    assert_eq!(memchr_hits[0].0, stream_hits[0].0, "行号应一致");
    assert_eq!(memchr_hits[1].0, stream_hits[1].0, "行号应一致");
    assert_eq!(memchr_hits[0].1, stream_hits[0].1, "行内容应一致");
}

#[test]
fn test_stream_scan_matches_memchr_field_predicate() {
    // cost_ms>800 查询：L1 粗筛命中 cost_ms= 前缀的行
    let content = b"cost_ms=900 user=alice\ncost_ms=100 user=bob\nno field here\ncost_ms=2000\n";
    let f = write_temp(content);
    let q = parse_query("cost_ms>800").expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    let memchr_hits = collect_memchr(&compiled, content);
    let stream_hits = collect_stream(&compiled, f.path());

    // L1 粗筛命中所有含 cost_ms= 的行（行 1, 2, 4）
    assert_eq!(memchr_hits.len(), stream_hits.len(), "命中数应一致");
    for (i, (m, s)) in memchr_hits.iter().zip(stream_hits.iter()).enumerate() {
        assert_eq!(m.0, s.0, "第 {} 个命中行号应一致", i);
    }
    assert_eq!(memchr_hits.len(), 3, "应命中 3 行（L1 粗筛）");
}

#[test]
fn test_stream_scan_matches_memchr_no_trailing_newline() {
    let content = b"info\nerror"; // 末尾无 \n
    let f = write_temp(content);
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    let memchr_hits = collect_memchr(&compiled, content);
    let stream_hits = collect_stream(&compiled, f.path());

    assert_eq!(memchr_hits.len(), 1);
    assert_eq!(stream_hits.len(), 1);
    assert_eq!(memchr_hits[0].0, 2, "末尾无 \\n 时最后一行也应命中");
    assert_eq!(stream_hits[0].0, 2);
}

#[test]
fn test_stream_scan_matches_memchr_empty_file() {
    let f = write_temp(b"");
    let q = parse_query(r#""error""#).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    let memchr_hits = collect_memchr(&compiled, b"");
    let stream_hits = collect_stream(&compiled, f.path());

    assert!(memchr_hits.is_empty());
    assert!(stream_hits.is_empty());
}

#[test]
fn test_stream_context_matches_memchr_context() {
    let content = b"l1\nl2\nl3\nl4\nl5\n";
    let f = write_temp(content);

    // 构造命中行（行 3）
    let matched = vec![MatchedLine::new(3, b"l3")];

    let memchr_ctx = apply_context(&matched, content, 1, 1);
    let stream_ctx =
        apply_context_streaming(&matched, f.path(), 1, 1).expect("流式 context 应成功");

    assert_eq!(memchr_ctx.len(), stream_ctx.len(), "context 行数应一致");
    for (i, (m, s)) in memchr_ctx.iter().zip(stream_ctx.iter()).enumerate() {
        assert_eq!(m.line_no, s.line_no, "第 {} 个 context 行号应一致", i);
        assert_eq!(m.line, s.line, "第 {} 个 context 行内容应一致", i);
    }
    // 行 3 前后各 1 → 行 2, 3, 4
    assert_eq!(memchr_ctx.len(), 3);
    assert_eq!(memchr_ctx[0].line_no, 2);
    assert_eq!(memchr_ctx[1].line_no, 3);
    assert_eq!(memchr_ctx[2].line_no, 4);
}

#[test]
fn test_stream_context_matches_memchr_overlapping() {
    let content = b"l1\nl2\nl3\nl4\nl5\nl6\nl7\n";
    let f = write_temp(content);

    // 命中行 2 和 5，before=1 after=1
    // 行 2 → 1,2,3；行 5 → 4,5,6；并集 → 1,2,3,4,5,6
    let matched = vec![MatchedLine::new(2, b"l2"), MatchedLine::new(5, b"l5")];

    let memchr_ctx = apply_context(&matched, content, 1, 1);
    let stream_ctx =
        apply_context_streaming(&matched, f.path(), 1, 1).expect("流式 context 应成功");

    assert_eq!(memchr_ctx.len(), stream_ctx.len(), "context 行数应一致");
    for (i, (m, s)) in memchr_ctx.iter().zip(stream_ctx.iter()).enumerate() {
        assert_eq!(m.line_no, s.line_no, "第 {} 个 context 行号应一致", i);
    }
    assert_eq!(memchr_ctx.len(), 6, "应含 6 行（去重后）");
}

#[test]
fn test_stream_context_clamps_at_start() {
    let content = b"l1\nl2\nl3\n";
    let f = write_temp(content);

    let matched = vec![MatchedLine::new(1, b"l1")];

    let memchr_ctx = apply_context(&matched, content, 5, 5);
    let stream_ctx =
        apply_context_streaming(&matched, f.path(), 5, 5).expect("流式 context 应成功");

    assert_eq!(memchr_ctx.len(), stream_ctx.len());
    // 行 1 before=5 → clamp 到 1；after=5 → clamp 到 3 → 1,2,3
    assert_eq!(memchr_ctx.len(), 3);
    assert_eq!(memchr_ctx[0].line_no, 1);
    assert_eq!(memchr_ctx[2].line_no, 3);
    assert_eq!(stream_ctx.len(), 3);
    assert_eq!(stream_ctx[0].line_no, 1);
    assert_eq!(stream_ctx[2].line_no, 3);
}
