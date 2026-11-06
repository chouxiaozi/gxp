//! 四段式报错：grep 短选项拒绝（设计文档 §1.5、§2.4 示例 11）。
//!
//! 覆盖 11 个 grep 短选项：`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`
//! （设计文档 §0 列出的完整集合）。
//!
//! 报错格式（设计文档 §1.5 四段式）：
//! ```text
//! gxp: error: invalid grep short option
//!
//! gxp 不接受 grep 短选项（-A / -B / -C ...）。等价功能请用 gxp 原生语法。
//!
//! grep 等价：           gxp 替代：
//!   grep -A 3 'ERROR'   gxp 'ERROR' | context 3
//!   grep -l 'pat'       gxp 'pat' | files
//!   grep -c 'pat'       gxp 'pat' | count
//!
//! 示例： gxp 'ERROR' app.log | context 3
//! ```

const GREP_SHORT_OPTS: &[&str] = &[
    "-A", "-B", "-C", "-e", "-i", "-v", "-o", "-l", "-L", "-c", "-w", "-x", "-n", "-h", "-r", "-q",
    "-s",
];

/// 扫描 argv，若发现 grep 短选项返回四段式错误。
pub fn check(args: &[String]) -> crate::Result<()> {
    for a in args {
        if GREP_SHORT_OPTS.contains(&a.as_str()) {
            return Err(crate::GxpError::InvalidGrepOption {
                raw: a.clone(),
                suggestion: suggestion(a),
            });
        }
    }
    Ok(())
}

/// 给出 grep 短选项的 gxp 等价语法提示。
fn grep_opt(opt: &str) -> &'static str {
    match opt {
        "-A" => "context N (after)",
        "-B" => "context N (before)",
        "-C" => "context N (both sides)",
        "-e" => "直接列多个 pattern（邻接 = AND）",
        "-i" => "在 pattern 内用 `(?i:...)` 正则内联 flag",
        "-v" => "!(pred) 取反",
        "-o" => "默认就是行匹配，输出整行；`-o` 等价于 `fields <match-group>`",
        "-l" => "| files",
        "-L" => "| files --invert",
        "-c" => "| count",
        "-w" => "在 pattern 内用 `\\b` 词边界",
        "-x" => "在 pattern 内用 `^...$` 全行锚定",
        "-n" => "默认输出行号",
        "-h" => "| fields <非文件名字段>",
        "-r" => "默认递归（gxp 目录参数即递归）",
        "-q" => "退出码 0/1 替代输出",
        "-s" => "gxp 默认大声失败（fail-loud）",
        _ => "见设计文档 §2.4",
    }
}

fn suggestion(opt: &str) -> String {
    format!("grep {opt} → gxp {}", grep_opt(opt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a() {
        let r = check(&["-A".into(), "3".into(), "ERROR".into(), "app.log".into()]);
        assert!(r.is_err());
    }

    #[test]
    fn allows_no_opts() {
        let r = check(&["ERROR".into(), "app.log".into()]);
        assert!(r.is_ok());
    }

    #[test]
    fn rejects_each_grep_short_opt() {
        for opt in GREP_SHORT_OPTS {
            assert!(check(&[(*opt).into()]).is_err(), "should reject {opt}");
        }
    }
}
