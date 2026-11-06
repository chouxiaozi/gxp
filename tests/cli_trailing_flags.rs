//! 后置 flag 解析测试（`--verify`/`--strict`/`--glob` 等写在位置参数之后）。
//!
//! `trailing_var_arg = true` + `allow_hyphen_values = true` 导致 clap 把
//! `gxp 'pattern' file --verify` 中的 `--verify` 吞进 `args`。
//! `Cli::run()` 中 `extract_trailing_flags` 手动提取后置 long flag 以修复此问题。
//!
//! 这些测试通过 `Cli::parse_from` + `Cli::run_with_writer` 走完整 clap 解析路径，
//! 验证后置 flag 被正确提取并生效。

mod common;
use common::*;

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

// ===== --verify 后置 =====

#[test]
fn test_trailing_verify_after_file() {
    // gxp '"error"' file --verify → 应触发 verify 模式
    let f = write_temp("error line\ninfo line\nerror again\n");
    let out = run_clap(&[r#""error""#, f.path().to_str().unwrap(), "--verify"])
        .expect("--verify 后置应成功");
    assert!(
        out.contains("verify: PASS"),
        "后置 --verify 应触发 verify 模式，实际: {out}"
    );
}

#[test]
fn test_leading_and_trailing_verify_equivalent() {
    // gxp --verify '"error"' file ≡ gxp '"error"' file --verify
    let f = write_temp("error line\ninfo line\n");
    let leading = run_clap(&["--verify", r#""error""#, f.path().to_str().unwrap()])
        .expect("前置 --verify 应成功");
    let trailing = run_clap(&[r#""error""#, f.path().to_str().unwrap(), "--verify"])
        .expect("后置 --verify 应成功");
    assert_eq!(leading, trailing, "前置和后置 --verify 输出应一致");
}

// ===== --strict 后置 =====

#[test]
fn test_trailing_strict_missing_field_fails() {
    // gxp '"error" && bogus>800' file --strict → 应触发严格模式报错
    let f = write_temp("error line cost_ms=100\n");
    let result = run_clap(&[
        r#""error" && bogus>800"#,
        f.path().to_str().unwrap(),
        "--strict",
    ]);
    let Err(msg) = result else {
        panic!("后置 --strict 应对缺失字段报错");
    };
    assert!(
        msg.contains("bogus") && msg.contains("从未出现"),
        "应报缺失字段 bogus，实际: {msg}"
    );
}

#[test]
fn test_trailing_strict_existing_field_passes() {
    // gxp '"error" && cost_ms>800' file --strict → 字段存在，不报错
    let f = write_temp("error line cost_ms=900\n");
    let out = run_clap(&[
        r#""error" && cost_ms>800"#,
        f.path().to_str().unwrap(),
        "--strict",
    ])
    .expect("--strict 后置且字段存在应成功");
    assert!(out.contains("error line"), "应有命中行，实际: {out}");
}

// ===== --glob 后置 =====

#[test]
fn test_trailing_glob_filters_directory() {
    // gxp '"error"' dir/ --glob '*.log' → 只扫 .log 文件
    let (dir, _) = write_temp_dir(&[("a.log", "error in log\n"), ("b.rs", "error in rust\n")]);
    let out = run_clap(&[
        r#""error""#,
        dir.path().to_str().unwrap(),
        "--glob",
        "*.log",
    ])
    .expect("--glob 后置应成功");
    assert!(
        out.contains("error in log") && !out.contains("error in rust"),
        "应只扫 .log 文件，实际: {out}"
    );
}

// ===== --no-gitignore 后置 =====

#[test]
fn test_trailing_no_gitignore_runs() {
    // gxp '"error"' file --no-gitignore → 单文件无效果但不应报错
    let f = write_temp("error line\n");
    let out = run_clap(&[r#""error""#, f.path().to_str().unwrap(), "--no-gitignore"])
        .expect("--no-gitignore 后置应成功");
    assert!(out.contains("error line"), "应有命中行，实际: {out}");
}

// ===== 多后置 flag 组合 =====

#[test]
fn test_trailing_verify_and_strict_combined() {
    // gxp '"error"' file --strict --verify → 两个后置 flag 都应被提取
    let f = write_temp("error line cost_ms=900\n");
    let out = run_clap(&[
        r#""error" && cost_ms>800"#,
        f.path().to_str().unwrap(),
        "--strict",
        "--verify",
    ])
    .expect("后置 --strict --verify 应成功");
    assert!(
        out.contains("verify: PASS"),
        "后置 --verify 应生效，实际: {out}"
    );
}

#[test]
fn test_trailing_glob_equals_form() {
    // gxp '"error"' dir/ --glob=*.log → --glob=PATTERN 形式
    let (dir, _) = write_temp_dir(&[("a.log", "error in log\n"), ("b.rs", "error in rust\n")]);
    let out = run_clap(&[r#""error""#, dir.path().to_str().unwrap(), "--glob=*.log"])
        .expect("--glob= 后置应成功");
    assert!(
        out.contains("error in log") && !out.contains("error in rust"),
        "应只扫 .log 文件（--glob= 形式），实际: {out}"
    );
}

// ===== grep 短选项仍被四段式报错拦截 =====

#[test]
fn test_grep_short_option_still_rejected() {
    // gxp -A 3 'pattern' file → 四段式报错（clap 的 allow_hyphen_values 让 -A 进入 args）
    let f = write_temp("error line\n");
    let result = run_clap(&["-A", "3", r#""error""#, f.path().to_str().unwrap()]);
    let Err(msg) = result else {
        panic!("-A 应被拒绝");
    };
    assert!(
        msg.contains("InvalidGrepOption") || msg.contains("grep -A"),
        "应四段式报错 grep -A，实际: {msg}"
    );
}

// ===== stdin - 仍可用 =====

#[test]
fn test_stdin_marker_still_works() {
    // gxp '"test"' - → stdin 标记 - 不被 flag 提取逻辑吞
    use gxp::cli::RunOpts;
    use gxp::io::file::FileContent;
    use gxp::mode::detect::dispatch_with_content;
    let mut buf: Vec<u8> = Vec::new();
    let fc = FileContent::Vec(b"test line\n".to_vec());
    dispatch_with_content(r#""test""#, "<stdin>", RunOpts::default(), fc, &mut buf)
        .expect("stdin 应成功");
    let out = String::from_utf8(buf).expect("UTF-8");
    assert!(out.contains("test line"), "stdin 应有命中，实际: {out}");
}
