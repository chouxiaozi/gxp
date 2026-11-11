//! `--query-file` 端到端集成测试（v0.50，场景 §0.2-I）。
//!
//! 走 `Cli::parse_from` + `run_with_writer` 全链（clap 解析 + 尾随旗标提取 +
//! 查询文件读取前置 + detect_and_dispatch）——不能绕过 `Cli` 层直调
//! `run_with_writer(args, opts)`，否则 --query-file 解析路径未被覆盖
//!（§6.4 三路径 + §6.6 强制 e2e）。
//!
//! 覆盖：
//! - 三路径：单文件 / 目录递归 / stdin（`-` 与隐式）
//! - 完整性证明：查询文件含内层双引号 DSL，`--explain` 落 L2 字段谓词
//!   （引号完整到达解析器——shell 引号穿透问题的对账口径）
//! - fail-loud：互斥 / 空文件 / 多行 / 文件不存在 / 悬空 --query-file
//! - 双形态：前置 clap 形态 + 后置 `--query-file P` / `--query-file=P`
//! - 组合：`--verify` / `--explain` / 后置 bool flag 与查询文件共存

mod common;
use common::*;

use clap::Parser;
use gxp::cli::Cli;
use tempfile::NamedTempFile;

/// 全链跑一次：argv → clap → 尾随旗标 → 查询文件前置 → 执行。
fn run_cli(argv: &[&str]) -> Result<String, String> {
    let cli = Cli::parse_from(std::iter::once("gxp").chain(argv.iter().copied()));
    let mut buf: Vec<u8> = Vec::new();
    match cli.run_with_writer(&mut buf) {
        Ok(()) => Ok(String::from_utf8(buf).expect("输出应为 UTF-8")),
        Err(e) => Err(format!("{e}")),
    }
}

/// 真实子进程 + piped stdin（stdin 路径无法经 `run_with_writer` 注入）。
fn run_cli_stdin(argv: &[&str], stdin_data: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_gxp"))
        .args(argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("子进程 spawn 失败");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(stdin_data.as_bytes())
        .expect("写 stdin 失败");
    // stdin handle 已 take+drop → 关闭管道，EOF 生效
    let out = child.wait_with_output().expect("等待子进程失败");
    assert!(out.status.success(), "子进程应成功: {argv:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 写查询文件（单行 + 尾随换行，模拟 agent Write 工具落盘形态）。
fn write_query(query: &str) -> NamedTempFile {
    write_temp(&format!("{query}\n"))
}

const DATA: &str = "2026-11-11T10:00:00 level=ERROR cost_ms=900 msg=boom\n\
                    2026-11-11T10:00:01 level=INFO cost_ms=12 msg=ok\n\
                    2026-11-11T10:00:02 level=ERROR cost_ms=800 msg=logfmt\n\
                    2026-11-11T10:00:03 level=WARN cost_ms=45 msg=meh\n";

// ===== 三路径 =====

#[test]
fn test_query_file_single_file() {
    let q = write_query("cost_ms>500 | count");
    let f = write_temp(DATA);
    let out = run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        f.path().to_str().unwrap(),
    ])
    .expect("应成功");
    assert_eq!(out, "2\n"); // 900 + 800
}

#[test]
fn test_query_file_directory() {
    let q = write_query("cost_ms>500 | count");
    let (dir, _) = write_temp_dir(&[
        ("a.log", "cost_ms=900 x\nnope\n"),
        ("b.log", "cost_ms=600 y\nnope\n"),
        ("c.txt", "cost_ms=999 z\n"), // 非 .log 也会扫（目录递归无扩展名过滤）
    ]);
    let out = run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        dir.path().to_str().unwrap(),
    ])
    .expect("应成功");
    assert_eq!(out.trim(), "3");
}

#[test]
fn test_query_file_stdin_explicit_dash() {
    let q = write_query("cost_ms>500");
    let out = run_cli_stdin(&["--query-file", q.path().to_str().unwrap(), "-"], DATA);
    // 谓词命中 cost_ms=900 / 800 两行（独立期望集：数据已知）
    assert_eq!(
        out,
        "1:2026-11-11T10:00:00 level=ERROR cost_ms=900 msg=boom\n3:2026-11-11T10:00:02 level=ERROR cost_ms=800 msg=logfmt\n"
    );
}

#[test]
fn test_query_file_stdin_implicit_no_path() {
    let q = write_query("cost_ms>500");
    let out = run_cli_stdin(&["--query-file", q.path().to_str().unwrap()], DATA);
    assert_eq!(
        out,
        "1:2026-11-11T10:00:00 level=ERROR cost_ms=900 msg=boom\n3:2026-11-11T10:00:02 level=ERROR cost_ms=800 msg=logfmt\n"
    );
}

