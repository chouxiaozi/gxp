//! `--verify` 暴力对账（设计文档 §6.4）：朴素全扫 vs 优化路径，行集合 diff。
//!
//! 暴力路径：直接对 pattern 逐行跑 L2 求值（不经 L1 字段前缀粗筛）。
//! 优化路径：L1 粗筛 + L2 精判。
//! 不变式 `L1 ⊇ L2` 保证优化路径 ⊆ 暴力路径；此函数做反向对账。
//!
//! 返回摘要：`PASS [N 行命中]` 或 `FAIL [差异详情]`。

use std::path::Path;

use rayon::prelude::*;

use crate::cli::RunOpts;
use crate::io::file::read_for_scan;
use crate::mode::detect::PreparedScan;

/// `--verify` 暴力对账（设计文档 §6.4）：朴素全扫 vs 优化路径，行集合 diff。
///
/// 暴力路径：直接对 pattern 逐行跑 L2 求值（不经 L1 字段前缀粗筛）。
/// 优化路径：L1 粗筛 + L2 精判。
/// 不变式 `L1 ⊇ L2` 保证优化路径 ⊆ 暴力路径；此函数做反向对账。
///
/// 返回摘要：`PASS [N 行命中]` 或 `FAIL [差异详情]`。
///
/// 签名改为接收 `bytes: &[u8]`，不再自己读文件。
/// 调用方（`run_gxp_mode`）统一读一次输入（文件或 stdin），复用给 verify 和主扫描。
pub(crate) fn run_verify(
    q: &crate::dsl::ast::Query,
    compiled: &crate::l1::compiler::CompiledL1,
    bytes: &[u8],
    file_ext: &str,
) -> crate::Result<String> {
    // L2 预编译 + L3 预计算（DRY：PreparedScan::build）
    let scan = PreparedScan::build(&q.expr, bytes, file_ext)?;

    // 暴力路径：逐行跑 L2 求值（不经 L1 字段前缀粗筛，只过必选组正则）
    let mut brute: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut start: usize = 0;
    let mut line_no: usize = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..])
            .map(|i| start + i)
            .unwrap_or(bytes.len());
        line_no += 1;
        let line = &bytes[start..end];
        if scan.needs_l2 || scan.needs_l3 {
            // 修复：暴力路径也必须先过 L1 必选组正则粗筛（字面量/正则），
            // 否则 eval_prepared_with_ctx_and_captures 把 Literal 当永真 → ast() && "literal" 假阳性。
            // 优化路径在 scan_lines_cb 内已做 L1 正则匹配，此处对齐。
            if !l1_match_no_prefix(compiled, line) {
                start = end + 1;
                if end == bytes.len() {
                    break;
                }
                continue;
            }
            // 暴力路径须用暴力路径用 is_hit_with_captures 而非 is_hit
            // capture(X) && X>Y 需要先提取 capture 值再供谓词使用，
            // is_hit 走 eval_prepared（无 capture 提取）→ 谓词找不到字段 → 误判
            let mut captures = crate::pipeline::CaptureMap::new();
            if scan.is_hit_with_captures(line, line_no, &mut captures) {
                brute.push((line_no, line.to_vec()));
            }
        } else {
            // 无字段谓词 + 无 ast()：用 L1 必选组精判（正则 is_match）
            if l1_match_no_prefix(compiled, line) {
                brute.push((line_no, line.to_vec()));
            }
        }
        start = end + 1;
        if end == bytes.len() {
            break;
        }
    }

    // 优化路径：L1 粗筛 + L2 精判（DRY: scan_and_collect → 转为 (行号, 行字节) 元组）
    // m.line 现在是 `Arc<[u8]>`，需 `.to_vec()` 转回 `Vec<u8>` 与 brute 路径对齐
    let optimized: Vec<(usize, Vec<u8>)> =
        crate::mode::detect::scan_and_collect(compiled, &scan, bytes)
            .into_iter()
            .map(|m| (m.line_no, m.line.to_vec()))
            .collect();

    // 行集合 diff：按 (行号, 行内容) 比较
    if brute == optimized {
        Ok(format!("PASS [{} 行命中]", brute.len()))
    } else {
        let brute_only: Vec<&(usize, Vec<u8>)> =
            brute.iter().filter(|b| !optimized.contains(b)).collect();
        let opt_only: Vec<&(usize, Vec<u8>)> =
            optimized.iter().filter(|o| !brute.contains(o)).collect();
        Ok(format!(
            "FAIL [暴力独有 {} 行，优化独有 {} 行]",
            brute_only.len(),
            opt_only.len()
        ))
    }
}

