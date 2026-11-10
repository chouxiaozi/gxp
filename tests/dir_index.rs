//! 移植 `prototype/test_gxp_dir.py::TestDirIndex`（6 项）。
//!
//! 覆盖 DirIndex 三模式（lax/strict/off 中的 lax + strict + 指纹变化 + symlink 环 +
//! stat 量级）。off 模式的 3 项测试在 `tests/dir_off.rs`。
//!
//! fixture：与 `tests/dir_pruning.rs` 共用（logs/YYYY-MM-DD/app.N.log × 6 天 × 3 分片
//! + src/{main,api/routes,api/v1/users}.py + note.txt，共 22 个文件）。

use std::fs;
use std::path::Path;

use gxp::dir::index::{DirIndex, FreshnessMode};
use gxp::dir::plan::PathPlan;
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

fn write_bytes(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn write_str(path: &Path, content: &str) {
    write_bytes(path, content.as_bytes());
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
fn test_lax_mode_skips_per_file_stat_when_dir_unchanged() {
    // lax 复用后，index_stats 应显著下降（只 readdir 不 stat）
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
    let (_, st1) = idx.list(&plan);
    let (_, st2) = idx.list(&plan);
    assert!(st2.dirs_reused > 0, "第二次应复用缓存: {:?}", st2);
    assert!(
        st2.index_stats < st1.index_stats,
        "第二次 stat 次数应更少: st1={}, st2={}",
        st1.index_stats,
        st2.index_stats
    );
}

#[test]
fn test_strict_mode_never_reuses() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Strict);
    idx.list(&plan);
    let (_, st) = idx.list(&plan);
    assert_eq!(st.dirs_reused, 0, "strict 不应复用: {:?}", st);
}

#[test]
fn test_fingerprint_detects_new_file_in_known_dir() {
    // 在已索引目录里新增文件 → 指纹变 → dirs_changed > 0，且新文件应被列出
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
    idx.list(&plan);
    write_str(
        &tmp.path().join("logs").join("2026-10-20").join("app.9.log"),
        "x\n",
    );
    let (files, st) = idx.list(&plan);
    assert!(
        files
            .iter()
            .any(|p| { p.to_string_lossy().contains("app.9.log") }),
        "新文件应出现在结果中"
    );
    assert!(st.dirs_changed > 0, "应有目录指纹变化: {:?}", st);
}

#[test]
fn test_symlink_loop_terminates() {
    // follow_links=true 时遇到 symlink 环应终止，不死循环
    // （依赖手写 seen_real 去重，与 Python 一致）
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let link_path = tmp.path().join("logs").join("selflink");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        match symlink(tmp.path(), &link_path) {
            Ok(_) => {
                let plan = PathPlan::new().with_follow_links(true).with_max_depth(6);
                let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
                let (files, st) = idx.list(&plan);
                // 能返回就说明没死循环
                let _ = files;
                assert!(st.links_skipped > 0, "应跳过环: {:?}", st);
            }
            Err(e) => {
                eprintln!("skip symlink test: {}", e);
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Windows：开发者模式/管理员可建 symlink 时走真实验证（B16 后三平台同覆盖），
        // 无特权（普通用户会话）则降级 smoke test
        match std::os::windows::fs::symlink_dir(tmp.path(), &link_path) {
            Ok(_) => {
                let plan = PathPlan::new().with_follow_links(true).with_max_depth(6);
                let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
                let (files, st) = idx.list(&plan);
                let _ = files;
                assert!(st.links_skipped > 0, "应跳过环: {:?}", st);
            }
            Err(e) => {
                eprintln!("skip symlink test (no privilege): {}", e);
                let plan = PathPlan::new().with_follow_links(true);
                let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
                let (files, _) = idx.list(&plan);
                assert!(files.iter().all(|f| f.is_file()));
            }
        }
    }
}

#[test]
fn test_index_stat_calls_scale_with_directories_not_files() {
    // lax 复用后，index_stats 应与目录数同阶，而不是文件数
    // Python 断言：st["index_stats"] <= st["dirs_visited"] + 2
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
    idx.list(&plan);
    let (_, st) = idx.list(&plan);
    // lax 复用：所有已缓存目录不再 stat 文件，index_stats 应为 0
    // （严格遵循 Python 断言：<= dirs_visited + 2）
    assert!(
        st.index_stats <= st.dirs_visited + 2,
        "stat 量级应与目录数同阶: index_stats={}, dirs_visited={}",
        st.index_stats,
        st.dirs_visited
    );
}

#[test]
fn test_lax_readdir_still_happens_after_cache() {
    // 诚实约束：lax 省的是逐文件 stat，不是 readdir
    // （不 readdir 就无法发现改名/新增，违背 fail-loud）
    // Python TestFreshnessOff.test_lax_still_readdir_to_validate
    // —— 但 TestDirIndex 未直接断言 readdir_calls，这里补一个对齐
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);
    idx.list(&plan);
    let (_, st) = idx.list(&plan);
    assert!(st.readdir_calls > 0, "lax 仍应 readdir: {:?}", st);
    assert_eq!(st.readdir_calls, st.dirs_visited);
}
