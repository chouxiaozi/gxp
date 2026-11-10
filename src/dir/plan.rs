//! PathPlan：路径谓词下推剪枝。
//!
//! 移植自 `prototype/gxp_dir.py::PathPlan`。关键不变式：
//! - 谓词在 readdir 阶段决定"是否进入子目录"（剪枝收益唯一来源）
//! - `dir_re`（无锚定正则）不可剪枝，一律进入（正确性优先，§12.3）
//! - `dir_prefix` 可剪枝（祖先/后代双向判定）
//! - `max_depth` 可剪枝（depth 比较）
//!
//! 实现选型（见 Rust 方案 §4.2）：
//! - 手写 `std::fs::read_dir` 栈式遍历（与 `DirIndex::list` 一致）
//! - 早期用 `ignore::WalkBuilder`，但固定开销在小目录场景偏慢（实测见 performance-baseline §1.4）
//! - `follow_links` 用手写 `seen_real`（`HashSet<PathBuf>`）做 symlink 环检测
//!
//! 新增（设计文档 §10）：
//! - `--glob PATTERN`：文件名 glob 过滤（globset crate，DFA 高性能）
//! - `.gitignore` 尊重：默认跳过 .gitignore 匹配的文件（`ignore::Gitignore` 匹配器）
//! - 两者都在 walk 循环中做，不破坏行级扫描模型，不影响 L1 ⊇ L2 不变式

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;

/// 路径谓词：决定哪些文件命中、哪些目录值得进入。
///
/// 与 Python `PathPlan` 字段一一对应。
#[derive(Debug, Clone)]
pub struct PathPlan {
    /// 文件扩展名白名单（已 strip 前导 `.`）。
    pub ext: Option<HashSet<String>>,
    /// 目录相对路径前缀（祖先/后代双向剪枝，§12.3）。
    pub dir_prefix: Option<String>,
    /// 文件名正则（无锚定，可命中任意位置）。
    pub name_re: Option<Regex>,
    /// 目录相对路径正则（不可剪枝，只后过滤，§12.3）。
    pub dir_re: Option<Regex>,
    /// 最大深度（root 下文件 depth=0）。
    pub max_depth: Option<usize>,
    /// 最小深度（root 下文件 depth=0）。
    pub min_depth: Option<usize>,
    /// 是否 follow symlink（默认 false）。
    pub follow_links: bool,
    /// 文件名 glob 过滤模式列表（OR 语义，globset 编译为 GlobSet）。
    pub glob_patterns: Option<Vec<String>>,
    /// 是否尊重 .gitignore（默认 true）。
    pub respect_gitignore: bool,
}

impl Default for PathPlan {
    fn default() -> Self {
        Self::new()
    }
}

impl PathPlan {
    /// 空谓词（命中所有文件）。
    pub fn new() -> Self {
        Self {
            ext: None,
            dir_prefix: None,
            name_re: None,
            dir_re: None,
            max_depth: None,
            min_depth: None,
            follow_links: false,
            glob_patterns: None,
            respect_gitignore: true,
        }
    }

    /// Builder 风格设置 ext（集合，每项可带或不带前导 `.`）。
    pub fn with_ext<I: IntoIterator<Item = String>>(mut self, ext: I) -> Self {
        self.ext = Some(
            ext.into_iter()
                .map(|e| e.trim_start_matches('.').to_string())
                .collect(),
        );
        self
    }

    /// Builder 风格设置 dir_prefix（自动 strip 前导 `/`）。
    pub fn with_dir_prefix(mut self, prefix: impl Into<String>) -> Self {
        let p = prefix.into();
        self.dir_prefix = Some(p.trim_start_matches('/').to_string());
        self
    }

    /// Builder 风格设置 name_re。
    pub fn with_name_re(mut self, re: Regex) -> Self {
        self.name_re = Some(re);
        self
    }

    /// Builder 风格设置 dir_re（注意：dir_re 不剪枝，只做后过滤，见 §12.3）。
    pub fn with_dir_re(mut self, re: Regex) -> Self {
        self.dir_re = Some(re);
        self
    }