/// `--verify` 目录场景：逐文件跑 run_verify + 汇总（Rust 方案 §7.6 + §11.2 V1）。
///
/// 对每个文件跑一次暴力 vs 优化对账，输出每文件结果 + 末尾汇总。
pub(crate) fn run_verify_directory(
    q: &crate::dsl::ast::Query,
    compiled: &crate::l1::compiler::CompiledL1,
    dir: &Path,
    opts: &RunOpts,
) -> crate::Result<String> {
    let plan = crate::mode::bare::build_pathplan_from_opts(opts);
    let (files, _stats) = plan.walk(dir);

    // rayon 并行逐文件 verify
    // 每文件独立 read_for_scan + run_verify，按原始文件顺序汇总结果
    let results: Vec<(String, Result<String, String>)> = files
        .par_iter()
        .map(|file_path| {
            let path_str = file_path.to_string_lossy().into_owned();
            let content = match read_for_scan(file_path) {
                Ok(c) => c,
                Err(e) => {
                    return (path_str, Err(format!("ERROR {e}")));
                }
            };
            let ext = crate::mode::detect::ext_from_path(&path_str);
            match run_verify(q, compiled, content.as_bytes(), &ext) {
                Ok(summary) => (path_str, Ok(summary)),
                Err(e) => (path_str, Err(format!("ERROR {e}"))),
            }
        })
        .collect();

    let mut checked = 0;
    let mut failed = 0;
    let mut details: Vec<String> = Vec::with_capacity(results.len());

    for (path_str, result) in results {
        match result {
            Ok(summary) => {
                if summary.starts_with("PASS") {
                    details.push(format!("{path_str}: {summary}"));
                } else {
                    failed += 1;
                    details.push(format!("{path_str}: FAILED {summary}"));
                }
            }
            Err(e) => {
                failed += 1;
                details.push(format!("{path_str}: {e}"));
            }
        }
        checked += 1;
    }

    let header = details.join("\n");
    Ok(format!(
        "{header}\n{checked} files checked, {failed} failed"
    ))
}

/// `--explain` 目录场景：输出 walk 计划（Rust 方案 §7.6）。
pub(crate) fn format_directory_explain(
    dir: &Path,
    q: &crate::dsl::ast::Query,
    compiled: &crate::l1::compiler::CompiledL1,
    opts: &RunOpts,
) -> crate::Result<String> {
    let plan = crate::mode::bare::build_pathplan_from_opts(opts);
    let (files, stats) = plan.walk(dir);

    let total_bytes: u64 = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    let plan = crate::plan::build_plan(q, compiled)?;
    let plan_text = plan.render_text();

    Ok(format!(
        "gxp execution plan (directory mode):\n  input: {} (directory, {} files via PathPlan::walk)\n  dir_index: lax (default, fingerprint校验)\n  walk stats: {} dirs visited, {} files matched\n  expected: ~{} files to scan, ~{} bytes total\n\n{}",
        dir.display(),
        files.len(),
        stats.dirs_visited,
        stats.files_matched,
        files.len(),
        total_bytes,
        plan_text
    ))
}

/// 暴力路径的 L1 精判（不走字段前缀粗筛，只走必选组正则）。
///
/// 用于 `--verify` 的 `needs_l2 == false` 场景：
/// 纯字面量/正则查询用必选组正则 `is_match` 判定即可。
pub(crate) fn l1_match_no_prefix(compiled: &crate::l1::compiler::CompiledL1, line: &[u8]) -> bool {
    for grp in &compiled.required_groups {
        if !grp.verify.is_match(line) {
            return false;
        }
    }
    true
}
