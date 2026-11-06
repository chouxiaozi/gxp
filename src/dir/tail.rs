//! TailIndex：单次查询内 append-only 增量读取计划。
//!
//! 移植自 `prototype/gxp_dir.py::TailIndex`。判定顺序（与 Python 完全一致）：
//! 1. missing：文件消失 → 清状态，返回 0/0/0
//! 2. cold：首次见到 → 全量读
//! 3. inode 变了（替换）→ 全量读
//! 4. size 缩小（truncate）→ 全量读
//! 5. size 不变 + mtime 变（rewrite）→ 全量读
//! 6. size 增长（append）→ 增量读 [prev_size, cur_size)
//! 7. 全不变 → 0 delta
//!
//! 契约约束（v1，与 Python docstring 对齐）：
//! - 只在**单次进程生命周期内**有效。进程退出后状态丢失，下次启动所有文件均为 cold
//! - 不落数据库、不写 sidecar 文件、不依赖跨查询状态
//! - 与 grep 一次性命令模型一致；gxp 默认不起守护进程
//!
//! 跨查询增量读取（append-only 的真正收益）需要守护进程或 sidecar 文件，
//! v1 明确不做。详见 Python `TailIndex` docstring 与 2026-10-20 对抗评审讨论。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// TailIndex 判定原因（与 Python 返回的 `reason` 字符串一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailReason {
    /// 文件消失
    Missing,
    /// 首次见到
    Cold,
    /// inode 变了（替换）
    Inode,
    /// size 缩小
    Truncate,
    /// size 不变但 mtime 变
    Rewrite,
    /// size 增长
    Append,
    /// 全不变
    Unchanged,
}

impl TailReason {
    /// 与 Python `reason` 字符串一一对应（snake_case）。
    pub fn as_str(self) -> &'static str {
        match self {
            TailReason::Missing => "missing",
            TailReason::Cold => "cold",
            TailReason::Inode => "inode",
            TailReason::Truncate => "truncate",
            TailReason::Rewrite => "rewrite",
            TailReason::Append => "append",
            TailReason::Unchanged => "unchanged",
        }
    }
}

/// 增量读取计划：从哪里开始读、读多少、文件总大小、判定原因。
#[derive(Debug, Clone, Copy)]
pub struct TailDelta {
    /// 从哪里开始读
    pub start: u64,
    /// 要读多少字节
    pub delta: u64,
    /// 文件当前总大小
    pub total: u64,
    /// 判定原因
    pub reason: TailReason,
}

#[derive(Clone, Copy)]
struct FileState {
    inode: u64,
    size: u64,
    mtime_ns: i128,
}

/// 单次查询内 append-only 增量读取状态机（无持久化，无跨查询）。
pub struct TailIndex {
    state: HashMap<PathBuf, FileState>,
}

impl Default for TailIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl TailIndex {
    /// 空状态（所有文件后续都判 Cold）。
    pub fn new() -> Self {
        Self {
            state: HashMap::new(),
        }
    }

    /// 判定单文件的增量读取计划。与 Python `TailIndex.check(path)` 一致。
    ///
    /// 副作用：调用后，状态被更新为当前文件元数据（missing 时移除）。
    pub fn check(&mut self, path: &Path) -> TailDelta {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => {
                self.state.remove(path);
                return TailDelta {
                    start: 0,
                    delta: 0,
                    total: 0,
                    reason: TailReason::Missing,
                };
            }
        };

        let cur = FileState {
            inode: file_id(&meta),
            size: meta.len(),
            mtime_ns: mtime_to_ns(meta.modified().ok()),
        };

        // Python: prev = self._st.get(path); self._st[path] = cur
        // 注意 Python 是"先取 prev 再覆盖"，这里用 same 语义
        let prev = self.state.insert(path.to_path_buf(), cur);
        // FileState 是 Copy，在 match 前先抽出 prev_size 供 Append arm 使用
        // （match prev 会消费 prev，arm 内访问 prev 会报 borrow checker 错误）
        let prev_size = prev.map(|p| p.size).unwrap_or(0);