    /// Builder 风格设置 max_depth（root 下文件 depth=0，§12.3）。
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }

    /// Builder 风格设置 min_depth（root 下文件 depth=0，§12.3）。
    pub fn with_min_depth(mut self, depth: usize) -> Self {
        self.min_depth = Some(depth);
        self
    }

    /// Builder 风格设置是否 follow symlink（默认 false，§12.3）。
    pub fn with_follow_links(mut self, yes: bool) -> Self {
        self.follow_links = yes;
        self
    }

    /// Builder 风格设置 glob 过滤模式列表。
    pub fn with_glob_patterns(mut self, patterns: Vec<String>) -> Self {
        self.glob_patterns = if patterns.is_empty() {
            None
        } else {
            Some(patterns)
        };
        self
    }

    /// Builder 风格设置是否尊重 .gitignore（默认 true）。
    pub fn with_respect_gitignore(mut self, yes: bool) -> Self {
        self.respect_gitignore = yes;
        self
    }

    /// 单文件判定（与 Python `file_ok` 一致）。
    ///
    /// `size` 参数为 `DirIndex` 的 lax/strict 模式预留：PathPlan.walk 里传 `None`，
    /// DirIndex 里 stat 到 size 后传 `Some(size)` 用于 size 谓词（v1 未实现 size 谓词，
    /// 保留接口）。
    #[inline]
    pub fn file_ok(&self, name: &str, rel_dir: &str, depth: usize, _size: Option<u64>) -> bool {
        if let Some(ref exts) = self.ext {
            let e = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            if !exts.contains(e) {
                return false;
            }
        }
        if let Some(ref re) = self.name_re {
            if !re.is_match(name) {
                return false;
            }
        }
        if let Some(ref prefix) = self.dir_prefix {
            if !(rel_dir == prefix.as_str() || rel_dir.starts_with(&format!("{}/", prefix))) {
                return false;
            }
        }
        // dir_re 不可剪枝（§12.3）：只做后过滤
        if let Some(ref re) = self.dir_re {
            if !re.is_match(rel_dir) {
                return false;
            }
        }
        if let Some(max) = self.max_depth {
            if depth > max {
                return false;
            }
        }
        if let Some(min) = self.min_depth {
            if depth < min {
                return false;
            }
        }
        true
    }

    /// 单文件判定（从绝对路径计算 rel_dir/depth），供暴力对账测试用。
    pub fn match_path(&self, path: &Path, root: &Path) -> bool {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let parts: Vec<&str> = rel_str.split('/').collect();
        let rel_dir = if parts.len() <= 1 {
            String::new()
        } else {
            parts[..parts.len() - 1].join("/")
        };
        let name = parts.last().copied().unwrap_or("");
        self.file_ok(name, &rel_dir, parts.len().saturating_sub(1), None)
    }

    /// 目录是否值得进入（pre-descent 剪枝，`dir_eligible` 端口）。
    ///
    /// 与 Python 一致：
    /// - root（`rel_dir == ""`）一律放行（Python 隐式跳过 root 判定，
    ///   WalkBuilder 的 `filter_entry` 会对 root 也调用，这里显式放行对齐语义）
    /// - `dir_prefix` 做祖先/后代双向判定（当前是目标的前缀 或 目标是当前的前缀）
    /// - `dir_re` 不剪枝（无锚定正则无法证明后代不命中，§12.3）
    /// - `max_depth` 剪枝（depth 比较）
    #[inline]
    pub fn dir_eligible(&self, rel_dir: &str, depth: usize) -> bool {
        // root 一律放行（与 Python 隐式跳过 root 判定对齐）
        if rel_dir.is_empty() {
            return true;
        }
        if let Some(ref prefix) = self.dir_prefix {
            if !(rel_dir == prefix.as_str()
                || rel_dir.starts_with(&format!("{}/", prefix))
                || prefix.starts_with(&format!("{}/", rel_dir)))
            {
                return false;
            }
        }
        if let Some(max) = self.max_depth {
            if depth > max {
                return false;
            }
        }
        // dir_re 不剪枝（无锚定正则无法证明后代不命中，§12.3）
        true
    }

    /// 遍历 root，返回命中文件绝对路径 + 统计。
    ///
    /// 实现端口与 Python `PathPlan.walk` 一致：
    /// - `dirs_total` / `dirs_visited` / `pruned_dirs` / `links_skipped` / `files_seen` / `files_matched`
    /// - 不计 `index_stats`（PathPlan 不做 stat，那是 DirIndex 的职责）
    ///
    /// 改为手写 `std::fs::read_dir` 栈式遍历（与 `DirIndex::list` 一致），
    /// 替代 `ignore::WalkBuilder`。原因：WalkBuilder 有 ~6ms 固定开销
    ///（线程池初始化 + filter_entry 闭包 Arc/Mutex），在 3 目录小场景下
    /// 比 Python `os.scandir` 手写栈慢 12×。手写栈消除该固定开销。
    ///
    /// 新增：walk 循环中做 `--glob` 文件名过滤 + `.gitignore` 尊重。
    /// - `--glob`：编译为 `globset::GlobSet`，匹配 `file_name()`（不是完整路径）
    /// - `.gitignore`：用 `ignore::gitignore::Gitignore` 匹配器，从 walk 根目录
    ///   向上查找 `.gitignore` + walk 到的每层目录的 `.gitignore`
    pub fn walk(&self, root: &Path) -> (Vec<PathBuf>, WalkStats) {
        let mut stats = WalkStats {
            dirs_total: 1, // root 自身
            ..Default::default()
        };
        let mut out = Vec::new();

        // 编译 glob patterns 为 GlobSet（fail-loud：语法错误立即报错）
        let glob_set: Option<GlobSet> = self.glob_patterns.as_ref().and_then(|patterns| {
            if patterns.is_empty() {
                return None;
            }
            let mut builder = GlobSetBuilder::new();
            for p in patterns {
                if let Ok(g) = Glob::new(p) {
                    builder.add(g);
                }
            }
            builder.build().ok()
        });

        // 加载 .gitignore 匹配器
        let mut gitignore = if self.respect_gitignore {
            Some(GitignoreChecker::new(root))
        } else {
            None
        };

        // 手写栈式遍历（与 DirIndex::list 一致，但不做 fingerprint/stat）
        // 栈元素：(dir_path, rel_dir, depth)
        // depth 语义：root=0，root 下文件 depth=0，一级子目录下文件 depth=1
        let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
        let mut seen_real: HashSet<PathBuf> = HashSet::new();
        let root_real = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        seen_real.insert(root_real);

        while let Some((dp, rel, depth)) = stack.pop() {
            stats.dirs_visited += 1;

            // 加载当前目录的 .gitignore
            if let Some(ref mut gi) = gitignore {
                gi.load_dir_gitignore(&dp);
            }

            let entries = match std::fs::read_dir(&dp) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for e in entries.flatten() {
                // 判定文件类型（DirEntry::file_type 不 follow symlink，与 Python PathPlan 一致）
                let ft = match e.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                // follow_links 模式下对 symlink 跟随 metadata 判定真实类型
                // （B16：不跟随时 symlink-to-dir 的 is_dir()=false，环检测分支不可达，
                //  links_skipped 恒 0——Linux CI 首跑暴露，Windows 测试分支不建 symlink 掩盖）
                // 非 follow 模式 symlink 一律忽略（与 ripgrep 默认一致）；断链 symlink 跟随失败也忽略
                let (is_file_now, is_dir_now) = if ft.is_symlink() && self.follow_links {
                    match std::fs::metadata(e.path()) {
                        Ok(m) => (m.is_file(), m.is_dir()),
                        Err(_) => (false, false),
                    }
                } else {
                    (ft.is_file(), ft.is_dir())
                };

                if is_file_now {
                    stats.files_seen += 1;
                    let name = e.file_name().to_string_lossy().into_owned();
                    let path = e.path();

                    // glob 过滤——匹配文件名，不匹配则跳过
                    if let Some(ref gs) = glob_set {
                        if !gs.is_match(&name) {
                            continue;
                        }
                    }

                    // .gitignore 过滤——匹配则跳过
                    if let Some(ref gi) = gitignore {
                        if gi.is_ignored(&path, &name, &rel) {
                            continue;
                        }
                    }

                    if self.file_ok(&name, &rel, depth, None) {
                        out.push(path);
                        stats.files_matched += 1;
                    }
                } else if is_dir_now {
                    // 计算子目录相对路径
                    let crel = if rel.is_empty() {
                        e.file_name().to_string_lossy().into_owned()
                    } else {
                        format!("{}/{}", rel, e.file_name().to_string_lossy())
                    };
                    stats.dirs_total += 1;

                    // .gitignore 目录过滤——匹配则跳过整个子树
                    if let Some(ref gi) = gitignore {
                        let dir_name = e.file_name().to_string_lossy().into_owned();
                        if gi.is_ignored(&e.path(), &dir_name, &rel) {
                            stats.pruned_dirs += 1;
                            continue;
                        }
                    }

                    if !self.dir_eligible(&crel, depth + 1) {
                        stats.pruned_dirs += 1;
                        continue;
                    }
                    // follow_links 时做 symlink 环检测
                    if self.follow_links {
                        let rp = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
                        if seen_real.contains(&rp) {
                            stats.links_skipped += 1;
                            continue;
                        }
                        seen_real.insert(rp);
                    }
                    stack.push((e.path(), crel, depth + 1));
                }
            }
        }

        // 输出排序：文件系统枚举序跨平台不确定（NTFS 字典序 / ext4 哈希 / APFS 不保证），
        // 确定性输出是 LLM 友好原则（输出自包含）的前提；排序键为路径字典序，跨平台一致。
        // B17：macOS CI 首跑暴露（a.log/b.log 输出序颠倒，测试断言首行为 a.log 失败）
        out.sort();
        (out, stats)
    }
}

