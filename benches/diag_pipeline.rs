//! 诊断：拆解 full_pipeline 的各阶段耗时。
//!
//! 运行：`cargo bench --bench diag_pipeline`

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use gxp::dsl::parser::parse_query;
use gxp::l1::compiler::compile;
use gxp::l1::scan::scan_lines_cb;
use gxp::l2::{eval_numeric, EvalResult, ExtractedFields};

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

/// 简化版 L2 判定：用 find_value（与生产 eval_prepared 对齐）
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
    let content_len = bytes.len();

    let q = parse_query(QUERY).expect("解析应成功");
    let compiled = compile(&q.expr).expect("编译应成功");

    println!(
        "content_len={} bytes ({} MB)",
        content_len,
        content_len / 1024 / 1024
    );

    // 预热
    for _ in 0..3 {
        let mut sink: Vec<u8> = Vec::with_capacity(8192);
        let _ = gxp::mode::detect::run_with_writer(
            &[QUERY.to_string(), path.to_string_lossy().to_string()],
            gxp::cli::RunOpts::default(),
            &mut sink,
        );
    }

    // 计时：全管道（含 parse/compile/read_file）
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let _ = gxp::mode::detect::run_with_writer(
                &[QUERY.to_string(), path.to_string_lossy().to_string()],
                gxp::cli::RunOpts::default(),
                &mut sink,
            );
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] 全管道 run_with_writer: {:.2}ms (min={:.2})",
            times[0], times[0]
        );
    }

    // 计时：read_for_scan
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let _content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] read_for_scan: {:.2}ms", times[0]);
    }

    // 计时：仅 L1 扫描（不收集行，只计数）
    {
        let n = 5;
        let mut count = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            count = 0;
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |_ln, _line| {
                count += 1;
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] L1 scan_lines_cb (count only): {:.2}ms (L1 命中 {count} 行)",
            times[0]
        );
    }

    // 计时：L1 + L2，不收集 MatchedLine
    {
        let n = 5;
        let mut l2_hits = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            l2_hits = 0;
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |_ln, line| {
                if l2_match(line) {
                    l2_hits += 1;
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] L1+L2 eval (no collect): {:.2}ms (L2 命中 {l2_hits} 行)",
            times[0]
        );
    }

    // 计时：L1 + L2 + 收集 MatchedLine（Vec<u8> clone）
    {
        let n = 5;
        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            matched.clear();
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |line_no, line| {
                if l2_match(line) {
                    matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        let n_matched = matched.len();
        let total_bytes_cloned: usize = matched.iter().map(|m| m.line.len()).sum();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[阶段] L1+L2+collect (MatchedLine Vec<u8> clone): {:.2}ms (命中 {n_matched} 行, clone 总 {total_bytes_cloned} 字节)", times[0]);
    }

    // 计时：pipeline 执行链（rank + fields）on pre-collected matched
    {
        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        scan_lines_cb(&compiled, bytes, |line_no, line| {
            if l2_match(line) {
                matched.push(gxp::pipeline::MatchedLine::new(line_no, line));
            }
            true
        });

        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let m = matched.clone();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let t0 = std::time::Instant::now();
            let _ = gxp::pipeline::execute_stages(
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
        println!("[阶段] execute_stages (rank+fields): {:.2}ms", times[0]);
    }

    // =====================================================================
    // T1a 归因阶段（2026-11-09，v0.49 Opt A/B/C 落地后改为对照形态）：
    // v0.48 生产路径相对简化阶段多做三件事（归因 ~16ms，perf-baseline §1.2）：
    //   1. PreparedPrim::Literal 的 regex.is_match 每行重校验字面量（eval.rs）→ Opt A 豁免
    //   2. find_match_bytes：每 L2 命中行二次 aho find + Vec 分配 → Opt B 快速路径 + Arc
    //   3. path 填充循环：每命中行 String clone → Opt C Arc<str>
    // 下面保留 v0.48 形态对照（regex 重校验 + 每行分配）与 v0.49 形态复刻
    // （豁免 + 零分配），A/B 差值即优化收益预估。
    // =====================================================================

    // 计时：L1 + Literal regex.is_match（生产 L2 字面量重校验复刻，无谓词）
    {
        let re = regex::bytes::Regex::new("timeout").unwrap();
        let n = 5;
        let mut hits = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            hits = 0;
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |_ln, line| {
                if re.is_match(line) {
                    hits += 1;
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] L1 + Literal regex.is_match 复刻（重校验增量 = 此值 − L1 scan）: {:.2}ms (regex 通过 {hits} 行)",
            times[0]
        );
    }

    // 计时：生产形态 collect v0.48 对照（regex 重校验 + 每行分配）
    // 对齐 v0.48 的 scan_and_collect 非 l1_exact 分支 + path 循环。
    // match_bytes 的 Vec clone 用 Arc::from(slice) 近似（同为一次堆分配，
    // refcount header 差异 <5%）；path 的 String clone 用同款 String clone。
    {
        let re = regex::bytes::Regex::new("timeout").unwrap();
        let ac = aho_corasick::AhoCorasick::new(["timeout"]).unwrap();
        let lit: &[u8] = b"timeout";
        let path_str = path.to_string_lossy().replace('\\', "/");
        let n = 5;
        let mut n_matched = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |line_no, line| {
                if re.is_match(line) && l2_match(line) {
                    let mut m = gxp::pipeline::MatchedLine::new(line_no, line);
                    m.match_bytes = ac.find(line).map(|_| std::sync::Arc::from(lit));
                    matched.push(m);
                }
                true
            });
            for m in &mut matched {
                if m.path.is_none() {
                    m.path = Some(std::sync::Arc::from(path_str.as_str()));
                }
            }
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            n_matched = matched.len();
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] 生产形态 collect v0.48 对照（+regex重校验+每行分配）: {:.2}ms (命中 {n_matched} 行)",
            times[0]
        );
    }

    // 计时：生产形态 collect v0.49（Opt A 豁免 + Opt B/C 零分配复刻）
    // timeout 是 And 侧单元素纯字面量组成员 → verified → 豁免 regex 重校验；
    // match_bytes 单元素组快速路径 → 直接 Arc clone 免 aho 二查；
    // path → 每文件一次 Arc 构造 + refcount clone。
    {
        let lit: std::sync::Arc<[u8]> = std::sync::Arc::from(b"timeout".as_ref());
        let path_arc: std::sync::Arc<str> =
            std::sync::Arc::from(path.to_string_lossy().replace('\\', "/").as_str());
        let n = 5;
        let mut n_matched = 0usize;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
            let t0 = std::time::Instant::now();
            scan_lines_cb(&compiled, bytes, |line_no, line| {
                if l2_match(line) {
                    let mut m = gxp::pipeline::MatchedLine::new(line_no, line);
                    m.match_bytes = Some(std::sync::Arc::clone(&lit));
                    m.path = Some(std::sync::Arc::clone(&path_arc));
                    matched.push(m);
                }
                true
            });
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            n_matched = matched.len();
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[阶段] 生产形态 collect v0.49 复刻（豁免+零分配）: {:.2}ms (命中 {n_matched} 行)",
            times[0]
        );
    }

    // 计时：execute_stages on 生产形态 matched（含 match_bytes/path 的真实结构）
    {
        let lit: std::sync::Arc<[u8]> = std::sync::Arc::from(b"timeout".as_ref());
        let path_arc: std::sync::Arc<str> =
            std::sync::Arc::from(path.to_string_lossy().replace('\\', "/").as_str());
        let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
        scan_lines_cb(&compiled, bytes, |line_no, line| {
            if l2_match(line) {
                let mut m = gxp::pipeline::MatchedLine::new(line_no, line);
                m.match_bytes = Some(std::sync::Arc::clone(&lit));
                m.path = Some(std::sync::Arc::clone(&path_arc));
                matched.push(m);
            }
            true
        });

        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let m = matched.clone();
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let t0 = std::time::Instant::now();
            let _ = gxp::pipeline::execute_stages(
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
        println!(
            "[阶段] execute_stages (生产形态 matched): {:.2}ms",
            times[0]
        );
    }

    // =====================================================================
    // 残差归因阶段（2026-11-09，v0.49 后：run_with_writer 106.74ms − 阶段累计
    // 88.83ms ≈ 17.9ms 未归因；阶段累计 = read 13.37 + L1 33.51 + L2 增量
    // 14.17 + collect 增量 9.62 + execute_stages 18.16）。判别实验用纯
    // public API 复刻，不改生产代码。两个嫌疑桶：
    //   H1 相位隔离：分阶段各自热循环 min（同一长寿命 buffer）vs 端到端串联
    //      单遍——fresh 44MB Vec 冷页/TLB（C2−C3）+ 链内 cache/分配器交错（C3−累计）
    //   H2 生产专属：PreparedScan::build + eval_prepared 机制（vs 简化
    //      l2_match）+ CaptureMap/with_captures + dispatch（run_with_writer−C2）
    // =====================================================================

    // C1: parse_query + compile（生产每次查询都做；阶段累计口径未计入）
    {
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let q1 = parse_query(QUERY).expect("解析应成功");
            let c1 = compile(&q1.expr).expect("编译应成功");
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box((&q1, &c1));
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("[残差] C1 parse_query+compile: {:.2}ms", times[0]);
    }

    // C2: 端到端复刻链（与生产同构：fresh read → parse/compile → scan+L2+
    // collect（v0.49 形态）→ path 填充 → execute_stages → sink write）。
    // 与生产差异全部落入 H2 桶：PreparedScan::build / eval_prepared 机制 /
    // CaptureMap / dispatch / fs::metadata。
    {
        let lit: std::sync::Arc<[u8]> = std::sync::Arc::from(b"timeout".as_ref());
        let path_arc: std::sync::Arc<str> =
            std::sync::Arc::from(path.to_string_lossy().replace('\\', "/").as_str());
        let path_str = path.to_string_lossy().to_string();
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let content = gxp::io::file::read_for_scan(std::path::Path::new(&path)).unwrap();
            let bytes2 = content.as_bytes();
            let q2 = parse_query(QUERY).expect("解析应成功");
            let compiled2 = compile(&q2.expr).expect("编译应成功");
            let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
            scan_lines_cb(&compiled2, bytes2, |line_no, line| {
                if l2_match(line) {
                    let mut m = gxp::pipeline::MatchedLine::new(line_no, line);
                    m.match_bytes = Some(std::sync::Arc::clone(&lit));
                    m.path = Some(std::sync::Arc::clone(&path_arc));
                    matched.push(m);
                }
                true
            });
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let _ = gxp::pipeline::execute_stages(
                &q2.stages, matched, bytes2, &path_str, &mut sink, false,
            );
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&sink);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[残差] C2 端到端复刻链（fresh read→parse/compile→collect→stages→write）: {:.2}ms",
            times[0]
        );
    }

    // C3: 同 C2 但扫描长寿命 warm buffer（跳过 read_for_scan）——
    // C2 − C3 = fresh 44MB Vec 分配 + 冷页 + 冷 TLB 扫描的效应（H1a）
    {
        let lit: std::sync::Arc<[u8]> = std::sync::Arc::from(b"timeout".as_ref());
        let path_arc: std::sync::Arc<str> =
            std::sync::Arc::from(path.to_string_lossy().replace('\\', "/").as_str());
        let path_str = path.to_string_lossy().to_string();
        let n = 5;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t0 = std::time::Instant::now();
            let q3 = parse_query(QUERY).expect("解析应成功");
            let compiled3 = compile(&q3.expr).expect("编译应成功");
            let mut matched: Vec<gxp::pipeline::MatchedLine> = Vec::new();
            scan_lines_cb(&compiled3, bytes, |line_no, line| {
                if l2_match(line) {
                    let mut m = gxp::pipeline::MatchedLine::new(line_no, line);
                    m.match_bytes = Some(std::sync::Arc::clone(&lit));
                    m.path = Some(std::sync::Arc::clone(&path_arc));
                    matched.push(m);
                }
                true
            });
            let mut sink: Vec<u8> = Vec::with_capacity(8192);
            let _ = gxp::pipeline::execute_stages(
                &q3.stages, matched, bytes, &path_str, &mut sink, false,
            );
            times.push(t0.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&sink);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "[残差] C3 复刻链（warm buffer，无 read）: {:.2}ms",
            times[0]
        );
    }
}
