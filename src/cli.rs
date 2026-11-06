//! gxp CLI 参数定义。
//!
//! **硬约束**（AGENTS.md D1、设计文档 §1.4）：gxp 不接受 grep 短选项
//! （`-A`/`-B`/`-C`/`-e`/`-i`/`-v`/`-o`/`-l`/`-L`/`-c`/`-w`/`-x`/`-n`/`-h`/`-r`/`--color`/`-q`/`-s`）。
//! 用户误写 grep 短选项时，走四段式报错（见 `mode::reject_grep_opts`）并给出 gxp 等价语法提示。
//!
//! v1 专属 flag（设计文档 §2.4 示例 3 + Rust 方案 §11.2）：
//! - `--explain`：输出执行计划，不执行扫描
//! - `--verify`：暴力全扫与优化路径对账
//! - `--strict`：fail-loud 严格模式（字段名未探测到即报错）
//! - `--json`：机器可读输出（与 `--explain` 联动产出结构化 JSON）

use clap::{CommandFactory, Parser};
use clap_complete::{generate, Shell};

/// gxp — grep-aligned query language for LLM agents (NOT a strict superset of grep).
#[derive(Parser, Debug)]
#[command(
    name = "gxp",
    version,
    about = "grep-aligned query language (not a strict superset)",
    before_help = include_str!("cli_help.txt"),
    after_help = include_str!("cli_help_after.txt")
)]
pub struct Cli {
    /// 输出执行计划（L1 必选组 + L2 字段谓词 + 管道动词摘要），不执行扫描。
    #[arg(long = "explain", default_value_t = false)]
    pub explain: bool,

    /// 暴力全扫对账：与优化路径结果 diff，要求 100% 一致（设计文档 §6.4）。
    #[arg(long = "verify", default_value_t = false)]
    pub verify: bool,

    /// 严格模式：字段名未探测到即报错（fail-loud，设计文档 §2.4 示例 5）。
    #[arg(long = "strict", default_value_t = false)]
    pub strict: bool,

    /// 机器可读 JSON 输出（与 `--explain` 联动产出结构化 JSON）。
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,

    /// 文件名 glob 过滤（可多次指定，OR 语义）。
    /// 支持 `*` / `?` / `[abc]` / `{a,b}` / `**` globstar。
    /// 目录递归时按文件名过滤，跳过不匹配的文件（walk 层过滤）。
    #[arg(long = "glob")]
    pub glob: Vec<String>,

    /// 不尊重 .gitignore（默认尊重）。
    /// 默认跳过 .gitignore 中匹配的文件；此 flag 覆盖默认行为。
    #[arg(long = "no-gitignore", default_value_t = false)]
    pub no_gitignore: bool,

    /// 生成 shell completion 脚本（bash/zsh/fish/powershell/elvish）。
    #[arg(long = "generate-completion", value_name = "SHELL")]
    pub generate_completion: Option<String>,

    /// Pattern + 可选 file/dir/stdin。BARE 模式下 pattern 是 ERE，gxp 模式下是 DSL 表达式。
    /// 不传路径或路径为 `-` 时从 stdin 读取。
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

impl Cli {
    /// 入口：模式判定 → 解析 → 执行。
    ///
    /// `trailing_var_arg = true` 导致后置 long flag（`--verify` 等）被吞进 `args`。
    /// 这里从 `args` 尾部提取已知 long flag，合并到 RunOpts，使后置 flag 生效。
    pub fn run(&self) -> crate::Result<()> {
        // --generate-completion <SHELL>：生成 shell completion 脚本后退出
        if let Some(shell_str) = &self.generate_completion {
            let mut cmd = Self::command();
            let shell = parse_shell(shell_str)?;
            generate(shell, &mut cmd, "gxp", &mut std::io::stdout());
            return Ok(());
        }

        let mut opts = RunOpts {
            explain: self.explain,
            verify: self.verify,
            strict: self.strict,
            json: self.json,
            glob: self.glob.clone(),
            respect_gitignore: !self.no_gitignore,
        };
        let cleaned = extract_trailing_flags(&self.args, &mut opts);
        check_unknown_long_flags(&cleaned)?;
        crate::mode::detect_and_dispatch(&cleaned, opts)
    }

