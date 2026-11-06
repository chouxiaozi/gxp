//! 诊断：生产代码统一 `read_to_end`（D10 改判），此 bench 保留为对照基准。
//!
//! 历史用途：早期对比 mmap vs fs::read 的 page fault 开销，
//! 为 D10 改判提供实测数据。
//!
//! 起此 bench 的两个读取路径均走 `read_to_end`（生产 `read_for_scan` 和
//! `std::fs::read`），差异收敛到 `Vec::with_capacity(size)` vs 默认容量。
//! 保留此 bench 用于：
//! - 监测未来 Vec 预分配策略变化的影响
//! - 若重新引入 mmap 混合策略时的回归基线
//!
//! 运行：`cargo bench --bench diag_mmap`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

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

    // 测试 1：read_for_scan（生产路径，统一 read_to_end）
    let mut times_read_for_scan = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
        times_read_for_scan.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = content;
    }
    times_read_for_scan.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 测试 2：fs::read（对照，走 std::fs::read 到 Vec）
    // 与 read_for_scan 路径一致，差异收敛到预分配策略
    let mut times_fsread = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let _bytes = fs::read(&path).unwrap();
        times_fsread.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_fsread.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 测试 3：read_for_scan 后扫描每个 4KB chunk（历史：触发 page fault；现状：纯顺序读）
    let mut times_scan_chunks = Vec::with_capacity(5);
    for _ in 0..5 {
        let content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
        let bytes = content.as_bytes();
        let t0 = std::time::Instant::now();
        let mut sum: u64 = 0;
        for chunk in bytes.chunks(4096) {
            sum = sum.wrapping_add(chunk[0] as u64);
        }
        times_scan_chunks.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = sum;
    }
    times_scan_chunks.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 测试 4：完整 run_with_writer
    let mut times_full = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path_str.clone()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
        times_full.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times_full.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!(
        "[阶段] read_for_scan (Vec): {:.2}ms",
        times_read_for_scan[0]
    );
    println!("[阶段] fs::read (Vec): {:.2}ms", times_fsread[0]);
    println!("[阶段] 扫描 4KB chunks: {:.2}ms", times_scan_chunks[0]);
    println!("[阶段] 完整 run_with_writer: {:.2}ms", times_full[0]);

    // L1 scan：read_for_scan 路径
    let q = gxp::dsl::parser::parse_query(QUERY).unwrap();
    let compiled = gxp::l1::compiler::compile(&q.expr).unwrap();
    let mut times_rfs_l1_scan = Vec::with_capacity(5);
    for _ in 0..5 {
        let content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
        let bytes = content.as_bytes();
        let mut count = 0usize;
        let t0 = std::time::Instant::now();
        gxp::l1::scan::scan_lines_cb(&compiled, bytes, |_, _| {
            count += 1;
            true
        });
        times_rfs_l1_scan.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = count;
    }
    times_rfs_l1_scan.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 对照：fs::read 后 L1 扫描
    let mut times_fsread_l1_scan = Vec::with_capacity(5);
    for _ in 0..5 {
        let bytes_owned = fs::read(&path).unwrap();
        let bytes: &[u8] = &bytes_owned;
        let mut count = 0usize;
        let t0 = std::time::Instant::now();
        gxp::l1::scan::scan_lines_cb(&compiled, bytes, |_, _| {
            count += 1;
            true
        });
        times_fsread_l1_scan.push(t0.elapsed().as_secs_f64() * 1000.0);
        let _ = count;
    }
    times_fsread_l1_scan.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!(
        "[阶段] L1 scan (read_for_scan): {:.2}ms",
        times_rfs_l1_scan[0]
    );
    println!(
        "[阶段] L1 scan (fs::read): {:.2}ms",
        times_fsread_l1_scan[0]
    );
}
