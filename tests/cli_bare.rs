//! BARE 模式 + gxp 模式 + 错误路径端到端集成测试。
//!
//! 验证 `gxp <pattern> <file>` 完整链路：
//! argv → detect_and_dispatch → (BARE: regex 扫描 | gxp: DSL → L1 → L2 → 输出)。
//!
//! 覆盖场景：BARE ERE 兼容扫描 / gxp 纯字面量+布尔查询 / grep 短选项拒绝 /
//! 文件不存在报错 / 参数不足报错。

mod common;
use common::*;

// ===== BARE 模式（ERE 兼容）=====

#[test]
fn test_bare_simple_pattern() {
    // gxp 'ERROR' file ≡ grep -E 'ERROR' file
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let out = run(&args("ERROR", f.path().to_str().unwrap())).expect("应成功");
    // 命中行 1 和行 3
    assert_eq!(out, "1:ERROR line 1\n3:ERROR line 3\n");
}

#[test]
fn test_bare_ere_or() {
    // gxp 'a|b' file ≡ grep -E 'a|b' file
    let f = write_temp("apple\nbanana\ncherry\n");
    let out = run(&args("a|b", f.path().to_str().unwrap())).expect("应成功");
    // 行 1 含 a，行 2 含 b
    assert_eq!(out, "1:apple\n2:banana\n");
}

