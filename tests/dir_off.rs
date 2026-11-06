//! 移植 `prototype/test_gxp_dir.py::TestFreshnessOff`（3 项）。
//!
//! 覆盖 DirIndex off 模式语义：
//! - 零 syscall（index_stats=0, readdir_calls=0）
//! - lax 仍做 readdir（诚实约束：不 readdir 就无法发现改名/新增）
//! - off 过期视图（看不见新文件——这是"缓存不能保证正确性"的证据）

use std::fs;
use std::path::Path;

use gxp::dir::index::{DirIndex, FreshnessMode};
use gxp::dir::plan::PathPlan;
use tempfile::TempDir;

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
    fs::write(path, content).unwrap();
}

fn fixture(root: &Path) -> usize {
    const SHARDS: usize = 3;
    let mut n = 0;
    for day in DAY_STRINGS.iter() {
        for i in 0..SHARDS {
            let p = root.join("logs").join(day).join(format!("app.{}.log", i));
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut content = String::new();
            for k in 0..20 {
                content.push_str(&format!(
                    "{}T00:00:{:02}Z INFO [checkout] timeout cost_ms={} req={}\n",
                    day, k, k, k
                ));
            }
            fs::write(&p, content).unwrap();
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

#[test]
fn test_off_mode_makes_zero_syscalls() {
    // off = 完全跳过校验：第二次列举不产生 stat，也不 readdir
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);
    idx.list(&plan);
    let (_, st) = idx.list(&plan);
    assert_eq!(st.index_stats, 0, "off 不应 stat: {:?}", st);
    assert_eq!(st.readdir_calls, 0, "off 不应 readdir: {:?}", st);
}

#[test]
fn test_lax_still_readdir_to_validate() {
    // 诚实约束：lax 省的是逐文件 stat，不是 readdir
    // 不 readdir 就无法发现改名/新增，违背 fail-loud
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
    idx.list(&plan);
    let (_, st) = idx.list(&plan);
    assert!(st.readdir_calls > 0, "lax 仍应 readdir: {:?}", st);
    assert_eq!(st.readdir_calls, st.dirs_visited);
}

#[test]
fn test_off_mode_returns_stale_listing_after_new_file() {
    // off 的代价：看不见新文件——这正是"缓存不能保证正确性"的证据
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);
    let (f1, _) = idx.list(&plan);
    write_str(
        &tmp.path().join("logs").join("2026-10-20").join("app.9.log"),
        "x\n",
    );
    let (f2, _) = idx.list(&plan);
    assert_eq!(f1.len(), f2.len(), "off 模式不应发现新文件（过期）");
    assert!(
        !f2.iter().any(|p| p.to_string_lossy().contains("app.9.log")),
        "off 不应看到新文件"
    );
}