        let reason = match prev {
            None => TailReason::Cold,
            Some(p) if p.inode != cur.inode => TailReason::Inode,
            Some(p) if cur.size < p.size => TailReason::Truncate,
            // size 不变 + mtime 变 → rewrite（注意 Python 用 != 判 mtime，
            // 即便 mtime 减小也认为 rewrite，这是 best-effort 语义）
            Some(p) if cur.size == p.size && cur.mtime_ns != p.mtime_ns => TailReason::Rewrite,
            Some(p) if cur.size > p.size => TailReason::Append,
            Some(_) => TailReason::Unchanged,
        };

        let (start, delta) = match reason {
            TailReason::Append => (prev_size, cur.size - prev_size),
            TailReason::Unchanged => (cur.size, 0),
            _ => (0, cur.size),
        };

        TailDelta {
            start,
            delta,
            total: cur.size,
            reason,
        }
    }
}

/// 跨平台 file id 抽象（对齐 Rust 方案 §4.3 跨平台路径）。
///
/// - Linux/macOS：`std::os::unix::fs::MetadataExt::ino()`
/// - Windows：稳定 Rust 下 `file_index()` 需要 unstable feature `windows_by_handle`，
///   方案 §4.3 原写法在 stable 不可用。按方案 §4.4 风险表"Windows 上 TailIndex
///   按 best-effort 设计，加 mtime 双保险"，Windows 下用 `creation_time` 作为伪 inode——
///   - creation_time 是 Windows FILETIME（100ns 精度），每个文件独立
///   - replace 后的新文件 creation_time 不同 → Inode 判定触发
///   - 同文件 truncate/rewrite/append 不改 creation_time → 不走 Inode，
///     走 Truncate/Rewrite/Append
///
/// 设计决策（L1，已更新方案 §4.3 跨平台路径 + §4.4 风险）：
/// - 不引入 `windows-sys` 直接依赖（AGENTS.md §7.3 禁止新增 crate）
/// - Windows `file_index` 需要 `GetFileInformationByHandle` + FFI，开销大
/// - 不把 size 纳入 file_id：size 变化应由 Truncate/Append 分支处理，
///   纳入会误判 append 为 inode 变化
/// - best-effort 语义与 Python `TailIndex` docstring 标注的"mtime 分辨率不足时漏检"
///   一致，不破坏 v1 契约
fn file_id(meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    }
    #[cfg(windows)]
    {
        // best-effort：用 creation_time 作伪 inode
        // - 不含 size：size 变化走 Truncate/Append 分支
        // - 不含 mtime：mtime 变化走 Rewrite 分支
        // - replace 后新文件 creation_time 不同 → 触发 Inode 判定
        // 漏检窗口 = "原地改写 + creation_time 不变"（与方案 §4.4 风险表标注一致）
        use std::os::windows::fs::MetadataExt;
        meta.creation_time()
    }
    #[cfg(not(any(unix, windows)))]
    {
        // fallback：无法获取稳定 inode，用 mtime_ns 作伪 inode
        // （mtime 变化会误判为 Inode，这是 fallback 平台的已知限制）
        mtime_to_ns(meta.modified().ok()) as u64
    }
}