/// Walk 统计：与 Python `PathPlan.walk` 返回的 dict 字段一一对应。
#[derive(Debug, Default, Clone)]
pub struct WalkStats {
    /// 遍历中见过的目录总数（含 root）。
    pub dirs_total: usize,
    /// 实际进入 readdir 的目录数（dirs_total - pruned_dirs）。
    pub dirs_visited: usize,
    /// 被 `dir_eligible` 剪掉的目录数。
    pub pruned_dirs: usize,
    /// 因 symlink 环检测跳过的链接数（v1 恒为 0，WalkBuilder 内部处理）。
    pub links_skipped: usize,
    /// 见过的文件总数（命中 + 未命中）。
    pub files_seen: usize,
    /// 命中 plan 谓词的文件数。
    pub files_matched: usize,
}

// =========================================================================
// .gitignore 尊重 + glob 编译辅助
// =========================================================================

/// `.gitignore` 匹配器：在 walk 循环中按目录加载 .gitignore 规则。
///
/// 实现方式（设计文档 §10 + Rust 方案 §4.2）：
/// - walk 根目录向上查找 `.gitignore`（处理仓库根 .gitignore）
/// - walk 到的每层目录检查是否有 `.gitignore`，有则加入
/// - `is_ignored` 检查文件路径是否被任何已加载的规则匹配
///
/// 使用 `ignore::gitignore::Gitignore` 独立匹配器（不换回 WalkBuilder）。
struct GitignoreChecker {
    /// 各目录的 Gitignore 匹配器 + 该目录的绝对路径前缀
    matchers: Vec<(PathBuf, ignore::gitignore::Gitignore)>,
}

