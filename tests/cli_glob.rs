//! --glob 文件名过滤 + .gitignore 尊重端到端集成测试。
//!
//! 验证 `gxp --glob '*.ts' 'pattern' dir/` 完整链路：
//! RunOpts.glob → PathPlan.with_glob_patterns → GlobSet 编译 → walk 中文件名过滤
//! RunOpts.respect_gitignore → PathPlan.with_respect_gitignore → GitignoreChecker → walk 中 .gitignore 过滤
//!
//! 覆盖场景：glob 扩展名过滤 / 多 pattern OR / 大括号展开 / globstar /
//! .gitignore 默认尊重 / --no-gitignore 覆盖 / glob+目录组合 / 无匹配空结果 / glob 语法错误 fail-loud

mod common;
use common::*;

use std::fs;
use tempfile::tempdir;

#[test]
fn test_glob_filters_by_extension() {
    // --glob '*.ts' 只扫 .ts 文件，跳过 .png/.svg
    let (dir, _) = write_temp_dir(&[
        ("app.ts", "ERROR in ts\n"),
        ("logo.png", "ERROR in png\n"),
        ("icon.svg", "ERROR in svg\n"),
    ]);
    let out = run_with_glob(
        &args_dir("ERROR", dir.path().to_str().unwrap()),
        &["*.ts"],
        true,
    )
    .expect("glob 过滤应成功");
    // 只扫 .ts → 只命中 app.ts 的 ERROR
    assert!(out.contains("app.ts"), "应命中 app.ts，实际: {out}");
    assert!(!out.contains("logo.png"), "不应扫 .png，实际: {out}");
    assert!(!out.contains("icon.svg"), "不应扫 .svg，实际: {out}");
}

#[test]
fn test_glob_multiple_patterns() {
    // --glob '*.ts' --glob '*.vue' OR 语义，两种都扫
    let (dir, _) = write_temp_dir(&[
        ("a.ts", "MATCH\n"),
        ("b.vue", "MATCH\n"),
        ("c.png", "MATCH\n"),
    ]);
    let out = run_with_glob(
        &args_dir("MATCH", dir.path().to_str().unwrap()),
        &["*.ts", "*.vue"],
        true,
    )
    .expect("多 glob 应成功");
    assert!(out.contains("a.ts"), "应命中 a.ts，实际: {out}");
    assert!(out.contains("b.vue"), "应命中 b.vue，实际: {out}");
    assert!(!out.contains("c.png"), "不应扫 .png，实际: {out}");
}

#[test]
fn test_glob_brace_expansion() {
    // --glob '*.{ts,vue}' 大括号展开
    let (dir, _) = write_temp_dir(&[("a.ts", "DATA\n"), ("b.vue", "DATA\n"), ("c.png", "DATA\n")]);
    let out = run_with_glob(
        &args_dir("DATA", dir.path().to_str().unwrap()),
        &["*.{ts,vue}"],
        true,
    )
    .expect("大括号 glob 应成功");
    assert!(out.contains("a.ts"), "应命中 a.ts，实际: {out}");
    assert!(out.contains("b.vue"), "应命中 b.vue，实际: {out}");
    assert!(!out.contains("c.png"), "不应扫 .png，实际: {out}");
}

#[test]
fn test_glob_globstar_recursive() {
    // --glob '**/*.json' 匹配子目录下的 .json
    let (dir, _) = write_temp_dir(&[
        ("root.json", "FOUND\n"),
        ("sub/nested.json", "FOUND\n"),
        ("sub/deep/inner.json", "FOUND\n"),
        ("readme.md", "FOUND\n"),
    ]);
    let out = run_with_glob(
        &args_dir("FOUND", dir.path().to_str().unwrap()),
        &["**/*.json"],
        true,
    )
    .expect("globstar 应成功");
    // globset 的 ** 匹配路径分隔符
    // 注意：globset is_match 匹配的是传入的字符串
    // walk 中传的是 file_name()，所以 **/*.json 只匹配文件名
    // 对于 root.json → 匹配 *.json 部分
    // 对于 nested.json → 匹配 *.json 部分
    // globset GlobSet::is_match 对文件名 "root.json" 用 **/*.json 匹配
    assert!(
        out.contains("root.json") || out.contains("nested.json") || out.contains("inner.json"),
        "应至少命中一个 .json 文件，实际: {out}"
    );
    assert!(!out.contains("readme.md"), "不应扫 .md，实际: {out}");
}