/// SystemTime → nanoseconds since UNIX_EPOCH（对齐 Python `st_mtime_ns`）。
fn mtime_to_ns(t: Option<SystemTime>) -> i128 {
    use std::time::UNIX_EPOCH;
    match t {
        Some(t) => t
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::TempDir;

    /// 写入文件内容并 flush，返回文件路径。
    fn write_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut f = File::create(&path).expect("create 失败");
        f.write_all(content).expect("write 失败");
        f.sync_all().expect("sync 失败");
        path
    }

    /// 在已有文件上覆写内容（truncate + write），不删除文件。
    fn overwrite_file(path: &Path, content: &[u8]) {
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .expect("open 失败");
        f.write_all(content).expect("write 失败");
        f.sync_all().expect("sync 失败");
    }

    /// 向已有文件追加内容。
    fn append_to_file(path: &Path, extra: &[u8]) {
        let mut f = OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open 失败");
        f.write_all(extra).expect("write 失败");
        f.sync_all().expect("sync 失败");
    }

    /// 强制文件 mtime 更新：等待短暂时间后 touch 文件。
    /// Windows 文件系统 mtime 精度约 100ns–15ms，需 sleep 足够久确保差值可测。
    fn force_mtime_change(path: &Path) {
        std::thread::sleep(Duration::from_millis(50));
        // 重新写一遍同内容来更新 mtime
        let content = std::fs::read(path).expect("read 失败");
        overwrite_file(path, &content);
    }

    // 1. missing 文件：check 一次后删除文件，再 check 应判 Missing
    #[test]
    fn test_missing_file_returns_need_scan_after_deletion() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "a.log", b"line1\n");

        let mut idx = TailIndex::new();
        // 第一次 check 记录状态（Cold）
        let _ = idx.check(&path);
        // 删除文件
        fs::remove_file(&path).expect("删除失败");

        let d = idx.check(&path);
        assert_eq!(d.reason, TailReason::Missing);
        assert_eq!(d.start, 0);
        assert_eq!(d.delta, 0);
        assert_eq!(d.total, 0);
    }

    // 2. cold 文件：首次见到 → 返回全量读
    #[test]
    fn test_cold_file_returns_full_scan() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "cold.log", b"hello\nworld\n");

        let mut idx = TailIndex::new();
        let d = idx.check(&path);
        assert_eq!(d.reason, TailReason::Cold);
        // 全量读：start=0, delta=文件大小, total=文件大小
        assert_eq!(d.start, 0);
        assert_eq!(d.delta, 12); // "hello\nworld\n" = 12 bytes
        assert_eq!(d.total, 12);
    }

    // 3. inode 变化：同路径但 inode 不同（replace 后新文件 creation_time 不同）
    #[test]
    fn test_inode_change_returns_full_scan() {
        use tempfile::NamedTempFile;

        let dir = TempDir::new().expect("创建临时目录失败");
        let target = dir.path().join("inode.log");

        // 原文件：用 NamedTempFile persist 写入，记录 creation_time
        let nt1 = NamedTempFile::new_in(dir.path()).expect("创建临时文件失败");
        std::fs::write(nt1.path(), b"old\n").expect("写入失败");
        nt1.persist(&target).expect("persist 失败");

        let mut idx = TailIndex::new();
        let d1 = idx.check(&target);
        assert_eq!(d1.reason, TailReason::Cold);

        // 用另一个 NamedTempFile persist 覆盖 → 新文件有独立 creation_time → Inode
        std::thread::sleep(Duration::from_millis(100));
        let nt2 = NamedTempFile::new_in(dir.path()).expect("创建临时文件失败");
        std::fs::write(nt2.path(), b"new\n").expect("写入失败");
        nt2.persist(&target).expect("persist 覆盖失败");

        let d2 = idx.check(&target);
        assert_eq!(d2.reason, TailReason::Inode);
        assert_eq!(d2.start, 0);
        assert_eq!(d2.delta, 4); // "new\n" = 4 bytes
        assert_eq!(d2.total, 4);
    }

    // 4. size 缩小：truncate 后 size 比上次小 → 返回全量读
    #[test]
    fn test_truncate_returns_full_scan() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "trunc.log", b"long content here\n");

        let mut idx = TailIndex::new();
        let d1 = idx.check(&path);
        assert_eq!(d1.reason, TailReason::Cold);

        // truncate 到更短内容（同文件覆写）
        force_mtime_change(&path);
        overwrite_file(&path, b"short\n");

        let d2 = idx.check(&path);
        assert_eq!(d2.reason, TailReason::Truncate);
        assert_eq!(d2.start, 0);
        assert_eq!(d2.delta, 6); // "short\n" = 6 bytes
        assert_eq!(d2.total, 6);
    }

    // 5. mtime 变化但 size 不变 → Rewrite
    #[test]
    fn test_rewrite_returns_full_scan() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "rewrite.log", b"samelength\n"); // 11 bytes

        let mut idx = TailIndex::new();
        let d1 = idx.check(&path);
        assert_eq!(d1.reason, TailReason::Cold);
        assert_eq!(d1.total, 11);

        // 同长度覆写：mtime 变，size 不变
        force_mtime_change(&path);
        overwrite_file(&path, b"DIFFLENGTH\n"); // 同为 11 bytes

        let d2 = idx.check(&path);
        assert_eq!(d2.reason, TailReason::Rewrite);
        assert_eq!(d2.start, 0);
        assert_eq!(d2.delta, 11);
        assert_eq!(d2.total, 11);
    }

    // 6. append：size 增长 → 增量读 [prev_size, cur_size)
    #[test]
    fn test_append_returns_tail_delta() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "append.log", b"base\n"); // 5 bytes

        let mut idx = TailIndex::new();
        let d1 = idx.check(&path);
        assert_eq!(d1.reason, TailReason::Cold);
        assert_eq!(d1.total, 5);

        // 追加内容 → size 增长
        force_mtime_change(&path);
        append_to_file(&path, b"more\n"); // +5 bytes → 10 bytes total

        let d2 = idx.check(&path);
        assert_eq!(d2.reason, TailReason::Append);
        assert_eq!(d2.start, 5); // 从 prev_size=5 开始
        assert_eq!(d2.delta, 5); // 增量 = cur_size - prev_size = 10 - 5
        assert_eq!(d2.total, 10);
    }

    // 7. 全不变：size/mtime/inode 全相同 → 0 delta
    #[test]
    fn test_unchanged_returns_zero_delta() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "static.log", b"unchanged\n"); // 10 bytes

        let mut idx = TailIndex::new();
        let d1 = idx.check(&path);
        assert_eq!(d1.reason, TailReason::Cold);
        assert_eq!(d1.total, 10);

        // 不做任何修改，立即再 check → 全不变
        let d2 = idx.check(&path);
        assert_eq!(d2.reason, TailReason::Unchanged);
        assert_eq!(d2.start, 10); // start = cur_size（文件末尾）
        assert_eq!(d2.delta, 0);
        assert_eq!(d2.total, 10);
    }

    // TailReason::as_str 覆盖
    #[test]
    fn test_tail_reason_as_str_all_variants() {
        assert_eq!(TailReason::Missing.as_str(), "missing");
        assert_eq!(TailReason::Cold.as_str(), "cold");
        assert_eq!(TailReason::Inode.as_str(), "inode");
        assert_eq!(TailReason::Truncate.as_str(), "truncate");
        assert_eq!(TailReason::Rewrite.as_str(), "rewrite");
        assert_eq!(TailReason::Append.as_str(), "append");
        assert_eq!(TailReason::Unchanged.as_str(), "unchanged");
    }

    // missing 后状态被清除：下次同文件再现 → 判 Cold（非 Inode）
    #[test]
    fn test_missing_clears_state_then_cold_on_reappear() {
        let dir = TempDir::new().expect("创建临时目录失败");
        let path = write_file(dir.path(), "reappear.log", b"data\n");

        let mut idx = TailIndex::new();
        let d1 = idx.check(&path);
        assert_eq!(d1.reason, TailReason::Cold);

        fs::remove_file(&path).expect("删除失败");
        let d2 = idx.check(&path);
        assert_eq!(d2.reason, TailReason::Missing);

        // 重新创建 → 此时状态已清除 → Cold（而非 Inode）
        std::thread::sleep(Duration::from_millis(50));
        let path = write_file(dir.path(), "reappear.log", b"data\n");
        let d3 = idx.check(&path);
        assert_eq!(d3.reason, TailReason::Cold);
    }

    // Default trait → new() 等价：两者对同一新文件都判 Cold
    #[test]
    fn test_default_equals_new_both_cold() {
        let mut a = TailIndex::default();
        let mut b = TailIndex::new();
        let dir = TempDir::new().expect("创建临时目录失败");
        let path_a = write_file(dir.path(), "default_a.log", b"x\n");
        let path_b = write_file(dir.path(), "default_b.log", b"y\n");
        let da = a.check(&path_a);
        let db = b.check(&path_b);
        assert_eq!(da.reason, TailReason::Cold);
        assert_eq!(db.reason, TailReason::Cold);
    }

    // TailDelta Debug/Clone 可用性
    #[test]
    fn test_tail_delta_debug_clone() {
        let d = TailDelta {
            start: 10,
            delta: 20,
            total: 30,
            reason: TailReason::Append,
        };
        let cloned = d;
        assert_eq!(cloned.start, 10);
        assert_eq!(cloned.delta, 20);
        assert_eq!(cloned.total, 30);
        assert_eq!(cloned.reason, TailReason::Append);
        // Debug trait 可用
        let s = format!("{:?}", d);
        assert!(s.contains("Append"));
    }
}
