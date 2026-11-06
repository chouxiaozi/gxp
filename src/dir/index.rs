//! DirIndex：目录树索引，lax/strict/off 三模式。
//!
//! 移植自 `prototype/gxp_dir.py::DirIndex`。语义契约：
//! - lax：目录名单指纹未变 → 跳过逐文件 stat（readdir 仍做，否则发现不了改名/新增）
//! - strict：每次重扫属性
//! - off：完全跳过校验，零系统调用（代价是过期视图）
//!
//! 与 `PathPlan::walk` 的区别：
//! - PathPlan 只下推剪枝，不缓存、不 stat（file_ok 的 size 参数恒为 None）
//! - DirIndex 自己控制栈式遍历，在 readdir 阶段做指纹校验决定是否复用，
//!   并对命中的文件做 stat 收集 size/mtime（为 v1 未实现的 size/mtime 谓词预留）
//!
//! 实现选型（Rust 方案 §4.3）：
//! - 不用 `ignore::WalkBuilder`：它不暴露"对每个目录 readdir 后能否复用 meta"的钩子，
//!   DirIndex 需要自己控制栈式遍历
//! - 指纹算法：`blake3` 截断 16B（替代 Python `blake2b(digest_size=16)`，方案 §15 决策）
//! - symlink 环检测：手写 `seen_real`（与 Python 一致，不依赖 WalkBuilder 去重）

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use blake3::Hasher;

use super::plan::PathPlan;

/// 新鲜度模式（与 Python `DirIndex(mode=...)` 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FreshnessMode {
    /// 名单指纹未变 → 跳过逐文件 stat（readdir 仍做）
    #[default]
    Lax,
    /// 每次重扫属性
    Strict,
    /// 完全跳过校验，零 syscall（过期视图）
    Off,
}

/// 目录条目记录：指纹 + 文件 meta（Arc 共享，lax 复用时避免 clone）。
#[derive(Debug, Clone)]
struct DirRecord {
    fp: [u8; 16],
    /// lax 二次复用时：`Arc::clone` 是 1 次 atomic op，替代 `Vec::clone` 的 N 次分配。
    files: Arc<Vec<FileMeta>>,
}

#[derive(Debug, Clone)]
struct FileMeta {
    path: PathBuf,
    name: String,
    size: u64,
    // v1 未实现 size/mtime 谓词，但 Python 原型收集了 mtime_ns，
    // 这里保留字段对齐，便于后续扩展。`allow(dead_code)` 避免 clippy 拦截。
    #[allow(dead_code)]
    mtime_ns: i128,
}

/// DirIndex 统计：与 Python `DirIndex.list` 返回的 dict 字段一一对应。
#[derive(Debug, Default, Clone)]
pub struct DirStats {
    /// 实际进入 readdir 的目录数。
    pub dirs_visited: usize,
    /// 指纹命中复用缓存的目录数（lax 模式收益指标）。
    pub dirs_reused: usize,
    /// 指纹未命中、重新 stat 的目录数。
    pub dirs_changed: usize,
    /// 被 `dir_eligible` 剪掉的目录数。
    pub pruned_dirs: usize,
    /// 调用 `metadata()` 的次数（strict 模式开销指标）。
    pub index_stats: usize,
    /// 因 symlink 环检测跳过的链接数。
    pub links_skipped: usize,
    /// `read_dir` 调用次数。
    pub readdir_calls: usize,
}

/// 目录索引：栈式遍历 + 指纹缓存（lax/strict/off 三模式）。
#[derive(Debug)]
pub struct DirIndex {
    root: PathBuf,
    mode: FreshnessMode,
    cache: HashMap<String, DirRecord>,
}

impl DirIndex {
    /// 构造 DirIndex：指定 root 与新鲜度模式。
    pub fn new(root: &Path, mode: FreshnessMode) -> Self {
        Self {
            root: root.to_path_buf(),
            mode,
            cache: HashMap::new(),
        }
    }

