//! 移植 `prototype/test_gxp_dir.py::TestAppendOnly`（6 项）。
//!
//! 覆盖 TailIndex 判定序列：
//! - cold（首次见）
//! - unchanged（全不变）
//! - append（size 增长）
//! - truncate（size 缩小）
//! - rewrite（size 不变 + mtime 变）
//! - inode（替换，文件被其他文件 replace 覆盖）
//!
//! 写入用二进制模式（`fs::write`），保证跨平台字节数一致（Windows 不做 CRLF 转换）。
//! sleep 用 `std::thread::sleep` 确保 mtime 分辨率（Windows FAT 系列精度 2s，
//! NTFS 100ns，Linux 多数 fs 1ns，这里统一睡 1.05s 保证 mtime 必变）。

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use gxp::dir::tail::{TailIndex, TailReason};
use tempfile::TempDir;

/// 以二进制模式写入 bytes（与 Python `_w(p, txt)` 对齐，避免 CRLF 转换）。
fn write_bytes(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn test_first_check_is_full_read() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n");
    let r = TailIndex::new().check(&p);
    assert_eq!(r.reason, TailReason::Cold);
    assert_eq!(r.start, 0);
    assert_eq!(r.delta, fs::metadata(&p).unwrap().len());
}

#[test]
fn test_unchanged_file_has_zero_delta() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n");
    let mut t = TailIndex::new();
    t.check(&p);
    let r = t.check(&p);
    // "line1\nline2\n" = 12 字节
    assert_eq!(r.start, 12);
    assert_eq!(r.delta, 0);
    assert_eq!(r.reason, TailReason::Unchanged);
}

#[test]
fn test_append_only_scans_only_new_bytes() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n");
    let mut t = TailIndex::new();
    t.check(&p);
    // 二进制追加，保持与原 _w 一致（无 CRLF）
    use std::io::Write;
    let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(b"line3\nline4\n").unwrap();
    f.flush().unwrap();
    drop(f);
    let r = t.check(&p);
    assert_eq!(r.start, 12);
    assert_eq!(r.delta, 12);
    assert_eq!(r.reason, TailReason::Append);
}

#[test]
fn test_truncation_forces_full_rescan() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n");
    let mut t = TailIndex::new();
    t.check(&p);
    write_bytes(&p, b"x\n");
    let r = t.check(&p);
    assert_eq!(r.start, 0);
    assert_eq!(r.reason, TailReason::Truncate);
}

#[test]
fn test_same_size_rewrite_forces_full_rescan() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n"); // 12 字节
    let mut t = TailIndex::new();
    t.check(&p);
    // 等 mtime 推进（Windows NTFS 100ns，但保险起见睡 1.05s）
    thread::sleep(Duration::from_millis(1050));
    write_bytes(&p, b"lineX\nlineY\n"); // 同样 12 字节
    assert_eq!(fs::metadata(&p).unwrap().len(), 12);
    let r = t.check(&p);
    assert_eq!(r.start, 0);
    assert_eq!(r.reason, TailReason::Rewrite);
}

#[test]
fn test_inode_replacement_forces_full_rescan() {
    // 用 os.replace 让新文件覆盖旧路径 → inode 变 → 全量重扫
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("a.log");
    write_bytes(&p, b"line1\nline2\n");
    let mut t = TailIndex::new();
    t.check(&p);
    let other = tmp.path().join("b.log");
    // 睡 50ms 确保 creation_time 与 p 不同（Windows FILETIME 100ns 精度，
    // 但 NTFS 在某些场景下会重用目标文件的 creation_time，sleep 避免此竞态）
    thread::sleep(Duration::from_millis(50));
    write_bytes(&other, b"zz\n");
    fs::rename(other, &p).unwrap(); // rename 同分区是 atomic replace
    let r = t.check(&p);
    assert_eq!(r.start, 0);
    assert_eq!(r.reason, TailReason::Inode);
}
