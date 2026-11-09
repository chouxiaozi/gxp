//! `trace [LANG]`：多行 stack trace 捕获（§9.1 候选 #2 落地）。
//!
//! # 语义
//!
//! - `trace`：自动检测语言（按命中首行特征识别 java/python/rust/js/go/csharp）
//! - `trace LANG`：指定语言规则（java/python/rust/js/go/csharp）
//!
//! # 实现模型
//!
//! L1 不变（仍只匹配首行模式）。pipeline 阶段对每个命中行：
//! 1. 确定语言（用户指定或自动检测）
//! 2. 调用该语言续行规则，从命中行下一行开始扫描
//! 3. 收集续行到 `extra_lines: Vec<Arc<[u8]>>`，输出带 extra_lines 的 MatchedLine
//!
//! # 6 语言续行规则
//!
//! | LANG | 续行特征 | 终止条件 |
//! |---|---|---|
//! | python | `^  File ".*", line \d+` 或 `^    \S` | 末行 `^[A-Za-z]\w*Error: ` 或下一非缩进行 |
//! | java | `^\s+at\s` | `^Caused by:` 子段开始或下一行非 `^\s+at\s` |
//! | js | `^\s+at\s` | 下一 `^Error:` 或下一非 `^\s+at\s` |
//! | rust | 新格式（≥1.65）头行尾 `:` 后紧跟 1 行顶格消息行；`note:` / `stack backtrace:` 顶格；`^\s+\d+:` 帧；`^\s+at ` 帧内位置行 | 下一非续行 |
//! | go | `^(goroutine \d+ \[running:\]|\s+\S+\.go:\d+)` | 下一 `^panic:` 或下一非续行 |
//! | csharp | `^\s+at\s` | `^--- End of inner exception` 或下一非 `^\s+at\s` |
//!
//! # --verify 对齐
//!
//! 确定性算法（行字节模式匹配），暴力路径与优化路径走同一逻辑，100% 对齐。
//!
//! # 流式路径不支持
//!
//! trace 需要跨行边界判定，但流式路径（>512MB BufReader）按行回调，
//! "看到下一行才能决定上一行是续行"在流式路径下天然不可。
//! 调用方检测到 trace stage + 流式路径 → fail-loud 提示用 `context N` 替代。

use std::sync::Arc;

use super::MatchedLine;
use crate::pipeline::context::build_line_offsets;

/// 执行 `trace` 管道：把命中行扩展为带 `extra_lines` 的多行记录。
///
/// - `matched`：扫描阶段（L1+L2 过滤后）的命中行（首行）
/// - `content`：整个文件内容（trace 需要访问命中行后续行）
/// - `lang`：`None` 自动检测；`Some(name)` 用指定语言规则
///
/// 返回：每个命中行 → 一条带 extra_lines 的 MatchedLine（数量不变）。
pub fn apply_trace(
    matched: &[MatchedLine],
    content: &[u8],
    lang: Option<&str>,
) -> Vec<MatchedLine> {
    if matched.is_empty() {
        return Vec::new();
    }
    let line_offsets = build_line_offsets(content);
    let total_lines = line_offsets.len();

    matched
        .iter()
        .map(|m| {
            let detected_lang = lang.map(canonical_lang).or_else(|| detect_lang(&m.line));
            let extra = if let Some(l) = detected_lang {
                collect_extra_lines(content, &line_offsets, total_lines, m.line_no, l)
            } else {
                // 自动检测失败 → fallback N=20 固定行数（保守拉窗，仍输出多行记录）
                collect_extra_lines_fallback(content, &line_offsets, total_lines, m.line_no, 20)
            };
            let mut out = m.clone();
            out.extra_lines = extra;
            out
        })
        .collect()
}

/// 把用户输入的语言名规范化（小写、`c#`→`csharp`、`c++`/`cpp`/`cxx`/`cc` 不支持但已在 parser 拦截）。
fn canonical_lang(s: &str) -> &'static str {
    match s {
        "java" => "java",
        "python" | "py" => "python",
        "rust" | "rs" => "rust",
        "js" | "javascript" => "js",
        "go" | "golang" => "go",
        "csharp" | "cs" | "c#" => "csharp",
        _ => "java", // parser 已限制合法值，理论上不会走到
    }
}

