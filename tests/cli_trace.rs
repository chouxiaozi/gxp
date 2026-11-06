//! `trace` 动词端到端集成测试（§9.1 候选 #2）。
//!
//! 验证 `trace` 管道动词：6 语言 stack trace 多行捕获 + 自动检测 +
//! JSON extra_lines 字段 + 目录场景 + stdin + fail-loud 规则。
//!
//! 组合矩阵（§6.4）：trace × {单文件, 目录, stdin} × {自动检测, 指定语言} × {文本, JSON}。

mod common;
use common::*;

use gxp::cli::RunOpts;

// ===== 单文件场景：6 语言 stack trace =====

#[test]
fn test_trace_java_at_lines() {
    // gxp 'Exception | trace java' file：java stack trace 含 `at` 帧
    let f = write_temp(
        "Exception in thread \"main\" java.lang.NullPointerException: msg\n\
         \tat com.example.Main.main(Main.java:10)\n\
         \tat com.example.MyClass.method(MyClass.java:42)\n\
         next non-trace line\n",
    );
    let out = run(&args(
        r#""Exception" | trace java"#,
        f.path().to_str().unwrap(),
    ))
    .expect("trace java 应成功");
    // 主行 + 2 个续行（`  → ` 前缀视觉区分）
    assert!(out.contains("1:Exception in thread"));
    assert!(out.contains("  → \tat com.example.Main.main(Main.java:10)"));
    assert!(out.contains("  → \tat com.example.MyClass.method(MyClass.java:42)"));
    // 非续行不应被收
    assert!(!out.contains("next non-trace line"));
}

#[test]
fn test_trace_python_file_lines() {
    // gxp 'Traceback | trace python' file：python traceback 含 `  File "..."` 帧
    // 真实 python traceback 形态：
    //   - `  File "..."` 2 空格缩进
    //   - `    code` 4 空格缩进的源代码行
    let f = write_temp(
        "Traceback (most recent call last):\n  File \"app.py\", line 10, in <module>\n    main()\n  File \"app.py\", line 5, in main\n    raise ValueError()\nnext non-trace line\n",
    );
    let out = run(&args(
        r#""Traceback" | trace python"#,
        f.path().to_str().unwrap(),
    ))
    .expect("trace python 应成功");
    assert!(out.contains("1:Traceback"));
    // 应收集 4 行续行（File + main() + File + raise）
    let arrow_count = out.matches("  → ").count();
    assert_eq!(arrow_count, 4, "应收集 4 行续行: {out}");
    assert!(!out.contains("next non-trace line"));
}