    /// 列举符合 plan 的文件。返回 (files, stats)。
    ///
    /// 与 Python `DirIndex.list(plan, collect_meta=True)` 一致。
    pub fn list(&mut self, plan: &PathPlan) -> (Vec<PathBuf>, DirStats) {
        let mut st = DirStats::default();
        let mut out: Vec<PathBuf> = Vec::new();

        // off 模式 + 已有缓存：零 syscall，直接从缓存返回（过期）
        if self.mode == FreshnessMode::Off && !self.cache.is_empty() {
            for (rel, rec) in self.cache.iter() {
                let depth = if rel.is_empty() {
                    0
                } else {
                    rel.matches('/').count() + 1
                };
                if !plan.dir_eligible(rel, depth) {
                    st.pruned_dirs += 1;
                    continue;
                }
                st.dirs_visited += 1;
                let files = &rec.files;
                for m in files.iter() {
                    if plan.file_ok(&m.name, rel, depth, Some(m.size)) {
                        out.push(m.path.clone());
                    }
                }
            }
            st.dirs_reused = st.dirs_visited;
            return (out, st);
        }

        // lax/strict：栈式遍历，与 Python 完全对齐
        let mut stack: Vec<(PathBuf, String, usize)> = vec![(self.root.clone(), String::new(), 0)];
        let mut seen_real: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        // root 规范化路径入 seen_real（symlink 环检测基线）
        let root_real = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        seen_real.insert(root_real);

        while let Some((dp, rel, depth)) = stack.pop() {
            st.dirs_visited += 1;
            let entries = match fs::read_dir(&dp) {
                Ok(e) => e,
                Err(_) => continue,
            };
            st.readdir_calls += 1;

            // 收集 entries 以便两次遍历：第一次算指纹 + 判断复用，第二次推子目录
            let entries_vec: Vec<_> = entries.flatten().collect();

            // 指纹
            let fp = fingerprint(&entries_vec);
            let reused = match self.mode {
                FreshnessMode::Strict => false,
                _ => self.cache.get(&rel).map(|r| r.fp == fp).unwrap_or(false),
            };

            let mut meta: Vec<FileMeta>;
            let files_ref: Arc<Vec<FileMeta>>;
            if reused {
                st.dirs_reused += 1;
                // lax 二次复用：Arc::clone 替代 Vec::clone，避免 N 次分配
                files_ref = Arc::clone(&self.cache.get(&rel).unwrap().files);
            } else {
                // 严格模式或指纹变化：重新 stat 每个文件
                // Python 的实现里 rec 已存在也算 dirs_changed，新进入也算 dirs_changed
                st.dirs_changed += 1;
                meta = Vec::with_capacity(entries_vec.len());
                for e in &entries_vec {
                    let is_file = match is_file_with_follow(e, false) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    if !is_file {
                        continue;
                    }
                    let (sz, mtn) = match e.metadata() {
                        Ok(m) => {
                            st.index_stats += 1;
                            (m.len(), mtime_to_ns(m.modified().ok()))
                        }
                        Err(_) => (0, 0),
                    };
                    meta.push(FileMeta {
                        path: e.path(),
                        name: e.file_name().to_string_lossy().into_owned(),
                        size: sz,
                        mtime_ns: mtn,
                    });
                }
                files_ref = Arc::new(meta);
                self.cache.insert(
                    rel.clone(),
                    DirRecord {
                        fp,
                        files: Arc::clone(&files_ref),
                    },
                );
            }

            // 输出命中的文件（从 Arc<Vec<FileMeta>> 借用迭代，避免 clone Vec）
            for m in files_ref.iter() {
                if plan.file_ok(&m.name, &rel, depth, Some(m.size)) {
                    out.push(m.path.clone());
                }
            }

            // 推子目录（与 Python 一致：dir_eligible 判定 + symlink 环检测）
            for e in &entries_vec {
                let is_dir = match is_dir_with_follow(e, plan.follow_links) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if !is_dir {
                    continue;
                }

                let crel = if rel.is_empty() {
                    e.file_name().to_string_lossy().into_owned()
                } else {
                    format!("{}/{}", rel, e.file_name().to_string_lossy())
                };
                if !plan.dir_eligible(&crel, depth + 1) {
                    st.pruned_dirs += 1;
                    continue;
                }
                if plan.follow_links {
                    let rp = fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
                    if seen_real.contains(&rp) {
                        st.links_skipped += 1;
                        continue;
                    }
                    seen_real.insert(rp);
                }
                stack.push((e.path(), crel, depth + 1));
            }
        }

        (out, st)
    }
}

