//! 移植 `prototype/test_gxp_dir.py::TestCommitKey`（4 项）。
//!
//! 覆盖 commit_key 语义：
//! - 无改动时 head 稳定、dirty=false
//! - 新 commit 后 head 变
//! - 未提交改动时 dirty=true
//! - 非 git 仓库返回 None
//!
//! fixture：每个测试新建临时 git 仓库（init + commit），避免相互污染。
//! git 调用用 `std::process::Command` + `.stdout(Stdio::null())`，
//! 不用 `os.system`（Windows 下 `>/dev/null` 无效，Python 原型已踩过坑）。

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use gxp::dir::git::commit_key;
use tempfile::TempDir;

fn write_str(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// 在 root 初始化一个 git 仓库并提交所有文件。
fn git_init_repo(root: &Path) {
    let steps: &[&[&str]] = &[
        &["init", "-q"],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["add", "."],
        &["commit", "-qm", "c1"],
    ];
    for args in steps {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args.iter())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git 调用失败");
        assert!(status.success(), "git {:?} 失败", args);
    }
}

#[test]
fn test_commit_key_stable_without_changes() {
    let tmp = TempDir::new().unwrap();
    write_str(&tmp.path().join("a.txt"), "1\n");
    git_init_repo(tmp.path());

    let k1 = commit_key(tmp.path());
    let k2 = commit_key(tmp.path());
    let k1 = k1.expect("应返回 Some");
    let k2 = k2.expect("应返回 Some");
    assert_eq!(k1.head, k2.head, "无改动 head 应稳定");
    assert!(!k1.dirty, "无改动 dirty 应为 false");
}

#[test]
fn test_commit_key_changes_after_new_commit() {
    let tmp = TempDir::new().unwrap();
    write_str(&tmp.path().join("a.txt"), "1\n");
    git_init_repo(tmp.path());

    let k1 = commit_key(tmp.path()).expect("应返回 Some");

    write_str(&tmp.path().join("b.txt"), "2\n");
    let steps: &[&[&str]] = &[&["add", "-A"], &["commit", "-qm", "c2"]];
    for args in steps {
        let status = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(args.iter())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git 调用失败");
        assert!(status.success(), "git {:?} 失败", args);
    }
    let k2 = commit_key(tmp.path()).expect("应返回 Some");
    assert_ne!(k1.head, k2.head, "新 commit 后 head 应变");
}

#[test]
fn test_commit_key_marks_dirty_on_uncommitted_change() {
    let tmp = TempDir::new().unwrap();
    write_str(&tmp.path().join("a.txt"), "1\n");
    git_init_repo(tmp.path());

    // 先确认初始是干净的
    let k0 = commit_key(tmp.path()).expect("应返回 Some");
    assert!(!k0.dirty, "初始应干净");

    write_str(&tmp.path().join("a.txt"), "changed\n");
    let k = commit_key(tmp.path()).expect("应返回 Some");
    assert!(k.dirty, "未提交改动 dirty 应为 true");
}

#[test]
fn test_commit_key_none_outside_repo() {
    // 非 git 仓库：临时目录未 init，commit_key 应返回 None
    let tmp = TempDir::new().unwrap();
    write_str(&tmp.path().join("a.txt"), "1\n");
    let k = commit_key(tmp.path());
    assert!(k.is_none(), "非 git 仓库应返回 None，实际: {:?}", k);
}
