//! --verify overhead 基准（度量暴力对账 vs 优化路径的额外开销）。
//!
//! 合成 44MB 日志，同一查询分别跑：
//! - 正常模式（L1 粗筛 + L2 精判 → 输出）
//! - --verify 模式（正常路径 + 暴力全扫 + 行集合 diff）
//!
//! --verify 的理论开销 ≈ 2× 扫描（暴力路径 + 优化路径各扫一遍）+ diff。
//! 实测比率用于评估 --verify 在生产场景的可行性。
//!
//! 运行：`cargo bench --bench verify_overhead`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::cli::RunOpts;
use gxp::mode::detect::run_with_writer;

/// 合成语料大小：~44MB（与 full_pipeline.rs / l1_scan.rs 共享同一份 44mb.log）。
const CORPUS_BYTES: usize = 44 * 1024 * 1024;

/// fixtures 路径。
fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("44mb.log");
    p
}

/// 构建合成语料（与 full_pipeline.rs 同一逻辑）。
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

/// 查询 1：纯 L1+L2 字段查询（`"timeout" && cost_ms>800`）。
/// L1 下推字面量 "timeout" + 字段前缀 `cost_ms=`，L2 精判数值。
const QUERY_L2: &str = r#""timeout" && cost_ms>800"#;

/// 查询 2：全管道 Top-K 投影（设计文档 §2.4 示例 2）。
/// L1 → L2 → rank → fields，覆盖完整管道。
const QUERY_PIPELINE: &str = r#""timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms"#;

/// 度量正常查询 vs --verify 模式的延迟，计算 overhead 比率。
fn bench_verify_overhead(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);

    let mut g = c.benchmark_group("verify_overhead");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    // 查询 1（L1+L2）：正常 vs verify
    let opts_normal = RunOpts::default();
    let opts_verify = RunOpts {
        verify: true,
        ..Default::default()
    };

    g.bench_with_input(BenchmarkId::new("l2_normal", "44mb"), &(), |b, _| {
        b.iter(|| {
            let args = vec![QUERY_L2.to_string(), path_str.clone()];
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            run_with_writer(black_box(&args), opts_normal.clone(), &mut sink)
                .expect("正常查询应成功");
            black_box(sink);
        });
    });

    g.bench_with_input(BenchmarkId::new("l2_verify", "44mb"), &(), |b, _| {
        b.iter(|| {
            let args = vec![QUERY_L2.to_string(), path_str.clone()];
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            run_with_writer(black_box(&args), opts_verify.clone(), &mut sink)
                .expect("--verify 应成功");
            black_box(sink);
        });
    });

    // 查询 2（全管道）：正常 vs verify
    g.bench_with_input(BenchmarkId::new("pipeline_normal", "44mb"), &(), |b, _| {
        b.iter(|| {
            let args = vec![QUERY_PIPELINE.to_string(), path_str.clone()];
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            run_with_writer(black_box(&args), opts_normal.clone(), &mut sink)
                .expect("管道查询应成功");
            black_box(sink);
        });
    });

    g.bench_with_input(BenchmarkId::new("pipeline_verify", "44mb"), &(), |b, _| {
        b.iter(|| {
            let args = vec![QUERY_PIPELINE.to_string(), path_str.clone()];
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            run_with_writer(black_box(&args), opts_verify.clone(), &mut sink)
                .expect("管道 --verify 应成功");
            black_box(sink);
        });
    });

    g.finish();
}

criterion_group!(benches, bench_verify_overhead);
criterion_main!(benches);
