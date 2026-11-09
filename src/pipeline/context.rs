//! `context N` / `context before:N after:M`：上下文窗口
//! （等价 `-A`/`-B`/`-C`，设计文档 §2.4 示例 8）。
//!
//! 语义（对齐 Rust 方案 §7 CLI 层 + §7.6 目录多文件聚合）：
//! - 给定命中行号集合 + 文件内容，每个命中行扩展为 `[line-before, line+after]` 窗口
//! - 多个命中窗口重叠时去重（按行号排序 + dedup）
//! - 窗口边界 clamp 到文件行数范围
//!
//! # 流式模式
//!
//! 文件 > 512MB 时，`apply_context_streaming` 通过二次遍历文件实现 context：
//! 1. 第一遍扫描已产出命中行号集合（`matched`）
//! 2. 第二遍流式读文件，按行号检查是否在 context 窗口内，收集对应行

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::MatchedLine;
use crate::l1::scan::strip_trailing_newline;

/// 执行 `context` 管道：把命中行扩展为含上下文的行集合。
///
/// 返回按行号升序、去重后的行（含原命中行）。
///
/// 保留命中行 captures + path。context 扩展行（非命中行）
/// 继承同文件命中行的 path（同文件内 path 相同）。
pub fn apply_context(
    matched: &[MatchedLine],
    content: &[u8],
    before: usize,
    after: usize,
) -> Vec<MatchedLine> {
    if matched.is_empty() {
        return Vec::new();
    }
    // 预扫 content 拆行偏移
    let line_offsets = build_line_offsets(content);
    let total_lines = line_offsets.len();

    // 命中行 captures 和 path 按行号索引（context 扩展行继承 path）
    let hit_captures: std::collections::HashMap<usize, crate::pipeline::CaptureMap> = matched
        .iter()
        .filter(|m| !m.captures.is_empty())
        .map(|m| (m.line_no, m.captures.clone()))
        .collect();
    let hit_match_bytes: std::collections::HashMap<usize, std::sync::Arc<[u8]>> = matched
        .iter()
        .filter_map(|m| {
            m.match_bytes
                .as_ref()
                .map(|mb| (m.line_no, std::sync::Arc::clone(mb)))
        })
        .collect();
    let hit_path: Option<std::sync::Arc<str>> = matched.iter().find_map(|m| m.path.clone());

    // 收集所有窗口内的行号（可能重复）
    let mut line_nos: Vec<usize> = Vec::new();
    for m in matched {
        let lo = m.line_no.saturating_sub(before).max(1);
        let hi = (m.line_no + after).min(total_lines);
        for ln in lo..=hi {
            line_nos.push(ln);
        }
    }
    // 去重 + 排序
    line_nos.sort_unstable();
    line_nos.dedup();

    // 按行号取内容
    line_nos
        .into_iter()
        .filter_map(|ln| {
            line_offsets.get(ln - 1).map(|&(start, end)| {
                let mut m = MatchedLine::new(ln, &content[start..end]);
                if let Some(caps) = hit_captures.get(&ln) {
                    m.captures = caps.clone();
                }
                if let Some(mb) = hit_match_bytes.get(&ln) {
                    m.match_bytes = Some(mb.clone());
                }
                m.path = hit_path.clone();
                m
            })
        })
        .collect()
}