/// 检测命中首行的语言。
///
/// 按命中行特征匹配 6 语言首行模式。返回语言名（`java`/`python`/`rust`/`js`/`go`/`csharp`）。
/// 全部不匹配 → None（fallback 到 N=20 固定行数）。
fn detect_lang(first_line: &[u8]) -> Option<&'static str> {
    let s = String::from_utf8_lossy(first_line);
    // Python: `Traceback (most recent call last):`
    if s.starts_with("Traceback (most recent call last):") {
        return Some("python");
    }
    // Java: `Exception in thread "..." java.lang.X` 或 `java.lang.X`
    if s.starts_with("Exception in thread") || s.starts_with("java.lang.") {
        return Some("java");
    }
    // Rust: `thread 'main' panicked at ...`
    if s.starts_with("thread ") && s.contains("panicked at") {
        return Some("rust");
    }
    // C#: `System.XException: msg`（在 JS 之前检查，避免被 `:Exception:` 误判为 JS）
    if s.starts_with("System.") && s.contains("Exception") {
        return Some("csharp");
    }
    // JS (V8): `Error: msg` 或 `XXXError: msg`（必须含 `Error` 关键字）
    if s.contains("Error: ") {
        return Some("js");
    }
    // Go: `panic: msg`
    if s.starts_with("panic:") || s.starts_with("panic ") {
        return Some("go");
    }
    None
}

/// 按语言规则收集命中行后续的续行到 stack trace 边界。
///
/// Go 特例：`goroutine N [running]:` 行作为 stack 头，
/// 其后到空行/EOF 都属于 stack frame（含 `main.main()` 无缩进函数名行）。
/// 但 `main.main()` 这种纯函数名行后续到含 `.go:N` 的栈帧才算 stack。
/// 实现：goroutine 头后宽松接收，但下一行既不是 `.go:N` 也不是已知 stack 标志时停止。
///
/// 其他语言走 `is_continuation` 严格判定。
///
/// 返回 `extra_lines`（不含末尾换行符）。
fn collect_extra_lines(
    content: &[u8],
    line_offsets: &[(usize, usize)],
    total_lines: usize,
    hit_line_no: usize,
    lang: &str,
) -> Vec<Arc<[u8]>> {
    let mut extras: Vec<Arc<[u8]>> = Vec::new();
    let mut idx = hit_line_no + 1; // 1-based，下一行
                                   // Go 状态机：遇到 goroutine [running]: 头后宽松接收，到空行或非 stack 形态终止
    let mut go_in_stack = false;
    // Rust 新格式（≥1.65）状态机：头行 `panicked at <loc>:` 以 `:` 结尾，
    // 错误消息在下一行顶格（任意文本，无法无状态识别）→ 仅头行后紧跟的 1 行无条件接收
    let mut rust_msg_pending = false;
    if lang == "rust" {
        let (s, e) = line_offsets[hit_line_no - 1];
        rust_msg_pending = is_new_format_panic_header(&content[s..e]);
    }
    while idx <= total_lines {
        let (s, e) = line_offsets[idx - 1];
        let line = &content[s..e];
        // 空行终止所有语言的 stack trace
        if line.is_empty() {
            break;
        }
        if lang == "go" {
            // 看到 goroutine [running]: 头，进入宽松模式
            if line.starts_with(b"goroutine ") && memchr::memmem::find(line, b"[running]").is_some()
            {
                go_in_stack = true;
                extras.push(Arc::from(line));
                idx += 1;
                continue;
            }
            if go_in_stack {
                // goroutine 头之后：
                // - 严格判定为续行（含 .go:N） → 收
                // - 函数调用行（无缩进、含括号、不含非 stack 字符）→ 收
                // - 否则终止
                if is_go_continuation(line) || is_go_function_call(line) {
                    extras.push(Arc::from(line));
                    idx += 1;
                    continue;
                }
                break;
            }
            // 没看到 goroutine 头，走严格判定
            if !is_go_continuation(line) {
                break;
            }
            extras.push(Arc::from(line));
            idx += 1;
        } else {
            // Rust 新格式消息行：头行后紧跟的 1 行任意顶格文本（一次性）
            if rust_msg_pending {
                rust_msg_pending = false;
                extras.push(Arc::from(line));
                idx += 1;
                continue;
            }
            if !is_continuation(line, lang) {
                break;
            }
            extras.push(Arc::from(line));
            idx += 1;
        }
    }
    extras
}

