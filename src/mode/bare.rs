//! BARE 模式执行：`gxp 'pattern' file` ≡ `grep -E 'pattern' file`。
//!
//! 用 `regex::bytes::Regex` 编译 ERE pattern，逐行 `is_match`，
//! 输出 `行号:行内容`（gxp 默认带行号）。
//!
//! 文件 > 512MB 走流式扫描（`bare_scan_stream_lines`）。

use std::io::{BufRead, BufReader};
use std::path::Path;

use rayon::prelude::*;
use regex::bytes::Regex;

use crate::cli::RunOpts;
use crate::io::file::read_for_scan;
use crate::l1::scan::strip_trailing_newline;

/// BARE 模式执行：`gxp 'pattern' file` ≡ `grep -E 'pattern' file`。
///
/// 用 `regex::bytes::Regex` 编译 ERE pattern，逐行 `is_match`，
/// 输出 `行号:行内容`（gxp 默认带行号）。
pub(crate) fn run_bare_mode<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    opts: &RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let content = crate::mode::detect::read_input(file_path).map_err(|e| {
        // 包装 io::Error 加 file_path 上下文（与 gxp 模式 run_gxp_mode 一致）
        crate::GxpError::Runtime {
            detail: format!("读取 '{file_path}' 失败: {e}"),
        }
    })?;

    // 流式 BARE 扫描
    if content.is_stream() {
        let path = content
            .stream_path()
            .ok_or_else(|| crate::GxpError::Runtime {
                detail: "流式模式缺少文件路径".to_string(),
            })?;
        let re = Regex::new(pattern).map_err(|e| crate::GxpError::Runtime {
            detail: format!("正则编译失败: {e}"),
        })?;
        // 传实际文件路径作为 prefix
        return bare_scan_stream_lines(path, &re, file_path, opts.json, out);
    }

    run_bare_mode_with_content(pattern, file_path, content, opts, out)
}

/// BARE 模式核心（用已读取的 content 执行）。
pub(crate) fn run_bare_mode_with_content<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    content: crate::io::file::FileContent,
    opts: &RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let re = Regex::new(pattern).map_err(|e| crate::GxpError::Runtime {
        detail: format!("正则编译失败: {e}"),
    })?;

    let bytes = content.as_bytes();

    // 逐行扫描，输出命中行（DRY: bare_scan_lines）
    // 单文件模式也传实际文件路径作为 prefix
    bare_scan_lines(bytes, &re, file_path, opts.json, out)
}

/// 流式 BARE 扫描：文件 > 512MB 时用 `BufReader` 逐行扫描 + 直接输出。
///
/// 流式 fallback：不读入全量文件，用 `BufReader::read_until(b'\n')`
/// 逐行读取 + `regex::bytes::Regex::is_match` 匹配，命中行直接写出到 `out`。
/// 内存峰值仅 BufReader buffer（8MB）+ 单行 buffer。
fn bare_scan_stream_lines<W: std::io::Write>(
    path: &Path,
    re: &Regex,
    prefix: &str,
    json: bool,
    out: &mut W,
) -> crate::Result<()> {
    let path_str = prefix.trim_end_matches(':').replace('\\', "/");
    let is_dir = prefix.ends_with(':');
    let dir_prefix = if is_dir {
        format!("{path_str}:")
    } else {
        String::new()
    };

    let file = std::fs::File::open(path).map_err(crate::GxpError::from)?;
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    let mut line_no: usize = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);

    loop {
        buf.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buf)
            .map_err(crate::GxpError::from)?;
        if bytes_read == 0 {
            break;
        }
        line_no += 1;
        let line = strip_cr(strip_trailing_newline(&buf));
        if re.is_match(line) {
            if json {
                let line_str = String::from_utf8_lossy(line);
                let match_bytes = re
                    .find(line)
                    .map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned());
                let mut obj = serde_json::json!({
                    "line_no": line_no,
                    "line": line_str,
                    "path": path_str,
                });
                if let Some(mb) = match_bytes {
                    obj["match"] = serde_json::Value::String(mb);
                }
                writeln!(out, "{obj}").map_err(crate::GxpError::from)?;
            } else {
                writeln!(
                    out,
                    "{dir_prefix}{line_no}:{}",
                    String::from_utf8_lossy(line)
                )
                .map_err(crate::GxpError::from)?;
            }
        }
    }
    Ok(())
}

