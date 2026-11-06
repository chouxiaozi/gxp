//! 诊断：拆解 pipeline execute_stages 各阶段耗时。
//!
//! 运行：`cargo bench --bench diag_stages`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;
use gxp::l2::{eval_numeric, EvalResult, ExtractedFields};
use gxp::pipeline::{execute_stages, MatchedLine};

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
        eval_numeric(cost, gxp::dsl::ast::CmpOp::Gt, 800.0),
        EvalResult::True
    )
}

fn main() {
    let path = setup_corpus();
    let bytes_owned = fs::read(&path).unwrap();
    let bytes: &[u8] = &bytes_owned;

    let q = parse_query(QUERY).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    // 收集 matched
    let mut matched: Vec<MatchedLine> = Vec::new();
    scan_lines_cb(&compiled, bytes, |line_no, line| {
        if l2_match(line) {
            matched.push(MatchedLine::new(line_no, line));
        }
        true
    });
    println!("matched: {} 行", matched.len());

    // 计时：完整 execute_stages
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let m = matched.clone();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let t0 = std::time::Instant::now();
            let _ = execute_stages(
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
        println!("[阶段] execute_stages 完整: {:.2}ms", times[0]);
    }

    // 计时：仅 apply_rank（partial-sort）
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let _ = gxp::pipeline::rank::apply_rank(&matched, b"cost_ms", 20, true);
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] apply_rank (partial-sort): {:.2}ms", times[0]);
    }

    // 计时：仅 apply_rank 中的 key extraction（ExtractedFields::extract + parse_numeric_value）
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let mut keys: Vec<f64> = Vec::with_capacity(matched.len());
            for m in &matched {
                let val = ExtractedFields::find_value(&m.line, b"cost_ms").unwrap_or(b"");
                let k = gxp::l2::parse_numeric_value(val).unwrap_or(f64::NAN);
                keys.push(k);
            }
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            let _ = keys;
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] rank key extraction only (find_value+parse): {:.2}ms",
            times[0]
        );
    }

    // 计时：仅 sort_by（不带 key extraction，假设 keys 已预计算）
    {
        // 预计算 keys
        let keys: Vec<f64> = matched
            .iter()
            .map(|m| {
                let v = ExtractedFields::find_value(&m.line, b"cost_ms").unwrap_or(b"");
                gxp::l2::parse_numeric_value(v).unwrap_or(f64::NAN)
            })
            .collect();

        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let mut idx: Vec<usize> = (0..matched.len()).collect();
            let t0 = std::time::Instant::now();
            idx.sort_by(|&a, &b| {
                let ka = keys[a];
                let kb = keys[b];
                if ka.is_nan() && kb.is_nan() {
                    std::cmp::Ordering::Equal
                } else if ka.is_nan() {
                    std::cmp::Ordering::Greater
                } else if kb.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal) // desc
                }
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            let _ = idx;
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] sort_by only (precomputed keys, desc): {:.2}ms",
            times[0]
        );
    }

    // 计时：select_nth_unstable_by + sort top K（partial-sort with precomputed keys）
    {
        let keys: Vec<f64> = matched
            .iter()
            .map(|m| {
                let v = ExtractedFields::find_value(&m.line, b"cost_ms").unwrap_or(b"");
                gxp::l2::parse_numeric_value(v).unwrap_or(f64::NAN)
            })
            .collect();

        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let mut idx: Vec<usize> = (0..matched.len()).collect();
            let t0 = std::time::Instant::now();
            let cmp = |a: &usize, b: &usize| {
                let ka = keys[*a];
                let kb = keys[*b];
                if ka.is_nan() && kb.is_nan() {
                    std::cmp::Ordering::Equal
                } else if ka.is_nan() {
                    std::cmp::Ordering::Greater
                } else if kb.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal) // desc
                }
            };
            let (front, _, _) = idx.select_nth_unstable_by(20, cmp);
            front.sort_by(cmp);
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            let _ = idx;
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] select_nth + sort K (precomputed keys, desc): {:.2}ms",
            times[0]
        );
    }

    // 计时：parse_numeric_value 单次开销
    {
        let n = 100000;
        let v = b"900";
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let _ = gxp::l2::parse_numeric_value(v);
        }
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        let per = elapsed / n as f64 * 1000.0; // µs
        println!(
            "[阶段] parse_numeric_value single call: {:.3}us (x{}={:.2}ms)",
            per, n, elapsed
        );
    }
}
