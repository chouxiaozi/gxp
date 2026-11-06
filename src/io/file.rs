//! FileContent：文件读取策略（D10 + 流式 fallback）。
//!
//! 不变式：
//! - 文件 ≤ `STREAMING_THRESHOLD`（512MB）：`read_to_end` 到 `Vec<u8>`（D10 生产路径）
//! - 文件 > `STREAMING_THRESHOLD`：返回 `Stream(PathBuf)`，不读入内存
//!   - 调用方通过 `is_stream()` 判定走流式扫描路径
//!   - 流式路径用 `BufReader::read_until(b'\n')` 逐行扫描（与 `scan_lines_cb` 语义对齐）
//! - 不使用 mmap（实测：mmap page fault 分摊三阶段反而慢 10ms，详见 Rust 方案 §6.2）
//! - 规避 SIGBUS：`read_to_end` 把数据拷贝到进程堆，文件被截断/写入不影响读取
//!
//! # 历史变更
//!
//! - 早期：mmap 阈值 64KB（小文件 `read_to_end` / 大文件 `memmap2::Mmap`）
//! - 改判 D10，统一 `read_to_end`。理由见 Rust 方案 §6.2
//! - 流式 fallback。> 512MB 文件返回 `Stream(PathBuf)`，
//!   调用方走流式扫描路径。V7 验证：1GB read_to_end 峰值 ~1.23GB，
//!   512MB 阈值在 16GB 工作站上安全（Vec + 候选行 ~630MB），容器环境需评估。
//!   read_to_end 比流式快 5-16%，阈值内仍走 read_to_end。
//! - mmap 路径仅保留 `benches/diag_mmap.rs` 作对照基准（不进生产代码）

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 流式 fallback 阈值：超过此大小的文件不 read_to_end，改走流式扫描。
///
/// 512MB 选择依据：V7 实测 1GB read_to_end 峰值 ~1.23GB（Vec + 候选行 clone），
/// 512MB Vec + ~120MB 候选行 = ~630MB 峰值，16GB 工作站轻松承载。
/// read_to_end 比流式快 5-16%，阈值内仍走 read_to_end 以保持性能优势。
pub const STREAMING_THRESHOLD: usize = 512 * 1024 * 1024;

/// stdin 读取常量：用作 `file_path` 参数时的虚拟路径。
pub const STDIN_PATH: &str = "<stdin>";

/// 文件内容抽象。
///
/// - `Vec`：文件 ≤ 512MB，全量 `read_to_end` 到 `Vec<u8>`（D10 生产路径）
/// - `Stream`：文件 > 512MB，仅持有路径供流式扫描使用（流式 fallback）
#[derive(Debug)]
pub enum FileContent {
    /// 文件内容：`read_to_end` 到 `Vec<u8>`（文件 ≤ 512MB）。
    Vec(Vec<u8>),
    /// 文件路径：文件 > 512MB，不读入内存，供流式扫描使用。
    Stream(PathBuf),
}

impl FileContent {
    /// 返回内容字节切片（仅 `Vec` 变体）。
    ///
    /// **注意**：`Stream` 变体返回空切片。调用方必须先通过 `is_stream()` 判定，
    /// 走流式路径而非调用此方法。直接对 `Stream` 调用 `as_bytes()` 会静默返回空数据。
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            FileContent::Vec(v) => v,
            FileContent::Stream(_) => &[],
        }
    }

    /// 是否为流式模式（文件 > 512MB）。
    pub fn is_stream(&self) -> bool {
        matches!(self, FileContent::Stream(_))
    }

    /// 返回流式模式的文件路径（仅 `Stream` 变体）。
    pub fn stream_path(&self) -> Option<&Path> {
        match self {
            FileContent::Stream(p) => Some(p.as_path()),
            FileContent::Vec(_) => None,
        }
    }
}

/// 读取文件用于扫描。
///
/// - 文件 ≤ `STREAMING_THRESHOLD`（512MB）：`read_to_end` 到 `Vec<u8>`（D10 生产路径）
/// - 文件 > `STREAMING_THRESHOLD`：返回 `Stream(PathBuf)`，不读入内存（流式 fallback）
///
/// 错误：文件不存在/无权限/IO 错误时返回 `io::Error`。
/// 空文件：返回 `FileContent::Vec(Vec::new())`（合法，扫描时 0 命中）。
pub fn read_for_scan(path: &Path) -> std::io::Result<FileContent> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    let size = meta.len() as usize;

    if size > STREAMING_THRESHOLD {
        // 超过阈值，不 read_to_end，返回路径供流式扫描
        return Ok(FileContent::Stream(path.to_path_buf()));
    }

    // D10：≤ 512MB 文件统一 read_to_end
    let mut buf = Vec::with_capacity(size);
    file.take(size as u64).read_to_end(&mut buf)?;
    Ok(FileContent::Vec(buf))
}

