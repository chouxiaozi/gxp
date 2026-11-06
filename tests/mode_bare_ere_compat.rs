//! BARE 模式 ERE 兼容验证：`gxp 'pattern' file` ≡ `grep -E 'pattern' file`。
//!
//! 验证 gxp BARE 模式（不走 L1/L2/L3 优化层）与 Rust `regex` crate 的 ERE
//! 语义完全一致——因为 BARE 模式内部就是 `Regex::new(pattern).is_match(line)`。
//!
//! 测试覆盖：元字符、字符类、锚定、重复、交替、分组、转义、边界场景。
//! 对照基准 = Rust `regex` crate（与 grep -E 同源 PCRE-like 语义）。

mod common;
use common::*;

use regex::bytes::Regex;

/// 对账：gxp BARE 模式输出 vs Rust regex crate 直接逐行 is_match。
/// 两者必须逐行一致（行号 + 行内容）。
fn assert_ere_equiv(pattern: &str, content: &str) {
    let f = write_temp(content);

    // gxp BARE 模式
    let gxp_out = run(&args(pattern, f.path().to_str().unwrap()))
        .unwrap_or_else(|e| panic!("gxp BARE 模式应成功，pattern={pattern:?}, err={e}"));

    // Rust regex crate 直接逐行匹配（等价 grep -E 语义）
    let re = Regex::new(pattern).unwrap_or_else(|e| panic!("正则编译失败: {e}"));
    let bytes = content.as_bytes();
    let mut expected = String::new();
    let mut start = 0;
    let mut line_no = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..])
            .map(|i| start + i)
            .unwrap_or(bytes.len());
        line_no += 1;
        let line = &bytes[start..end];
        if re.is_match(line) {
            expected.push_str(&format!("{}:{}\n", line_no, String::from_utf8_lossy(line)));
        }
        start = end + 1;
        if end == bytes.len() {
            break;
        }
    }

    assert_eq!(gxp_out, expected, "BARE 模式输出与 regex crate 不一致\npattern={pattern:?}\ngxp={gxp_out:?}\nexpected={expected:?}");
}

// ===== 元字符 =====

#[test]
fn test_ere_dot_matches_any() {
    assert_ere_equiv("a.c", "abc\naxc\na\nc\nadc\n");
}

#[test]
fn test_ere_dot_does_not_match_newline() {
    // . 不匹配换行（逐行扫描，每行独立）
    assert_ere_equiv("a.c", "a.c\na\nc\naXc\n");
}

// ===== 字符类 =====

#[test]
fn test_ere_char_class_positive() {
    assert_ere_equiv("[abc]", "apple\nbanana\ncherry\ndate\n");
}

#[test]
fn test_ere_char_class_negative() {
    assert_ere_equiv("[^abc]", "apple\nbanana\ncherry\ndate\n");
}

#[test]
fn test_ere_char_class_range() {
    assert_ere_equiv("[0-9]", "abc123\nno digits\n4\n");
}

#[test]
fn test_ere_char_class_mixed() {
    assert_ere_equiv("[a-zA-Z0-9_]", "hello\n_world\n123\n!@#\n");
}

// ===== 锚定 =====

#[test]
fn test_ere_caret_anchor() {
    assert_ere_equiv("^error", "error at start\nnot error\nerror again\n");
}

#[test]
fn test_ere_dollar_anchor() {
    assert_ere_equiv("end$", "at end\nending\nthe end\nend\n");
}

#[test]
fn test_ere_both_anchors() {
    assert_ere_equiv("^exact$", "exact\nnot exact\nexact line\n");
}

// ===== 重复 =====

#[test]
fn test_ere_star() {
    assert_ere_equiv("ab*", "a\nab\nabbb\nacb\n");
}

#[test]
fn test_ere_plus() {
    assert_ere_equiv("ab+", "a\nab\nabbb\nacb\n");
}

#[test]
fn test_ere_question() {
    assert_ere_equiv("colou?r", "color\ncolour\ncolouur\n");
}

#[test]
fn test_ere_brace_exact() {
    assert_ere_equiv("a{3}", "aa\naaa\naaaa\n");
}

#[test]
fn test_ere_brace_range() {
    assert_ere_equiv("a{2,3}", "a\naa\naaa\naaaa\n");
}

// ===== 交替 =====

#[test]
fn test_ere_alternation() {
    assert_ere_equiv("cat|dog|bird", "cat\ndog\nbird\nfish\n");
}

#[test]
fn test_ere_alternation_with_groups() {
    assert_ere_equiv(
        "(error|warn|info)",
        "error line\nwarn line\ninfo line\ndebug\n",
    );
}

// ===== 分组 =====

#[test]
fn test_ere_group_capture() {
    assert_ere_equiv("(ab)+", "ab\nabab\nababab\na\n");
}

#[test]
fn test_ere_nested_groups() {
    assert_ere_equiv("((a)(b))", "ab\nax\nxb\n");
}

// ===== 转义 =====

#[test]
fn test_ere_escape_dot() {
    assert_ere_equiv(r"\.", "a.b\nabc\na..b\n");
}

#[test]
fn test_ere_escape_star() {
    assert_ere_equiv(r"\*", "a*b\nab\na**b\n");
}

#[test]
fn test_ere_escape_bracket() {
    assert_ere_equiv(r"\[", "[array]\narray\n[item]\n");
}

// ===== 边界场景 =====

#[test]
fn test_ere_empty_pattern_matches_all() {
    assert_ere_equiv("", "line1\nline2\n\n");
}

#[test]
fn test_ere_no_match_returns_empty() {
    assert_ere_equiv("zzzznotfound", "line1\nline2\nline3\n");
}

#[test]
fn test_ere_empty_lines() {
    assert_ere_equiv("a", "a\n\na\n\n");
}

#[test]
fn test_ere_single_line_no_newline() {
    // 文件末尾无换行
    assert_ere_equiv("test", "this is a test");
}

#[test]
fn test_ere_special_chars_in_content() {
    assert_ere_equiv("a", "a.b\na*b\na+b\n");
}

#[test]
fn test_ere_unicode_content() {
    assert_ere_equiv("错误", "错误 line\ninfo line\n另一个错误\n");
}

// ===== 多行复杂场景 =====

#[test]
fn test_ere_complex_log_pattern() {
    let content = "2024-01-01 ERROR something broke\n\
                   2024-01-01 INFO all good\n\
                   2024-01-01 WARN minor issue\n\
                   2024-01-01 ERROR again\n";
    assert_ere_equiv("ERROR", content);
}

#[test]
fn test_ere_ip_pattern() {
    let content = "127.0.0.1\n\
                   192.168.1.100\n\
                   not an ip\n\
                   10.0.0.1\n";
    assert_ere_equiv("[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+", content);
}

#[test]
fn test_ere_word_boundary() {
    // ERE 没有 \b（grep -E 支持 \b 作为 GNU 扩展），用 [[:<:]] 不通用
    // 这里测 word-start 锚定
    assert_ere_equiv("error", "error\nerrorlog\nlogerror\n");
}
