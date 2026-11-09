//! 全管道基准（对齐 §7.1 基线锚点：本机 106.74ms = v0.49 diag median，劣化 >10% 需归因）。
//!
//! 合成 44MB 日志，查询：
//! `"timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms`
//!
//! 全管道 = L1 粗筛 → L2 精判 → rank Top-20 → fields 投影 → 输出
//!
//! 对照：
//! - Python 原型 835ms（历史基线，已删除）
//! - GNU grep 子进程近似（grep 无字段语义/数值比较/Top-K 投影）
//!
//! 运行：`cargo bench --bench full_pipeline`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::cli::RunOpts;
use gxp::mode::detect::run_with_writer;

/// 合成语料大小：~44MB（对齐 §7.1 指标口径）。
const CORPUS_BYTES: usize = 44 * 1024 * 1024;

/// fixtures 路径（与 l1_scan.rs 共享同一份 44mb.log）。
fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("44mb.log");
    p
}

/// 构建合成语料（与 l1_scan.rs 同一逻辑，但这里确保两 bench 共享 fixture）。
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

/// gxp 全管道查询：`"timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms`
///
/// 这是设计文档 §2.4 示例 2，也是 §11.1 MVP 验收的 Top20 + 投影场景。
const QUERY: &str = r#""timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms"#;

/// gxp 全管道：L1 → L2 → rank → fields → 输出（通过 `run_with_writer` 串联）。
fn bench_gxp_full(c: &mut Criterion) {
    let path = setup_corpus();
    let path_str = path.to_string_lossy().to_string();
    let content_len = fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(CORPUS_BYTES as u64);

    let opts = RunOpts::default();

    let mut g = c.benchmark_group("full_pipeline");
    g.throughput(Throughput::Bytes(content_len));
    g.sample_size(15);
    g.measurement_time(std::time::Duration::from_secs(8));

    g.bench_with_input(BenchmarkId::new("gxp_rust", "44mb"), &(), |b, _| {
        b.iter(|| {
            let args = vec![QUERY.to_string(), path_str.clone()];
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            run_with_writer(black_box(&args), opts.clone(), &mut sink)
                .expect("pipeline 执行应成功");
            black_box(sink);
        });
    });

    g.finish();
}

/// GNU grep 近似对照：gxp 的字段语义和 Top-K 投影 grep 无法表达，
/// 只能跑字面量+前缀的存在性筛 + 手工 sort + cut 投影，近似但不等价。
fn bench_gnu_grep(c: &mut Criterion) {
    let path = setup_corpus();

    let mut g = c.benchmark_group("full_pipeline");
    g.throughput(Throughput::Bytes(CORPUS_BYTES as u64));
    g.sample_size(10);
    g.measurement_time(std::time::Duration::from_secs(10));

    g.bench_with_input(BenchmarkId::new("gnu_grep_pipe", "44mb"), &(), |b, _| {
        b.iter(|| {
            // 近似：timeout 命中 + [checkout] 或 [billing] + cost_ms= 存在性筛
            // + 按数值排序 Top20 + 投影 _ts,_tag,cost_ms
            // grep 无 Top-K 投影，用 sort + head + sed 拼出近似行为
            let out = std::process::Command::new("sh")
                .args([
                    "-c",
                    format!(
                        "grep -E 'timeout' {} | grep -E '\\[checkout\\]|\\[billing\\]' | grep -E 'cost_ms=' | \
                         sort -t= -k4 -rn | head -20",
                        path.to_string_lossy()
                    )
                    .as_str(),
                ])
                .output()
                .expect("grep 子进程失败");
            black_box(out.stdout);
        });
    });

    g.finish();
}

criterion_group!(benches, bench_gxp_full, bench_gnu_grep);
criterion_main!(benches);
