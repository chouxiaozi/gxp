//! L2 字段抽取基准（对齐 §7.1 v1 性能目标：< 20ms / 31931 候选行）。
//!
//! 合成 ~31931 候选行（L1 粗筛后），每行 logfmt 格式含 `cost_ms=<value>`。
//! L2 抽取 + 数值比较：`cost_ms > 800`。
//!
//! 无对照（Python 原型已删除，§12 对照表只留历史 327ms 基线）。
//!
//! 运行：`cargo bench --bench l2_extract`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::dsl::ast::CmpOp;
use gxp::l2::eval_numeric;
use gxp::l2::ExtractedFields;

/// 目标候选行数（对齐 Python 原型 31931 候选行 / 327ms 基线）。
const CANDIDATE_LINES: usize = 31_931;

/// fixtures 路径。
fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("l2_candidates.log");
    p
}

/// 构建 31931 候选行：每行含 `cost_ms=<value>`，value 在 1..=3000 循环。
///
/// 每行格式：
/// ```text
/// 2026-10-20T00:00:00Z INFO [checkout] timeout cost_ms=900 req=1
/// ```
fn setup_corpus() -> PathBuf {
    let path = fixture_path();
    if path.exists() {
        let line_count = fs::read(&path)
            .unwrap()
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        if line_count == CANDIDATE_LINES {
            return path;
        }
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    let svcs = ["checkout", "billing", "auth", "cart", "search"];
    let f = fs::File::create(&path).unwrap();
    let mut w = std::io::BufWriter::new(f);
    for k in 0..CANDIDATE_LINES {
        let svc = svcs[k % svcs.len()];
        let cost = k % 3000 + 1;
        let line = format!(
            "2026-10-20T{:02}:{:02}:{:02}Z INFO [{}] timeout cost_ms={} req={}\n",
            k / 3600 % 24,
            k / 60 % 60,
            k % 60,
            svc,
            cost,
            k
        );
        w.write_all(line.as_bytes()).unwrap();
    }
    w.flush().unwrap();
    path
}

/// L2 抽取 + 数值比较：对每行 `ExtractedFields::extract` → `get("cost_ms")` → `eval_numeric(Gt, 800)`。
fn bench_l2_extract(c: &mut Criterion) {
    let path = setup_corpus();
    let content = fs::read(&path).unwrap();
    // 按行切分（模拟 L1 粗筛后的候选行集合）
    let lines: Vec<&[u8]> = content
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        CANDIDATE_LINES,
        "候选行数应为 {}，实际 {}",
        CANDIDATE_LINES,
        lines.len()
    );

    let mut g = c.benchmark_group("l2_extract");
    g.throughput(Throughput::Elements(CANDIDATE_LINES as u64));
    g.sample_size(30);
    g.measurement_time(std::time::Duration::from_secs(5));

    g.bench_with_input(
        BenchmarkId::new("extract_eval", "31931_lines"),
        &lines,
        |b, lines| {
            b.iter(|| {
                let mut hits = 0usize;
                let mut misses = 0usize;
                for line in lines {
                    let fields = ExtractedFields::extract(black_box(line));
                    if let Some(val) = fields.get(b"cost_ms") {
                        match eval_numeric(val, CmpOp::Gt, 800.0) {
                            gxp::l2::EvalResult::True => hits += 1,
                            gxp::l2::EvalResult::False => misses += 1,
                            gxp::l2::EvalResult::NotNumeric => misses += 1,
                        }
                    } else {
                        misses += 1;
                    }
                }
                black_box((hits, misses));
            });
        },
    );

    g.finish();
}

criterion_group!(benches, bench_l2_extract);
criterion_main!(benches);