/// 判断 entry 是否为目录，支持 follow_symlinks 参数（对齐 Python `is_dir(follow_symlinks=...)`）。
///
/// - `follow=false`：用 `file_type()`（不 follow symlink），symlink 指向目录返回 false
/// - `follow=true`：用 `metadata()`（follow symlink），symlink 指向目录返回 true
fn is_dir_with_follow(e: &fs::DirEntry, follow: bool) -> std::io::Result<bool> {
    if follow {
        e.metadata().map(|m| m.is_dir())
    } else {
        e.file_type().map(|t| t.is_dir())
    }
}

/// 判断 entry 是否为文件（不 follow symlink，与 Python `is_file(follow_symlinks=False)` 对齐）。
fn is_file_with_follow(e: &fs::DirEntry, _follow: bool) -> std::io::Result<bool> {
    // Python DirIndex 里固定 follow_symlinks=False（只判定真实文件）
    e.file_type().map(|t| t.is_file())
}

/// blake3 目录指纹（截断 16B，兼容 Python blake2b digest_size=16）。
///
/// 签名约定（与 Python `_fp(entries)` 对齐）：
/// sig = "|".join(sorted(f"{name}:{'d'|'f'}"))
/// 排序键：name 字符串
fn fingerprint(entries: &[fs::DirEntry]) -> [u8; 16] {
    let mut sig_parts: Vec<String> = entries
        .iter()
        .map(|e| {
            let ft = e.file_type().ok();
            let kind = match ft {
                Some(t) if t.is_dir() => 'd',
                _ => 'f',
            };
            format!("{}:{}", e.file_name().to_string_lossy(), kind)
        })
        .collect();
    sig_parts.sort();
    let sig = sig_parts.join("|");
    let mut hasher = Hasher::new();
    hasher.update(sig.as_bytes());
    let full = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&full.as_bytes()[..16]);
    out
}