/// 流式 `context` 管道：二次遍历文件，按行号收集 context 窗口内的行。
///
/// 流式 fallback：文件 > 512MB 时不持有完整 `content`，
/// 改为二次遍历文件：第一遍已产出 `matched`（命中行号集合），
/// 第二遍逐行读文件，检查行号是否在 context 窗口内。
///
/// 内存：仅 `HashSet<usize>` 存储需要的行号 + `Vec<MatchedLine>` 存结果。
/// 对于命中行数 << 文件行数的场景，内存远低于 `read_to_end`。
pub fn apply_context_streaming(
    matched: &[MatchedLine],
    path: &Path,
    before: usize,
    after: usize,
) -> std::io::Result<Vec<MatchedLine>> {
    if matched.is_empty() {
        return Ok(Vec::new());
    }

    // 收集所有需要的行号
    let mut needed: HashSet<usize> = HashSet::new();
    // 同时记录命中行的 captures（用于保留 capture 字段）
    let mut hit_captures: std::collections::HashMap<usize, crate::pipeline::CaptureMap> =
        std::collections::HashMap::new();
    let hit_path: Option<std::sync::Arc<str>> = matched.iter().find_map(|m| m.path.clone());
    let mut hit_match_bytes: std::collections::HashMap<usize, std::sync::Arc<[u8]>> =
        std::collections::HashMap::new();
    for m in matched {
        let lo = m.line_no.saturating_sub(before).max(1);
        // after 窗口上限未知（不知文件总行数），全部加入 needed
        for ln in lo..=m.line_no + after {
            needed.insert(ln);
        }
        if !m.captures.is_empty() {
            hit_captures.insert(m.line_no, m.captures.clone());
        }
        if let Some(ref mb) = m.match_bytes {
            hit_match_bytes.insert(m.line_no, mb.clone());
        }
    }

    // 二次遍历文件，按行号收集
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    let mut result: Vec<MatchedLine> = Vec::new();
    let mut line_no: usize = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);

    loop {
        buf.clear();
        let bytes_read = reader.read_until(b'\n', &mut buf)?;
        if bytes_read == 0 {
            break;
        }
        line_no += 1;
        if needed.contains(&line_no) {
            // 去掉末尾 \n
            let line = strip_trailing_newline(&buf);
            let mut m = if let Some(caps) = hit_captures.get(&line_no) {
                MatchedLine::with_captures(line_no, line, caps.clone())
            } else {
                MatchedLine::new(line_no, line)
            };
            m.path = hit_path.clone();
            if let Some(mb) = hit_match_bytes.get(&line_no) {
                m.match_bytes = Some(mb.clone());
            }
            result.push(m);
        }
    }

    // 按行号排序（read_until 顺序已是升序，但保险起见）
    result.sort_unstable_by_key(|m| m.line_no);
    Ok(result)
}

