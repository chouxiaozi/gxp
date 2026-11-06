//! shell completion 生成测试（`--generate-completion <SHELL>`）。
//!
//! 验证 `--generate-completion` 能正确生成各 shell 的补全脚本，
//! 并对不支持的 shell 名称报友好错误。

use clap::Parser;
use gxp::cli::Cli;

/// 通过 clap parse + run 执行，捕获 stdout。
fn run_clap(argv: &[&str]) -> Result<String, String> {
    let mut argv_full = vec!["gxp"];
    argv_full.extend_from_slice(argv);
    let cli = Cli::parse_from(argv_full);
    let mut buf: Vec<u8> = Vec::new();
    match cli.run_with_writer(&mut buf) {
        Ok(()) => Ok(String::from_utf8(buf).expect("输出应为 UTF-8")),
        Err(e) => Err(format!("{e}")),
    }
}

#[test]
fn test_generate_completion_bash_produces_function() {
    let out = run_clap(&["--generate-completion", "bash"]).unwrap();
    assert!(
        out.contains("_gxp") && out.contains("COMPREPLY"),
        "bash completion should define _gxp function and COMPREPLY, got: {}",
        &out[..80.min(out.len())]
    );
}

#[test]
fn test_generate_completion_zsh_produces_compdef() {
    let out = run_clap(&["--generate-completion", "zsh"]).unwrap();
    assert!(
        out.contains("#compdef gxp"),
        "zsh completion should start with #compdef, got: {}",
        &out[..80.min(out.len())]
    );
}

#[test]
fn test_generate_completion_powershell_produces_register() {
    let out = run_clap(&["--generate-completion", "powershell"]).unwrap();
    assert!(
        out.contains("Register-ArgumentCompleter"),
        "powershell completion should contain Register-ArgumentCompleter, got: {}",
        &out[..80.min(out.len())]
    );
}

#[test]
fn test_generate_completion_invalid_shell_reports_error() {
    let Err(e) = run_clap(&["--generate-completion", "tcsh"]) else {
        panic!("invalid shell should error");
    };
    assert!(
        e.contains("unsupported shell 'tcsh'"),
        "error should mention unsupported shell, got: {e}"
    );
    assert!(
        e.contains("bash") && e.contains("zsh") && e.contains("fish"),
        "error should list supported shells, got: {e}"
    );
}