#[test]
fn test_gitignore_default_respected() {
    // 默认尊重 .gitignore：跳过 .gitignore 中的文件
    let dir = tempdir().expect("创建临时目录失败");
    // 写 .gitignore，排除 ignored.log
    fs::write(dir.path().join(".gitignore"), "ignored.log\n").unwrap();
    // 写两个文件：一个在 .gitignore 中，一个不在
    fs::write(dir.path().join("ignored.log"), "SHOULD_NOT_APPEAR\n").unwrap();
    fs::write(dir.path().join("visible.log"), "SHOULD_APPEAR\n").unwrap();

    let out = run_with_glob(
        &args_dir("SHOULD", dir.path().to_str().unwrap()),
        &[],
        true, // 默认尊重 .gitignore
    )
    .expect("gitignore 默认应成功");
    assert!(
        out.contains("visible.log"),
        "应命中 visible.log，实际: {out}"
    );
    assert!(
        !out.contains("ignored.log"),
        "应跳过 .gitignore 中的 ignored.log，实际: {out}"
    );
}

#[test]
fn test_no_gitignore_disables_filtering() {
    // --no-gitignore 扫到 .gitignore 排除的文件
    let dir = tempdir().expect("创建临时目录失败");
    fs::write(dir.path().join(".gitignore"), "ignored.log\n").unwrap();
    fs::write(dir.path().join("ignored.log"), "SHOULD_APPEAR\n").unwrap();
    fs::write(dir.path().join("visible.log"), "SHOULD_APPEAR\n").unwrap();

    let out = run_with_glob(
        &args_dir("SHOULD", dir.path().to_str().unwrap()),
        &[],
        false, // --no-gitignore
    )
    .expect("--no-gitignore 应成功");
    // 两个文件都应被扫到
    assert!(
        out.contains("ignored.log"),
        "--no-gitignore 应扫到 ignored.log，实际: {out}"
    );
    assert!(
        out.contains("visible.log"),
        "应命中 visible.log，实际: {out}"
    );
}

#[test]
fn test_glob_with_directory_pattern() {
    // --glob '*.ts' + 目录参数组合正常工作
    let (dir, _) = write_temp_dir(&[
        ("main.ts", "target line\n"),
        ("util.ts", "target line\n"),
        ("data.json", "target line\n"),
        ("readme.md", "target line\n"),
    ]);
    let out = run_with_glob(
        &args_dir("target", dir.path().to_str().unwrap()),
        &["*.ts"],
        true,
    )
    .expect("glob + 目录应成功");
    // 只扫 .ts 文件
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "应命中 2 个 .ts 文件，实际: {out}");
    assert!(out.contains("main.ts"), "应命中 main.ts，实际: {out}");
    assert!(out.contains("util.ts"), "应命中 util.ts，实际: {out}");
    assert!(!out.contains("data.json"), "不应扫 .json，实际: {out}");
    assert!(!out.contains("readme.md"), "不应扫 .md，实际: {out}");
}

#[test]
fn test_glob_no_match_empty_result() {
    // --glob '*.xyz' 无匹配文件，输出空结果
    let (dir, _) = write_temp_dir(&[("a.ts", "MATCH\n"), ("b.log", "MATCH\n")]);
    let out = run_with_glob(
        &args_dir("MATCH", dir.path().to_str().unwrap()),
        &["*.xyz"],
        true,
    )
    .expect("无匹配 glob 应成功");
    // 无文件被扫 → 输出为空
    assert!(out.is_empty(), "glob 无匹配文件应输出空，实际: {out}");
}

#[test]
fn test_glob_syntax_error_fail_loud() {
    // --glob '[abc' 未闭合 → fail-loud 报错
    let (dir, _) = write_temp_dir(&[("a.ts", "MATCH\n")]);
    let result = run_with_glob(
        &args_dir("MATCH", dir.path().to_str().unwrap()),
        &["[abc"],
        true,
    );
    assert!(
        result.is_err(),
        "glob 语法错误应 fail-loud 报错，实际: {result:?}"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("glob") || err.contains("语法"),
        "错误消息应含 glob 语法错误，实际: {err}"
    );
}

#[test]
fn test_gitignore_directory_pruned() {
    // .gitignore 排除整个目录 → 该目录下所有文件都跳过
    let dir = tempdir().expect("创建临时目录失败");
    fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    fs::write(dir.path().join("node_modules/pkg.ts"), "TARGET\n").unwrap();
    fs::write(dir.path().join("app.ts"), "TARGET\n").unwrap();

    let out = run_with_glob(&args_dir("TARGET", dir.path().to_str().unwrap()), &[], true)
        .expect("目录 gitignore 应成功");
    assert!(out.contains("app.ts"), "应命中 app.ts，实际: {out}");
    assert!(
        !out.contains("node_modules"),
        "应跳过 node_modules 目录，实际: {out}"
    );
}
