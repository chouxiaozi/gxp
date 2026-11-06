//! 移植自 `prototype/test_gxp_dir.py::TestPruning`（9 项）。
//!
//! fixture：logs/YYYY-MM-DD/app.N.log（6 天 × 3 分片 = 18 个 .log）
//!        + src/main.py + src/api/routes.py + src/api/v1/users.py + note.txt
//! 日期范围：2026-10-15 ~ 2026-10-20（从 2026-10-20 倒推 6 天）

use std::fs;
use std::path::{Path, PathBuf};

use gxp::dir::plan::PathPlan;
use regex::Regex;
use tempfile::TempDir;

/// 固定日期序列（2026-10-20 倒推 6 天）。
/// 避免引入 chrono crate（AGENTS.md §7.3 不允许新增依赖）。
const DAY_STRINGS: [&str; 6] = [
    "2026-10-20",
    "2026-10-19",
    "2026-10-18",
    "2026-10-17",
    "2026-10-16",
    "2026-10-15",
];

// 写入 bytes（不经过 CRLF 转换，与 Python `_w(p, txt)` 二进制写入对齐）。
fn write_bytes(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

// 写入字符串（UTF-8）。
fn write_str(path: &Path, content: &str) {
    write_bytes(path, content.as_bytes());
}

// 与 Python `fixture(root, days=6, shards=3, extra=True)` 一致。
// today 固定为 2026-10-20（与 Python `datetime(2026, 10, 20)` 对齐）。
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
    // extra: src/main.py + src/api/routes.py + src/api/v1/users.py + note.txt
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

// 归一化路径分隔符到 forward-slash（对齐 Python 测试里的 `.replace(os.sep, "/")`）。
fn norm(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

// Python `os.walk` 暴力对账端口：全量遍历 + 同谓词判定。
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

// 计算文件所在目录相对 root 的嵌套深度（forward-slash 分割）。
// root=0；root 下一级目录=1；以此类推。
fn dir_depth(f: &Path, root: &Path) -> usize {
    let rel = f.strip_prefix(root).unwrap_or(f);
    let parent = rel.parent().unwrap_or(Path::new(""));
    let s = parent.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        0
    } else {
        s.split('/').count()
    }
}

#[test]
fn test_ext_predicate_selects_only_matching_files() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_ext(["log".to_string()]);
    let (files, _st) = plan.walk(tmp.path());
    assert!(!files.is_empty());
    assert!(files
        .iter()
        .all(|f| f.extension().and_then(|e| e.to_str()) == Some("log")));
    assert_eq!(files.len(), 18);
}

#[test]
fn test_dir_prefix_prunes_sibling_directories_at_traversal() {
    // 谓词必须在 readdir 阶段剪枝，不是遍历后再过滤。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_dir_prefix("logs/2026-10-20");
    let (files, st) = plan.walk(tmp.path());
    let norm_files: Vec<String> = files.iter().map(|f| norm(f)).collect();
    assert!(!norm_files.is_empty());
    assert!(norm_files.iter().all(|f| f.contains("logs/2026-10-20/")));
    assert!(
        st.pruned_dirs > 0,
        "没有目录被剪枝说明是后过滤而非下推: {:?}",
        st
    );
    assert!(st.dirs_visited < st.dirs_total);
}

#[test]
fn test_depth_predicate_limits_traversal() {
    // max_depth=N 指"文件所在目录的嵌套深度 ≤ N"（root=0）。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    // 验证 fixture 里 src/api/v1/users.py 真实存在
    let v1_file = tmp
        .path()
        .join("src")
        .join("api")
        .join("v1")
        .join("users.py");
    assert!(v1_file.exists(), "fixture 缺失: {}", v1_file.display());
    let (files, st) = PathPlan::new().with_max_depth(2).walk(tmp.path());
    let root = tmp.path();
    assert!(!files.is_empty());
    let violators: Vec<_> = files.iter().filter(|f| dir_depth(f, root) > 2).collect();
    let violators_preview: Vec<&&PathBuf> = violators.iter().take(3).collect();
    assert!(violators.is_empty(), "超深度: {:?}", violators_preview);
    assert!(!files.contains(&root.join("src").join("api").join("v1").join("users.py")));
    assert!(
        st.pruned_dirs > 0,
        "pruned_dirs={}, dirs_total={}",
        st.pruned_dirs,
        st.dirs_total
    );
}

