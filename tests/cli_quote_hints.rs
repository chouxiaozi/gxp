//! 引号丢失提示四挂点 e2e（v0.50，对抗评审 P2/P3）。
//!
//! 背景（实测矩阵见 changelog v0.50）：shell 剥掉/污染查询内层双引号后，
//! 查询与合法字面量**逐字节相同**，gxp 无法在 argv 层检测——只能在
//! （a）已 fail-loud 的错误上附加根因提示；（b）`--explain` 计划上给自检行。
//!
//! 四挂点：
//! 1. 多文件参数错（detect.rs）——查询碎片形态 → shell 剥引号根因提示
//! 2. ast() 语法错（parser.rs）——输入无任何双引号 → 同上
//! 3. BARE --explain/--verify 拒绝（detect.rs）——`level=ERROR` 形态 → 修复形态
//! 4. `--explain` 渲染（plan.rs）——bare 字面量含 `=` → 自检提示行

mod common;
use common::*;
use gxp::cli::RunOpts;

// ===== 挂点 1：多文件参数错 × shell 碎裂启发式 =====

#[test]
fn test_multi_file_error_shell_split_hint_present() {
    // PS 5.1 剥掉 ast() 引号后按空格碎裂的典型 argv 形态
    let f = write_temp("fn main() {}\n");
    let err = match run(&[
        "ast(rs,".to_string(),
        "fn".to_string(),
        "$F()\"".to_string(),
        f.path().to_str().unwrap().to_string(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("多文件参数暂不支持"), "得到: {err}");
    assert!(err.contains("疑似被 shell 剥掉引号"), "应含根因提示: {err}");
    assert!(err.contains("--query-file"), "应含出路③: {err}");
}

#[test]
fn test_multi_file_error_normal_paths_no_hint() {
    // 正常多文件路径参数：不触发 shell 提示（假阳性面收敛）
    let f1 = write_temp("a\n");
    let f2 = write_temp("b\n");
    let err = match run(&[
        "ERROR".to_string(),
        f1.path().to_str().unwrap().to_string(),
        f2.path().to_str().unwrap().to_string(),
    ]) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("多文件参数暂不支持"), "得到: {err}");
    assert!(!err.contains("疑似被 shell 剥掉引号"), "不应误报: {err}");
}

// ===== 挂点 2：ast() 语法错 × 无引号检测 =====

#[test]
fn test_ast_error_quote_loss_hint_present() {
    // `ast(rs, $OBJ.unwrap())`：PATTERN 双引号被 shell 剥掉后的形态
    let f = write_temp("fn main() {}\n");
    let err = match run(&args("ast(rs, $OBJ.unwrap())", f.path().to_str().unwrap())) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("ast() 语法错误"), "得到: {err}");
    assert!(err.contains("输入不含双引号"), "应含引号丢失提示: {err}");
    assert!(err.contains("shell 可能剥掉了"), "得到: {err}");
}

#[test]
fn test_ast_error_with_quotes_no_shell_hint() {
    // 引号完好但 DSL 结构错（STRING 后非 `)`）：报 ast() 语法错误，
    // 但根因不是 shell 剥引号 → 不附加该提示（hint 条件 `!input.contains('"')`）
    let f = write_temp("fn main() {}\n");
    let err = match run(&args("ast(ts, \"a\" \"b\")", f.path().to_str().unwrap())) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("ast() 语法错误"), "得到: {err}");
    assert!(
        !err.contains("输入不含双引号"),
        "不应误报 shell 根因: {err}"
    );
}

// ===== 挂点 3：BARE --explain 拒绝 × `level=ERROR` 形态 =====

#[test]
fn test_bare_explain_reject_quote_hint() {
    // `level=ERROR`（引号被剥后的典型残留）+ --explain → 修复形态提示
    let f = write_temp("level=ERROR x\n");
    let err = match run_with_opts(
        &args("level=ERROR", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..RunOpts::default()
        },
    ) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("仅在 gxp 模式可用"), "得到: {err}");
    assert!(err.contains("引号自检"), "应含自检提示: {err}");
    assert!(err.contains("level=\"ERROR\""), "应含修复形态: {err}");
}

#[test]
fn test_bare_explain_reject_plain_no_hint() {
    // 普通无 = 的 BARE pattern：不附加自检
    let f = write_temp("ERROR x\n");
    let err = match run_with_opts(
        &args("ERROR", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..RunOpts::default()
        },
    ) {
        Ok(_) => panic!("应 fail-loud 报错"),
        Err(e) => e,
    };
    assert!(err.contains("仅在 gxp 模式可用"), "得到: {err}");
    assert!(!err.contains("引号自检"), "不应误报: {err}");
}

// ===== 挂点 4：--explain 渲染 × bare 含 '=' 自检行 =====

#[test]
fn test_explain_bare_eq_advisory_line() {
    // `level=ERROR | count` 静默解析为字面量查询——explain 上给出自检提示
    let f = write_temp("level=ERROR x\n");
    let out = run_with_opts(
        &args("level=ERROR | count", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..RunOpts::default()
        },
    )
    .expect("应成功");
    assert!(out.contains("bare 字面量含 '='"), "应含自检行: {out}");
    assert!(out.contains("level=\"ERROR\""), "应含修复形态: {out}");
}

#[test]
fn test_explain_predicate_query_no_advisory() {
    // 正常谓词查询：无自检行（对照组）
    let f = write_temp("cost_ms=800 x\n");
    let out = run_with_opts(
        &args("cost_ms>500 | count", f.path().to_str().unwrap()),
        RunOpts {
            explain: true,
            ..RunOpts::default()
        },
    )
    .expect("应成功");
    assert!(!out.contains("bare 字面量含"), "不应误报: {out}");
    assert!(out.contains("cost_ms > 500"), "谓词应正常落 L2: {out}");
}