#[test]
fn test_trace_rust_panicked_continuation() {
    // gxp 'panic | trace rust' file：rust panic 含 note/stack backtrace/N: 帧
    let f = write_temp(
        "thread 'main' panicked at 'x', src/main.rs:42:3\n\
         note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\
         stack backtrace:\n\
         \t   0: std::panicking::begin_panic\n\
         \t   1: main::main\n\
         next non-trace line\n",
    );
    let out = run(&args(r#""panic" | trace rust"#, f.path().to_str().unwrap()))
        .expect("trace rust 应成功");
    assert!(out.contains("panicked at 'x'"));
    let arrow_count = out.matches("  → ").count();
    // 首命中行 4 个续行（note/stack backtrace/0:/1:），命中行 4 也含 `panic`，
    // 后续 1 个续行（main::main）——共 5 个箭头
    assert!(arrow_count >= 4, "应至少 4 个续行: {out}");
    assert!(!out.contains("next non-trace line"));
}

#[test]
fn test_trace_go_goroutine_frame() {
    // gxp 'panic | trace go' file：go panic 含 goroutine [running]: + .go:N 帧
    let f = write_temp(
        "panic: runtime error: index out of range\n\
         goroutine 1 [running]:\n\
         main.main()\n\
         \t/usr/local/go/src/x.go:5 +0x40\n\
         next non-trace line\n",
    );
    let out =
        run(&args(r#""panic" | trace go"#, f.path().to_str().unwrap())).expect("trace go 应成功");
    assert!(out.contains("1:panic: runtime error"));
    let arrow_count = out.matches("  → ").count();
    assert_eq!(arrow_count, 3, "应收集 3 行续行: {out}");
    assert!(!out.contains("next non-trace line"));
}

#[test]
fn test_trace_js_at_lines() {
    // gxp 'Error | trace js' file：js v8 stack trace 含 `at` 帧
    let f = write_temp("Error: something failed\n    at Object.<anonymous> (/path/file.js:10:5)\n    at Module._compile (module.js:5:1)\nnext non-trace line\n");
    let out =
        run(&args(r#""Error" | trace js"#, f.path().to_str().unwrap())).expect("trace js 应成功");
    assert!(out.contains("1:Error: something failed"));
    let arrow_count = out.matches("  → ").count();
    assert_eq!(arrow_count, 2, "应收集 2 行续行: {out}");
}

#[test]
fn test_trace_csharp_at_lines() {
    // gxp 'Exception | trace csharp' file：c# stack trace 含 `at` 帧
    let f = write_temp("System.NullReferenceException: Object reference not set\n   at com.example.MyClass.Method() in /path/File.cs:line 42\n   at com.example.Main.Main() in /path/Main.cs:line 10\nnext non-trace line\n");
    let out = run(&args(
        r#""NullReferenceException" | trace csharp"#,
        f.path().to_str().unwrap(),
    ))
    .expect("trace csharp 应成功");
    assert!(out.contains("1:System.NullReferenceException"));
    let arrow_count = out.matches("  → ").count();
    assert_eq!(arrow_count, 2, "应收集 2 行续行: {out}");
}

// ===== 自动检测 =====

#[test]
fn test_trace_auto_detect_java() {
    // 不指定 lang，按首行特征自动检测
    let f = write_temp(
        "Exception in thread \"main\" java.lang.NullPointerException: msg\n\
         \tat com.example.Main.main(Main.java:10)\n\
         next non-trace line\n",
    );
    let out = run(&args(r#""Exception" | trace"#, f.path().to_str().unwrap()))
        .expect("trace 自动检测应成功");
    assert!(out.contains("  → \tat com.example.Main.main"));
}

#[test]
fn test_trace_auto_detect_python() {
    let f = write_temp(
        "Traceback (most recent call last):\n  File \"app.py\", line 10, in <module>\nnext non-trace line\n",
    );
    let out = run(&args(r#""Traceback" | trace"#, f.path().to_str().unwrap()))
        .expect("trace 自动检测应成功");
    assert!(out.contains("  →   File \"app.py\""));
}

// ===== JSON 输出 =====

#[test]
fn test_trace_json_includes_extra_lines() {
    let f = write_temp(
        "Exception in thread \"main\" java.lang.NullPointerException: msg\n\
         \tat com.example.Main.main(Main.java:10)\n\
         next non-trace line\n",
    );
    let opts = RunOpts {
        json: true,
        ..Default::default()
    };
    let args = vec![
        r#""Exception" | trace java"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, opts).expect("JSON trace 应成功");
    // JSON 应含 extra_lines 数组字段
    assert!(
        out.contains("\"extra_lines\""),
        "JSON 应含 extra_lines: {out}"
    );
    assert!(
        out.contains("at com.example.Main.main"),
        "extra_lines 应含 at 行: {out}"
    );
}

#[test]
fn test_trace_json_omits_extra_lines_when_empty() {
    // 无 stack trace 续行时，JSON 不应含 extra_lines 字段
    let f = write_temp("panic: just a log line\n");
    let opts = RunOpts {
        json: true,
        ..Default::default()
    };
    let args = vec![
        r#""panic" | trace"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, opts).expect("JSON trace 应成功");
    assert!(
        !out.contains("\"extra_lines\""),
        "无续行时 JSON 不应含 extra_lines: {out}"
    );
}

// ===== 目录场景：跨文件不连续 =====

#[test]
fn test_trace_directory_each_file_independent() {
    // 目录模式：trace 每文件独立执行，跨文件不拼 stack trace
    let (dir, _paths) = write_temp_dir(&[
        (
            "a.log",
            "Exception in thread \"main\" java.lang.NPE\n\tat com.a.Main.run(Main.java:1)\n",
        ),
        ("b.log", "panic: runtime error: nil\nnext non-trace line\n"),
    ]);
    let dir_path = dir.path().to_str().unwrap();
    let out =
        run(&args_dir(r#""Exception" || "panic" | trace"#, dir_path)).expect("目录 trace 应成功");
    // 文件 a 应有 stack 续行
    assert!(out.contains("a.log:1:Exception"));
    assert!(out.contains("a.log:  → \tat com.a.Main.run"));
    // 文件 b 命中 panic 但无续行（next non-trace line 非续行）
    assert!(out.contains("b.log:1:panic: runtime error"));
    assert!(!out.contains("b.log:  → next"));
}

// ===== stdin 场景 =====

#[test]
fn test_trace_stdin_works() {
    let content =
        "Exception in thread \"main\" java.lang.NPE\n\tat com.Main.run(Main.java:1)\nnext line\n";
    let out = run_stdin(content, r#""Exception" | trace java"#).expect("stdin trace 应成功");
    assert!(out.contains("1:Exception in thread"));
    assert!(out.contains("  → \tat com.Main.run"));
}

// ===== fail-loud：终止 stage 后有非空 stage =====

#[test]
fn test_trace_then_head_fails_loud() {
    // trace 是终止 stage，head 在后应 fail-loud
    let f = write_temp("panic msg\n");
    let out = run(&args(
        r#""panic" | trace | head 1"#,
        f.path().to_str().unwrap(),
    ));
    let Err(e) = out else {
        panic!("trace | head 应 fail-loud 报错");
    };
    assert!(e.contains("trace"), "错误消息应含 trace: {e}");
    assert!(e.contains("head"), "错误消息应含 head: {e}");
    assert!(e.contains("终止动词"), "应说明是终止动词: {e}");
}

#[test]
fn test_trace_then_count_fails_loud() {
    let f = write_temp("panic msg\n");
    let out = run(&args(
        r#""panic" | trace | count"#,
        f.path().to_str().unwrap(),
    ));
    let Err(e) = out else {
        panic!("trace | count 应 fail-loud 报错");
    };
    assert!(e.contains("trace"));
    assert!(e.contains("count"));
}

#[test]
fn test_trace_alone_ok() {
    // trace 单独使用应正常工作（无终止后 stage）
    let f = write_temp("panic msg\n");
    let out =
        run(&args(r#""panic" | trace"#, f.path().to_str().unwrap())).expect("trace 单独使用应成功");
    assert!(out.contains("1:panic msg"));
}

// ===== --explain 输出 =====

#[test]
fn test_explain_trace_with_lang() {
    let f = write_temp("panic msg\n");
    let opts = RunOpts {
        explain: true,
        ..Default::default()
    };
    let args = vec![
        r#""panic" | trace go"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, opts).expect("--explain 应成功");
    assert!(out.contains("trace go"), "explain 应含 trace go: {out}");
    assert!(out.contains("multi-line stack trace"));
}

#[test]
fn test_explain_trace_auto_detect() {
    let f = write_temp("panic msg\n");
    let opts = RunOpts {
        explain: true,
        ..Default::default()
    };
    let args = vec![
        r#""panic" | trace"#.to_string(),
        f.path().to_str().unwrap().to_string(),
    ];
    let out = run_with_opts(&args, opts).expect("--explain 应成功");
    assert!(
        out.contains("trace (auto-detect"),
        "explain 应含 auto-detect: {out}"
    );
}