#[test]
fn test_max_depth_zero_only_root_files() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let (files, _st) = PathPlan::new().with_max_depth(0).walk(tmp.path());
    assert!(files.iter().all(|f| f.parent() == Some(tmp.path())));
    assert_eq!(files, vec![tmp.path().join("note.txt")]);
}

#[test]
fn test_name_regex_predicate() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new().with_name_re(Regex::new(r"^app\.[01]\.log$").unwrap());
    let (files, _st) = plan.walk(tmp.path());
    assert_eq!(files.len(), 12);
}

#[test]
fn test_pruned_walk_equals_bruteforce() {
    // 超集不变式：剪枝结果必须等于全量遍历 + 同样的谓词判定。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let plan = PathPlan::new()
        .with_ext(["log".to_string(), "py".to_string()])
        .with_dir_prefix("logs/2026-10-1")
        .with_name_re(Regex::new(r"app\.\d").unwrap());
    let (fast, _st) = plan.walk(tmp.path());
    let mut slow = bruteforce_walk(tmp.path(), &plan);
    let mut fast_sorted = fast.clone();
    fast_sorted.sort();
    slow.sort();
    assert_eq!(fast_sorted, slow);
}

#[test]
fn test_min_depth_skips_shallow_files() {
    // min_depth=N 指"文件所在目录的嵌套深度 ≥ N"（root=0）。
    // Python 原型有 min_depth 字段但 TestPruning 未直接覆盖；此处补一个直接测试。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let (files, _st) = PathPlan::new().with_min_depth(2).walk(tmp.path());
    let root = tmp.path();
    let violators: Vec<_> = files.iter().filter(|f| dir_depth(f, root) < 2).collect();
    let violators_preview: Vec<&&PathBuf> = violators.iter().take(3).collect();
    assert!(violators.is_empty(), "深度低于 2: {:?}", violators_preview);
    // note.txt（root 下）不应命中
    assert!(!files.contains(&root.join("note.txt")));
    // src/main.py（深度 1）不应命中
    assert!(!files.contains(&root.join("src").join("main.py")));
    // src/api/v1/users.py（深度 3）应命中
    assert!(files.contains(&root.join("src").join("api").join("v1").join("users.py")));
}

#[test]
fn test_follow_links_terminates_on_loop() {
    // symlink 环检测（依赖 WalkBuilder 内置 device/inode 去重）。
    // Windows 非开发者模式无法创建 symlink，跳过（与 Python 一致）。
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let link_path = tmp.path().join("logs").join("selflink");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        match symlink(tmp.path(), &link_path) {
            Ok(_) => {
                let plan = PathPlan::new().with_follow_links(true).with_max_depth(6);
                let (files, _st) = plan.walk(tmp.path());
                // 能返回就说明没死循环
                assert!(files.iter().all(|f| f.is_file()));
            }
            Err(e) => {
                eprintln!("skip symlink test: {}", e);
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Windows symlink 需要开发者模式或管理员，这里仅做 smoke test
        let _ = link_path;
        let plan = PathPlan::new().with_follow_links(true);
        let (files, _st) = plan.walk(tmp.path());
        assert!(files.iter().all(|f| f.is_file()));
    }
}

#[test]
fn test_empty_plan_matches_all_files() {
    // 空谓词 = 命中所有文件（与 Python PathPlan() 默认行为一致）。
    let tmp = TempDir::new().unwrap();
    let n = fixture(tmp.path());
    let (files, st) = PathPlan::new().walk(tmp.path());
    assert_eq!(
        files.len(),
        n,
        "空谓词应命中所有 {} 个文件，实际 {}",
        n,
        files.len()
    );
    assert_eq!(st.files_matched, n);
    assert_eq!(st.pruned_dirs, 0, "空谓词不应剪枝任何目录");
}
