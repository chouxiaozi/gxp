//! 规模化验证基准——移植自 `prototype/bench_dir.py` 6 项实验。
//!
//! 与 Python `bench_dir.py` 的对齐点：
//! - 数据规模：DAYS=30, SHARDS=2, LINES=40000, 300 个 src/api/m*.py
//! - 6 项实验：路径谓词剪枝 / 索引三档新鲜度 / append-only 增量 /
//!   分区并集 = 全树 / git commit 锚点 / 总览
//! - 输出格式：与 Python 报告对齐（ms + 目录数 + 文件数 + MB），便于对比
//!
//! 不引入新 crate（AGENTS.md §7.3）：
//! - 不用 `rand`：Python 的 `random.randint(1,3000)` 改用确定性循环 `k % 3000 + 1`
//! - 不用 `criterion::bench_function`：Python 是单次跑报告，这里保持单次跑语义
//!   （criterion 微基准由 `benches/l1_scan.rs` 等承担）
//! - 用 `std::time::Instant` 测量（对齐 Python `time.perf_counter`）
//!
//! 运行：`cargo bench --bench scale_dir`（harness=false，直接跑 main）
//!
//! 对照基线（Python bench_dir.py，Windows，2026-10-20）：
//! ```text
//! 构建语料 5168 ms｜目录 35｜文件 361｜数据 156.4 MB
//! 全树 .log               4.43 ms｜目录访问 35/35｜剪掉  0｜文件 60｜读 156.4 MB
//! 单日 logs/2026-10-20    0.58 ms｜目录访问  3/33｜剪掉 30｜文件  2｜读 5.2 MB
//! 六天 dir_re(不可剪枝)    3.49 ms｜目录访问 35/35｜剪掉  0｜文件 12｜读 31.3 MB
//! src 下 .py              0.78 ms｜目录访问  4/5｜剪掉 1｜文件 300｜读 0.0 MB
//! 深度≤1                  0.80 ms｜目录访问  3/34｜剪掉 31｜文件 1｜读 0.0 MB
//! strict 首次 3.69 ms → 再次 3.44 ms｜stat 361
//! lax    首次 3.45 ms → 再次 3.26 ms｜stat   0
//! off    首次 3.66 ms → 再次 0.07 ms｜readdir 0
//! lax 遇新增: 3.18 ms｜复用 34｜变更 1
//! append 8w: 5.5 MB / 1.9 ms｜跳过 32.1% 重复扫描
//! 分区并集: 全树 944,032 行 / 724 ms｜按天 30 次 5546 ms｜一致=True
//! git commit_key: 无 HEAD None｜提交后 311 ms｜工作区改动 317 ms
//! ```

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gxp::dir::git::commit_key;
use gxp::dir::index::{DirIndex, FreshnessMode};
use gxp::dir::plan::PathPlan;
use gxp::dir::tail::TailIndex;

use regex::bytes::Regex as BytesRegex; // 行扫描用 bytes 版本
use regex::Regex; // PathPlan::with_dir_re 需要 string 版本
use tempfile::TempDir;

/// 手工格式化千分位（Rust 标准库不支持 Python 的 `{:,}`）。
fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// 规模参数（与 Python `bench_dir.py` 顶部常量对齐）。
const DAYS: usize = 30;
const SHARDS: usize = 2;
const LINES: usize = 40_000;
const SRC_FILES: usize = 300;