    /// 与 `run()` 相同，但输出写到任意 writer（测试用）。
    pub fn run_with_writer<W: std::io::Write>(&self, out: &mut W) -> crate::Result<()> {
        if let Some(shell_str) = &self.generate_completion {
            let mut cmd = Self::command();
            let shell = parse_shell(shell_str)?;
            generate(shell, &mut cmd, "gxp", out);
            return Ok(());
        }

        let mut opts = RunOpts {
            explain: self.explain,
            verify: self.verify,
            strict: self.strict,
            json: self.json,
            glob: self.glob.clone(),
            respect_gitignore: !self.no_gitignore,
        };
        let cleaned = extract_trailing_flags(&self.args, &mut opts);
        check_unknown_long_flags(&cleaned)?;
        crate::mode::detect::run_with_writer(&cleaned, opts, out)
    }
}

/// 检测未知 long flag：以 `--` 开头但不是已知 gxp long flag 的元素。
///
/// `trailing_var_arg = true` + `allow_hyphen_values = true` 让 clap 把
/// `gxp --unknown` 中的 `--unknown` 吞进 `args`，后续当作 pattern 走 BARE
/// 模式从 stdin 读，输出空 exit=0，误以为被接受。
///
/// 此函数在 extract_trailing_flags 之后调用，扫描 cleaned 中以 `--` 开头
/// 的元素，若不是已知 gxp long flag（`--verify`/`--strict`/`--explain`/
/// `--json`/`--no-gitignore`/`--glob`/`--glob=...`/`--help`/`--version`/
/// `--generate-completion`）则报错。
///
/// 仅检查第一个元素：避免误判 DSL pattern 中合法的 `--` 字面（如
/// `gxp 'cost_ms > --strict'`，虽然罕见但理论可能）。
fn check_unknown_long_flags(cleaned: &[String]) -> crate::Result<()> {
    /// 已知 long flag 名（含 `--help`/`--version` 由 clap 直接处理）。
    const KNOWN_LONG_FLAGS: &[&str] = &[
        "--verify",
        "--strict",
        "--explain",
        "--json",
        "--no-gitignore",
        "--glob",
        "--help",
        "--version",
        "--generate-completion",
    ];

    // 仅检查第一个元素：以 `--` 开头但不是已知 flag 则报错
    if let Some(first) = cleaned.first() {
        if first.starts_with("--") {
            // 提取 flag 名（处理 `--glob=...` 形式）
            let flag_name = match first.find('=') {
                Some(idx) => &first[..idx],
                None => first.as_str(),
            };
            if !KNOWN_LONG_FLAGS.contains(&flag_name) {
                return Err(crate::GxpError::Runtime {
                    detail: format!(
                        "未知选项 '{first}'（gxp 不支持此选项）\n\
                         已知选项：--explain / --verify / --strict / --json / --glob <PATTERN> / --no-gitignore\n\
                         示例：gxp 'ERROR' file --explain   |   gxp 'cost_ms>800' file --verify\n\
                         参见 gxp --help"
                    ),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_unknown_long_flags;

    #[test]
    fn test_unknown_long_flag_rejected() {
        let args = vec!["--unknown".to_string()];
        let r = check_unknown_long_flags(&args);
        assert!(r.is_err());
        let err = r.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("未知选项 '--unknown'"));
        assert!(msg.contains("参见 gxp --help"));
    }

    #[test]
    fn test_known_long_flag_accepted() {
        // 已知 flag 不报错（不在此函数职责内判断 flag 是否在正确位置）
        for known in &[
            "--verify",
            "--strict",
            "--explain",
            "--json",
            "--no-gitignore",
            "--glob",
            "--help",
            "--version",
            "--generate-completion",
        ] {
            let args = vec![(*known).to_string()];
            assert!(
                check_unknown_long_flags(&args).is_ok(),
                "should accept {known}"
            );
        }
    }

    #[test]
    fn test_glob_eq_form_accepted() {
        // `--glob=*.log` 形式：flag_name 提取为 `--glob`，应通过
        let args = vec!["--glob=*.log".to_string()];
        assert!(check_unknown_long_flags(&args).is_ok());
    }

    #[test]
    fn test_pattern_arg_no_check() {
        // 非 `--` 开头的 pattern 不触发检查
        let args = vec!["ERROR".to_string(), "/tmp/file".to_string()];
        assert!(check_unknown_long_flags(&args).is_ok());
    }

    #[test]
    fn test_empty_args_no_check() {
        let args: Vec<String> = vec![];
        assert!(check_unknown_long_flags(&args).is_ok());
    }

    #[test]
    fn test_unknown_flag_with_value_rejected() {
        // `--unknown=value` 形式：flag_name 提取为 `--unknown`，应报错
        let args = vec!["--unknown=value".to_string()];
        let r = check_unknown_long_flags(&args);
        assert!(r.is_err());
        let msg = format!("{}", r.unwrap_err());
        // 错误消息显示完整的原始 arg（含 `=value`）
        assert!(msg.contains("--unknown=value"));
    }
}

/// 解析 shell 名称字符串为 `clap_complete::Shell`。
fn parse_shell(s: &str) -> crate::Result<Shell> {
    match s.to_lowercase().as_str() {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        "powershell" | "pwsh" => Ok(Shell::PowerShell),
        "elvish" => Ok(Shell::Elvish),
        other => Err(crate::error::GxpError::Runtime {
            detail: format!(
                "unsupported shell '{other}'. Supported: bash, zsh, fish, powershell, elvish"
            ),
        }),
    }
}

/// 从 `args` 中提取后置 long flag，合并到 `opts`，返回清理后的 args。
///
/// `trailing_var_arg` 让 clap 把 `gxp 'pat' file --verify` 中的 `--verify`
/// 吞进 `args`。此函数扫描 args，把已知 long flag 移出并设置 opts。
///
/// 支持的 flag 形式：
/// - bool flag: `--verify`/`--strict`/`--explain`/`--json`/`--no-gitignore`
/// - 带值 flag: `--glob PATTERN` / `--glob=PATTERN`
fn extract_trailing_flags(args: &[String], opts: &mut RunOpts) -> Vec<String> {
    /// 已知 bool long flag 名 → 对应 RunOpts 字段写入闭包。
    /// 返回 true 表示已消费。
    fn try_bool_flag(arg: &str, opts: &mut RunOpts) -> bool {
        match arg {
            "--verify" => {
                opts.verify = true;
                true
            }
            "--strict" => {
                opts.strict = true;
                true
            }
            "--explain" => {
                opts.explain = true;
                true
            }
            "--json" => {
                opts.json = true;
                true
            }
            "--no-gitignore" => {
                opts.respect_gitignore = false;
                true
            }
            _ => false,
        }
    }

    let mut cleaned: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        // bool flag: --verify / --strict / --explain / --json / --no-gitignore
        if try_bool_flag(arg, opts) {
            i += 1;
            continue;
        }

        // --glob=PATTERN 形式
        if let Some(value) = arg.strip_prefix("--glob=") {
            if !value.is_empty() {
                opts.glob.push(value.to_string());
                i += 1;
                continue;
            }
        }

        // --glob PATTERN 形式（两个 args）
        if arg == "--glob" && i + 1 < args.len() {
            let value = &args[i + 1];
            // 下一个 arg 不以 -- 开头才当作 glob 值
            if !value.starts_with("--") {
                opts.glob.push(value.clone());
                i += 2;
                continue;
            }
        }

        cleaned.push(arg.clone());
        i += 1;
    }
    cleaned
}

/// 运行时选项（从 CLI flag 传入）。
///
/// 单独抽出便于测试直接构造（不经 clap parse）。
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// `--explain`：输出执行计划。
    pub explain: bool,
    /// `--verify`：暴力对账。
    pub verify: bool,
    /// `--strict`：fail-loud 严格模式。
    pub strict: bool,
    /// `--json`：JSON 输出。
    pub json: bool,
    /// `--glob PATTERN`：文件名 glob 过滤（可多次指定，OR 语义）。
    pub glob: Vec<String>,
    /// `--no-gitignore` 的反值：是否尊重 .gitignore（默认 true）。
    pub respect_gitignore: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            explain: false,
            verify: false,
            strict: false,
            json: false,
            glob: Vec::new(),
            respect_gitignore: true,
        }
    }
}
