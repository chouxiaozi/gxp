//! 移植自 `prototype/test_gxp_dir.py::TestDirRegex`（3 项）。
//!
//! 关键不变式：dir_re（无锚定正则）**不剪枝**，只做后过滤（§12.3）。
//! 即 `dirs_visited == dirs_total`，`pruned_dirs == 0`。
//! 剪枝只能用 `dir_prefix`（祖先/后代双向判定）。

use std::fs;
use std::path::{Path, PathBuf};

use gxp::dir::plan::PathPlan;
use regex::Regex;
use tempfile::TempDir;

/// 固定日期序列（2026-10-20 倒推 6 天）。
const DAY_STRINGS: [&str; 6] = [
    "2026-10-20",
    "2026-10-19",
    "2026-10-18",
    "2026-10-17",
    "2026-10-16",
    "2026-10-15",
];

fn write_str(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content.as_bytes()).unwrap();
}

/// 与 Python `fixture(root, days=6, shards=3, extra=True)` 一致。
fn fixture(root: &Path) -> usize {
    const SHARDS: usize = 3;
    let mut n = 0;
    for day in DAY_STRINGS.iter() {
        for i in 0..SHARDS {
            let p = root.join("logs").join(day).join(format!("app.{}.log", i));
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&p, b"line\n").unwrap();
            n += 1;
        }
    }
    for sub in ["src", "src/api", "src/api/v1"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    write_str(&root.join("src").join("main.py"), "def main(): pass\n");
    write_str(
        &root.join("src").join("api").join("routes.py"),
        "def get(): pass\n",
    );
    write_str(
        &root.join("src").join("api").join("v1").join("users.py"),
        "class U: pass\n",
    );
    write_str(&root.join("note.txt"), "hello\n");
    n += 4;
    n
}

/// Python `os.walk` 暴力对账端口：全量遍历 + 同谓词判定。
fn bruteforce_walk(root: &Path, plan: &PathPlan) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dp) = stack.pop() {
        let entries = match fs::read_dir(&dp) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() && plan.match_path(&entry.path(), root) {
                out.push(entry.path());
            }
        }
    }
    out
}

#[test]
fn test_dir_re_finds_files_in_matching_dirs() {
    // 正则路径谓词必须能找到命中目录里的文件（当前实现错误地把它当祖先约束剪掉了）。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new()
        .with_ext(["log".to_string()])
        .with_dir_re(Regex::new(r"2026-10-(1[5-9]|20)").unwrap());
    let (files, _st) = plan.walk(tmp.path());
    assert_eq!(
        files.len(),
        18,
        "6 天 × 3 分片应为 18，实际 {}",
        files.len()
    );
}

#[test]
fn test_dir_re_soundness_against_bruteforce() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new()
        .with_ext(["log".to_string()])
        .with_dir_re(Regex::new(r"2026-10-(1[5-9]|20)").unwrap());
    let (fast, _st) = plan.walk(tmp.path());
    let mut slow = bruteforce_walk(tmp.path(), &plan);
    let mut fast_sorted = fast.clone();
    fast_sorted.sort();
    slow.sort();
    assert_eq!(fast_sorted, slow);
}

#[test]
fn test_dir_re_cannot_prune_only_prefix_can() {
    // 诚实约束：无锚定的 dir_re 不剪枝，dirs_visited 必须等于 dirs_total。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let (_files, st) = PathPlan::new()
        .with_dir_re(Regex::new(r"2026-10-20").unwrap())
        .walk(tmp.path());
    assert_eq!(
        st.dirs_visited, st.dirs_total,
        "dir_re 不应剪枝：dirs_visited={} dirs_total={}",
        st.dirs_visited, st.dirs_total
    );
    assert_eq!(st.pruned_dirs, 0, "dir_re 不应产生 pruned_dirs");
}