#[test]
fn test_bare_substring_with_space() {
    // gxp 'ERROR timeout' file：匹配连续子串
    let f = write_temp("ERROR timeout line 1\nERROR only\ntimeout only\n");
    let out = run(&args("ERROR timeout", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1:ERROR timeout line 1\n");
}

#[test]
fn test_bare_no_match() {
    let f = write_temp("info\nwarn\ndebug\n");
    let out = run(&args("error", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "");
}

#[test]
fn test_bare_regex_metachar() {
    // ERE 元字符：^ 锚定行首
    let f = write_temp("error line\nno error\nerror again\n");
    let out = run(&args("^error", f.path().to_str().unwrap())).expect("应成功");
    // 只命中行首是 error 的行（行 1 和行 3）
    assert_eq!(out, "1:error line\n3:error again\n");
}

#[test]
fn test_bare_trailing_newline() {
    let f = write_temp("error\ninfo\n");
    let out = run(&args("error", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1:error\n");
}

#[test]
fn test_bare_no_trailing_newline() {
    let f = write_temp("info\nerror");
    let out = run(&args("error", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "2:error\n");
}

// ===== gxp 模式（纯字面量/布尔）=====

#[test]
fn test_gxp_single_string() {
    // gxp '"error"' file
    let f = write_temp("error line 1\ninfo\nerror line 3\n");
    let out = run(&args(r#""error""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1:error line 1\n3:error line 3\n");
}

#[test]
fn test_gxp_and_query() {
    // gxp '"error" && "timeout"' file
    let f = write_temp("error timeout line 1\nerror only\ntimeout only\nboth error timeout\n");
    let out = run(&args(r#""error" && "timeout""#, f.path().to_str().unwrap())).expect("应成功");
    // 行 1 和行 4 同时含两者
    assert_eq!(out, "1:error timeout line 1\n4:both error timeout\n");
}

#[test]
fn test_gxp_or_query() {
    // gxp '"error" || "timeout"' file
    let f = write_temp("error line\ninfo\ntimeout line\nboth\n");
    let out = run(&args(r#""error" || "timeout""#, f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1:error line\n3:timeout line\n");
}

#[test]
fn test_gxp_bare_word() {
    // gxp 'ERROR' file（BARE 词，走 gxp 模式判定？）
    // 注：'ERROR' 不含 &&/||/!/|/字段谓词 → BARE 模式
    // 但 'ERROR' 也应是合法 gxp 查询——测试 BARE 模式覆盖
    let f = write_temp("ERROR line 1\ninfo\nERROR line 3\n");
    let out = run(&args("ERROR", f.path().to_str().unwrap())).expect("应成功");
    assert_eq!(out, "1:ERROR line 1\n3:ERROR line 3\n");
}

// ===== 错误路径 =====

#[test]
fn test_reject_grep_short_option() {
    let f = write_temp("error\n");
    let r = run(&args("-A", f.path().to_str().unwrap()));
    assert!(r.is_err(), "grep 短选项应被拒绝");
    let err = r.unwrap_err();
    assert!(
        err.contains("invalid grep short option"),
        "错误消息应含 'invalid grep short option'，得到: {err}"
    );
    assert!(
        err.contains("context"),
        "错误消息应含 gxp 等价语法提示 'context'，得到: {err}"
    );
}

#[test]
fn test_reject_grep_short_options_multiple() {
    // 覆盖多个 grep 短选项（D1 硬约束：不是 grep 超集）
    let f = write_temp("error\n");
    for opt in ["-B", "-C", "-i", "-v", "-l", "-c", "-n", "-h", "-r"] {
        let r = run(&args(opt, f.path().to_str().unwrap()));
        assert!(r.is_err(), "grep 短选项 {opt} 应被拒绝");
        let err = r.unwrap_err();
        assert!(
            err.contains("invalid grep short option"),
            "grep 短选项 {opt} 错误消息应含 'invalid grep short option'，得到: {err}"
        );
    }
}

#[test]
fn test_nonexistent_file_errors() {
    let r = run(&args("error", "/nonexistent/path/does/not/exist.log"));
    assert!(r.is_err(), "不存在的文件应报错");
    let err = r.unwrap_err();
    // IO 错误应含路径上下文 + io 错误详情
    assert!(
        err.contains("读取") && err.contains("失败"),
        "错误消息应含'读取 ... 失败'上下文，得到: {err}"
    );
    assert!(
        err.contains("/nonexistent/path/does/not/exist.log"),
        "错误消息应含用户输入的路径，得到: {err}"
    );
    assert!(
        err.contains("os error") || err.contains("io"),
        "错误消息应含 io/os 错误指示，得到: {err}"
    );
}

#[test]
fn test_too_few_args() {
    // 零参数报错（需要至少 pattern）；单参数走隐式 stdin
    let r = run(&[]);
    assert!(r.is_err(), "零参数应报错");
    let err = r.unwrap_err();
    assert!(
        err.contains("用法") || err.contains("gxp <pattern>"),
        "错误消息应含用法提示，得到: {err}"
    );
}

// ===== Compare 谓词（L2 字段比较）=====

#[test]
fn test_compare_predicate_executes_with_l2() {
    // gxp 'cost_ms>800' file 应成功执行（L1 粗筛 + L2 精判）
    let f = write_temp("cost_ms=900 user=alice\ncost_ms=100 user=bob\nlatency=50 user=carol\n");
    let out =
        run(&args("cost_ms>800", f.path().to_str().unwrap())).expect("Compare 谓词应成功执行");
    // 只命中第 1 行（cost_ms=900 > 800）
    assert!(
        out.contains("1:cost_ms=900 user=alice"),
        "应输出第 1 行，得到: {out}"
    );
    assert!(!out.contains("cost_ms=100"), "第 2 行 cost_ms=100 不应命中");
    assert!(
        !out.contains("latency=50"),
        "第 3 行无 cost_ms 字段不应命中"
    );
}

#[test]
fn test_compare_predicate_rejects_non_numeric_values() {
    // 千分位/单位等 11 维契约拒绝格式不应命中
    let f = write_temp("a=1,000\nb=900ms\nc=950\n");
    let out = run(&args("c>900", f.path().to_str().unwrap())).expect("应成功执行");
    // c=950 > 900 命中
    assert!(out.contains("c=950"));
}

// ===== Not 谓词 =====

#[test]
fn test_not_predicate_executes() {
    // gxp '!(cost_ms>800)' file：Not 三态取反（§3.3 字段不存在语义）
    // - cost_ms=900 > 800 → True → Not → False（不命中）
    // - cost_ms=100 > 800 → False → Not → True（命中）
    // - 无 cost_ms 字段 → NotNumeric → Not → True（命中，§3.3）
    let f = write_temp("cost_ms=900 msg=a\ncost_ms=100 msg=b\nno_cost msg=c\n");
    let out = run(&args("!(cost_ms>800)", f.path().to_str().unwrap())).expect("Not 谓词应执行成功");
    // cost_ms=900 → 不命中
    assert!(!out.contains("cost_ms=900"), "cost_ms=900 应被 Not 淘汰");
    // cost_ms=100 → 命中
    assert!(out.contains("cost_ms=100"), "cost_ms=100 应被 Not 命中");
    // 无 cost_ms 字段 → 命中（§3.3 字段不存在语义）
    assert!(out.contains("no_cost"), "无字段行应被 Not 命中");
}

// ===== In 谓词 =====

#[test]
fn test_in_predicate_executes() {
    // gxp 'tag in [info, error]' file：tag 字段值在集合中任一相等
    let f = write_temp("tag=info msg=a\ntag=error msg=b\ntag=warn msg=c\nno_tag msg=d\n");
    let out =
        run(&args("tag in [info, error]", f.path().to_str().unwrap())).expect("In 谓词应执行成功");
    assert!(out.contains("tag=info"), "tag=info 应命中");
    assert!(out.contains("tag=error"), "tag=error 应命中");
    assert!(!out.contains("tag=warn"), "tag=warn 应被淘汰");
    assert!(
        !out.contains("no_tag"),
        "无 tag 字段行应被淘汰（In 对字段不存在返回 False）"
    );
}

// ===== Between 谓词 =====

#[test]
fn test_between_predicate_executes() {
    // gxp 'cost between 100 and 200' file：闭区间 100 ≤ cost ≤ 200
    let f = write_temp(
        "cost=150 msg=a\ncost=100 msg=b\ncost=200 msg=c\ncost=99 msg=d\ncost=201 msg=e\n",
    );
    let out = run(&args(
        "cost between 100 and 200",
        f.path().to_str().unwrap(),
    ))
    .expect("Between 谓词应执行成功");
    // 区间内
    assert!(out.contains("cost=150"), "区间内应命中");
    // 边界（闭区间含端点）
    assert!(out.contains("cost=100"), "下界 lo=100 应命中（闭区间）");
    assert!(out.contains("cost=200"), "上界 hi=200 应命中（闭区间）");
    // 区间外
    assert!(!out.contains("cost=99"), "cost=99 < lo 应被淘汰");
    assert!(!out.contains("cost=201"), "cost=201 > hi 应被淘汰");
}

// ===== Exists 谓词 =====

#[test]
fn test_exists_predicate_executes() {
    // gxp 'user_id exists' file：字段存在性
    let f = write_temp("user_id=alice msg=a\nno_user msg=b\nuser_id=bob msg=c\n");
    let out =
        run(&args("user_id exists", f.path().to_str().unwrap())).expect("Exists 谓词应执行成功");
    assert!(out.contains("user_id=alice"), "user_id=alice 应命中");
    assert!(!out.contains("no_user"), "无 user_id 字段行应被淘汰");
    assert!(out.contains("user_id=bob"), "user_id=bob 应命中");
}

// ===== 多文件参数 fail-loud（v0.42 修复静默丢失 bug）=====

#[test]
fn test_multi_file_args_rejected_with_friendly_hint() {
    // gxp 'pat' f1 f2 f3 → fail-loud，列多余参数 + 给等价路径
    let args = vec![
        "ERROR".to_string(),
        "/tmp/f1.log".to_string(),
        "/tmp/f2.log".to_string(),
        "/tmp/f3.log".to_string(),
    ];
    let r = run(&args);
    assert!(r.is_err(), "多文件参数应 fail-loud 报错");
    let err = r.unwrap_err();
    assert!(err.contains("多文件参数暂不支持"), "得到: {err}");
    assert!(err.contains("/tmp/f2.log"), "应列出多余参数 f2: {err}");
    assert!(err.contains("/tmp/f3.log"), "应列出多余参数 f3: {err}");
    assert!(err.contains("目录递归"), "应给等价路径提示: {err}");
    assert!(err.contains("--glob"), "应给 glob 等价提示: {err}");
    assert!(err.contains("stdin 管道"), "应给 stdin 等价提示: {err}");
}

#[test]
fn test_single_file_arg_still_works_after_multi_file_fix() {
    // gxp 'pat' f1 → 正常执行（不触发多文件 fail-loud）
    let f = write_temp("ERROR here\ninfo\n");
    let out = run(&args("ERROR", f.path().to_str().unwrap())).expect("单文件应正常执行");
    assert!(out.contains("1:ERROR here"), "单文件仍应工作: {out}");
}

#[test]
fn test_stdin_marker_with_extra_files_rejected_by_mutual_exclusion() {
    // gxp 'pat' - f2 → stdin 与 file 互斥报错（先于多文件 fail-loud）
    // 因为 positional[1..].contains("-") && .any(!= "-") 触发互斥检测
    let args = vec![
        "ERROR".to_string(),
        "-".to_string(),
        "/tmp/f2.log".to_string(),
    ];
    let r = run(&args);
    assert!(r.is_err(), "stdin + file 应报互斥错误");
    let err = r.unwrap_err();
    assert!(
        err.contains("互斥"),
        "应报互斥错误（先于多文件检测）: {err}"
    );
}

// ===== v0.42 修复：Not(String/Bare/Regex) L2 求值 =====

#[test]
fn test_not_string_returns_lines_without_match() {
    // ! "ERROR" → 返回不含 ERROR 的行（L2 必须实际校验字面量）
    let f = write_temp("ERROR line 1\ninfo line 2\nERROR line 3\n");
    let out = run(&args(r#"! "ERROR""#, f.path().to_str().unwrap())).expect("Not 应执行成功");
    // 不含 ERROR 的行应命中（line 2）
    assert!(
        out.contains("2:info line 2"),
        "Not 应返回不含 ERROR 的行: {out}"
    );
    // 含 ERROR 的行不应命中
    assert!(
        !out.contains("ERROR line 1"),
        "含 ERROR 的行应被 Not 淘汰: {out}"
    );
    assert!(
        !out.contains("ERROR line 3"),
        "含 ERROR 的行应被 Not 淘汰: {out}"
    );
}

#[test]
fn test_not_bare_returns_lines_without_match() {
    // ! ERROR → 同 Not(String)，但 Bare 词在 gxp 模式下解析为 Prim::Bare
    let f = write_temp("ERROR line 1\ninfo line 2\nERROR line 3\n");
    let out = run(&args(r#"! ERROR"#, f.path().to_str().unwrap())).expect("Not Bare 应执行成功");
    assert!(
        out.contains("2:info line 2"),
        "Not Bare 应返回不含 ERROR 的行: {out}"
    );
    assert!(
        !out.contains("ERROR line 1"),
        "含 ERROR 的行应被 Not Bare 淘汰: {out}"
    );
}

#[test]
fn test_not_paren_string_returns_lines_without_match() {
    // !("ERROR") → 同 Not(String)，括号分组
    let f = write_temp("ERROR line 1\ninfo line 2\nERROR line 3\n");
    let out =
        run(&args(r#"!("ERROR")"#, f.path().to_str().unwrap())).expect("Not paren 应执行成功");
    assert!(
        out.contains("2:info line 2"),
        "Not paren 应返回不含 ERROR 的行: {out}"
    );
    assert!(
        !out.contains("ERROR line 1"),
        "含 ERROR 的行应被 Not paren 淘汰: {out}"
    );
}

#[test]
fn test_not_string_and_string_compound() {
    // ! "ERROR" && "info" → 不含 ERROR 且含 info 的行
    let f = write_temp("ERROR line\ninfo only\nboth ERROR and info\n");
    let out = run(&args(r#"! "ERROR" && "info""#, f.path().to_str().unwrap()))
        .expect("compound Not 应执行成功");
    // 只有 line 2 满足：不含 ERROR 且含 info
    assert!(
        out.contains("2:info only"),
        "compound Not 应返回 line 2: {out}"
    );
    assert!(!out.contains("ERROR line"), "含 ERROR 的行不应命中: {out}");
    assert!(
        !out.contains("both ERROR and info"),
        "含 ERROR 的行不应命中: {out}"
    );
}

// ===== v0.42 修复：未闭合字段谓词静默吞 bug =====

#[test]
fn test_unclosed_in_list_rejected_with_friendly_hint() {
    // gxp '_tag in [a, b' file（缺 `]`）→ fail-loud
    let f = write_temp("_tag=info msg=a\n");
    let r = run(&args("_tag in [a, b", f.path().to_str().unwrap()));
    assert!(r.is_err(), "未闭合 `]` 应 fail-loud");
    let err = r.unwrap_err();
    assert!(
        err.contains("查询被识别为 gxp 模式"),
        "错误消息应含 gxp 模式识别提示: {err}"
    );
    assert!(err.contains("_tag in [a, b"), "应含用户输入: {err}");
    assert!(err.contains("in [...]"), "应含语法提示: {err}");
    assert!(err.contains("gxp --help"), "应含帮助引导: {err}");
}

#[test]
fn test_unclosed_between_rejected_with_friendly_hint() {
    // gxp 'cost between 100' file（缺 `and 200`）→ fail-loud
    let f = write_temp("cost=150\n");
    let r = run(&args("cost between 100", f.path().to_str().unwrap()));
    assert!(r.is_err(), "未闭合 between 应 fail-loud");
    let err = r.unwrap_err();
    assert!(
        err.contains("查询被识别为 gxp 模式"),
        "错误消息应含 gxp 模式识别提示: {err}"
    );
    assert!(err.contains("between X and Y"), "应含语法提示: {err}");
}

#[test]
fn test_unclosed_paren_in_pred_rejected() {
    // gxp '!(cost_ms>800' file（缺 `)`）→ fail-loud
    let f = write_temp("cost_ms=900\n");
    let r = run(&args("!(cost_ms>800", f.path().to_str().unwrap()));
    assert!(r.is_err(), "未闭合 `)` 应 fail-loud");
    let err = r.unwrap_err();
    // 注意：此场景由 prim 的 `(` 开头检测触发，错误消息为 "解析失败：error Char at:"
    assert!(
        err.contains("解析失败") || err.contains("gxp 模式"),
        "错误消息应含 parse 失败信息: {err}"
    );
}

#[test]
fn test_closed_pred_still_works_after_unclosed_fix() {
    // gxp 'tag in [a, b]' file → 正常解析（确认修复未误伤闭合语法）
    // 注：用普通字段 `tag`（非伪字段 `_tag`）以避免 _tag 走 bracket 路径
    let f = write_temp("tag=info msg=a\ntag=error msg=b\ntag=warn msg=c\n");
    let out = run(&args("tag in [info, error]", f.path().to_str().unwrap()))
        .expect("闭合 in 列表应正常解析");
    assert!(out.contains("tag=info"), "tag=info 应命中: {out}");
    assert!(out.contains("tag=error"), "tag=error 应命中: {out}");
    assert!(!out.contains("tag=warn"), "tag=warn 不应命中: {out}");
}