/// Go 函数调用行判定：`main.main()` / `created by ...` 等无缩进函数名行
///
/// Go stack 中常见无缩进函数名行：
/// - `main.main()` — 函数调用
/// - `created by main.goroutine` — goroutine 创建来源
///
/// 判定规则：行不含空格、含 `()` 括号、或以 `created by` 开头。
fn is_go_function_call(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }
    // `created by ...` 是 goroutine 来源行
    if line.starts_with(b"created by ") {
        return true;
    }
    // 简单判定：含 `()` 括号、不含空格（main.main() 形态）
    let has_parens =
        memchr::memmem::find(line, b"(").is_some() && memchr::memmem::find(line, b")").is_some();
    let no_spaces = !line.contains(&b' ');
    has_parens && no_spaces
}

/// Fallback：固定拉 N 行续行（保守窗口，仍输出多行记录）。
fn collect_extra_lines_fallback(
    content: &[u8],
    line_offsets: &[(usize, usize)],
    total_lines: usize,
    hit_line_no: usize,
    n: usize,
) -> Vec<Arc<[u8]>> {
    let mut extras: Vec<Arc<[u8]>> = Vec::new();
    let start_idx = hit_line_no + 1;
    let end_idx = (hit_line_no + n).min(total_lines);
    for idx in start_idx..=end_idx {
        let (s, e) = line_offsets[idx - 1];
        extras.push(Arc::from(&content[s..e]));
    }
    extras
}

/// 判断一行是否是 stack trace 的续行（按语言规则）。
fn is_continuation(line: &[u8], lang: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    match lang {
        "python" => is_python_continuation(line),
        "java" => is_java_continuation(line),
        "js" => is_js_continuation(line),
        "rust" => is_rust_continuation(line),
        "go" => is_go_continuation(line),
        "csharp" => is_csharp_continuation(line),
        _ => false,
    }
}

/// Python 续行：`^  File ".*", line \d+` 或 `^    \S`（4 空格缩进 + 非空格）
fn is_python_continuation(line: &[u8]) -> bool {
    if line.starts_with(b"  File \"") {
        return true;
    }
    // 4 空格缩进 + 非空格字符
    if line.len() >= 5 && &line[..4] == b"    " && line[4] != b' ' && line[4] != b'\t' {
        return true;
    }
    false
}

/// Java 续行：`^\s+at\s`（`\t` 或空格开头 + `at `）
fn is_java_continuation(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }
    let first = line[0];
    if first != b' ' && first != b'\t' {
        return false;
    }
    // 找首个非空白
    let trimmed = line
        .iter()
        .skip_while(|&&b| b == b' ' || b == b'\t')
        .copied()
        .collect::<Vec<u8>>();
    trimmed.starts_with(b"at ")
}

/// JS (V8) 续行：`^\s+at\s`（与 Java 同形式）
fn is_js_continuation(line: &[u8]) -> bool {
    is_java_continuation(line)
}

/// Rust 续行（两种 panic 格式）：
/// - 顶格 `note:` / `stack backtrace:`
/// - 帧 `^\s+\d+:`（`   0: std::panicking::begin_panic`）
/// - 帧内位置行 `^\s+at `（`             at ./src/foo.rs:10:5`，帧与帧之间，与 Java/JS 同形态）
/// - 新格式（≥1.65）消息行不在此判定——任意顶格文本无法无状态识别，
///   由 `collect_extra_lines` 的 `rust_msg_pending` 状态一次性接收
fn is_rust_continuation(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }
    // 顶格关键字（不缩进）
    if line.starts_with(b"note:") || line.starts_with(b"stack backtrace:") {
        return true;
    }
    // 帧内位置行（缩进 + at，两格式回溯中均出现）
    if is_java_continuation(line) {
        return true;
    }
    // 缩进 + 数字+:（栈帧编号）
    let first = line[0];
    if first != b' ' && first != b'\t' {
        return false;
    }
    let trimmed: Vec<u8> = line
        .iter()
        .skip_while(|&&b| b == b' ' || b == b'\t')
        .copied()
        .collect();
    starts_with_number_colon(&trimmed)
}