/// 计时器：对齐 Python `timed(fn) = (ms, result)`。
fn timed<R>(f: impl FnOnce() -> R) -> (Duration, R) {
    let t0 = Instant::now();
    let r = f();
    (t0.elapsed(), r)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

fn total_bytes(files: &[PathBuf]) -> u64 {
    files
        .iter()
        .filter_map(|p| fs::metadata(p).ok().map(|m| m.len()))
        .sum()
}

/// 构建语料：logs/YYYY-MM-DD/app.N.log（DAYS × SHARDS × LINES 行）
///         + src/api/m{0..300}.py + README.md
///
/// 与 Python `bench_dir.py::build()` 对齐：
/// - 日期：从 2026-10-20 倒推 DAYS 天
/// - 每行：`{day}T{H:02}:{M:02}:{S:02}Z INFO [{svc}] {kw} cost_ms={c} req={k}\n`
/// - Python 用 random.choice / random.randint，这里用确定性循环替代
///   （svc 5 选 1，kw 5 选 1，cost_ms 1..=3000）
fn build_corpus(root: &Path) -> Duration {
    let t0 = Instant::now();
    // base date = 2026-10-20，倒推 d 天
    // Python: (datetime(2026, 10, 20) - timedelta(days=d)).strftime("%Y-%m-%d")
    // 为了不引入 chrono，手工计算 d 天前的日期（d ≤ 30，月份 10 月，无需跨月）
    // 2026-10-20 是第 293 天（非闰年）；倒推 30 天到 2026-09-20。
    // 简化：直接用 day_of_month = 20 - d，跨月用 9 月补足。
    let svcs = ["checkout", "billing", "auth", "cart", "search"];
    let kws = ["timeout", "latency", "retry", "cache", "refund"];

    // 优化：BufWriter + 批量写（早期逐行 write_all = 40000 次 syscall/文件，
    // Python 用 buffered write 5.2s，Rust 10.6s = 2× 慢）。
    // 预分配 line buffer 避免每行 format! alloc——但保持与 Python 行格式逐字节对齐。
    use std::io::BufWriter;
    for d in 0..DAYS {
        // 10/20 - d：d ≤ 19 在 10 月，d=20..29 在 9 月
        let (year, month, day) = if d <= 19 {
            (2026, 10, 20 - d)
        } else {
            // 9 月：20 - (d-19) = 39 - d，d=20 → 19，d=29 → 10
            (2026, 9, 39 - d)
        };
        let day_str = format!("{}-{:02}-{:02}", year, month, day);
        for i in 0..SHARDS {
            let p = root
                .join("logs")
                .join(&day_str)
                .join(format!("app.{}.log", i));
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            let f = fs::File::create(&p).unwrap();
            let mut w = BufWriter::new(f);
            for k in 0..LINES {
                let svc = svcs[k % svcs.len()];
                let kw = kws[k % kws.len()];
                // Python: random.randint(1,3000)；用 k % 3000 + 1 替代
                let cost = k % 3000 + 1;
                // Python: f"{day}T{k//600:02}:{k//60%60:02}:{k%60:02}Z INFO [svc] kw cost_ms=c req=k\n"
                let line = format!(
                    "{}T{:02}:{:02}:{:02}Z INFO [{}] {} cost_ms={} req={}\n",
                    day_str,
                    k / 600,
                    k / 60 % 60,
                    k % 60,
                    svc,
                    kw,
                    cost,
                    k
                );
                w.write_all(line.as_bytes()).unwrap();
            }
            w.flush().unwrap();
        }
    }

    // src/api/v1 + 300 个 m{N}.py + README.md
    // （300 个小文件用 fs::write 即可，单文件 15 字节，BufWriter 无收益）
    fs::create_dir_all(root.join("src").join("api").join("v1")).unwrap();
    for n in 0..SRC_FILES {
        let p = root.join("src").join("api").join(format!("m{}.py", n));
        fs::write(&p, "def f(): pass\n").unwrap();
    }
    fs::write(root.join("README.md"), "# x\n").unwrap();

    t0.elapsed()
}

fn print_section(title: &str) {
    println!("\n== {} ==", title);
}

/// 实验 1：路径谓词——遍历层剪枝 vs 后过滤。
/// 5 个 plan × walk，对齐 Python §1。
fn exp1_path_predicates(root: &Path, all_files: &[PathBuf], tree_bytes: u64) {
    println!(
        "  {:22} {:>7} {:>18} {:>10} {:>8} {:>10}",
        "plan", "ms", "目录访问", "剪掉", "文件", "需读 MB"
    );
    let plans: Vec<(&str, PathPlan)> = vec![
        ("全树 .log", PathPlan::new().with_ext(["log".to_string()])),
        (
            "单日 logs/2026-10-20",
            PathPlan::new()
                .with_ext(["log".to_string()])
                .with_dir_prefix("logs/2026-10-20"),
        ),
        (
            "六天 dir_re(不可剪枝)",
            PathPlan::new()
                .with_ext(["log".to_string()])
                .with_dir_re(Regex::new(r"2026-10-(1[5-9]|20)").unwrap()),
        ),
        (
            "src 下 .py",
            PathPlan::new()
                .with_ext(["py".to_string()])
                .with_dir_prefix("src"),
        ),
        ("深度≤1", PathPlan::new().with_max_depth(1)),
    ];
    for (label, plan) in plans {
        let (ms, (files, st)) = timed(|| plan.walk(root));
        let b = total_bytes(&files);
        let pct = if tree_bytes > 0 {
            b as f64 / tree_bytes as f64 * 100.0
        } else {
            0.0
        };
        let _ = all_files; // 占位：Python 用 allf 做总览，这里省略
        println!(
            "  {:22} {:7.2} ms｜目录访问 {:>3}/{:<3}｜剪掉 {:>3}｜文件 {:>5}｜需读 {:>6.1} MB = {:5.1}%",
            label,
            ms.as_secs_f64() * 1000.0,
            st.dirs_visited,
            st.dirs_total,
            st.pruned_dirs,
            files.len(),
            mb(b),
            pct
        );
    }
}

/// 实验 2：索引三档新鲜度（同一棵树连续两次列举）+ lax 遇新增文件。
fn exp2_freshness_modes(root: &Path, plan: &PathPlan) {
    let modes = [
        ("strict", FreshnessMode::Strict),
        ("lax", FreshnessMode::Lax),
        ("off", FreshnessMode::Off),
    ];
    for (label, mode) in modes {
        let mut idx = DirIndex::new(root, mode);
        let (ms1, (_f1, s1)) = timed(|| idx.list(plan));
        let (ms2, (_f2, s2)) = timed(|| idx.list(plan));
        println!(
            "  {:6} 首次 {:7.2} ms (readdir {:>3}, stat {:>5}) → 再次 {:7.2} ms (readdir {:>3}, stat {:>5}, 复用 {:>3})",
            label,
            ms1.as_secs_f64() * 1000.0,
            s1.readdir_calls,
            s1.index_stats,
            ms2.as_secs_f64() * 1000.0,
            s2.readdir_calls,
            s2.index_stats,
            s2.dirs_reused
        );
    }

    // lax 遇新增文件
    let mut idx = DirIndex::new(root, FreshnessMode::Lax);
    let _ = idx.list(plan);
    let newp = root.join("logs").join("2026-10-20").join("app.new.log");
    fs::write(
        &newp,
        "2026-10-20T00:00:00Z INFO [checkout] timeout cost_ms=9999 req=0\n",
    )
    .unwrap();
    let (ms, (files, s)) = timed(|| idx.list(plan));
    let visible = files
        .iter()
        .any(|p| p.to_string_lossy().contains("app.new.log"));
    println!(
        "  lax 遇新增文件: {:7.2} ms｜复用 {}｜变更 {}｜新文件可见={}",
        ms.as_secs_f64() * 1000.0,
        s.dirs_reused,
        s.dirs_changed,
        visible
    );
    let _ = fs::remove_file(&newp);
}

/// 实验 3：append-only 尾部增量。
fn exp3_append_only(root: &Path) {
    let big = root.join("logs").join("2026-10-20").join("app.0.log");
    let mut t = TailIndex::new();

    let (ms1, r1) = timed(|| t.check(&big));
    let (ms_full, d_full) = timed(|| fs::read(&big).unwrap());

    // 追加 8 万行
    {
        let mut f = fs::OpenOptions::new().append(true).open(&big).unwrap();
        for k in 0..80_000 {
            let line = format!(
                "2026-10-20T10:00:00Z WARN [checkout] timeout cost_ms={} req={}\n",
                k % 2200 + 801,
                k
            );
            f.write_all(line.as_bytes()).unwrap();
        }
        f.flush().unwrap();
    }

    let (ms2, r2) = timed(|| t.check(&big));
    let (ms_tail, d_tail) = timed(|| {
        let mut f = fs::File::open(&big).unwrap();
        use std::io::{Read, Seek, SeekFrom};
        f.seek(SeekFrom::Start(r2.start)).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        buf
    });

    let (ms3, r3) = timed(|| t.check(&big));

    // 截断（轮转）
    let _ = fs::write(&big, "x\n");
    let (ms4, r4) = timed(|| t.check(&big));

    println!(
        "  首次 {:?}: 读 {:.1} MB / {:.1} ms",
        r1.reason,
        mb(d_full.len() as u64),
        ms1.as_secs_f64() * 1000.0
    );
    let skip_pct = if r2.total > 0 {
        1.0 - r2.delta as f64 / r2.total as f64
    } else {
        0.0
    };
    println!(
        "  追加 8 万行后 {:?}: 只读 {:.1} MB / {:.1} ms｜跳过 {:.1}% 重复扫描",
        r2.reason,
        mb(d_tail.len() as u64),
        ms2.as_secs_f64() * 1000.0,
        skip_pct * 100.0
    );
    println!(
        "  无新写再查: {:?} delta={} B（{:.3} ms）",
        r3.reason,
        r3.delta,
        ms3.as_secs_f64() * 1000.0
    );
    println!(
        "  日志被截断(轮转): {:?} → 强制全扫 start={}（{:.2} ms）",
        r4.reason,
        r4.start,
        ms4.as_secs_f64() * 1000.0
    );
    let _ = ms_full;
    let _ = ms_tail;
}

/// 实验 4：分区并集 == 全树（正确性锚点）。
///
/// Python 用 subprocess 跑 `grep -cE 'timeout|latency' files`，Windows 下 grep
/// 不一定可用。改用 Rust 正则直接扫——不影响"分区并集 = 全树"的正确性锚点
/// （扫的是同一份内容，分区与全树结果必然相等）。
fn exp4_partition_union(root: &Path, all_files: &[PathBuf]) {
    let re = BytesRegex::new(r"timeout|latency").unwrap();

    let count_hits = |files: &[PathBuf]| -> usize {
        let mut n = 0;
        for f in files {
            if let Ok(bytes) = fs::read(f) {
                // 按行扫，统计含 `timeout` 或 `latency` 的行数
                for line in bytes.split(|b| *b == b'\n') {
                    if re.is_match(line) {
                        n += 1;
                    }
                }
            }
        }
        n
    };

    let (ms_all, n_all) = timed(|| count_hits(all_files));

    // 按天分区：DAYS 天各跑一次
    let (ms_union, union_n) = timed(|| {
        let mut u = 0;
        for d in 0..DAYS {
            let (year, month, day) = if d <= 19 {
                (2026, 10, 20 - d)
            } else {
                (2026, 9, 39 - d)
            };
            let day_str = format!("{}-{:02}-{:02}", year, month, day);
            let plan = PathPlan::new()
                .with_ext(["log".to_string()])
                .with_dir_prefix(format!("logs/{}", day_str));
            let (files, _) = plan.walk(root);
            u += count_hits(&files);
        }
        u
    });

    println!(
        "  全树一次扫: {} 行 / {:.0} ms",
        fmt_thousands(n_all),
        ms_all.as_secs_f64() * 1000.0
    );
    println!(
        "  按天分区扫 {} 次再并集: {} 行 / {:.0} ms｜一致={}",
        DAYS,
        fmt_thousands(union_n),
        ms_union.as_secs_f64() * 1000.0,
        union_n == n_all
    );
}

/// 实验 5：git commit 锚点。
fn exp5_commit_key(root: &Path, plan: &PathPlan) {
    // 注意：与 Python 一致，先 init 仓库（此时尚无 commit，rev-parse 失败 → None）
    let _ = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("init")
        .arg("-q")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let (ms, k1) = timed(|| commit_key(root));
    println!(
        "  无 HEAD 时 commit_key={:?}（{:.1} ms）",
        k1,
        ms.as_secs_f64() * 1000.0
    );

    // 配置 user + 提交 README.md
    for args in [
        ["config", "user.email", "t@t"].as_slice(),
        ["config", "user.name", "t"].as_slice(),
        ["add", "README.md"].as_slice(),
        ["commit", "-qm", "c1"].as_slice(),
    ] {
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    let (ms, k2) = timed(|| commit_key(root));
    let head_short = k2
        .as_ref()
        .map(|k| k.head.get(..8).unwrap_or(&k.head))
        .unwrap_or("(None)");
    let dirty = k2.as_ref().map(|k| k.dirty).unwrap_or(false);
    println!(
        "  提交后: head={} dirty={}（{:.1} ms 一次调用拿到全量状态）",
        head_short,
        dirty,
        ms.as_secs_f64() * 1000.0
    );

    // 工作区改动
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(root.join("README.md"))
        .unwrap();
    f.write_all(b"more\n").unwrap();
    drop(f);

    let (ms, k3) = timed(|| commit_key(root));
    let dirty3 = k3.as_ref().map(|k| k.dirty).unwrap_or(false);
    println!(
        "  工作区改动: dirty={}（head 未变，{:.1} ms）→ 缓存 key 可用 (repo,head,dirty)",
        dirty3,
        ms.as_secs_f64() * 1000.0
    );

    // 对照：git 目录上 lax 列树
    let mut idx = DirIndex::new(root, FreshnessMode::Lax);
    let (ms, (_f, s)) = timed(|| idx.list(plan));
    println!(
        "  对照：git 目录上 lax 列树 {:.2} ms（readdir {}, stat {}），比 git 调用便宜一个量级",
        ms.as_secs_f64() * 1000.0,
        s.readdir_calls,
        s.index_stats
    );
}

fn main() {
    // 用 TempDir 隔离，不污染 /tmp/w/repo_tree（Python 硬编码路径）
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    println!("== 构建语料 ==");
    let ms = build_corpus(&root);
    let plan_all = PathPlan::new().with_ext(["log".to_string()]);
    let (all_files, st_all) = plan_all.walk(&root);
    let tree_bytes = total_bytes(&all_files);
    println!(
        "构建 {:.0} ms｜目录 {}｜文件 {}｜数据 {:.1} MB",
        ms.as_secs_f64() * 1000.0,
        st_all.dirs_total,
        all_files.len(),
        mb(tree_bytes)
    );

    print_section("1) 路径谓词：遍历层剪枝 vs 后过滤");
    exp1_path_predicates(&root, &all_files, tree_bytes);

    print_section("2) 索引三档新鲜度（同一棵树连续两次列举）");
    exp2_freshness_modes(&root, &plan_all);

    print_section("3) append-only 尾部增量");
    exp3_append_only(&root);

    print_section("4) 分区并集 == 全树（剪枝不改变结果，正确性锚点）");
    exp4_partition_union(&root, &all_files);

    print_section("5) git commit 锚点（代码库场景的更新策略）");
    exp5_commit_key(&root, &plan_all);
}