/// BARE 模式目录递归执行：`gxp 'pattern' dir/` ≡ `grep -Er 'pattern' dir/`。
///
/// 用 `PathPlan`（含 glob/gitignore 过滤）枚举目录下所有文件，
/// 逐文件用 `regex::bytes::Regex` 编译 ERE pattern 逐行 `is_match`，
/// 输出 `文件路径:行号:行内容`（目录模式带文件路径前缀，与 grep -r 一致）。
pub(crate) fn run_bare_mode_directory<W: std::io::Write>(
    pattern: &str,
    dir: &Path,
    opts: RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let re = Regex::new(pattern).map_err(|e| crate::GxpError::Runtime {
        detail: format!("正则编译失败: {e}"),
    })?;

    // 从 RunOpts 构建 PathPlan（glob + gitignore）
    let plan = build_pathplan_from_opts(&opts);
    let (files, _stats) = plan.walk(dir);

    // rayon 并行跨文件扫描
    // 每文件独立扫描收集命中行（行号+行字节），按原始文件顺序串行输出
    let hits_per_file: Vec<FileHits> = files
        .par_iter()
        .filter_map(|file_path| {
            let content = match read_for_scan(file_path) {
                Ok(c) => c,
                // 文件不可读（权限/删除）：跳过，与 grep -r 行为一致
                Err(_) => return None,
            };
            let bytes = content.as_bytes();
            let prefix = format!("{}:", file_path.to_string_lossy());
            let hits = bare_scan_collect(bytes, &re);
            Some((prefix, hits))
        })
        .collect();

    // 串行输出（保持文件顺序）
    let json = opts.json;
    for (prefix, hits) in hits_per_file {
        // 统一路径分隔符为正斜杠
        let path_normalized = prefix.trim_end_matches(':').replace('\\', "/");
        let prefix_normalized = format!("{}:", path_normalized);
        for (line_no, line) in hits {
            if json {
                let line_str = String::from_utf8_lossy(&line);
                let match_bytes = re
                    .find(&line)
                    .map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned());
                let mut obj = serde_json::json!({
                    "line_no": line_no,
                    "line": line_str,
                    "path": path_normalized,
                });
                if let Some(mb) = match_bytes {
                    obj["match"] = serde_json::Value::String(mb);
                }
                writeln!(out, "{obj}").map_err(crate::GxpError::from)?;
            } else {
                writeln!(
                    out,
                    "{prefix_normalized}{line_no}:{}",
                    String::from_utf8_lossy(&line)
                )
                .map_err(crate::GxpError::from)?;
            }
        }
    }
    Ok(())
}

/// 从 RunOpts 构建 PathPlan（glob + gitignore 过滤）。
///
/// 无 `--glob` 且 `respect_gitignore = true` 时等价 `PathPlan::default()`。
pub(crate) fn build_pathplan_from_opts(opts: &RunOpts) -> crate::dir::plan::PathPlan {
    crate::dir::plan::PathPlan::new()
        .with_glob_patterns(opts.glob.clone())
        .with_respect_gitignore(opts.respect_gitignore)
}

/// DRY helper: BARE 模式逐行扫描 + 输出命中行（单文件/目录统一）。
///
/// `prefix` 为文件路径前缀（单文件为文件路径，目录为 `"{path}:"`），输出格式 `{prefix}{line_no}:{line}`。
/// 单文件模式也传实际文件路径（不再传空串）。路径分隔符统一为正斜杠。
pub(crate) fn bare_scan_lines<W: std::io::Write>(
    bytes: &[u8],
    re: &Regex,
    prefix: &str,
    json: bool,
    out: &mut W,
) -> crate::Result<()> {
    // 统一路径分隔符 + 单文件模式 JSON 输出实际路径
    // 非 JSON：目录模式加路径前缀（prefix 含 `:`），单文件模式不加（grep -E 兼容）
    // JSON：始终输出 path 字段（单文件为实际路径，目录为目录路径）
    let path_str = prefix.trim_end_matches(':').replace('\\', "/");
    let is_dir = prefix.ends_with(':');
    let dir_prefix = if is_dir {
        format!("{path_str}:")
    } else {
        String::new()
    };

    let mut line_no: usize = 0;
    let mut start: usize = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..])
            .map(|i| start + i)
            .unwrap_or(bytes.len());
        line_no += 1;
        let line = strip_cr(&bytes[start..end]);
        if re.is_match(line) {
            if json {
                let line_str = String::from_utf8_lossy(line);
                let match_bytes = re
                    .find(line)
                    .map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned());
                let mut obj = serde_json::json!({
                    "line_no": line_no,
                    "line": line_str,
                    "path": path_str,
                });
                if let Some(mb) = match_bytes {
                    obj["match"] = serde_json::Value::String(mb);
                }
                writeln!(out, "{obj}").map_err(crate::GxpError::from)?;
            } else {
                // 非 JSON：目录模式加路径前缀，单文件模式不加（grep -E 兼容）
                writeln!(
                    out,
                    "{dir_prefix}{line_no}:{}",
                    String::from_utf8_lossy(line)
                )
                .map_err(crate::GxpError::from)?;
            }
        }
        start = end + 1;
        if end == bytes.len() {
            break;
        }
    }
    Ok(())
}

/// 每文件命中行集合：`(路径前缀, [(行号, 行字节)])`。
type FileHits = (String, Vec<(usize, Vec<u8>)>);

/// DRY helper: BARE 模式逐行扫描，收集命中行（行号+行字节）。
///
/// 目录并行扫描用此函数收集，再串行输出。
/// 逻辑与 `bare_scan_lines` 一致，但不写 writer 而是 collect。
pub(crate) fn bare_scan_collect(bytes: &[u8], re: &Regex) -> Vec<(usize, Vec<u8>)> {
    let mut hits: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut line_no: usize = 0;
    let mut start: usize = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..])
            .map(|i| start + i)
            .unwrap_or(bytes.len());
        line_no += 1;
        let line = strip_cr(&bytes[start..end]);
        if re.is_match(line) {
            hits.push((line_no, line.to_vec()));
        }
        start = end + 1;
        if end == bytes.len() {
            break;
        }
    }
    hits
}

/// 去掉行尾 `\r`（CRLF → LF，与 grep 行为对齐）。
///
/// `memchr(b'\n')` 切行时 `\r` 留在行内容中，输出会变成 `\r\n`。
/// grep 逐行输出只有 `\n`，故 strip 尾部 `\r` 对齐。
#[inline]
fn strip_cr(line: &[u8]) -> &[u8] {
    if line.last() == Some(&b'\r') {
        &line[..line.len() - 1]
    } else {
        line
    }
}