/// Rust 新格式（≥1.65）panic 头行判定：`thread '<name>' panicked at <loc>:` 行尾是 `:`。
///
/// 旧格式消息在头行内（`panicked at 'msg', src/main.rs:42:3`），行尾是列号数字；
/// 新格式错误消息移到下一行顶格。以行尾是否 `:` 区分（trim_end 兼容 CRLF）。
fn is_new_format_panic_header(hit_line: &[u8]) -> bool {
    let s = String::from_utf8_lossy(hit_line);
    let t = s.trim_end();
    t.starts_with("thread ") && t.contains("panicked at") && t.ends_with(':')
}

/// 判断字节串是否以 `数字+:` 开头（如 `0:`、`1:`、`42:`）
fn starts_with_number_colon(s: &[u8]) -> bool {
    let mut has_digit = false;
    for &c in s {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if c == b':' {
            return has_digit;
        } else {
            return false;
        }
    }
    false
}

/// Go 续行：`^goroutine \d+ \[running\]:` 或 `^\s+something.go:\d+`
///
/// Go panic 输出特征：
/// - `goroutine 1 [running]:` 是 stack trace 头（注意 `[running]:` 冒号在右括号后）
/// - `main.main()` 是函数调用（缩进或不缩进，但属于 stack 一部分——Go 默认不缩进）
/// - `\t/usr/local/go/src/x.go:5 +0x40` 是带文件路径的栈帧（tab 缩进）
fn is_go_continuation(line: &[u8]) -> bool {
    // `goroutine N [running]:` 头
    if line.starts_with(b"goroutine ") && memchr::memmem::find(line, b"[running]").is_some() {
        return true;
    }
    if line.is_empty() {
        return false;
    }
    // 缩进 + 文件路径 `.go:N` 或函数调用
    let first = line[0];
    if first != b' ' && first != b'\t' {
        return false;
    }
    let trimmed: Vec<u8> = line
        .iter()
        .skip_while(|&&b| b == b' ' || b == b'\t')
        .copied()
        .collect();
    // 检查含 `.go:` 后跟数字
    let s = String::from_utf8_lossy(&trimmed);
    if let Some(pos) = s.find(".go:") {
        let rest = &s[pos + 4..];
        // 紧跟数字
        rest.chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    } else {
        false
    }
}

