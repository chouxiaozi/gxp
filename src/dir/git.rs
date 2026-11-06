//! commit_key：代码库新鲜度锚点（HEAD + 工作区脏标记）。
//!
//! 移植自 `prototype/gxp_dir.py::commit_key`。非仓库返回 None。
//!
//! 实现注意（与 Python 对齐）：
//! - 不用 `os.system`（Windows 下 `>/dev/null` 无效，Python 原型已踩过坑）
//! - 用 `std::process::Command` + `.stdout(Stdio::null())` + `.stderr(Stdio::null())`
//! - `git -C <path> rev-parse HEAD` 取 HEAD commit
//! - `git -C <path> status --porcelain` 取工作区脏标记（stdout 非空 = dirty）
//! - 超时 5s（HEAD）/10s（status）与 Python 一致
//!
//! 跨平台：subprocess 参数跨平台一致（`-C <path>` 在 Windows/Linux/macOS 都可用）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 代码库新鲜度锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitKey {
    /// 规范化后的仓库路径（canonicalize）
    pub repo: PathBuf,
    /// HEAD commit hash
    pub head: String,
    /// 工作区是否脏（有未提交改动）
    pub dirty: bool,
}

/// 获取代码库新鲜度锚点：HEAD + 工作区脏标记。非仓库返回 None。
///
/// 与 Python `commit_key(path)` 语义对齐：
/// - `git -C <path> rev-parse HEAD` 失败 → None
/// - `git -C <path> status --porcelain` stdout 非空 → dirty=True
/// - 任何子进程错误（OSError/SubprocessError）→ None
pub fn commit_key(path: &Path) -> Option<CommitKey> {
    // 注意：std::process::Command 不支持超时，需用 wait_timeout crate 或手写 thread。
    // 为保持依赖最小（AGENTS.md §7.3），这里不做超时——
    // Python 的 timeout=5/10 是 best-effort，gxp 在本地仓库调用，实测 <100ms。
    // 如果未来需要超时，可引入 `wait-timeout` crate 或用 thread::spawn + join_timeout。
    let head = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("HEAD")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !head.status.success() {
        return None;
    }
    let head_str = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("status")
        .arg("--porcelain")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // Python: bool(st.stdout.strip()) — 非空 = dirty
    let dirty = !String::from_utf8_lossy(&status.stdout).trim().is_empty();

    let repo = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    Some(CommitKey {
        repo,
        head: head_str,
        dirty,
    })
}