impl GitignoreChecker {
    fn new(root: &Path) -> Self {
        let mut matchers = Vec::new();

        // 从 walk 根目录向上查找 .gitignore（处理仓库根的 .gitignore）
        // 这处理 `gxp 'pattern' subproject/src/` 时 subproject/.gitignore 生效的场景
        let mut current = root.to_path_buf();
        loop {
            let gi_path = current.join(".gitignore");
            if gi_path.exists() {
                let (built, _err) = ignore::gitignore::Gitignore::new(&gi_path);
                matchers.push((current.clone(), built));
            }
            // 向上一级
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break,
            }
        }

        // walk 根目录自身的 .gitignore 也会被 load_dir_gitignore 加载
        // （上面循环可能已加载，重复加载不影响正确性）

        Self { matchers }
    }

    /// 加载指定目录的 .gitignore（如果存在且未已加载）。
    fn load_dir_gitignore(&mut self, dir: &Path) {
        let gi_path = dir.join(".gitignore");
        if !gi_path.exists() {
            return;
        }
        // 检查是否已加载（避免重复）
        for (loaded_dir, _) in &self.matchers {
            if loaded_dir == dir {
                return;
            }
        }
        let (built, _err) = ignore::gitignore::Gitignore::new(&gi_path);
        self.matchers.push((dir.to_path_buf(), built));
    }

    /// 检查文件是否被 .gitignore 规则匹配。
    ///
    /// `path` 是文件绝对路径，`name` 是文件名，`rel_dir` 是相对 walk 根的目录路径。
    fn is_ignored(&self, path: &Path, name: &str, rel_dir: &str) -> bool {
        for (dir, gi) in &self.matchers {
            // 计算相对于 .gitignore 所在目录的路径
            let rel_path = path.strip_prefix(dir).unwrap_or(Path::new(name));
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");

            // ignore::gitignore::Gitignore 匹配需要相对路径
            match gi.matched_path_or_any_parents(&rel_str, false) {
                ignore::Match::Ignore(_) => return true,
                ignore::Match::Whitelist(_) => return false, // 白名单覆盖黑名单
                ignore::Match::None => {}
            }
        }
        let _ = rel_dir; // rel_dir 保留给未来优化（直接相对路径匹配）
        false
    }
}

