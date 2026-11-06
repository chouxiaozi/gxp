//! 诊断：深入拆解 run_with_writer 内部各阶段（不依赖 pub(crate) 项）。
//!
//! 运行：`cargo bench --bench diag_breakdown`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;
use gxp::l2::ExtractedFields;
use gxp::pipeline::execute_stages;

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

/// 简化版 L2 判定（用 find_value，与生产路径对齐）
fn l2_match(line: &[u8]) -> bool {
    let tag = ExtractedFields::find_value(line, b"_tag").unwrap_or(b"");
    if tag != b"checkout" && tag != b"billing" {
        return false;
    }
    let cost = ExtractedFields::find_value(line, b"cost_ms").unwrap_or(b"");
    matches!(
        gxp::l2::eval_numeric(cost, gxp::dsl::ast::CmpOp::Gt, 800.0),
        gxp::l2::EvalResult::True
    )
}

fn main() {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();

    // 预热
    for _ in 0..3 {
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path_str.clone()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
    }

    // 计时：完整 run_with_writer
    let mut times_total = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path_str.clone()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
        times_total.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_total.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 手动组合（不依赖 pub(crate) 项，用简化 L2）
    let bytes_owned = fs::read(&path).unwrap();
    let bytes: &[u8] = &bytes_owned;
    let q = parse_query(QUERY).expect("解析应成功");
    let _compiled = compile(&q.expr).expect("编译应成功");

    let mut times_parse_compile = Vec::with_capacity(5);
    let mut times_scan_l2_collect = Vec::with_capacity(5);
    let mut times_execute = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let q = parse_query(QUERY).expect("解析应成功");
        let compiled = compile(&q.expr).expect("编译应成功");
        times_parse_compile.push(t0.elapsed().as_secs_f64() * 1000.0);

        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        let t0 = std::time::Instant::now();
        scan_lines_cb(&compiled, bytes, |line_no, line| {
            if l2_match(line) {
                matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
            }
            true
        });
        times_scan_l2_collect.push(t0.elapsed().as_secs_f64() * 1000.0);

        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let t0 = std::time::Instant::now();
        let _ = execute_stages(&q.stages, matched, bytes, &path_str, &mut sink, false);
        times_execute.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_parse_compile.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times_scan_l2_collect.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times_execute.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!("[阶段] run_with_writer 完整: {:.2}ms", times_total[0]);
    println!("[阶段] parse + compile: {:.2}ms", times_parse_compile[0]);
    println!(
        "[阶段] scan + L2(simplified) + collect: {:.2}ms",
        times_scan_l2_collect[0]
    );
    println!("[阶段] execute_stages: {:.2}ms", times_execute[0]);

    let sum = times_parse_compile[0] + times_scan_l2_collect[0] + times_execute[0];
    let diff = times_total[0] - sum;
    println!(
        "[阶段] 手工组合 sum: {:.2}ms (run_with_writer - sum = {:.2}ms 差异)",
        sum, diff
    );

    // 单独测：read_for_scan
    let mut times_read = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let _content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
        times_read.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_read.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("[阶段] read_for_scan: {:.2}ms", times_read[0]);
}