/// 把 `SystemTime` 转成 ns（与 Python `st_mtime_ns` 对齐）。
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
    use crate::dir::plan::PathPlan;
    use std::fs;
    use tempfile::TempDir;

    /// 辅助：在 tmp 下创建一个含 2 文件的简单目录。
    fn setup_two_files(tmp: &TempDir) {
        fs::write(tmp.path().join("a.txt"), "alpha\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "beta\n").unwrap();
    }

    // ================================================================
    // 1. lax 模式：文件变化时返回需要重扫
    // ================================================================

    #[test]
    fn test_lax_unchanged_dir_reuses_cache() {
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);

        let (files1, st1) = idx.list(&plan);
        let (files2, st2) = idx.list(&plan);

        // 两次列举结果一致
        assert_eq!(files1.len(), files2.len());
        assert!(
            st2.dirs_reused > 0,
            "第二次应复用缓存: dirs_reused={}",
            st2.dirs_reused
        );
        // lax 复用后不 stat 文件
        assert_eq!(
            st2.index_stats, 0,
            "lax 复用后不应 stat: index_stats={}",
            st2.index_stats
        );
        assert!(st2.index_stats < st1.index_stats);
    }

    #[test]
    fn test_lax_detects_file_content_change_needs_rescan() {
        // lax 模式下文件内容变化不影响指纹（指纹基于 entries 名单）
        // 但文件数量变化（新增/删除）会改变指纹 → dirs_changed > 0
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);

        let (files1, _) = idx.list(&plan);
        assert_eq!(files1.len(), 2);

        // 新增文件 → 指纹变化 → 需要重扫
        fs::write(tmp.path().join("c.txt"), "gamma\n").unwrap();
        let (files2, st2) = idx.list(&plan);

        assert_eq!(files2.len(), 3, "应发现新增的 c.txt");
        assert!(
            st2.dirs_changed > 0,
            "指纹变化应触发 dirs_changed: {:?}",
            st2
        );
        assert_eq!(st2.dirs_reused, 0, "指纹变化后不应复用");
    }

    #[test]
    fn test_lax_file_rename_triggers_rescan() {
        // 改名 → entries 名单变 → 指纹变 → 需要重扫
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);

        let (files1, _) = idx.list(&plan);
        assert_eq!(files1.len(), 2);

        // 改名 a.txt → d.txt
        fs::rename(tmp.path().join("a.txt"), tmp.path().join("d.txt")).unwrap();
        let (files2, st2) = idx.list(&plan);

        assert_eq!(files2.len(), 2, "文件数不变");
        assert!(
            files2.iter().any(|p| p.to_string_lossy().contains("d.txt")),
            "应列出改名后的 d.txt"
        );
        assert!(
            !files2.iter().any(|p| p.to_string_lossy().contains("a.txt")),
            "不应再列出旧名 a.txt"
        );
        assert!(st2.dirs_changed > 0, "改名应触发 dirs_changed: {:?}", st2);
    }

    // ================================================================
    // 2. strict 模式：更严格的指纹匹配
    // ================================================================

    #[test]
    fn test_strict_never_reuses_even_unchanged() {
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Strict);

        let (_, st1) = idx.list(&plan);
        let (_, st2) = idx.list(&plan);

        assert_eq!(
            st2.dirs_reused, 0,
            "strict 不应复用: dirs_reused={}",
            st2.dirs_reused
        );
        // strict 每次都 stat 文件
        assert!(
            st2.index_stats > 0,
            "strict 应每次 stat: index_stats={}",
            st2.index_stats
        );
        assert_eq!(
            st1.index_stats, st2.index_stats,
            "strict 两次 stat 次数应相同"
        );
    }

    #[test]
    fn test_strict_detects_file_delete() {
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Strict);

        let (files1, _) = idx.list(&plan);
        assert_eq!(files1.len(), 2);

        fs::remove_file(tmp.path().join("b.txt")).unwrap();
        let (files2, st2) = idx.list(&plan);

        assert_eq!(files2.len(), 1, "删除后应只剩 1 个文件");
        assert!(files2.iter().any(|p| p.to_string_lossy().contains("a.txt")));
        assert_eq!(st2.dirs_reused, 0, "strict 不复用");
        assert_eq!(st2.dirs_changed, 1, "strict 总是 changed");
    }

    // ================================================================
    // 3. off 模式：不做增量，总是返回需要全扫
    // ================================================================

    #[test]
    fn test_off_mode_second_call_zero_syscalls() {
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);

        let (files1, st1) = idx.list(&plan);
        let (files2, st2) = idx.list(&plan);

        assert_eq!(files1.len(), files2.len());
        // off 第二次零 syscall
        assert_eq!(st2.index_stats, 0, "off 不应 stat");
        assert_eq!(st2.readdir_calls, 0, "off 不应 readdir");
        assert_eq!(st2.dirs_reused, st1.dirs_visited, "off 全部复用");
    }

    #[test]
    fn test_off_mode_stale_view_after_new_file() {
        // off 的代价：看不见新文件（过期视图）
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);

        let (f1, _) = idx.list(&plan);
        fs::write(tmp.path().join("c.txt"), "gamma\n").unwrap();
        let (f2, _) = idx.list(&plan);

        assert_eq!(f1.len(), f2.len(), "off 不应发现新文件");
        assert!(
            !f2.iter().any(|p| p.to_string_lossy().contains("c.txt")),
            "off 不应看到新文件"
        );
    }

    #[test]
    fn test_off_mode_stale_view_after_file_delete() {
        // off 的代价：已删除文件仍出现在结果中
        let tmp = TempDir::new().unwrap();
        setup_two_files(&tmp);
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);

        let (f1, _) = idx.list(&plan);
        fs::remove_file(tmp.path().join("a.txt")).unwrap();
        let (f2, _) = idx.list(&plan);

        assert_eq!(f1.len(), f2.len(), "off 不应发现删除");
        assert!(
            f2.iter().any(|p| p.to_string_lossy().contains("a.txt")),
            "off 仍列出已删除的 a.txt（过期视图）"
        );
    }

    // ================================================================
    // 4. 空目录：索引为空时任何文件都返回需要扫描
    // ================================================================

    #[test]
    fn test_empty_dir_returns_no_files() {
        let tmp = TempDir::new().unwrap();
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);

        let (files, st) = idx.list(&plan);

        assert!(files.is_empty(), "空目录应返回空文件列表");
        assert_eq!(st.dirs_visited, 1, "应访问 root: {:?}", st);
        assert_eq!(st.dirs_changed, 1, "root 指纹未命中缓存: {:?}", st);
        assert_eq!(st.dirs_reused, 0, "无缓存可复用");
    }

    #[test]
    fn test_empty_dir_strict_mode_no_files() {
        let tmp = TempDir::new().unwrap();
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Strict);

        let (files, st) = idx.list(&plan);

        assert!(files.is_empty());
        assert_eq!(st.dirs_reused, 0, "strict 不复用");
        assert_eq!(st.index_stats, 0, "无文件可 stat");
    }

    #[test]
    fn test_empty_dir_off_mode_no_files() {
        let tmp = TempDir::new().unwrap();
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Off);

        let (files, st) = idx.list(&plan);

        assert!(files.is_empty());
        assert_eq!(st.dirs_reused, 0, "off 首次无缓存");
        // off + 空 cache → 走正常遍历路径
        assert_eq!(st.readdir_calls, 1, "首次应 readdir");
    }

    // ================================================================
    // 5. 单文件：目录中只有一个文件
    // ================================================================

    #[test]
    fn test_single_file_lax_mode() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("only.txt"), "solo\n").unwrap();
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Lax);

        let (files1, st1) = idx.list(&plan);
        assert_eq!(files1.len(), 1);
        assert!(files1
            .iter()
            .any(|p| p.to_string_lossy().contains("only.txt")));

        // 第二次复用
        let (files2, st2) = idx.list(&plan);
        assert_eq!(files2.len(), 1);
        assert!(st2.dirs_reused > 0, "单文件也应复用: {:?}", st2);
        assert_eq!(st2.index_stats, 0, "lax 复用后不 stat");
        assert_eq!(st1.index_stats, 1, "首次 stat 1 个文件");
    }

    #[test]
    fn test_single_file_strict_mode() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("only.txt"), "solo\n").unwrap();
        let plan = PathPlan::new();
        let mut idx = DirIndex::new(tmp.path(), FreshnessMode::Strict);

        let (files, st) = idx.list(&plan);
        assert_eq!(files.len(), 1);
        assert_eq!(st.index_stats, 1, "strict 应 stat 1 个文件");
        assert_eq!(st.dirs_reused, 0);
        assert_eq!(st.dirs_changed, 1);
    }

    // ================================================================
    // 6. 指纹计算：相同内容产生相同指纹，不同内容产生不同指纹
    // ================================================================

    #[test]
    fn test_fingerprint_same_entries_same_hash() {
        // 相同 entries 名单 → 相同指纹
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        fs::write(tmp1.path().join("a.txt"), "content1\n").unwrap();
        fs::write(tmp1.path().join("b.txt"), "content2\n").unwrap();
        fs::write(tmp2.path().join("a.txt"), "completely different\n").unwrap();
        fs::write(tmp2.path().join("b.txt"), "also different\n").unwrap();

        let e1: Vec<_> = fs::read_dir(tmp1.path()).unwrap().flatten().collect();
        let e2: Vec<_> = fs::read_dir(tmp2.path()).unwrap().flatten().collect();

        let fp1 = fingerprint(&e1);
        let fp2 = fingerprint(&e2);

        // 指纹基于 entries 名单（name+type），内容不同但名单相同 → 指纹相同
        assert_eq!(fp1, fp2, "相同名单不同内容应产生相同指纹");
    }

    #[test]
    fn test_fingerprint_different_entries_different_hash() {
        // 不同 entries 名单 → 不同指纹
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        fs::write(tmp1.path().join("a.txt"), "x\n").unwrap();
        fs::write(tmp1.path().join("b.txt"), "y\n").unwrap();
        fs::write(tmp2.path().join("a.txt"), "x\n").unwrap();
        fs::write(tmp2.path().join("c.txt"), "z\n").unwrap();

        let e1: Vec<_> = fs::read_dir(tmp1.path()).unwrap().flatten().collect();
        let e2: Vec<_> = fs::read_dir(tmp2.path()).unwrap().flatten().collect();

        let fp1 = fingerprint(&e1);
        let fp2 = fingerprint(&e2);

        assert_ne!(fp1, fp2, "不同名单应产生不同指纹");
    }

    #[test]
    fn test_fingerprint_order_independent() {
        // 指纹对 entries 排序后计算，顺序无关
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("z.txt"), "z\n").unwrap();
        fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
        fs::write(tmp.path().join("m.txt"), "m\n").unwrap();

        let mut entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().flatten().collect();
        let fp_sorted = fingerprint(&entries);

        // 反转顺序
        entries.reverse();
        let fp_reversed = fingerprint(&entries);

        assert_eq!(fp_sorted, fp_reversed, "指纹应与 entry 顺序无关");
    }

    #[test]
    fn test_fingerprint_distinguishes_file_and_dir() {
        // 同名 entry 但类型不同（文件 vs 目录）→ 指纹不同
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        // tmp1: "foo" 是文件
        fs::write(tmp1.path().join("foo"), "file\n").unwrap();
        // tmp2: "foo" 是目录
        fs::create_dir(tmp2.path().join("foo")).unwrap();

        let e1: Vec<_> = fs::read_dir(tmp1.path()).unwrap().flatten().collect();
        let e2: Vec<_> = fs::read_dir(tmp2.path()).unwrap().flatten().collect();

        let fp1 = fingerprint(&e1);
        let fp2 = fingerprint(&e2);

        assert_ne!(fp1, fp2, "同名但类型不同应产生不同指纹");
    }

    // ================================================================
    // 交叉验证：三模式行为一致性
    // ================================================================

    #[test]
    fn test_three_modes_same_file_set_first_call() {
        // 首次列举：三模式应返回相同文件集
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let tmp3 = TempDir::new().unwrap();
        for tmp in [&tmp1, &tmp2, &tmp3] {
            fs::write(tmp.path().join("a.txt"), "alpha\n").unwrap();
            fs::write(tmp.path().join("b.txt"), "beta\n").unwrap();
        }
        let plan = PathPlan::new();

        let (f_lax, _) = DirIndex::new(tmp1.path(), FreshnessMode::Lax).list(&plan);
        let (f_strict, _) = DirIndex::new(tmp2.path(), FreshnessMode::Strict).list(&plan);
        let (f_off, _) = DirIndex::new(tmp3.path(), FreshnessMode::Off).list(&plan);

        assert_eq!(f_lax.len(), 2);
        assert_eq!(f_strict.len(), 2);
        assert_eq!(f_off.len(), 2);
    }
}
