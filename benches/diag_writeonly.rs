//! 诊断：单独测各组件。
//! 运行：`cargo bench --bench diag_writeonly`

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

    // 1. run_with_writer 完整
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

    // 2. 只调用 run_gxp_mode（跳过 reject_grep_opts 和 positional 提取）
    let bytes_owned = fs::read(&path).unwrap();
    let bytes: &[u8] = &bytes_owned;
    let q = parse_query(QUERY).unwrap();
    let compiled = compile(&q.expr).unwrap();

    // 3. 只测 execute_stages + writeln output
    let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
    scan_lines_cb(&compiled, bytes, |line_no, line| {
        if l2_match(line) {
            matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
        }
        true
    });
    let matched_snapshot = matched.clone();

    // 计时：execute_stages 写入到 Vec<u8>
    let mut times_exec = Vec::with_capacity(5);
    for _ in 0..5 {
        let m = matched_snapshot.clone();
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let t0 = std::time::Instant::now();
        let _ = execute_stages(&q.stages, m, bytes, &path_str, &mut sink, false);
        times_exec.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_exec.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 计时：execute_stages 写入到 std::io::stdout()
    // (跳过，可能干扰测试)

    // 计时：String::from_utf8_lossy 输出 20 行
    let ranked = gxp::pipeline::rank::apply_rank(&matched_snapshot, b"cost_ms", 20, true);
    let names: Vec<&[u8]> = vec![b"_ts", b"_tag", b"cost_ms"];
    let projected = gxp::pipeline::project::apply_projection(&ranked, &names);
    let mut times_writeln = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let t0 = std::time::Instant::now();
        for m in &projected {
            let _ = writeln!(sink, "{}:{}", m.line_no, String::from_utf8_lossy(&m.line));
        }
        times_writeln.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_writeln.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 计时：完整 run_with_writer 的写入器：Vec<u8> 收集 + 最终 sink drop
    let mut times_sink_drop = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path_str.clone()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
        let t0 = std::time::Instant::now();
        let len = sink.len();
        let _ = sink;
        times_sink_drop.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = len;
    }
    times_sink_drop.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 计时：reject_grep_opts 和 positional 提取
    let mut times_setup = Vec::with_capacity(5);
    let args = &[QUERY.to_string(), path_str.clone()];
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let _ = gxp::mode::reject_grep_opts::check(args);
        let positional: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        times_setup.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = positional;
    }
    times_setup.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!("[阶段] run_with_writer 完整: {:.2}ms", times_total[0]);
    println!(
        "[阶段] execute_stages + output 写入器: {:.2}ms",
        times_exec[0]
    );
    println!("[阶段] writeln output (20 行): {:.2}ms", times_writeln[0]);
    println!(
        "[阶段] setup (reject + positional): {:.2}ms",
        times_setup[0]
    );
    println!(
        "[阶段] sink drop (after run_with_writer): {:.2}ms",
        times_sink_drop[0]
    );
}
