//! 诊断：拆解 full_pipeline 的各阶段耗时。
//!
//! 运行：`cargo bench --bench diag_pipeline`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;
use gxp::l2::{eval_numeric, EvalResult, ExtractedFields};

const CORPUS_BYTES: usize = 44 * 1024 * 1024;
const QUERY: &str = r#""timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms"#;

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("44mb.log");
    p
}

fn setup_corpus() -> PathBuf {
    let path = fixture_path();
    if path.exists() && fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0) >= CORPUS_BYTES {
        return path;
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    let svcs = ["checkout", "billing", "auth", "cart", "search"];
    let kws = ["timeout", "latency", "retry", "cache", "refund"];

    let f = fs::File::create(&path).unwrap();
    let mut w = std::io::BufWriter::new(f);
    let mut total = 0usize;
    let mut k = 0usize;
    while total < CORPUS_BYTES {
        let svc = svcs[k % svcs.len()];
        let kw = kws[k % kws.len()];
        let cost = k % 3000 + 1;
        let line = format!(
            "2026-10-20T{:02}:{:02}:{:02}Z INFO [{}] {} cost_ms={} req={}\n",
            k / 3600 % 24,
            k / 60 % 60,
            k % 60,
            svc,
            kw,
            cost,
            k
        );
        let bytes = line.as_bytes();
        w.write_all(bytes).unwrap();
        total += bytes.len();
        k += 1;
    }
    w.flush().unwrap();
    path
}

/// 简化版 L2 判定：用 find_value（与生产 eval_prepared 对齐）
fn l2_match(line: &[u8]) -> bool {
    let tag = ExtractedFields::find_value(line, b"_tag").unwrap_or(b"");
    if tag != b"checkout" && tag != b"billing" {
        return false;
    }
    let cost = ExtractedFields::find_value(line, b"cost_ms").unwrap_or(b"");
    matches!(
        eval_numeric(cost, gxp::dsl::ast::CmpOp::Gt, 800.0),
        EvalResult::True
    )
}

fn main() {
    let path = setup_corpus();
    let bytes_owned = fs::read(&path).unwrap();
    let bytes: &[u8] = &bytes_owned;
    let content_len = bytes.len();

    let q = parse_query(QUERY).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    println!(
        "content_len={} bytes ({} MB)",
        content_len,
        content_len / 1024 / 1024
    );

    // 预热
    for _ in 0..3 {
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path.to_string_lossy().to_string()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
    }

    // 计时：全管道（含 parse/compile/read_file）
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let _ = gxp::mode::detect::run_with_writer(
                &[QUERY.to_string(), path.to_string_lossy().to_string()],
                gxp::cli::RunOpts::default(),
                &mut sink,
            );
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] 全管道 run_with_writer: {:.2}ms (min={:.2})",
            times[0], times[0]
        );
    }

    // 计时：read_for_scan
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let _content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] read_for_scan: {:.2}ms", times[0]);
    }

    // 计时：仅 L1 扫描（不收集行，只计数）
    {
        let n = 5;
        let mut count = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            count = 0;
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |_ln, _line| {
                count += 1;
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] L1 scan_lines_cb (count only): {:.2}ms (L1 命中 {count} 行)",
            times[0]
        );
    }

    // 计时：L1 + L2，不收集 MatchedLine
    {
        let n = 5;
        let mut l2_hits = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            l2_hits = 0;
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |_ln, line| {
                if l2_match(line) {
                    l2_hits += 1;
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] L1+L2 eval (no collect): {:.2}ms (L2 命中 {l2_hits} 行)",
            times[0]
        );
    }

    // 计时：L1 + L2 + 收集 MatchedLine（Vec<u8> clone）
    {
        let n = 5;
        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            matched.clear();
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |line_no, line| {
                if l2_match(line) {
                    matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        let n_matched = matched.len();
        let total_bytes_cloned: usize = matched.iter().map(|m| m.line.len()).sum();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] L1+L2+collect (MatchedLine Vec<u8> clone): {:.2}ms (命中 {n_matched} 行, clone 总 {total_bytes_cloned} 字节)", times[0]);
    }

    // 计时：pipeline 执行链（rank + fields）on pre-collected matched
    {
        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        scan_lines_cb(&compiled, bytes, |line_no, line| {
            if l2_match(line) {
                matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
            }
            true
        });

        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let m = matched.clone();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let t0 = std::time::Instant::now();
            let _ = gxp::pipeline::execute_stages(
                &q.stages,
                m,
                bytes,
                path.to_str().unwrap(),
                &mut sink,
                false,
            );
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] execute_stages (rank+fields): {:.2}ms", times[0]);
    }
}