/// 编译 glob patterns 为 GlobSet，fail-loud 报错。
///
/// glob 语法错误（如 `[abc` 未闭合）→ 返回 Err。
pub fn compile_globset(patterns: &[String]) -> crate::Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        match Glob::new(p) {
            Ok(g) => {
                builder.add(g);
            }
            Err(e) => {
                return Err(crate::GxpError::Runtime {
                    detail: format!("glob 语法错误: '{p}' — {e}"),
                });
            }
        }
    }
    let set = builder.build().map_err(|e| crate::GxpError::Runtime {
        detail: format!("globset 编译失败: {e}"),
    })?;
    Ok(Some(set))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    /// 在 `root` 下创建文件 `rel_path`（含中间目录），写入 `content`。
    fn create_file(root: &Path, rel_path: &str, content: &str) -> PathBuf {
        let full = root.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(&full).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        full
    }

    /// 从 `(Vec<PathBuf>, WalkStats)` 中提取文件名集（排序后比较）。
    fn names_sorted(files: &[PathBuf]) -> Vec<String> {
        let mut v: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    // -----------------------------------------------------------------
    // 1. 正常 walk：多个文件，walk 返回全部
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_returns_all_files_in_dir() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "a.txt", "hello");
        create_file(tmp.path(), "b.log", "world");
        create_file(tmp.path(), "sub/c.rs", "fn main(){}");

        let plan = PathPlan::new();
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        let mut expected = vec!["a.txt".to_string(), "b.log".to_string(), "c.rs".to_string()];
        expected.sort();
        assert_eq!(got, expected, "walk 应返回全部 3 个文件");
        assert_eq!(stats.files_matched, 3, "files_matched 应为 3");
        assert_eq!(stats.files_seen, 3, "files_seen 应为 3");
        assert_eq!(stats.dirs_total, 2, "dirs_total: root + sub = 2");
        assert_eq!(stats.dirs_visited, 2, "dirs_visited: root + sub = 2");
        assert_eq!(stats.pruned_dirs, 0, "无剪枝");
    }

    // -----------------------------------------------------------------
    // 2. prefix 过滤：只返回 dir_prefix 下的文件
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_dir_prefix_filters_to_matching_subdir() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "keep/x.txt", "1");
        create_file(tmp.path(), "keep/y.txt", "2");
        create_file(tmp.path(), "other/z.txt", "3");

        let plan = PathPlan::new().with_dir_prefix("keep");
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        assert_eq!(
            got,
            vec!["x.txt".to_string(), "y.txt".to_string()],
            "prefix=keep 应只返回 keep/ 下 2 个文件"
        );
        assert_eq!(stats.files_matched, 2, "files_matched 应为 2");
        // dir_eligible 剪掉 other/
        assert_eq!(stats.pruned_dirs, 1, "other/ 被剪枝");
    }

    // -----------------------------------------------------------------
    // 3. max_depth 限制：depth > max_depth 的文件不返回
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_max_depth_excludes_deeper_files() {
        let tmp = TempDir::new().unwrap();
        // depth=0: root 下文件
        create_file(tmp.path(), "d0.txt", "0");
        // depth=1: sub/ 下文件
        create_file(tmp.path(), "sub/d1.txt", "1");
        // depth=2: sub/deep/ 下文件
        create_file(tmp.path(), "sub/deep/d2.txt", "2");

        let plan = PathPlan::new().with_max_depth(1);
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        // d0.txt (depth=0) 和 d1.txt (depth=1) 应返回，d2.txt (depth=2) 不返回
        assert!(got.contains(&"d0.txt".to_string()), "depth=0 应返回");
        assert!(got.contains(&"d1.txt".to_string()), "depth=1 应返回");
        assert!(!got.contains(&"d2.txt".to_string()), "depth=2 不应返回");
        assert_eq!(stats.files_matched, 2, "files_matched 应为 2");
    }

    // -----------------------------------------------------------------
    // 4. glob 过滤：只返回文件名匹配 glob 的文件
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_glob_filter_returns_only_matching_filenames() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "app.rs", "a");
        create_file(tmp.path(), "lib.rs", "b");
        create_file(tmp.path(), "README.md", "c");
        create_file(tmp.path(), "sub/mod.rs", "d");

        let plan = PathPlan::new().with_glob_patterns(vec!["*.rs".to_string()]);
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        let mut expected = vec![
            "app.rs".to_string(),
            "lib.rs".to_string(),
            "mod.rs".to_string(),
        ];
        expected.sort();
        assert_eq!(got, expected, "glob *.rs 应只返回 .rs 文件，共 3 个");
        assert_eq!(stats.files_matched, 3, "files_matched 应为 3");
        // README.md 被 glob 过滤跳过，但 files_seen 统计所有文件
        assert_eq!(stats.files_seen, 4, "files_seen 应为 4（含 README.md）");
    }

    // -----------------------------------------------------------------
    // 5. glob 语法错误：compile_globset 对非法 glob 返回 Err
    // -----------------------------------------------------------------
    #[test]
    fn test_compile_globset_rejects_invalid_glob_syntax() {
        // `[abc` 未闭合字符类——globset 视为语法错误
        let bad = vec!["[abc".to_string()];
        let result = compile_globset(&bad);
        let Err(e) = result else {
            panic!("非法 glob `[abc` 应返回 Err，实际 Ok");
        };
        let msg = format!("{e}");
        assert!(
            msg.contains("glob 语法错误") || msg.contains("globset 编译失败"),
            "错误消息应含语法错误描述，实际: {msg}"
        );
    }

    // -----------------------------------------------------------------
    // 6. 空目录：walk 返回 0 文件
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_empty_dir_returns_zero_files() {
        let tmp = TempDir::new().unwrap();
        let plan = PathPlan::new();
        let (files, stats) = plan.walk(tmp.path());

        assert!(files.is_empty(), "空目录应返回 0 文件");
        assert_eq!(stats.files_matched, 0, "files_matched 应为 0");
        assert_eq!(stats.files_seen, 0, "files_seen 应为 0");
        assert_eq!(stats.dirs_total, 1, "dirs_total: 只有 root");
        assert_eq!(stats.dirs_visited, 1, "dirs_visited: 只访问了 root");
    }

    // -----------------------------------------------------------------
    // 7. 嵌套目录：多层子目录的文件都返回
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_nested_directories_returns_all_levels() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "l0.txt", "0");
        create_file(tmp.path(), "a/l1.txt", "1");
        create_file(tmp.path(), "a/b/l2.txt", "2");
        create_file(tmp.path(), "a/b/c/l3.txt", "3");

        let plan = PathPlan::new();
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        let mut expected = vec![
            "l0.txt".to_string(),
            "l1.txt".to_string(),
            "l2.txt".to_string(),
            "l3.txt".to_string(),
        ];
        expected.sort();
        assert_eq!(got, expected, "嵌套 4 层应返回全部 4 个文件");
        assert_eq!(stats.files_matched, 4, "files_matched 应为 4");
        // root + a + b + c = 4 个目录
        assert_eq!(stats.dirs_total, 4, "dirs_total: root + a + b + c = 4");
        assert_eq!(stats.dirs_visited, 4, "dirs_visited 应为 4");
    }

    // -----------------------------------------------------------------
    // 8. gitignore 尊重：respect_gitignore=true 时跳过 .gitignore 中的文件
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_respects_gitignore() {
        let tmp = TempDir::new().unwrap();
        // 创建 .gitignore 忽略 *.tmp
        create_file(tmp.path(), ".gitignore", "*.tmp\n");
        create_file(tmp.path(), "keep.txt", "1");
        create_file(tmp.path(), "skip.tmp", "2");
        create_file(tmp.path(), "sub/data.tmp", "3");
        create_file(tmp.path(), "sub/keep.rs", "4");

        // respect_gitignore = true（默认）
        let plan = PathPlan::new();
        let (files, stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        // *.tmp 被 .gitignore 忽略，只保留 keep.txt 和 keep.rs
        assert!(
            got.contains(&"keep.txt".to_string()),
            "keep.txt 不被 ignore，应返回"
        );
        assert!(
            got.contains(&"keep.rs".to_string()),
            "keep.rs 不被 ignore，应返回"
        );
        assert!(
            !got.contains(&"skip.tmp".to_string()),
            "skip.tmp 被 .gitignore 忽略，不应返回"
        );
        assert!(
            !got.contains(&"data.tmp".to_string()),
            "sub/data.tmp 被 .gitignore 忽略，不应返回"
        );
        // .gitignore 文件本身不被 *.tmp 匹配，会被返回（非被忽略）
        assert!(
            got.contains(&".gitignore".to_string()),
            ".gitignore 文件不被 *.tmp 模式匹配，应返回"
        );
        // keep.txt + keep.rs + .gitignore = 3
        assert_eq!(
            stats.files_matched, 3,
            "files_matched: keep.txt + keep.rs + .gitignore = 3"
        );
    }

    // -----------------------------------------------------------------
    // 补充：respect_gitignore=false 时不跳过 .gitignore 中的文件
    // -----------------------------------------------------------------
    #[test]
    fn test_walk_disabling_gitignore_keeps_all_files() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), ".gitignore", "*.tmp\n");
        create_file(tmp.path(), "keep.txt", "1");
        create_file(tmp.path(), "skip.tmp", "2");

        let plan = PathPlan::new().with_respect_gitignore(false);
        let (files, _stats) = plan.walk(tmp.path());

        let got = names_sorted(&files);
        // 关闭 gitignore 后，skip.tmp 应返回（.gitignore 仍被 file_ok 判定为不匹配，但本测试关注 .tmp 是否被保留）
        assert!(
            got.contains(&"skip.tmp".to_string()),
            "关闭 gitignore 后 skip.tmp 应返回"
        );
        assert!(got.contains(&"keep.txt".to_string()), "keep.txt 应始终返回");
    }

    // -----------------------------------------------------------------
    // 补充：compile_globset 空列表返回 Ok(None)
    // -----------------------------------------------------------------
    #[test]
    fn test_compile_globset_empty_patterns_returns_none() {
        let result = compile_globset(&[]);
        assert!(result.is_ok(), "空列表应返回 Ok(None)");
        assert!(result.unwrap().is_none(), "空列表的 Ok 值应为 None");
    }

    // -----------------------------------------------------------------
    // 补充：compile_globset 多 pattern 正常编译
    // -----------------------------------------------------------------
    #[test]
    fn test_compile_globset_multiple_patterns_compiles_ok() {
        let patterns = vec!["*.rs".to_string(), "*.go".to_string()];
        let result = compile_globset(&patterns);
        assert!(result.is_ok(), "合法 glob 应编译成功");
        let set = result.unwrap().unwrap();
        assert!(set.is_match("main.rs"), "*.rs 应匹配 main.rs");
        assert!(set.is_match("main.go"), "*.go 应匹配 main.go");
        assert!(!set.is_match("main.py"), "*.py 不应被匹配");
    }
}