/// 读取 stdin 用于扫描：`read_to_end` 到 `Vec<u8>`，与文件路径统一。
///
/// stdin 管道支持（Rust 方案 §11.2 V7 验证）：stdin 全量读入 Vec 后
/// 可多次扫描，`--verify` 无需特殊处理即可工作（D10 `read_to_end` 模型）。
///
/// 空输入：返回 `FileContent::Vec(Vec::new())`（合法，扫描时 0 命中）。
pub fn read_stdin_for_scan() -> std::io::Result<FileContent> {
    let stdin = std::io::stdin();
    let mut buf = Vec::new();
    stdin.lock().read_to_end(&mut buf)?;
    Ok(FileContent::Vec(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("创建临时文件失败");
        f.write_all(content).expect("写入失败");
        f.flush().expect("flush 失败");
        f
    }

    #[test]
    fn test_read_small_file_uses_vec() {
        let content = b"error line\ninfo line";
        let f = write_temp(content);
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(matches!(fc, FileContent::Vec(_)));
        assert_eq!(fc.as_bytes(), content);
    }

    #[test]
    fn test_read_empty_file() {
        let f = write_temp(b"");
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(matches!(fc, FileContent::Vec(_)));
        assert!(fc.as_bytes().is_empty());
    }

    #[test]
    fn test_read_large_file_uses_vec() {
        // 大文件也走 read_to_end（不再走 mmap）
        // 构造 > 64KB 的文件（64KB + 1 字节）
        let size = (64 * 1024 + 1) as usize;
        let content: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let f = write_temp(&content);
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(matches!(fc, FileContent::Vec(_)));
        assert_eq!(fc.as_bytes().len(), size);
        // 验证内容正确（抽样首尾）
        assert_eq!(fc.as_bytes()[0], 0);
        assert_eq!(fc.as_bytes()[size - 1], ((size - 1) % 256) as u8);
    }

    #[test]
    fn test_streaming_threshold_boundary() {
        // 阈值边界：恰好 = STREAMING_THRESHOLD → Vec（> 才走 Stream）
        let size = STREAMING_THRESHOLD;
        let content: Vec<u8> = vec![b'x'; size];
        let f = write_temp(&content);
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(matches!(fc, FileContent::Vec(_)), "恰好等于阈值应走 Vec");
        assert_eq!(fc.as_bytes().len(), size);
    }

    #[test]
    fn test_streaming_threshold_exceeded() {
        // 超过阈值 → Stream（不读入内存）
        let size = STREAMING_THRESHOLD + 1;
        let content: Vec<u8> = vec![b'x'; size];
        let f = write_temp(&content);
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(fc.is_stream(), "超过阈值应走 Stream");
        assert!(fc.as_bytes().is_empty(), "Stream 不应读入内容");
        assert_eq!(fc.stream_path(), Some(f.path()), "Stream 应持有文件路径");
    }

    #[test]
    fn test_read_nonexistent_file_errors() {
        let r = read_for_scan(Path::new("/nonexistent/path/does/not/exist.log"));
        assert!(r.is_err(), "不存在的文件应报错");
        let err = r.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "期望 NotFound，得到 {:?}",
            err.kind()
        );
    }

    #[test]
    fn test_read_64kb_boundary_uses_vec() {
        // 64KB 边界也走 read_to_end（不再有 mmap 阈值判定）
        let size = 64 * 1024;
        let content: Vec<u8> = vec![b'x'; size];
        let f = write_temp(&content);
        let fc = read_for_scan(f.path()).expect("读取应成功");
        assert!(matches!(fc, FileContent::Vec(_)));
        assert_eq!(fc.as_bytes().len(), size);
    }

    // --- stdin 读取测试 ---

    #[test]
    fn test_stdin_path_constant() {
        assert_eq!(STDIN_PATH, "<stdin>");
    }

    // 注：read_stdin_for_scan() 依赖进程级 stdin，无法在单元测试中隔离。
    // stdin 全链路测试在 tests/cli_e2e.rs 集成测试中覆盖（通过 run_with_writer + pipe 模拟）。
}
