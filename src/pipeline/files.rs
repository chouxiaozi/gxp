//! `files` / `files --invert`：列出含/不含命中行的文件
//! （等价 `-l`/`-L`，设计文档 §2.4 示例 9）。
//!
//! 语义（对齐 Rust 方案 §7.6 骨架）：
//! - `files`：有命中行 → 输出文件名（等价 `grep -l`）
//! - `files --invert`：无命中行 → 输出文件名（等价 `grep -L`）
//!
//! v1 为单文件模式（`gxp 'PATTERN' file`），多文件 glob 由后续版本接入 `dir/`。

use std::io::Write;

use super::MatchedLine;
use crate::error::Result;

/// 执行 `files` 管道：根据命中情况输出文件名。
///
/// - `invert=false`：有命中（`matched` 非空）→ 输出文件名
/// - `invert=true`：无命中（`matched` 为空）→ 输出文件名
pub fn apply_files<W: Write>(
    matched: &[MatchedLine],
    invert: bool,
    file_path: &str,
    out: &mut W,
) -> Result<()> {
    let has_hit = !matched.is_empty();
    let emit = if invert { !has_hit } else { has_hit };
    if emit {
        writeln!(out, "{file_path}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MatchedLine;

    fn run(matched: &[MatchedLine], invert: bool) -> String {
        let mut buf = Vec::new();
        apply_files(matched, invert, "app.log", &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_files_outputs_on_hit() {
        let m = vec![MatchedLine::new(1, b"hit")];
        assert_eq!(run(&m, false), "app.log\n");
    }

    #[test]
    fn test_files_silent_when_no_hit() {
        assert_eq!(run(&[], false), "");
    }

    #[test]
    fn test_files_invert_outputs_on_no_hit() {
        assert_eq!(run(&[], true), "app.log\n");
    }

    #[test]
    fn test_files_invert_silent_when_hit() {
        let m = vec![MatchedLine::new(1, b"hit")];
        assert_eq!(run(&m, true), "");
    }
}
