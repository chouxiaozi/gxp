//! V7 benchmark：>1GB 文件 read_to_end vs BufReader 流式扫描。
//!
//! D10 决策（统一 read_to_end）在 44MB 文件上验证通过，但 V7 待验证项
//! 要求在 >1GB 文件上对比 read_to_end 与流式 BufReader 的内存/延迟差异。
//!
//! 运行：`cargo bench --bench diag_v7_streaming`
//!
//! 产出数据用于 D10 决策：是否需要为大文件添加流式 fallback。

use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::time::Instant;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;
use gxp::l2::{eval_numeric, EvalResult, ExtractedFields};

const QUERY: &str = r#""timeout" && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms"#;

fn fixture_path_1gb() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures");
    p.push("1gb.log");
    p
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
    let path = fixture_path_1gb();
    if !path.exists() {
        eprintln!("1GB 基准文件不存在: {:?}", path);
        eprintln!("请先生成：python -c \"data=open('fixtures/44mb.log','rb').read(); open('fixtures/1gb.log','wb').write(data*24)\"");
        std::process::exit(1);
    }

    let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "文件大小: {:.2} GB ({:.0} MB)",
        file_size as f64 / 1e9,
        file_size as f64 / 1e6
    );

    let q = parse_query(QUERY).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    // ===== 方案 A: read_to_end（当前 D10 生产路径） =====
    {
        let n = 3;
        let mut times = Vec::with_capacity(n);
        let mut peak_mem = 0u64;
        let mut count = 0usize;
        for _ in 0..n {
            count = 0;
            let t0 = Instant::now();
            let bytes_owned = fs::read(&path).expect("read 应成功");
            let elapsed = t0.elapsed();
            peak_mem = peak_mem.max(bytes_owned.len() as u64);

            let bytes: &[u8] = &bytes_owned;
            let t1 = Instant::now();
            scan_lines_cb(&compiled, bytes, |_line_no, line| {
                if l2_match(line) {
                    count += 1;
                }
                true
            });
            let scan_elapsed = t1.elapsed();

            times.push((elapsed, scan_elapsed));

            // 读完后 + 扫描后都测一次进程内存
            let rss_after_read = get_process_memory();
            let _ = rss_after_read;
        }
        let best = times.iter().min_by_key(|(r, _)| r.as_nanos()).unwrap();
        println!(
            "[A] read_to_end: read {:.0}ms + scan {:.0}ms = total {:.0}ms (命中 {} 行, Vec 大小 {:.0} MB, 进程峰值 RSS {:.0} MB)",
            best.0.as_secs_f64() * 1000.0,
            best.1.as_secs_f64() * 1000.0,
            (best.0 + best.1).as_secs_f64() * 1000.0,
            count,
            peak_mem as f64 / 1e6,
            get_process_memory(),
        );
    }

    // ===== 方案 B: BufReader 流式扫描（逐块读入 + 逐行扫描） =====
    {
        let n = 3;
        let mut times = Vec::with_capacity(n);
        let mut peak_mem = 0u64;
        let mut count = 0usize;
        for _ in 0..n {
            count = 0;
            peak_mem = 0;
            let t0 = Instant::now();

            let file = fs::File::open(&path).expect("open 应成功");
            let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file); // 8MB buffer
            let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8MB buffer（vec! 初始化长度）
            let mut leftover: Vec<u8> = Vec::new();

            loop {
                let read = reader.read(&mut buf).expect("read 应成功");
                if read == 0 {
                    // 处理最后未结束的行
                    if !leftover.is_empty() {
                        let line = &leftover[..];
                        if compiled.l1_match(line) && l2_match(line) {
                            count += 1;
                        }
                    }
                    break;
                }

                // 合并 leftover + new data
                leftover.extend_from_slice(&buf[..read]);
                peak_mem = peak_mem.max(leftover.len() as u64);

                // 逐行扫描
                let mut start = 0;
                while let Some(pos) = leftover[start..].iter().position(|&b| b == b'\n') {
                    let end = start + pos;
                    let line = &leftover[start..end];
                    if compiled.l1_match(line) && l2_match(line) {
                        count += 1;
                    }
                    start = end + 1;
                }
                // 保留未完成的行
                if start < leftover.len() {
                    leftover.drain(..start);
                } else {
                    leftover.clear();
                }
            }
            let elapsed = t0.elapsed();
            times.push(elapsed);
        }
        let best = times.iter().min_by_key(|t| t.as_nanos()).unwrap();
        println!(
            "[B] BufReader 8MB: total {:.0}ms (命中 {} 行, 峰值内存 {:.0} MB)",
            best.as_secs_f64() * 1000.0,
            count,
            peak_mem as f64 / 1e6
        );
    }

    // ===== 方案 C: BufReader 小 buffer（64KB，模拟保守流式） =====
    {
        let n = 3;
        let mut times = Vec::with_capacity(n);
        let mut peak_mem = 0u64;
        let mut count = 0usize;
        for _ in 0..n {
            count = 0;
            peak_mem = 0;
            let t0 = Instant::now();

            let file = fs::File::open(&path).expect("open 应成功");
            let mut reader = BufReader::with_capacity(64 * 1024, file); // 64KB buffer
            let mut buf = vec![0u8; 64 * 1024]; // 64KB buffer
            let mut leftover: Vec<u8> = Vec::new();

            loop {
                let read = reader.read(&mut buf).expect("read 应成功");
                if read == 0 {
                    if !leftover.is_empty() {
                        let line = &leftover[..];
                        if compiled.l1_match(line) && l2_match(line) {
                            count += 1;
                        }
                    }
                    break;
                }

                leftover.extend_from_slice(&buf[..read]);
                peak_mem = peak_mem.max(leftover.len() as u64);

                let mut start = 0;
                while let Some(pos) = leftover[start..].iter().position(|&b| b == b'\n') {
                    let end = start + pos;
                    let line = &leftover[start..end];
                    if compiled.l1_match(line) && l2_match(line) {
                        count += 1;
                    }
                    start = end + 1;
                }
                if start < leftover.len() {
                    leftover.drain(..start);
                } else {
                    leftover.clear();
                }
            }
            let elapsed = t0.elapsed();
            times.push(elapsed);
        }
        let best = times.iter().min_by_key(|t| t.as_nanos()).unwrap();
        println!(
            "[C] BufReader 64KB: total {:.0}ms (命中 {} 行, 峰值内存 {:.0} MB)",
            best.as_secs_f64() * 1000.0,
            count,
            peak_mem as f64 / 1e6
        );
    }

    // ===== 方案 D: read_to_end 但只统计 read 时间（隔离 I/O vs 计算） =====
    {
        let n = 3;
        let mut read_times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = Instant::now();
            let _ = fs::read(&path).expect("read 应成功");
            read_times.push(t0.elapsed());
        }
        let best = read_times.iter().min_by_key(|t| t.as_nanos()).unwrap();
        println!(
            "[D] read_to_end only (no scan): {:.0}ms",
            best.as_secs_f64() * 1000.0
        );
    }

    // ===== 总结 =====
    println!("---");
    println!("D10 决策验证（>1GB 文件）：");
    println!("  方案 A (read_to_end):  峰值内存 ~1GB，但全量读入后扫描零页错误");
    println!("  方案 B (BufReader 8MB): 峰值内存 ~8MB+leftover，但每行需 memchr 两遍（leftover + new chunk）");
    println!("  方案 C (BufReader 64KB): 峰值内存 ~64KB+leftover，buffer 太小 syscall 过多");
    println!(
        "  如果方案 A 总延迟 <= 方案 B/C，D10 在 1GB 场景下仍成立（read_to_end 无需 fallback）"
    );
}

/// 获取当前进程的 Working Set（物理内存占用），单位 MB。
/// 用 tasklist 命令读取，避免引入 winapi 依赖。
#[cfg(windows)]
fn get_process_memory() -> f64 {
    let pid = std::process::id();
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            // tasklist CSV: "gxp.exe","1234","Console","1","123,456 K"
            for line in s.lines() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 5 {
                    let mem_str = parts[4].trim_matches('"').trim_matches(' ');
                    // "123,456 K" → 123456
                    let kb: u64 = mem_str.replace([',', 'K'], "").trim().parse().unwrap_or(0);
                    return kb as f64 / 1024.0; // KB → MB
                }
            }
            -1.0
        }
        Err(_) => -1.0,
    }
}

#[cfg(not(windows))]
fn get_process_memory() -> f64 {
    -1.0
}
