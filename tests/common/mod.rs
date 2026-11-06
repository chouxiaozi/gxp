//! cli_e2e 测试共享辅助函数。
//!
//! 提取自原 `tests/cli_e2e.rs`，供所有 `cli_*.rs` 测试文件复用。
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

use gxp::cli::RunOpts;
use gxp::io::file::FileContent;
use gxp::mode::detect::{dispatch_with_content, run_with_writer};

/// 构造临时文件并写入内容。
pub fn write_temp(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("创建临时文件失败");
    f.write_all(content.as_bytes()).expect("写入失败");
    f.flush().expect("flush 失败");
    f
}

/// 构造临时目录，写入多个文件。返回 (dir, file_paths)。
pub fn write_temp_dir(files: &[(&str, &str)]) -> (TempDir, Vec<std::path::PathBuf>) {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let mut paths = Vec::new();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let mut f = fs::File::create(&path).expect("创建文件失败");
        f.write_all(content.as_bytes()).expect("写入失败");
        f.flush().expect("flush 失败");
        paths.push(path);
    }
    (dir, paths)
}

/// 构造 args: `["pattern", "file_path"]`。
pub fn args(pattern: &str, path: &str) -> Vec<String> {
    vec![pattern.to_string(), path.to_string()]
}

/// 构造目录 args: `["pattern", "dir_path"]`。
pub fn args_dir(pattern: &str, dir_path: &str) -> Vec<String> {
    vec![pattern.to_string(), dir_path.to_string()]
}

/// 跑 `run_with_writer` 并返回输出字符串。
pub fn run(args: &[String]) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    match run_with_writer(args, RunOpts::default(), &mut buf) {
        Ok(()) => Ok(String::from_utf8(buf).expect("输出应为 UTF-8")),
        Err(e) => Err(format!("{e}")),
    }
}

/// 跑 `run_with_writer` 带自定义 opts。
pub fn run_with_opts(args: &[String], opts: RunOpts) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    match run_with_writer(args, opts, &mut buf) {
        Ok(()) => Ok(String::from_utf8(buf).expect("输出应为 UTF-8")),
        Err(e) => Err(format!("{e}")),
    }
}

/// 用 `dispatch_with_content` 模拟 stdin：传入预读的 FileContent，
/// `file_path` 设为 `<stdin>`，绕过实际 stdin 读取（测试中无法管道 stdin）。
pub fn run_stdin(content: &str, pattern: &str) -> Result<String, String> {
    run_stdin_with_opts(content, pattern, RunOpts::default())
}

/// 同 `run_stdin` 但带自定义 opts。
pub fn run_stdin_with_opts(content: &str, pattern: &str, opts: RunOpts) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    let fc = FileContent::Vec(content.as_bytes().to_vec());
    match dispatch_with_content(pattern, "<stdin>", opts, fc, &mut buf) {
        Ok(()) => Ok(String::from_utf8(buf).expect("输出应为 UTF-8")),
        Err(e) => Err(format!("{e}")),
    }
}

/// stdin 单参数（隐式）辅助。
pub fn run_stdin_implicit(content: &str, pattern: &str) -> Result<String, String> {
    run_stdin(content, pattern)
}

/// 跑 `run_with_writer` 带 glob/gitignore opts。
pub fn run_with_glob(
    args: &[String],
    glob: &[&str],
    respect_gitignore: bool,
) -> Result<String, String> {
    let opts = RunOpts {
        glob: glob.iter().map(|s| s.to_string()).collect(),
        respect_gitignore,
        ..Default::default()
    };
    run_with_opts(args, opts)
}
