//! `sample` 管道动词基准（v0.33）。
//!
//! 复用 `full_pipeline.rs` 的 44MB 语料（`fixtures/44mb.log`），量化：
//! - `sample N` 小样本（N=20）：测 O(take) 常数——Fisher-Yates 前 take 轮
//! - `sample P%` 比例（50%）：测 O(len/2)——命中集一半
//! - `sample N` 大样本（N=10000）：测 O(take) 边界
//! - `head N` 对照：同场景证明 sample 的洗牌开销（应 < 1ms 级）
//!
//! 验证点：
//! - O(take) 而非 O(n)：sample 20 vs sample 10000 时间比应 ≈ 500×，非 1×
//! - sample 与 head 时间差 = 洗牌开销（LCG + Fisher-Yates）
//!
//! 运行：`cargo bench --bench sample`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::cli::RunOpts;
use gxp::mode::detect::run_with_writer;

/// 合成语料大小：~44MB（与 full_pipeline.rs / l1_scan.rs 共享同一份 44mb.log）。
const CORPUS_BYTES: usize = 44 * 1024 * 1024;

/// fixtures 路径（与 full_pipeline.rs / l1_scan.rs 共享同一份 44mb.log）。
fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("44mb.log");
    p
}

/// 构建合成语料（与 full_pipeline.rs 同一逻辑，共享 fixture）。
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

/// sample N 基准：测不同 N 下 Fisher-Yates 部分洗牌开销。
///
/// 查询：`"timeout" | sample N`
/// - L1+L2 粗筛命中 ~1/5 kws（timeout）× 全部行 ≈ 300K+ 命中
/// - sample N 从 300K+ 命中里抽 N，Fisher-Yates 前 N 轮
/// - N=20 vs N=10000 时间比应 ≈ 500×（O(take)），证明非 O(n) 全洗
fn bench_sample_count(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("sample_count");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // 三档 N：小（20）、中（1000）、大（10000）
    // 目的：证明 O(take) 而非 O(n)——时间应随 N 线性增长
    for n in [20usize, 1000, 10000] {
        let query = format!(r#""timeout" | sample {n}"#);
        g.bench_with_input(BenchmarkId::new("gxp_sample", n), &query, |b, q| {
            b.iter(|| {
                let args = vec![q.clone(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sample 执行应成功");
                black_box(sink);
            });
        });
    }

    g.finish();
}

/// sample P% 基准：测比例形式的开销。
///
/// 查询：`"timeout" | sample P%`
/// - P=10% vs P=50% vs P=90%：命中数随 P 线性增长，时间也应线性
fn bench_sample_ratio(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("sample_ratio");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    for p in [10u32, 50, 90] {
        let query = format!(r#""timeout" | sample {p}%"#);
        g.bench_with_input(BenchmarkId::new("gxp_sample", p), &query, |b, q| {
            b.iter(|| {
                let args = vec![q.clone(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sample % 执行应成功");
                black_box(sink);
            });
        });
    }

    g.finish();
}

/// head N 对照：同 N 下 head vs sample 时间差 = 洗牌开销。
///
/// 查询：`"timeout" | head N`
/// - head N 是 O(1) truncate，sample N 是 O(N) Fisher-Yates
/// - 差值即洗牌开销（应 < 1ms 级，LCG + swap）
fn bench_head_baseline(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("head_baseline");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // 与 sample_count 同档 N，便于对照
    for n in [20usize, 1000, 10000] {
        let query = format!(r#""timeout" | head {n}"#);
        g.bench_with_input(BenchmarkId::new("gxp_head", n), &query, |b, q| {
            b.iter(|| {
                let args = vec![q.clone(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("head 执行应成功");
                black_box(sink);
            });
        });
    }

    g.finish();
}

criterion_group!(
    benches,
    bench_sample_count,
    bench_sample_ratio,
    bench_head_baseline
);
criterion_main!(benches);
