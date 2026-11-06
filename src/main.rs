//! gxp 二进制入口。
//!
//! v1 骨架：解析 CLI、调度模式（BARE/gxp）、报四段式错误。
//! 实际查询执行见 `lib.rs` 各模块。

use clap::Parser;
use gxp::cli::Cli;
use gxp::GxpError;
use std::process;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = cli.run() {
        // 管道下游提前关闭（如 `gxp ... | head -1`）：stdout 写入返回
        // `ErrorKind::BrokenPipe`（Windows os error 109/232，POSIX EPIPE）。
        // 这是正常情况，不应作为错误退出（与 GNU grep / ripgrep 一致）。
        if let GxpError::Io(io_err) = &e {
            if io_err.kind() == std::io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
        }
        // 用 Display（user-facing）而非 Debug（开发者结构）输出错误。
        // 否则 Rust runtime 默认 Debug 打印会泄漏 `Error: Runtime { detail: "..." }`
        // 包装层，违反四段式报错模板（设计文档 §1.5）的"人类描述"原则。
        eprintln!("{e}");
        process::exit(1);
    }
}