// ===== 完整性证明：内层双引号 DSL 原样到达解析器 =====

#[test]
fn test_query_file_inner_quotes_intact_via_explain() {
    // 查询含内层双引号（shell 引号穿透的高危形态）；
    // --explain 应落 L2 字段谓词 level in [ERROR]——证明引号完整进入解析器
    let q = write_query("level=\"ERROR\" | count");
    let f = write_temp(DATA);
    let out = run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        f.path().to_str().unwrap(),
        "--explain",
    ])
    .expect("应成功");
    assert!(out.contains("L2 字段谓词"), "查询未按 DSL 解析: {out}");
    assert!(
        out.contains("level in [ERROR]"),
        "引号丢失或谓词未识别: {out}"
    );
}

#[test]
fn test_query_file_bare_mode_still_works() {
    // 查询文件内容是无 gxp 特征的 ERE → BARE 模式（模式判定不受 --query-file 影响）
    let q = write_query("ERROR");
    let f = write_temp(DATA);
    let out = run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        f.path().to_str().unwrap(),
    ])
    .expect("应成功");
    assert_eq!(out, "1:2026-11-11T10:00:00 level=ERROR cost_ms=900 msg=boom\n3:2026-11-11T10:00:02 level=ERROR cost_ms=800 msg=logfmt\n");
}

// ===== 双形态（后置 trailing flags）=====

#[test]
fn test_query_file_trailing_two_arg_form() {
    // `gxp app.log --query-file q`（trailing_var_arg 吞入 args，由 extract 提取）
    let q = write_query("cost_ms>500 | count");
    let f = write_temp(DATA);
    let out = run_cli(&[
        f.path().to_str().unwrap(),
        "--query-file",
        q.path().to_str().unwrap(),
    ])
    .expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_query_file_trailing_eq_form() {
    let q = write_query("cost_ms>500 | count");
    let f = write_temp(DATA);
    let out = run_cli(&[
        f.path().to_str().unwrap(),
        &format!("--query-file={}", q.path().to_str().unwrap()),
    ])
    .expect("应成功");
    assert_eq!(out, "2\n");
}

#[test]
fn test_query_file_trailing_with_bool_flag() {
    // 后置 bool flag 与 --query-file 共存：`gxp f --verify --query-file q`
    // --verify 输出 `PASS [N 行命中]`（mode/verify.rs）
    let q = write_query("cost_ms>500");
    let f = write_temp(DATA);
    let out = run_cli(&[
        f.path().to_str().unwrap(),
        "--verify",
        "--query-file",
        q.path().to_str().unwrap(),
    ])
    .expect("应成功");
    assert!(out.contains("PASS"), "--verify 应 PASS: {out}");
    assert!(out.contains("2 行命中"), "应命中 2 行: {out}");
}

// ===== fail-loud 契约 =====

#[test]
fn test_query_file_mutual_exclusion_with_positional_pattern() {
    // --query-file + 位置 pattern + 路径 → 互斥错误（非多文件错误）
    let q = write_query("ERROR");
    let f = write_temp(DATA);
    let err = match run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        "ERROR",
        f.path().to_str().unwrap(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("互斥"), "得到: {err}");
}

#[test]
fn test_query_file_empty_rejected() {
    let q = write_temp("   \n");
    let f = write_temp(DATA);
    let err = match run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        f.path().to_str().unwrap(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("内容为空"), "得到: {err}");
}

#[test]
fn test_query_file_multiline_rejected() {
    let q = write_temp("ERROR\ncost_ms>500\n");
    let f = write_temp(DATA);
    let err = match run_cli(&[
        "--query-file",
        q.path().to_str().unwrap(),
        f.path().to_str().unwrap(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("单查询单行"), "得到: {err}");
}

#[test]
fn test_query_file_missing_path_rejected() {
    let f = write_temp(DATA);
    let err = match run_cli(&[
        "--query-file",
        "Z:/no/such/query.gxp",
        f.path().to_str().unwrap(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("读取 --query-file"), "得到: {err}");
    assert!(err.contains("失败"), "得到: {err}");
}

#[test]
fn test_query_file_dangling_flag_rejected() {
    // `--query-file` 后无值 → fail-loud（不静默吞）
    let f = write_temp(DATA);
    let err = match run_cli(&[f.path().to_str().unwrap(), "--query-file"]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("--query-file 需要一个文件路径"), "得到: {err}");
}
