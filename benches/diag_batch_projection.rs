//! 多字段批量提取阈值 bench（§11.3 roadmap）。
//!
//! **命名纠正**：原 roadmap 名"SIMD 多字段投影"误导——本 bench 测的是
//! **算法层**优化（N 遍 memchr2 调用 → 1 遍），SIMD 仍在 memchr crate 内部，
//! gxp 不碰 intrinsics（D7 约束不变）。
//!
//! Exp3 只覆盖 2 字段（batch 慢 19.5%）。§5.5 标注"5+ 字段反超阈值估算，
//! 中，需实测"。本 bench 覆盖 2/5/8/12 字段，对比：
//! - **baseline**：`find_value × N`（当前生产路径，每字段一遍 memchr2）
//! - **batch**：`batch_find_values`（一次遍历 memchr2，分隔符位置后查 key 匹配多字段）
//!
//! 运行：`cargo bench --bench diag_batch_projection`
//!
//! 数据存档到 `docs/performance-baseline.md §5.5 Exp4`。

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use gxp::l2::ExtractedFields;
use memchr::memchr2;

/// 候选行数（与 l2_extract.rs 对齐）。
const CANDIDATE_LINES: usize = 31_931;

fn fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("batch_projection.log");
    p
}

/// 构建 31931 候选行，每行 12 字段 + 时间戳 + tag：
/// `2026-10-20T00:00:00Z INFO [checkout] f1=v1 f2=v2 ... f12=v12 req=1`
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
        let mut line = format!(
            "2026-10-20T{:02}:{:02}:{:02}Z INFO [{}] ",
            k / 3600 % 24,
            k / 60 % 60,
            k % 60,
            svc,
        );
        for n in 1..=12 {
            line.push_str(&format!("f{}=v{} ", n, k % 100 + n));
        }
        line.push_str(&format!("req={}\n", k));
        w.write_all(line.as_bytes()).unwrap();
    }
    w.flush().unwrap();
    path
}

/// 批量提取多字段：一次遍历 `memchr2('=', ':')`，在分隔符位置向左匹配 key。
///
/// 与 `find_value` 单字段路径语义一致：key 必须非空、全字母数字下划线、等于目标字段名。
/// 伪字段（`_ts`/`_tag`）各自走单路径，不并入 batch。
/// 返回 `Vec<Option<&[u8]>>`，顺序与 `names` 对齐。
fn batch_find_values<'a>(line: &'a [u8], names: &[&[u8]]) -> Vec<Option<&'a [u8]>> {
    let mut results: Vec<Option<&'a [u8]>> = vec![None; names.len()];
    let mut found = 0usize;
    let mut start = 0usize;

    while start < line.len() && found < names.len() {
        let Some(offset) = memchr2(b'=', b':', &line[start..]) else {
            break;
        };
        let sep_pos = start + offset;
        let key_end = sep_pos;
        let key_start = line[..key_end]
            .iter()
            .rposition(|&c| c == b' ' || c == b'\t')
            .map_or(0, |i| i + 1);
        let key = &line[key_start..key_end];

        // key 必须非空且全字母数字下划线
        if !key.is_empty() && key.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_') {
            // 检查是否匹配任一目标字段
            for (i, name) in names.iter().enumerate() {
                if results[i].is_none() && key == *name {
                    let val_start = sep_pos + 1;
                    let val_end = memchr2(b' ', b'\t', &line[val_start..])
                        .map_or(line.len(), |i| val_start + i);
                    let value = &line[val_start..val_end];
                    if !value.is_empty() {
                        results[i] = Some(value);
                        found += 1;
                    }
                    break;
                }
            }
        }
        start = sep_pos + 1;
    }
    results
}

fn bench_batch_projection(c: &mut Criterion) {
    let path = setup_corpus();
    let content = fs::read(&path).unwrap();
    let lines: Vec<&[u8]> = content
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(lines.len(), CANDIDATE_LINES);

    // 字段集：f1..f12
    // n=2 对照 Exp3（batch 慢 19.5% 的结论），验证新实现是否一致
    let field_sets: Vec<(usize, Vec<&[u8]>)> = vec![
        (2, vec![b"f1", b"f2"]),
        (5, vec![b"f1", b"f2", b"f3", b"f4", b"f5"]),
        (
            8,
            vec![b"f1", b"f2", b"f3", b"f4", b"f5", b"f6", b"f7", b"f8"],
        ),
        (
            12,
            vec![
                b"f1", b"f2", b"f3", b"f4", b"f5", b"f6", b"f7", b"f8", b"f9", b"f10", b"f11",
                b"f12",
            ],
        ),
    ];

    let mut g = c.benchmark_group("batch_projection");
    g.throughput(Throughput::Elements(CANDIDATE_LINES as u64));
    g.sample_size(30);
    g.measurement_time(std::time::Duration::from_secs(5));

    for (n, names) in field_sets {
        // baseline：find_value × N
        g.bench_with_input(
            BenchmarkId::new("find_value_n", format!("n{}", n)),
            &names,
            |b, names| {
                b.iter(|| {
                    let mut total = 0usize;
                    for line in lines.iter() {
                        let line = black_box(line);
                        for name in names {
                            if ExtractedFields::find_value(line, name).is_some() {
                                total += 1;
                            }
                        }
                    }
                    black_box(total);
                });
            },
        );

        // batch：一次遍历批量提取
        g.bench_with_input(
            BenchmarkId::new("batch_find", format!("n{}", n)),
            &names,
            |b, names| {
                b.iter(|| {
                    let mut total = 0usize;
                    for line in lines.iter() {
                        let line = black_box(line);
                        let results = batch_find_values(line, names);
                        for r in results {
                            if r.is_some() {
                                total += 1;
                            }
                        }
                    }
                    black_box(total);
                });
            },
        );
    }

    g.finish();
}

criterion_group!(benches, bench_batch_projection);
criterion_main!(benches);