/// C# 续行：`^\s+at\s`（与 Java/JS 同形式）
fn is_csharp_continuation(line: &[u8]) -> bool {
    is_java_continuation(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::CaptureMap;

    fn make_line(line_no: usize, content: &str) -> MatchedLine {
        MatchedLine::new(line_no, content.as_bytes())
    }

    #[test]
    fn test_detect_lang_python() {
        assert_eq!(
            detect_lang(b"Traceback (most recent call last):"),
            Some("python")
        );
    }

    #[test]
    fn test_detect_lang_java() {
        assert_eq!(
            detect_lang(b"Exception in thread \"main\" java.lang.NullPointerException: x"),
            Some("java")
        );
        assert_eq!(
            detect_lang(b"java.lang.RuntimeException: fail"),
            Some("java")
        );
    }

    #[test]
    fn test_detect_lang_rust() {
        assert_eq!(
            detect_lang(b"thread 'main' panicked at 'x', src/main.rs:42:3"),
            Some("rust")
        );
    }

    #[test]
    fn test_detect_lang_js() {
        assert_eq!(detect_lang(b"Error: something failed"), Some("js"));
        assert_eq!(detect_lang(b"TypeError: invalid type"), Some("js"));
    }

    #[test]
    fn test_detect_lang_go() {
        assert_eq!(detect_lang(b"panic: runtime error: x"), Some("go"));
    }

    #[test]
    fn test_detect_lang_csharp() {
        assert_eq!(
            detect_lang(b"System.NullReferenceException: x"),
            Some("csharp")
        );
    }

    #[test]
    fn test_detect_lang_unknown_returns_none() {
        assert_eq!(detect_lang(b"some random log line"), None);
        assert_eq!(detect_lang(b"ERROR user=alice"), None);
    }

    #[test]
    fn test_apply_trace_empty_input() {
        let out = apply_trace(&[], b"", None);
        assert!(out.is_empty());
    }

    #[test]
    fn test_apply_trace_preserves_hit_count() {
        // 命中行数量不变（trace 只扩展 extra_lines，不增加记录数）
        let m = make_line(1, "panic: test");
        let content = b"panic: test\ngoroutine 1 [running]:\nmain.main()\n\t/x.go:1\nnext line";
        let out = apply_trace(&[m], content, Some("go"));
        assert_eq!(out.len(), 1, "trace 不应改变命中行数量");
    }

    #[test]
    fn test_apply_trace_java_collects_at_lines() {
        let content = b"\
Exception in thread \"main\" java.lang.NullPointerException: msg
\tat com.example.MyClass.method(MyClass.java:42)
\tat com.example.Main.main(Main.java:10)
next non-trace line";
        let m = make_line(
            1,
            "Exception in thread \"main\" java.lang.NullPointerException: msg",
        );
        let out = apply_trace(&[m], content, Some("java"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].extra_lines.len(), 2, "应收集 2 行 at-frame");
        assert_eq!(
            &out[0].extra_lines[0][..],
            b"\tat com.example.MyClass.method(MyClass.java:42)"
        );
        assert_eq!(
            &out[0].extra_lines[1][..],
            b"\tat com.example.Main.main(Main.java:10)"
        );
    }

    #[test]
    fn test_apply_trace_python_collects_file_lines() {
        let content = b"\
Traceback (most recent call last):
  File \"example.py\", line 5, in <module>
    say('Micheal')
  File \"example.py\", line 3, in say
    print('Hello, ' + nam)
NameError: name 'nam' is not defined
next line";
        let m = make_line(1, "Traceback (most recent call last):");
        let out = apply_trace(&[m], content, Some("python"));
        // 收集 4 行续行（2 个 File + 2 个缩进代码行）；末行 NameError 是终止（非缩进）
        // 注：NameError 在末行模式匹配前会先经 is_python_continuation 判定为 false → break
        assert_eq!(out[0].extra_lines.len(), 4);
        assert!(out[0].extra_lines[0].starts_with(b"  File"));
    }

    #[test]
    fn test_apply_trace_rust_collects_panicked_continuation() {
        let content = b"\
thread 'main' panicked at 'x', src/main.rs:42:3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
stack backtrace:
   0: std::panicking::begin_panic
   1: main::main
next non-trace line";
        let m = make_line(1, "thread 'main' panicked at 'x', src/main.rs:42:3");
        let out = apply_trace(&[m], content, Some("rust"));
        assert_eq!(out[0].extra_lines.len(), 4);
    }

    #[test]
    fn test_new_format_panic_header_discriminates() {
        // 新格式：行尾 `:`（位置后无消息）
        assert!(is_new_format_panic_header(
            b"thread 'main' panicked at src/main.rs:42:18:"
        ));
        // CRLF：trim_end 兼容
        assert!(is_new_format_panic_header(
            b"thread 'worker-1' panicked at src/lib.rs:7:5:\r"
        ));
        // 旧格式：消息在头行内，行尾是列号数字 → false
        assert!(!is_new_format_panic_header(
            b"thread 'main' panicked at 'x', src/main.rs:42:3"
        ));
        // 非 panic 头行 → false
        assert!(!is_new_format_panic_header(b"note: something:"));
    }

    #[test]
    fn test_apply_trace_rust_new_format_message_then_note() {
        // B13：新格式（≥1.65）消息行顶格，后跟 note:（RUST_BACKTRACE 未设）
        let content = b"\
thread 'main' panicked at src/main.rs:42:18:
called `Result::unwrap()` on an `Err` value: ParseError
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
2026-11-09T10:00:08.000Z INFO [api] msg=after_panic";
        let m = make_line(1, "thread 'main' panicked at src/main.rs:42:18:");
        let out = apply_trace(&[m], content, Some("rust"));
        // 消息行（msg 槽位）+ note: = 2 个续行；正常日志行终止
        assert_eq!(out[0].extra_lines.len(), 2, "{:?}", out[0].extra_lines);
        assert!(out[0].extra_lines[0].starts_with(b"called `Result::unwrap()`"));
        assert!(out[0].extra_lines[1].starts_with(b"note:"));
    }

    #[test]
    fn test_apply_trace_rust_new_format_backtrace_with_at_lines() {
        // B13 同族：真实回溯帧间 `at` 位置行（RUST_BACKTRACE=1 形态）
        let content = b"\
thread 'main' panicked at src/main.rs:42:18:
called `Result::unwrap()` on an `Err` value: ParseError
stack backtrace:
   0: gxp::l2::extract::find_value
             at ./src/l2/extract.rs:88:24
   1: gxp::mode::eval::eval_prepared
             at ./src/mode/eval.rs:210:9
next non-trace line";
        let m = make_line(1, "thread 'main' panicked at src/main.rs:42:18:");
        let out = apply_trace(&[m], content, Some("rust"));
        // 消息 1 + stack backtrace: 1 + 帧 2 + at 2 = 6 个续行
        assert_eq!(out[0].extra_lines.len(), 6, "{:?}", out[0].extra_lines);
        assert!(!out[0]
            .extra_lines
            .iter()
            .any(|l| l.starts_with(b"next non-trace")));
    }

    #[test]
    fn test_apply_trace_rust_old_format_backtrace_with_at_lines() {
        // 旧格式 + 帧间 at 行（at 规则两格式通用）
        let content = b"\
thread 'main' panicked at 'x', src/main.rs:42:3
stack backtrace:
   0: std::panicking::begin_panic
             at /rustc/abc/library/std/src/panicking.rs:662:3
   1: main::main
next non-trace line";
        let m = make_line(1, "thread 'main' panicked at 'x', src/main.rs:42:3");
        let out = apply_trace(&[m], content, Some("rust"));
        // stack backtrace: + 帧 2 + at 1 = 4 个续行（at 行后帧继续收）
        assert_eq!(out[0].extra_lines.len(), 4, "{:?}", out[0].extra_lines);
    }

    #[test]
    fn test_apply_trace_go_collects_goroutine_frame() {
        let content = b"\
panic: runtime error: index out of range
goroutine 1 [running]:
main.main()
\t/usr/local/go/src/x.go:5 +0x40
next non-trace line";
        let m = make_line(1, "panic: runtime error: index out of range");
        let out = apply_trace(&[m], content, Some("go"));
        assert_eq!(out[0].extra_lines.len(), 3);
    }

    #[test]
    fn test_apply_trace_csharp_collects_at_lines() {
        let content = b"\
System.NullReferenceException: Object reference not set
   at com.example.MyClass.Method() in /path/File.cs:line 42
   at com.example.Main.Main() in /path/Main.cs:line 10
--- End of inner exception stack trace ---
next line";
        let m = make_line(1, "System.NullReferenceException: Object reference not set");
        let out = apply_trace(&[m], content, Some("csharp"));
        // 收集 2 行 at frame；`--- End of inner exception` 终止（非 `^\s+at\s`）
        assert_eq!(out[0].extra_lines.len(), 2);
    }

    #[test]
    fn test_apply_trace_js_collects_at_lines() {
        let content = b"\
Error: something failed
    at Object.<anonymous> (/path/file.js:10:5)
    at Module._compile (module.js:5:1)
next line";
        let m = make_line(1, "Error: something failed");
        let out = apply_trace(&[m], content, Some("js"));
        assert_eq!(out[0].extra_lines.len(), 2);
    }

    #[test]
    fn test_apply_trace_auto_detect_java() {
        // 不指定 lang，按首行特征自动检测
        let content = b"\
Exception in thread \"main\" java.lang.NullPointerException: msg
\tat com.example.Main.main(Main.java:10)
next line";
        let m = make_line(
            1,
            "Exception in thread \"main\" java.lang.NullPointerException: msg",
        );
        let out = apply_trace(&[m], content, None);
        assert_eq!(out[0].extra_lines.len(), 1);
    }

    #[test]
    fn test_apply_trace_auto_detect_fallback_when_unknown() {
        // 自动检测失败 → fallback N=20（保守拉窗，仍输出多行记录）
        let content = b"random log line\nline 2\nline 3\nline 4\nline 5";
        let m = make_line(1, "random log line");
        let out = apply_trace(&[m], content, None);
        // fallback 拉 4 行（命中行+1 到末尾）
        assert_eq!(out[0].extra_lines.len(), 4);
    }

    #[test]
    fn test_apply_trace_at_eof_no_extras() {
        // 命中行是文件最后一行 → 无续行
        let content = b"line 1\npanic: test";
        let m = make_line(2, "panic: test");
        let out = apply_trace(&[m], content, Some("go"));
        assert!(out[0].extra_lines.is_empty(), "EOF 后应无续行");
    }

    #[test]
    fn test_apply_trace_empty_line_terminates() {
        // 空行终止 stack trace
        let content = b"panic: test\n\t/x.go:1\n\npanic: next";
        let m = make_line(1, "panic: test");
        let out = apply_trace(&[m], content, Some("go"));
        // 命中后只有 1 行 `.go:` 续行，空行终止
        assert_eq!(out[0].extra_lines.len(), 1);
    }

    #[test]
    fn test_apply_trace_preserves_existing_fields() {
        // trace 不应破坏 captures/path/match_bytes
        let content = b"panic: test\n\t/x.go:1";
        let mut m = MatchedLine::with_captures(1, b"panic: test", {
            let mut map = CaptureMap::new();
            map.insert("trace_id".to_string(), b"req-123".to_vec());
            map
        });
        m.path = Some(std::sync::Arc::from("app.log"));
        m.match_bytes = Some(std::sync::Arc::from(b"panic".as_ref()));
        let out = apply_trace(&[m], content, Some("go"));
        assert_eq!(out[0].captures.get("trace_id"), Some(&b"req-123".to_vec()));
        assert_eq!(out[0].path.as_deref(), Some("app.log"));
        assert_eq!(out[0].match_bytes.as_deref(), Some(b"panic".as_ref()));
        assert_eq!(out[0].extra_lines.len(), 1);
    }

    #[test]
    fn test_canonical_lang_aliases() {
        assert_eq!(canonical_lang("python"), "python");
        assert_eq!(canonical_lang("py"), "python");
        assert_eq!(canonical_lang("rust"), "rust");
        assert_eq!(canonical_lang("rs"), "rust");
        assert_eq!(canonical_lang("javascript"), "js");
        assert_eq!(canonical_lang("golang"), "go");
        assert_eq!(canonical_lang("cs"), "csharp");
        assert_eq!(canonical_lang("c#"), "csharp");
    }

    #[test]
    fn test_is_python_continuation() {
        assert!(is_python_continuation(
            b"  File \"x.py\", line 5, in <module>"
        ));
        assert!(is_python_continuation(b"    print('Hello, ' + nam)"));
        assert!(!is_python_continuation(
            b"NameError: name 'nam' is not defined"
        ));
        assert!(!is_python_continuation(b"  non-indented-prefix")); // 2 空格非 4
    }

    #[test]
    fn test_is_java_continuation() {
        assert!(is_java_continuation(
            b"\tat com.example.Main.main(Main.java:10)"
        ));
        assert!(is_java_continuation(
            b"    at com.example.Main.main(Main.java:10)"
        ));
        assert!(!is_java_continuation(
            b"Caused by: java.lang.RuntimeException"
        ));
        assert!(!is_java_continuation(b"next line"));
    }

    #[test]
    fn test_is_rust_continuation() {
        assert!(is_rust_continuation(b"note: run with `RUST_BACKTRACE=1`"));
        assert!(is_rust_continuation(b"stack backtrace:"));
        assert!(is_rust_continuation(b"   0: std::panicking::begin_panic"));
        assert!(!is_rust_continuation(b"next non-trace line"));
    }

    #[test]
    fn test_is_go_continuation() {
        assert!(is_go_continuation(b"goroutine 1 [running]:"));
        assert!(is_go_continuation(b"\t/usr/local/go/src/x.go:5 +0x40"));
        assert!(!is_go_continuation(b"panic: next panic"));
        assert!(!is_go_continuation(b"random log"));
    }

    #[test]
    fn test_is_csharp_continuation() {
        assert!(is_csharp_continuation(
            b"   at com.example.Main.Main() in /path/Main.cs:line 10"
        ));
        assert!(!is_csharp_continuation(
            b"--- End of inner exception stack trace ---"
        ));
    }

    #[test]
    fn test_starts_with_number_colon() {
        assert!(starts_with_number_colon(b"0:"));
        assert!(starts_with_number_colon(b"42:"));
        assert!(!starts_with_number_colon(b"abc"));
        assert!(!starts_with_number_colon(b":"));
    }
}
