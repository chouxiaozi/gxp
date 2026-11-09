//! L1 扫描基准（对齐 §7.1 基线锚点：本机 33.51ms = v0.49 diag median）。
//!
//! 合成 44MB 日志（~400k 行），查询 `"timeout" && cost_ms>800`：
//! - L1 两遍链 = aho-corasick 字面量 "timeout" + 字段名前缀 `cost_ms=` 粗筛
//! - 端到端 `scan_lines_cb` 扫一遍，统计 L1 命中行数
//!
//! 对照 GNU grep -E 子进程（近似，grep 无字段语义，只能做字面量+前缀近似的对照）：
//! - gxp L1 的 `cost_ms>800` 数值比较交给 L2，L1 只看 `cost_ms=` 前缀存在
//! - grep 对照用 `grep -cE 'timeout' file.log | grep -cE 'cost_ms='`（字面量+前缀存在性）
//!
//! 运行：`cargo bench --bench l1_scan`
//! 生成 fixtures：首次运行时 `setup_corpus()` 写入 `fixtures/44mb.log`（.gitignore 忽略）

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::dsl::parser::parse_query_inline;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;

/// 合成语料大小：~44MB（对齐 §7.1 指标口径 + §11.1 MVP 验收 44MB/40 万行）。
const CORPUS_BYTES: usize = 44 * 1024 * 1024;

/// fixtures 路径（.gitignore 忽略，不进 git）。
fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("44mb.log");
    p
}

/// 构建合成语料：5 种 service × 5 种 keyword × cost_ms 1..=3000 循环。
///
/// 每行格式（对齐 scale_dir.rs 的 build_corpus，但无目录结构，单文件）：
/// ```text
/// 2026-10-20T00:00:00Z INFO [checkout] timeout cost_ms=900 req=1
/// ```
fn setup_corpus() -> PathBuf {
    let path = fixture_path();
    if path.exists() && fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0) >= CORPUS_BYTES {
        // 已存在且足够大，跳过重建
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

/// 查询字符串：`"timeout" && cost_ms>800`。
///
/// - L1 编译后：1 个必选组（字面量 "timeout"）+ 1 个字段前缀（`cost_ms=`）
/// - L1 命中 = 含 "timeout" 子串 + 含 `cost_ms=` 或 `cost_ms:` 前缀的行
/// - 不做 L2 精判（数值比较在 L2，本 bench 只测 L1）
const QUERY: &str = r#""timeout" && cost_ms>800"#;

/// 测 gxp L1 两遍链：aho-corasick 字面量 + 字段前缀粗筛 + scan_lines_cb 扫一遍。
fn bench_gxp_l1(c: &mut Criterion) {
    let path = setup_corpus();
    let content = fs::read(&path).unwrap();

    let expr = parse_query_inline(QUERY).expect("解析查询应成功");
    let compiled = compile(&expr).expect("L1 编译应成功");

    let mut g = c.benchmark_group("l1_scan");
    g.throughput(Throughput::Bytes(content.len() as u64));
    g.sample_size(20);
    g.measurement_time(std::time::Duration::from_secs(5));

    g.bench_with_input(
        BenchmarkId::new("gxp_rust", "44mb"),
        &content,
        |b, content| {
            b.iter(|| {
                let mut count = 0usize;
                scan_lines_cb(black_box(&compiled), black_box(content), |_ln, _line| {
                    count += 1;
                    true
                });
                black_box(count);
            });
        },
    );

    g.finish();
}

/// 对照 GNU grep -E 子进程：近似字面量+前缀存在性筛（grep 无字段语义）。
///
/// 两条 grep 管道：
/// 1. `grep -E 'timeout' file.log`  ←  等价 L1 必选组（字面量 "timeout"）
/// 2. 管道给 `grep -cE 'cost_ms='`   ←  等价 L1 字段前缀粗筛（只看前缀存在）
///
/// 注：这是 **近似** 对照——grep 的 `cost_ms=` 是字面子串匹配，不是字段语义；
/// gxp L1 的 `cost_ms>` 下推到 `cost_ms=` 前缀粗筛，行为等价（都不做数值比较）。
fn bench_gnu_grep(c: &mut Criterion) {
    let path = setup_corpus();

    let mut g = c.benchmark_group("l1_scan");
    g.throughput(Throughput::Bytes(CORPUS_BYTES as u64));
    g.sample_size(10);
    g.measurement_time(std::time::Duration::from_secs(8));

    g.bench_with_input(BenchmarkId::new("gnu_grep_pipe", "44mb"), &(), |b, _| {
        b.iter(|| {
            let out = std::process::Command::new("sh")
                .args([
                    "-c",
                    format!(
                        "grep -E 'timeout' {} | grep -cE 'cost_ms='",
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

criterion_group!(benches, bench_gxp_l1, bench_gnu_grep);
criterion_main!(benches);
