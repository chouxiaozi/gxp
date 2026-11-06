//! `sort` 管道动词基准（v0.35）。
//!
//! 复用 `full_pipeline.rs` 的 44MB 语料（`fixtures/44mb.log`），量化：
//! - `sort by FIELD` 全量 O(N log N) 排序开销
//! - `sort`（整行字节序）vs `sort by FIELD`（字段提取 + 数值比较）开销差
//! - `rank top 20 by FIELD`（partial-sort O(N + K log K)）vs `sort by FIELD`（全量 O(N log N)）对比
//! - `sort | head 20` vs `rank top 20`：sort+截断 vs partial-sort 的代价对比
//!
//! 验证点：
//! - sort 在大命中集（300K+ 行）上是否成为 <80ms 全管道预算的瓶颈
//! - partial-sort（rank top 20）vs 全量 sort 的时间比应显著（4× 理论加速）
//! - 整行字节序 vs 字段提取：字段提取（memchr + parse_numeric）额外开销
//!
//! 运行：`cargo bench --bench sort`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::cli::RunOpts;
use gxp::mode::detect::run_with_writer;

/// 合成语料大小：~44MB（与 full_pipeline.rs / sample.rs 共享同一份 44mb.log）。
const CORPUS_BYTES: usize = 44 * 1024 * 1024;

/// fixtures 路径（与 full_pipeline.rs / sample.rs 共享同一份 44mb.log）。
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

/// `sort by FIELD` 全量 O(N log N) 排序基准。
///
/// 查询：`"timeout" | sort by cost_ms`
/// - L1+L2 粗筛命中 ~1/5 kws（timeout）× 全部行 ≈ 300K+ 命中
/// - sort by cost_ms 对全集做 O(N log N) 稳定排序
/// - 与 rank top 20 by cost_ms（partial-sort）对比，验证全量排序代价
fn bench_sort_by_field(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("sort_by_field");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    g.bench_with_input(
        BenchmarkId::new("gxp_sort_by", "cost_ms"),
        &r#""timeout" | sort by cost_ms"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sort by cost_ms 执行应成功");
                black_box(sink);
            });
        },
    );

    g.finish();
}

/// `sort`（整行字节序）vs `sort by FIELD`：字段提取开销对比。
///
/// - `sort` 整行字节序：直接 cmp 行字节，零字段提取
/// - `sort by FIELD`：每行 ExtractedFields::find_value（memchr 定位 + parse_numeric）
/// - 差值即字段提取开销
fn bench_sort_line_vs_field(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("sort_line_vs_field");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // 整行字节序（零字段提取）
    g.bench_with_input(
        BenchmarkId::new("gxp_sort_line", "no_field"),
        &r#""timeout" | sort"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sort 整行字节序 执行应成功");
                black_box(sink);
            });
        },
    );

    // 字段值排序（含字段提取 + 数值解析）
    g.bench_with_input(
        BenchmarkId::new("gxp_sort_by_field", "cost_ms"),
        &r#""timeout" | sort by cost_ms"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sort by cost_ms 执行应成功");
                black_box(sink);
            });
        },
    );

    g.finish();
}

/// `rank top 20`（partial-sort）vs `sort by FIELD`（全量 O(N log N)）对比。
///
/// - rank top 20：select_nth_unstable_by + 前 20 稳定排序（O(N) + O(K log K)）
/// - sort by FIELD：全量 sort_by（O(N log N)）
/// - 理论比例（N=300K, K=20）：partial-sort ≈ 443k 次比较 vs 全量 ≈ 1.75M 次，~4× 加速
fn bench_rank_partial_vs_sort_full(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("rank_partial_vs_sort_full");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // rank top 20（partial-sort）
    g.bench_with_input(
        BenchmarkId::new("gxp_rank_top20", "partial_sort"),
        &r#""timeout" | rank top 20 by cost_ms desc"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("rank top 20 执行应成功");
                black_box(sink);
            });
        },
    );

    // sort by cost_ms desc（全量排序）
    g.bench_with_input(
        BenchmarkId::new("gxp_sort_full", "full_sort"),
        &r#""timeout" | sort by cost_ms desc"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sort by cost_ms desc 执行应成功");
                black_box(sink);
            });
        },
    );

    g.finish();
}

/// `sort | head 20` vs `rank top 20`：sort+截断 vs partial-sort。
///
/// - sort | head 20：先全量 O(N log N) 排序，再 truncate 20
/// - rank top 20：partial-sort O(N + K log K) 直接取 Top-20
/// - 差值即全量排序的"无用功"代价（排了 N-K 条不进入结果的行）
fn bench_sort_head_vs_rank_topk(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);
    let opts = RunOpts::default();

    let mut g = c.benchmark_group("sort_head_vs_rank_topk");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // sort by cost_ms desc | head 20（全量排序后截断）
    g.bench_with_input(
        BenchmarkId::new("gxp_sort_head", "full_sort_then_truncate"),
        &r#""timeout" | sort by cost_ms desc | head 20"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("sort | head 执行应成功");
                black_box(sink);
            });
        },
    );

    // rank top 20 by cost_ms desc（partial-sort 直接取）
    g.bench_with_input(
        BenchmarkId::new("gxp_rank_top20", "partial_sort"),
        &r#""timeout" | rank top 20 by cost_ms desc"#,
        |b, q| {
            b.iter(|| {
                let args = vec![q.to_string(), path_str.clone()];
                let mut sink: Vec<u8> = Vec::with_capacity(8192);
                run_with_writer(black_box(&args), opts.clone(), &mut sink)
                    .expect("rank top 20 执行应成功");
                black_box(sink);
            });
        },
    );

    g.finish();
}

criterion_group!(
    benches,
    bench_sort_by_field,
    bench_sort_line_vs_field,
    bench_rank_partial_vs_sort_full,
    bench_sort_head_vs_rank_topk
);
criterion_main!(benches);