/// 预扫 content，返回每行的 `(start, end)` 偏移（end 含 `\n` 位置，但切片不含）。
///
/// `pub(crate)` 给 `trace` 模块复用（多行 stack trace 续行需要按行偏移访问 content）。
pub(crate) fn build_line_offsets(content: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let end = memchr::memchr(b'\n', &content[start..])
            .map(|i| start + i)
            .unwrap_or(content.len());
        out.push((start, end));
        start = end + 1;
        if end == content.len() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(nos: &[usize], content: &[u8]) -> Vec<MatchedLine> {
        let offsets = build_line_offsets(content);
        nos.iter()
            .filter_map(|&ln| {
                offsets
                    .get(ln - 1)
                    .map(|&(s, e)| MatchedLine::new(ln, &content[s..e]))
            })
            .collect()
    }

    fn texts(lines: &[MatchedLine]) -> Vec<String> {
        lines
            .iter()
            .map(|m| String::from_utf8_lossy(&m.line).to_string())
            .collect()
    }

    #[test]
    fn test_context_expands_around_hit() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let m = matched(&[3], content);
        let out = apply_context(&m, content, 1, 1);
        // 行 3 前后各 1：2,3,4
        assert_eq!(texts(&out), vec!["l2", "l3", "l4"]);
    }

    #[test]
    fn test_context_clamps_at_file_bounds() {
        let content = b"l1\nl2\nl3\n";
        let m = matched(&[1], content);
        let out = apply_context(&m, content, 2, 2);
        // 行 1：before clamp 到 1，after clamp 到 3 → 1,2,3
        assert_eq!(texts(&out), vec!["l1", "l2", "l3"]);
    }

    #[test]
    fn test_context_dedups_overlapping_windows() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let m = matched(&[2, 4], content);
        let out = apply_context(&m, content, 1, 1);
        // 行 2 → 1,2,3；行 4 → 3,4,5；并集 1,2,3,4,5 去重
        assert_eq!(texts(&out), vec!["l1", "l2", "l3", "l4", "l5"]);
    }

    #[test]
    fn test_context_empty_match_returns_empty() {
        let content = b"l1\nl2\n";
        let out = apply_context(&[], content, 1, 1);
        assert!(out.is_empty());
    }

    #[test]
    fn test_context_before_only_after_only() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let m = matched(&[3], content);
        // before:2 after:0 → 1,2,3
        let out = apply_context(&m, content, 2, 0);
        assert_eq!(texts(&out), vec!["l1", "l2", "l3"]);
        // before:0 after:2 → 3,4,5
        let out = apply_context(&m, content, 0, 2);
        assert_eq!(texts(&out), vec!["l3", "l4", "l5"]);
    }

    // --- 流式 context 测试 ---

    fn write_temp(content: &[u8]) -> std::path::PathBuf {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("创建临时文件失败");
        f.write_all(content).expect("写入失败");
        f.flush().expect("flush 失败");
        let p = f.into_temp_path();
        p.keep().expect("keep 失败")
    }

    #[test]
    fn test_streaming_context_expands_around_hit() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let path = write_temp(content);
        let m = matched(&[3], content);
        let out = apply_context_streaming(&m, &path, 1, 1).expect("流式 context 应成功");
        // 行 3 前后各 1：2,3,4
        assert_eq!(texts(&out), vec!["l2", "l3", "l4"]);
    }

    #[test]
    fn test_streaming_context_clamps_at_start() {
        let content = b"l1\nl2\nl3\n";
        let path = write_temp(content);
        let m = matched(&[1], content);
        let out = apply_context_streaming(&m, &path, 2, 2).expect("流式 context 应成功");
        // 行 1：before clamp 到 1，after → 3 → 1,2,3
        assert_eq!(texts(&out), vec!["l1", "l2", "l3"]);
    }

    #[test]
    fn test_streaming_context_dedups_overlapping() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let path = write_temp(content);
        let m = matched(&[2, 4], content);
        let out = apply_context_streaming(&m, &path, 1, 1).expect("流式 context 应成功");
        // 行 2 → 1,2,3；行 4 → 3,4,5；并集去重 → 1,2,3,4,5
        assert_eq!(texts(&out), vec!["l1", "l2", "l3", "l4", "l5"]);
    }

    #[test]
    fn test_streaming_context_empty_match() {
        let content = b"l1\nl2\n";
        let path = write_temp(content);
        let out = apply_context_streaming(&[], &path, 1, 1).expect("流式 context 应成功");
        assert!(out.is_empty());
    }

    #[test]
    fn test_streaming_context_preserves_captures() {
        let content = b"l1\nl2\nl3\n";
        let path = write_temp(content);
        // 构造一个带 captures 的命中行
        let mut m = MatchedLine::new(2, b"l2");
        m.captures.insert("key".to_string(), b"value".to_vec());
        let out = apply_context_streaming(&[m], &path, 1, 0).expect("流式 context 应成功");
        // 行 2 before:1 → 1,2；命中行 2 应保留 captures
        assert_eq!(out.len(), 2);
        assert!(out[0].captures.is_empty(), "非命中行不应有 captures");
        assert_eq!(
            out[1]
                .captures
                .get("key")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("value".to_string()),
            "命中行应保留 captures"
        );
    }

    // 上下文行 path 继承

    #[test]
    fn test_context_inherits_path_on_all_lines() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let mut m = matched(&[3], content);
        for ml in &mut m {
            ml.path = Some(std::sync::Arc::from("app.log"));
        }
        let out = apply_context(&m, content, 1, 1);
        // 所有行（含 context 扩展行）应继承命中行 path
        assert_eq!(out.len(), 3, "行 2,3,4 共 3 行");
        for ml in &out {
            assert_eq!(
                ml.path,
                Some(std::sync::Arc::from("app.log")),
                "每行应继承 path: line_no={}",
                ml.line_no
            );
        }
    }

    #[test]
    fn test_context_inherits_path_and_captures_on_hit() {
        let content = b"l1\nl2\nl3\n";
        let mut m = matched(&[2], content);
        m[0].path = Some(std::sync::Arc::from("app.log"));
        m[0].captures.insert("uid".to_string(), b"abc".to_vec());
        m[0].match_bytes = Some(std::sync::Arc::from(b"l2".as_ref()));
        let out = apply_context(&m, content, 1, 0);
        // 行 1,2 共 2 行
        assert_eq!(out.len(), 2);
        // 命中行 2 应保留 captures + match_bytes + path
        assert!(out[1].captures.contains_key("uid"), "命中行应保留 captures");
        assert!(out[1].match_bytes.is_some(), "命中行应保留 match_bytes");
        assert_eq!(out[1].path, Some(std::sync::Arc::from("app.log")));
        // 非命中行 1 应继承 path，但 captures 和 match_bytes 不继承
        assert!(out[0].captures.is_empty(), "非命中行不应有 captures");
        assert!(out[0].match_bytes.is_none(), "非命中行不应有 match_bytes");
        assert_eq!(
            out[0].path,
            Some(std::sync::Arc::from("app.log")),
            "非命中行应继承 path"
        );
    }

    #[test]
    fn test_streaming_context_inherits_path() {
        let content = b"l1\nl2\nl3\nl4\nl5\n";
        let path = write_temp(content);
        let mut m = matched(&[3], content);
        for ml in &mut m {
            ml.path = Some(std::sync::Arc::from("app.log"));
        }
        let out = apply_context_streaming(&m, &path, 1, 1).expect("流式 context 应成功");
        assert_eq!(out.len(), 3, "行 2,3,4 共 3 行");
        for ml in &out {
            assert_eq!(
                ml.path,
                Some(std::sync::Arc::from("app.log")),
                "每行应继承 path: line_no={}",
                ml.line_no
            );
        }
    }
}
