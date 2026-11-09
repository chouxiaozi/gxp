//! 入口解析：扫描 argv 决定 BARE/gxp 模式（Rust 方案 §7.1）。
//!
//! BARE 模式（设计文档 §2.4 示例 1）：`gxp 'pattern' file`
//!   行为等价 `grep -E 'pattern' file`
//!
//! gxp 模式：含管道动词 `|`、字段谓词 `cost_ms>800`、shell 操作符 `&&`/`||`/`!`
//!
//! 输出格式：`行号:行内容`（gxp 默认带行号，等价 `grep -n`）。
//!
//! # 当前端到端范围
//!
//! - BARE 模式：完整支持（ERE 正则扫描）
//! - gxp 模式纯字面量/布尔查询：L1 命中即输出
//! - gxp 模式含 `Compare` / `In` / `Between` / `Exists` 谓词：L1 粗筛 + L2 精判
//!   - L2 支持 4 种格式（logfmt / JSON / `[tag]` / ISO 时间戳，设计文档 §3.3）
//!   - L2 数值比较按 11 维契约（D5）
//! - gxp 模式含 `Not` 操作符：递归求值 + 三态取反（True→False、False→True、
//!   NotNumeric→True，对齐 §3.3 "字段不存在语义"——`!(cost>800)` 对无字段行视为谓词假再取反→真）
//! - 管道动词（`|`）：L1 扫描收集命中行 → [`crate::pipeline::execute_stages`]
//!   按语义顺序执行（where → context → count/group → rank → head → fields → files）

use std::path::Path;

use crate::cli::RunOpts;
use crate::dsl::ast::{Expr, Prim};
use crate::io::file::{read_for_scan, read_stdin_for_scan, STDIN_PATH};
use crate::l1::scan::{scan_lines_cb, scan_stream_lines_cb};
use crate::l2::EvalResult;
use crate::mode::bare::{run_bare_mode, run_bare_mode_directory, run_bare_mode_with_content};
use crate::mode::eval::eval_prepared_with_captures;
#[cfg(feature = "l3-structural")]
use crate::mode::eval::eval_prepared_with_ctx_and_captures;
#[cfg(feature = "l3-structural")]
use crate::mode::eval::EvalCtx;
use crate::mode::gxp::{run_gxp_mode, run_gxp_mode_directory, run_gxp_mode_with_content};
use crate::mode::prepared::{prepare_expr, PreparedExpr};

/// 判定是否为 gxp 模式（含管道动词 / shell 操作符 / 字段谓词 / STRING 字面量）。
///
/// 规则（设计文档 §1.5、§2.1 方案 D'）：
/// - 含双引号 `"` → gxp 模式（STRING 字面量，BARE 模式不接受双引号）
/// - 含 `&&` / `||` / `!` → gxp 模式（shell 操作符）
/// - 含 `|`（两侧空白边界，区别于 ERE 的 `|`）→ gxp 模式（管道动词）
/// - 含字段谓词特征（`<name>>` / `<name><` / `<name>=` / `<name>!=` /
///   ` exists` / ` in [` / ` between `）→ gxp 模式
/// - 含 `capture(` → gxp 模式（正则捕获组）
/// - 含 `stats ` → gxp 模式（管道动词）
/// - 否则 → BARE 模式（ERE 兼容）
///
/// 注：`|` 的歧义判定见 experience_dsl_pipe_ere_or_vs_pipeline.md——
/// 单 `|` 两侧空白/边界才是管道；紧贴字母是 ERE OR。
pub(crate) fn is_gxp_mode(pattern: &str) -> bool {
    // ast()/capture() 关键字 → gxp 模式（fail-loud，不走 BARE 兜底）
    let trimmed = pattern.trim_start();
    if trimmed.starts_with("ast(") || trimmed.starts_with("ast (") {
        return true;
    }
    if trimmed.starts_with("capture(") || trimmed.starts_with("capture (") {
        return true;
    }
    // STRING 字面量：含双引号（设计文档 §2.1 方案 D'，BARE 不含双引号）
    if pattern.contains('"') {
        return true;
    }
    // shell 操作符
    if pattern.contains("&&") || pattern.contains("||") || pattern.contains('!') {
        return true;
    }
    // 管道动词：`|` 两侧是空白或字符串边界
    if pattern.contains(" | ") || pattern.starts_with("| ") || pattern.ends_with(" |") {
        return true;
    }
    // 字段谓词特征
    if has_field_predicate(pattern) {
        return true;
    }
    false
}

/// 检测字段谓词特征（粗筛，精确判定由 DSL 解析器做）。
///
/// 检测的 token（后面跟字母/数字/空格）：
/// - `<name>>` / `<name><` / `<name>>=` / `<name><=` / `<name>==` / `<name>!=`
/// - ` exists` / ` in [` / ` between `
fn has_field_predicate(pattern: &str) -> bool {
    // 检测比较操作符：字母紧跟操作符
    // 简化：扫描每个字符，看是否是字母后跟 >、<、=、!
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        let next = bytes[i + 1];
        // 字母后跟 >、<、!（可能是字段谓词，如 cost_ms>800）
        if b.is_ascii_alphabetic() && (next == b'>' || next == b'<' || next == b'!') {
            return true;
        }
        // v0.48：`=` 需 RHS 前瞻——越过连续 `=` 与空白后首字符是数字/`-`/`+`/`.`
        // 才判谓词特征（数值等值，D5）；带引号 RHS（字符串等值）由外层双引号规则
        // 先行命中。单 `=` 非数值 RHS（如 `a=b`）是 ERE 字面量（设计文档 §2.2：
        // `=` 不在 BARE 谓词分歧操作符列表），不触发 gxp 模式；`==`（≥2 个连续）
        // 是有意分歧操作符，无条件触发（非数值 RHS 由编译期 D5 拒绝，fail-loud 不变）。
        if b.is_ascii_alphabetic() && next == b'=' {
            let mut j = i + 1;
            let mut eq_count = 0usize;
            while j < bytes.len() && bytes[j] == b'=' {
                eq_count += 1;
                j += 1;
            }
            if eq_count >= 2 {
                return true;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if let Some(&c) = bytes.get(j) {
                if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
                    return true;
                }
            }
            // 非数值 RHS：不是谓词特征，继续扫描（后面可能还有其他特征）
        }
        i += 1;
    }
    // ` exists` / ` in [` / ` between `
    if pattern.contains(" exists") || pattern.contains(" in [") || pattern.contains(" between ") {
        return true;
    }
    // `capture(` 正则捕获组
    if pattern.contains("capture(") {
        return true;
    }
    // `stats ` 管道动词
    if pattern.contains("stats ") || pattern.ends_with(" stats") {
        return true;
    }
    false
}

/// 宽松扫描（v0.48，仅错误文案用）：`"` 前最近的非空白 token 是否为非等值比较
/// 操作符（`!=`/`>`/`>=`/`<`/`<=`）——`level>"ERROR"` / `level!="ERROR"` 类误写。
///
/// 语义拦截在 parser 层（`quoted_wrong_op_shape` fail-loud，REGEX/STRING 已优先
/// 解析不会误伤）；本函数只在解析已失败后决定是否附加 hint，误报仅影响文案。
pub(crate) fn has_quoted_wrong_op(pattern: &str) -> bool {
    let b = pattern.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'"' {
            continue;
        }
        // 向前收集连续的操作符字符（= ! < >）
        let mut j = i;
        while j > 0 && matches!(b[j - 1], b'=' | b'!' | b'>' | b'<') {
            j -= 1;
        }
        let token = &pattern[j..i];
        // 存在操作符 token 且不是纯等值（= / ==）→ 误用形态
        if !token.is_empty() && token != "=" && token != "==" {
            return true;
        }
    }
    false
}

/// 判断 AST 是否含任何字段谓词（`Prim::Pred`）或需要 L2 求值的结构。
///
/// 用于 `run_gxp_mode` 决定是否需要走 L2 精判路径：
/// - 无 L2 触发条件 → L1 命中即输出（字面量/正则已由 L1 必选组保证）
/// - 有 L2 触发条件 → 每行抽字段，递归求值 `PreparedExpr`
///
/// **L2 触发条件**（原仅检测 Pred，遗漏 Not(String/Bare/Regex)）：
/// - `Prim::Pred(_)`：字段谓词，需 L2 数值/集合比较
/// - `Prim::Capture { .. }`：正则捕获组，需 L2 每行正则提取
/// - `Expr::Not(_)`：**Not 操作符必须走 L2**——L1 编译器对 Not 保守不下推
///   字面量（`compile.rs:258` "保守不漏：NOT 不下推字面量"），导致 L1
///   无必选组（match-all）。若 `needs_l2=false` 跳过 L2，`! "ERROR"` 会
///   静默匹配所有行（应为不含 ERROR 的行）。原 bug：`has_any_predicate`
///   对 `Not(String("ERROR"))` 返回 false → needs_l2=false → 漏判。
pub(crate) fn has_any_predicate(expr: &Expr) -> bool {
    match expr {
        Expr::And(l, r) | Expr::Or(l, r) => has_any_predicate(l) || has_any_predicate(r),
        // v0.42 修复：Not(_) 必须走 L2（L1 不下推 Not 内部字面量）
        Expr::Not(_) => true,
        Expr::Prim(Prim::Paren(e)) => has_any_predicate(e),
        Expr::Prim(Prim::Pred(_)) => true,
        // capture() 需要 L2 每行正则提取
        Expr::Prim(Prim::Capture { .. }) => true,
        Expr::Prim(Prim::Ast { .. }) => false,
        Expr::Prim(Prim::String(_) | Prim::Regex(_) | Prim::Bare(_)) => false,
    }
}

/// 判定表达式的字面量语义是否被 L1 必选组「精确编码」（B14）。
///
/// [`scan_and_collect`] 的捷径（L1 命中即收集，跳过 L2 全树求值）只在
/// 此返回 `true` 时安全：
/// - `String`/`Bare` 字面量经 `&&` 拆组（组间 AND）、`||` 合组（组内
///   any-hit）组合时，L1 组与表达式语义完全一致
/// - 其余形态一律 `false`，必须走 L2 全树精判：
///   - `Regex`：L1 不推进字面量（`"a" && /re/` 的正则右支 L1 从不检查）
///   - `Not`：v0.42 起 L1 不下推 Not 内部字面量，L2 承担取反
///   - `Pred`/`Capture`：needs_l2 本就为 true
///   - `Ast`：needs_l3 路径
///   - **Or 混合非字面量分支**：L1 组被整体放弃（B14，见
///     `l1::compiler::collect_or_literals` 完整性规则——Or 分支判定
///     必须镜像该规则，不能递归 [`l1_exact`]：`Or(And(a,b), c)` 中
///     `l1_exact(And)=true`，但该 And 分支根本没进组）
pub(crate) fn l1_exact(expr: &Expr) -> bool {
    match expr {
        Expr::And(l, r) => l1_exact(l) && l1_exact(r),
        // Or 合组只对「两侧均为可进组的字面量组合」精确（or_complete
        // 与 compiler::collect_or_literals 完整性规则同构）
        Expr::Or(l, r) => or_complete(l) && or_complete(r),
        Expr::Not(_) => false,
        Expr::Prim(prim) => prim_literal_complete(prim),
    }
}

/// Or 子树的完整性：每个分支都是推进字面量的 prim（String/Bare，
/// 或括号包裹的同构组合）。与 `l1::compiler::collect_or_literals` 同构。
fn or_complete(expr: &Expr) -> bool {
    match expr {
        Expr::Or(l, r) => or_complete(l) && or_complete(r),
        Expr::Prim(prim) => prim_literal_complete(prim),
        Expr::And(..) | Expr::Not(_) => false,
    }
}

/// prim 是否推进字面量（可进 Or 组）。与
/// `l1::compiler::collect_prim_literals` 的返回值语义同构。
fn prim_literal_complete(prim: &Prim) -> bool {
    match prim {
        Prim::String(_) | Prim::Bare(_) => true,
        Prim::Paren(e) => or_complete(e),
        Prim::Regex(_) | Prim::Pred(_) | Prim::Ast { .. } | Prim::Capture { .. } => false,
    }
}

/// 判断 AST 是否含任何 `ast(NAME, STRING)` 结构模式匹配节点（设计文档 §2.2 + §8.2）。
///
/// 用于 `run_gxp_mode` 决定是否需要走 L3 精判路径：
/// - 无 ast 节点 → L1+L2 命中即输出
/// - 有 ast 节点 → 扫描前对整个文件跑 ast-grep find_all 得到命中行号集合，
///   L2 求值时 `PreparedPrim::Ast` 查此集合
#[cfg(feature = "l3-structural")]
pub(crate) fn has_any_ast(expr: &Expr) -> bool {
    match expr {
        Expr::And(l, r) | Expr::Or(l, r) => has_any_ast(l) || has_any_ast(r),
        Expr::Not(e) => has_any_ast(e),
        Expr::Prim(Prim::Paren(e)) => has_any_ast(e),
        Expr::Prim(Prim::Ast { .. }) => true,
        Expr::Prim(
            Prim::Pred(_) | Prim::String(_) | Prim::Regex(_) | Prim::Bare(_) | Prim::Capture { .. },
        ) => false,
    }
}

/// `has_any_ast` 的 no-feature 镜像版本。
///
/// 默认 build (`default = []`) 不含 `l3-structural` feature，
/// `has_any_ast` 被 cfg-gated 不存在。但用户仍可输入 `ast(LANG, PATTERN)`——
/// 解析器会生成 `Prim::Ast` 节点，L1 视为无字面量约束 → match-all，L2 视 Ast
/// 节点为 `EvalResult::True` → **静默输出整个文件每一行**（数据正确性灾难）。
///
/// 此函数在 `PreparedScan::build` 的 no-feature 分支被调用，检测到 `Prim::Ast`
/// 节点时 fail-loud 报错，引导用户用 `--features l3-structural` 重建。
///
/// 与 `has_any_ast` 逻辑完全一致（仅 cfg gate 不同），DRY 无法消除——
/// cfg-gated 函数对其他 cfg 分支不可见。
#[cfg(not(feature = "l3-structural"))]
pub(crate) fn has_any_ast_nofeature(expr: &Expr) -> bool {
    match expr {
        Expr::And(l, r) | Expr::Or(l, r) => has_any_ast_nofeature(l) || has_any_ast_nofeature(r),
        Expr::Not(e) => has_any_ast_nofeature(e),
        Expr::Prim(Prim::Paren(e)) => has_any_ast_nofeature(e),
        Expr::Prim(Prim::Ast { .. }) => true,
        Expr::Prim(
            Prim::Pred(_) | Prim::String(_) | Prim::Regex(_) | Prim::Bare(_) | Prim::Capture { .. },
        ) => false,
    }
}

/// DRY helper: BARE mode --explain/--verify rejection.
fn reject_bare_explain_verify() -> crate::GxpError {
    crate::GxpError::Runtime {
        detail: "--explain/--verify 仅在 gxp 模式可用；当前 pattern 是 BARE 模式（ERE 兼容，无优化层）\n\
                 触发 gxp 模式：在 pattern 中加 `|` 管道 / `&&`/`||`/`!` 操作符 / 字段谓词（如 cost_ms>800）\n\
                 示例：gxp 'ERROR | count' file   |   gxp 'cost_ms>800' file   |   参见 gxp --help"
            .to_string(),
    }
}

/// L2 求值所需上下文（DRY：统一封装，消除 3 处 cfg 分支复制）。
///
/// 一次 `PreparedScan::build` 取代每个调用点手写的 `prepare_expr` + `has_any_predicate`
/// + L3 预计算 + `needs_l2`/`needs_l3` 赋值 + `EvalCtx` 构造（原共 ~40 行/处 × 3 处）。
///
/// 字段语义：
/// - `prepared`：AST 编译后的 `PreparedExpr`（阈值/边界已预解析为 f64）
/// - `needs_l2`：是否含字段谓词（`Compare`/`In`/`Between`/`Exists`），决定是否抽字段求值
/// - `needs_l3`：是否含 `ast()` 节点（feature `l3-structural` 开启时由 `prepare_l3` 判定）
/// - `ast_lines`：L3 ast() 预计算行号集合（`needs_l3 == false` 时为 `None`）
pub(crate) struct PreparedScan {
    /// 预编译表达式树（`prepare_expr` 产物）。
    pub prepared: PreparedExpr,
    /// 是否含字段谓词（`has_any_predicate` 产物）。
    pub needs_l2: bool,
    /// 字面量语义是否被 L1 必选组精确编码（`l1_exact` 产物，B14）。
    /// `scan_and_collect` 捷径（L1 命中即收集）仅在此为 true 时启用。
    pub l1_exact: bool,
    /// 是否含 ast() 节点（feature 关闭时恒 false）。
    pub needs_l3: bool,
    /// L3 ast() 预计算行号集合（`needs_l3 == false` 时为 `None`）。
    #[cfg(feature = "l3-structural")]
    pub ast_lines: Option<crate::l3::structural::AstLineSet>,
}

impl PreparedScan {
    /// 一次性构建 L2 求值上下文：`prepare_expr` + `has_any_predicate` + L3 预计算。
    ///
    /// `verified`：`CompiledL1::verified_literals`（v0.49 Opt A）——L1 已构造性
    /// 保证存在的字面量，L2 豁免重校验。调用方（gxp 模式 / verify）均持有
    /// 同一 `compiled`，传 `&compiled.verified_literals`。
    ///
    /// 取代原 3 处复制粘贴的 cfg block（`run_gxp_mode_with_content` /
    /// `run_gxp_mode_directory` / `run_verify`）。
    pub(crate) fn build(
        expr: &Expr,
        bytes: &[u8],
        file_ext: &str,
        verified: &[Vec<u8>],
    ) -> crate::Result<Self> {
        let prepared = prepare_expr(expr, verified)?;
        let needs_l2 = has_any_predicate(expr);
        let l1_exact = l1_exact(expr);

        #[cfg(feature = "l3-structural")]
        {
            let ast_lines = prepare_l3(expr, &prepared, bytes, file_ext)?;
            let needs_l3 = ast_lines.is_some();
            Ok(Self {
                prepared,
                needs_l2,
                l1_exact,
                needs_l3,
                ast_lines,
            })
        }
        #[cfg(not(feature = "l3-structural"))]
        {
            let _ = (bytes, file_ext);
            // 默认 build (default=[]) 不含 l3-structural feature，
            // 但用户仍可输入 ast(LANG, PATTERN)。原行为是 L1 无字面量约束 → match-all，
            // L2 视 Ast 节点为 True → 静默输出整个文件每一行。
            // 修复：fail-loud 报错，引导用户用 --features l3-structural 重建。
            if has_any_ast_nofeature(expr) {
                return Err(crate::GxpError::Runtime {
                    detail: "查询含 ast(LANG, PATTERN) 结构模式匹配节点，但当前 gxp 二进制\
                         未编译 l3-structural feature。\n\
                         重建：cargo install --path . --force --features l3-structural\n\
                         或：cargo build --release --features l3-structural\n\
                         参见 AGENTS.md §2 D11 + 设计文档 §8.2 ast() 支持矩阵"
                        .to_string(),
                });
            }
            Ok(Self {
                prepared,
                needs_l2,
                l1_exact,
                needs_l3: false,
            })
        }
    }

    /// L2 求值上下文视图（feature `l3-structural` 开启时编译）。
    ///
    /// 从 `ast_lines` 构造 `EvalCtx`，供 `eval_line` 使用。
    #[cfg(feature = "l3-structural")]
    pub(crate) fn ctx(&self) -> EvalCtx<'_> {
        EvalCtx {
            ast_lines: self.ast_lines.as_ref(),
        }
    }

    /// 单行求值：`needs_l3` 分发到 `eval_prepared_with_ctx_and_captures` 或 `eval_prepared_with_captures`。
    ///
    /// 取代原 4 处调用点的 `#[cfg]` 分支（`run_gxp_mode_with_content` /
    /// `run_gxp_mode_directory` / `run_verify` 暴力+优化路径）。
    ///
    /// 原 `eval` / `is_hit`（无 capture 提取）被移除，
    /// `capture(X) && X>Y` 需 capture 提取供谓词使用，全部改走
    /// `eval_with_captures` / `is_hit_with_captures`。
    #[inline]
    pub(crate) fn eval_with_captures(
        &self,
        line: &[u8],
        line_no: usize,
        captures: &mut crate::pipeline::CaptureMap,
    ) -> EvalResult {
        #[cfg(feature = "l3-structural")]
        {
            if self.needs_l3 {
                eval_prepared_with_ctx_and_captures(
                    &self.prepared,
                    line,
                    line_no,
                    &self.ctx(),
                    captures,
                )
            } else {
                eval_prepared_with_captures(&self.prepared, line, captures)
            }
        }
        #[cfg(not(feature = "l3-structural"))]
        {
            let _ = line_no;
            eval_prepared_with_captures(&self.prepared, line, captures)
        }
    }

    /// 是否命中 + capture 提取：`eval_with_captures` 结果为 `True` 时收集 captures。
    #[inline]
    pub(crate) fn is_hit_with_captures(
        &self,
        line: &[u8],
        line_no: usize,
        captures: &mut crate::pipeline::CaptureMap,
    ) -> bool {
        matches!(
            self.eval_with_captures(line, line_no, captures),
            EvalResult::True
        )
    }

    /// 每文件构建：复用顶层已算好的 `prepared`/`needs_l2`/`needs_l3`，仅重算 `ast_lines`。
    ///
    /// 用于目录模式逐文件扫描——`prepared`/`needs_l2`/`needs_l3` 是查询级常量
    ///（跨文件不变），但 `ast_lines` 必须每文件单独跑 ast-grep find_all。
    pub(crate) fn build_per_file(
        expr: &Expr,
        scan: &PreparedScan,
        bytes: &[u8],
        file_ext: &str,
    ) -> crate::Result<Self> {
        #[cfg(feature = "l3-structural")]
        {
            let ast_lines = prepare_l3(expr, &scan.prepared, bytes, file_ext)?;
            Ok(Self {
                prepared: scan.prepared.clone(),
                needs_l2: scan.needs_l2,
                l1_exact: scan.l1_exact,
                needs_l3: scan.needs_l3,
                ast_lines,
            })
        }
        #[cfg(not(feature = "l3-structural"))]
        {
            let _ = (expr, bytes, file_ext);
            Ok(Self {
                prepared: scan.prepared.clone(),
                needs_l2: scan.needs_l2,
                l1_exact: scan.l1_exact,
                needs_l3: false,
            })
        }
    }
}

/// DRY helper: L3 ast() pre-computation.
///
/// 二进制文件（非 UTF-8）返回空集合而非报错——二进制文件不是 ast() 的有效输入，
/// 与 grep 对二进制文件的行为一致（跳过，不报错）。
/// 其他 L3 错误（pattern 解析失败、不支持语言）仍 fail-loud。
///
/// **扩展名过滤**：`file_ext` 为文件扩展名（不含 `.`，如 `"ts"`）。
/// 若查询中所有 ast() 语言都不匹配该扩展名 → 跳过 ast-grep 解析，返回空集合。
/// `file_ext` 为空字符串时不过滤（单文件模式 / stdin / 顶层 build 无路径信息）。
#[cfg(feature = "l3-structural")]
fn prepare_l3(
    expr: &Expr,
    prepared: &PreparedExpr,
    bytes: &[u8],
    file_ext: &str,
) -> crate::Result<Option<crate::l3::structural::AstLineSet>> {
    if has_any_ast(expr) {
        // 扩展名过滤：非目标语言文件跳过 ast-grep 解析
        if !file_ext.is_empty()
            && !crate::l3::structural::expr_has_matching_ast_lang(prepared, file_ext)
        {
            return Ok(Some(crate::l3::structural::AstLineSet::new()));
        }
        match crate::l3::structural::run_all_ast(prepared, bytes) {
            Ok(set) => Ok(Some(set)),
            Err(crate::l3::structural::L3Error::InvalidUtf8(_)) => {
                // 二进制文件：空 L3 命中集合，不报错
                Ok(Some(crate::l3::structural::AstLineSet::new()))
            }
            Err(e) => Err(crate::GxpError::Runtime {
                detail: format!("L3 ast-grep error: {e}"),
            }),
        }
    } else {
        Ok(None)
    }
}

/// DRY helper: L1 scan + L2 eval → `Vec<MatchedLine>`。
///
/// 取代 4 处 `scan_lines_cb` + 回调内 `eval_line` + `cfg` 分支的复制粘贴：
/// `run_gxp_mode_with_content` / `run_gxp_mode_directory` / `run_verify` 暴力+优化路径。
///
/// 语义（B14 修复后）：捷径（L1 命中即收集，跳过 L2）只在 `scan.l1_exact`
/// 时启用——即表达式是纯 String/Bare 字面量经 `&&`/`||` 组合、L1 必选组
/// 与语义完全一致（And 拆组组间 AND、Or 合组组内 any-hit）。
/// 原条件 `!needs_l2 && !needs_l3` 不健全：`needs_l2` 只看字段谓词，
/// Regex / 混合 Or 的字面量语义 L1 并未编码（`"a" && /re/` 的正则右支
/// 从不检查、`"a" && "b" || "c"` 的 And 分支被部分组误杀）。
/// 非 `l1_exact` 时每行调 `scan.is_hit_with_captures` 做 L2 全树精判 + capture 提取。
///
/// capture 提取的值写入 `MatchedLine::captures`，供 `fields` 投影使用。
pub(crate) fn scan_and_collect(
    compiled: &crate::l1::compiler::CompiledL1,
    scan: &PreparedScan,
    bytes: &[u8],
) -> Vec<crate::pipeline::MatchedLine> {
    let mut matched: Vec<crate::pipeline::MatchedLine> = Vec::new();
    scan_lines_cb(compiled, bytes, |line_no, line| {
        if scan.l1_exact {
            let mut m = crate::pipeline::MatchedLine::new(line_no, line);
            m.match_bytes = find_match_bytes(compiled, line);
            matched.push(m);
            return true;
        }
        let mut captures = crate::pipeline::CaptureMap::new();
        if scan.is_hit_with_captures(line, line_no, &mut captures) {
            let mut m = crate::pipeline::MatchedLine::with_captures(line_no, line, captures);
            m.match_bytes = find_match_bytes(compiled, line);
            matched.push(m);
        }
        true
    });
    matched
}

/// 流式扫描收集：文件 > 512MB 时使用 `scan_stream_lines_cb` 逐行扫描。
///
/// 流式 fallback：与 `scan_and_collect` 语义完全一致，但从文件
/// `BufReader` 逐行读取而非全量 `read_to_end`。命中行同样收集到
/// `Vec<MatchedLine>`，供 `execute_stages` 消费。
///
/// 不变式：`L1(粗筛) ⊇ L2(精判)`——与 `scan_and_collect` 相同。
///
/// **L3 限制**：流式模式不支持 `ast(NAME, PATTERN)` L3 结构查询。
/// ast-grep 需要完整文件内容做 AST 解析，流式逐行无法提供。
/// 若查询含 `ast()` 节点且文件 > 512MB，返回错误。
pub(crate) fn scan_and_collect_stream(
    compiled: &crate::l1::compiler::CompiledL1,
    scan: &PreparedScan,
    path: &Path,
) -> crate::Result<Vec<crate::pipeline::MatchedLine>> {
    if scan.needs_l3 {
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "文件 {} 超过 512MB 流式阈值，不支持 L3 ast() 结构查询。\
                 请减小文件或去掉 ast() 查询。",
                path.display()
            ),
        });
    }

    let mut matched: Vec<crate::pipeline::MatchedLine> = Vec::new();
    scan_stream_lines_cb(compiled, path, |line_no, line| {
        // B14：与 scan_and_collect 同款捷径门槛——仅 l1_exact（纯字面量
        // 且 L1 组精确编码语义）时 L1 命中即收集；Regex/混合 Or 走 L2 全树。
        if scan.l1_exact {
            let mut m = crate::pipeline::MatchedLine::new(line_no, line);
            m.match_bytes = find_match_bytes(compiled, line);
            matched.push(m);
            return true;
        }
        let mut captures = crate::pipeline::CaptureMap::new();
        if scan.is_hit_with_captures(line, line_no, &mut captures) {
            let mut m = crate::pipeline::MatchedLine::with_captures(line_no, line, captures);
            m.match_bytes = find_match_bytes(compiled, line);
            matched.push(m);
        }
        true
    })
    .map_err(|e| crate::GxpError::Runtime {
        detail: format!("流式扫描失败: {e}"),
    })?;
    Ok(matched)
}

/// 查找 L1 匹配的字节内容（用于伪字段 `_match`，等价 `grep -o`）。
///
/// 从第一个必选字面量组提取第一个命中的字面量。
/// - 单元素纯字面量组（v0.49 Opt B 快速路径）：L1 幸存行必含该字面量，
///   直接 Arc clone 免 aho 二次查找 + 每行 Vec 分配（~3.6ms/44MB 实测）。
/// - 多元素纯字面量组：aho-corasick `find` 返回的字面量（需鉴别命中者）。
/// - 含 Regex 组：`verify.find` 返回的正则匹配字节。
/// - 无必选组（纯字段谓词）：返回 None（无字面量可提取）。
fn find_match_bytes(
    compiled: &crate::l1::compiler::CompiledL1,
    line: &[u8],
) -> Option<std::sync::Arc<[u8]>> {
    let grp = compiled.required_groups.first()?;
    // Opt B 快速路径：单元素纯字面量组命中即确定（组 any-hit ≡ 该字面量存在）
    if grp.is_pure_literal && grp.literals.len() == 1 {
        return Some(std::sync::Arc::clone(&grp.literals[0]));
    }
    // 优先用 aho-corasick find（获取命中的字面量 pattern）
    if let Some(m) = grp.fast.find(line) {
        // 纯字面量组：返回命中的字面量字节
        if grp.is_pure_literal {
            let pat_idx = m.pattern().as_usize();
            if let Some(lit) = grp.literals.get(pat_idx) {
                return Some(std::sync::Arc::clone(lit));
            }
        }
        // 含 Regex 组：用 verify.find 获取正则匹配的字节
        if let Some(regex_match) = grp.verify.find(line) {
            return Some(std::sync::Arc::from(regex_match.as_bytes()));
        }
    }
    None
}

///
/// argv 约定（支持 stdin）：
/// - `gxp <pattern> <file>` — 单文件
/// - `gxp <pattern> <dir>` — 目录递归
/// - `gxp <pattern> -` — 显式 stdin
/// - `gxp <pattern>` — 隐式 stdin（无文件参数）
/// - 不接受 grep 短选项（`reject_grep_opts::check` 拒绝）
/// - pattern 含管道/操作符/字段谓词 → gxp 模式；否则 BARE 模式
pub fn detect_and_dispatch(args: &[String], opts: RunOpts) -> crate::Result<()> {
    let stdout = std::io::stdout();
    run_with_writer(args, opts, &mut stdout.lock())
}

/// 与 `detect_and_dispatch` 相同，但输出写到任意 writer（测试用）。
///
/// stdin 支持——`gxp 'pattern' -`（显式）或 `gxp 'pattern'`（隐式无文件参数）。
pub fn run_with_writer<W: std::io::Write>(
    args: &[String],
    opts: RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    // 1. 拒 grep 短选项
    super::reject_grep_opts::check(args)?;

    // 2. 提取位置参数（pattern + 可选 file/dir/stdin）
    let positional: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    if positional.is_empty() {
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "用法: gxp <pattern> <file|dir|->\n收到 {} 个参数: {:?}",
                positional.len(),
                positional
            ),
        });
    }
    let pattern = positional[0];

    // 无文件参数 → 隐式 stdin
    if positional.len() == 1 {
        return dispatch_stdin(pattern, opts, out);
    }

    // stdin 与 file/dir 互斥检测（设计文档 §1.5 入口解析）
    // positional 中出现 "-" 与其他路径参数同时出现 → 报错
    // 先于多文件 fail-loud：`gxp 'pat' - f2` 是互斥违规，更具体的错误优先
    if positional[1..].contains(&"-") && positional[1..].iter().any(|s| *s != "-") {
        return Err(crate::GxpError::Runtime {
            detail: "stdin 与 file/dir 参数互斥\n收到 stdin 标记 '-' 与其他路径参数同时出现\n\
                     请使用单一输入源：gxp 'pattern' dir/ 或 gxp 'pattern' - （stdin）"
                .to_string(),
        });
    }

    // 多文件参数 → fail-loud（静默只扫第一个文件是 bug）
    //
    // 设计文档模型（§2.4 示例 1/8/9）：单文件 / 目录递归 / stdin。
    // 多文件聚合未实现（需 MatchedLine 跨文件 path 字段 + 单文件模式输出格式
    // 决策——是否加 `path:` 前缀对齐 grep 多文件行为）。当前静默只扫
    // positional[1] 是数据丢失 bug，违反"诚实 > 承诺"。
    //
    // fail-loud 而非静默兜底：让用户明确知道行为，引导到已支持的等价路径。
    if positional.len() > 2 {
        let extra: Vec<&str> = positional[2..].to_vec();
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "多文件参数暂不支持（收到 {} 个文件参数，多余被忽略：[{}])\n\
                 gxp 当前模型：单文件 / 目录递归 / stdin\n\
                 多文件扫描请用以下等价路径：\n\
                 - 目录递归：gxp '{pattern}' <dir>/\n\
                 - glob 过滤：gxp '{pattern}' <dir>/ --glob '*.log'\n\
                 - stdin 管道：cat f1 f2 | gxp '{pattern}' -\n\
                 参见 gxp --help",
                extra.len(),
                extra.join(", "),
            ),
        });
    }

    let file_path = positional[1];

    // 显式 stdin（`-`）→ stdin 分支
    if file_path == "-" {
        return dispatch_stdin(pattern, opts, out);
    }

    // 3. 判定输入类型：目录 → 目录分支；文件 → 单文件分支
    let path = Path::new(file_path);
    let is_dir = match std::fs::metadata(path) {
        Ok(m) => m.is_dir(),
        // 文件不存在或无权限：让单文件分支的 read_for_scan 报具体 IO 错误
        Err(_) => false,
    };

    if is_dir {
        // 编译 glob patterns（fail-loud：语法错误立即报错）
        if !opts.glob.is_empty() {
            crate::dir::plan::compile_globset(&opts.glob)?;
        }
        // 目录模式：--explain/--verify 在 BARE 模式下仍不适用
        if !is_gxp_mode(pattern) {
            if opts.explain || opts.verify {
                return Err(reject_bare_explain_verify());
            }
            return run_bare_mode_directory(pattern, path, opts, out);
        }
        run_gxp_mode_directory(pattern, path, opts, out)
    } else {
        // 单文件模式
        if is_gxp_mode(pattern) {
            run_gxp_mode(pattern, file_path, opts, out)
        } else {
            if opts.explain || opts.verify {
                return Err(reject_bare_explain_verify());
            }
            run_bare_mode(pattern, file_path, &opts, out)
        }
    }
}

/// 统一读取输入：文件路径 → `read_for_scan`，`<stdin>` → `read_stdin_for_scan`。
pub(crate) fn read_input(file_path: &str) -> std::io::Result<crate::io::file::FileContent> {
    if file_path == STDIN_PATH {
        read_stdin_for_scan()
    } else {
        read_for_scan(Path::new(file_path))
    }
}

/// 从文件路径提取扩展名（不含 `.`，小写）。用于 L3 扩展名过滤。
///
/// `"src/main.ts"` → `"ts"`；`"app.config.ts"` → `"ts"`；
/// 无扩展名 / stdin / 空路径 / 前导点文件（`.tmpXXXXXX`）→ `""`（不过滤）。
///
/// **前导点处理**：`.tmpXXXXXX`（tempfile 默认文件名）的 `.` 在位置 0，
/// 是隐藏文件前缀而非扩展名分隔符 → 返回 `""`（不过滤，兼容测试）。
pub(crate) fn ext_from_path(path: &str) -> String {
    // 取最后一个路径分隔符后的 basename，再取最后一个 `.` 后的扩展名
    let base = path.rsplit(['/', '\\']).next().unwrap_or("");
    match base.rfind('.') {
        // 跳过位置 0 的前导点（隐藏文件如 .tmpXXXXXX / .gitignore）
        Some(i) if i > 0 && i < base.len() - 1 => base[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// stdin 分支：读 stdin → BARE 或 gxp 模式执行。
///
/// `--explain` 显示 `<stdin>` 作为输入源。
/// `--verify` 支持：stdin 全量读入 Vec 后可多次扫描（D10 `read_to_end` 模型）。
/// `files` stage 输出 `<stdin>`。
fn dispatch_stdin<W: std::io::Write>(
    pattern: &str,
    opts: RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let content = read_stdin_for_scan().map_err(|e| {
        // 包装 io::Error 加 <stdin> 上下文（与文件路径错误一致）
        crate::GxpError::Runtime {
            detail: format!("读取 '<stdin>' 失败: {e}"),
        }
    })?;
    dispatch_with_content(pattern, STDIN_PATH, opts, content, out)
}

/// 用已读取的 `FileContent` 执行查询（核心逻辑，被 `dispatch_stdin` 和测试复用）。
///
/// `file_path` 仅为显示用途（`<stdin>` 或文件路径），不再用于读取。
pub fn dispatch_with_content<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    opts: RunOpts,
    content: crate::io::file::FileContent,
    out: &mut W,
) -> crate::Result<()> {
    if is_gxp_mode(pattern) {
        run_gxp_mode_with_content(pattern, file_path, opts, content, out)
    } else {
        if opts.explain || opts.verify {
            return Err(reject_bare_explain_verify());
        }
        run_bare_mode_with_content(pattern, file_path, content, &opts, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::ast::{CmpOp, Pred};

    #[test]
    fn test_is_gxp_mode_bare_simple() {
        assert!(!is_gxp_mode("ERROR"));
        assert!(!is_gxp_mode("ERROR timeout"));
        assert!(!is_gxp_mode("a|b")); // ERE OR，紧贴字母
    }

    #[test]
    fn test_is_gxp_mode_string_literal() {
        // 含双引号 → gxp 模式（STRING 字面量）
        assert!(is_gxp_mode(r#""error""#));
        assert!(is_gxp_mode(r#""error" && "timeout""#));
    }

    #[test]
    fn test_is_gxp_mode_shell_operators() {
        assert!(is_gxp_mode(r#""error" && "timeout""#));
        assert!(is_gxp_mode(r#""error" || "timeout""#));
        assert!(is_gxp_mode(r#"!"error""#));
    }

    #[test]
    fn test_is_gxp_mode_eq_predicate_lookahead_v048() {
        // `=` 数值 RHS → 谓词特征（gxp 模式），v0.47 行为保留
        assert!(is_gxp_mode("cost_ms=800"));
        assert!(is_gxp_mode("cost_ms= 800"));
        assert!(is_gxp_mode("cost_ms=-5"));
        assert!(is_gxp_mode("cost_ms=.5"));
        // `=` 单等号非数值 RHS → ERE 字面量，BARE 模式（v0.48 对齐设计文档）
        assert!(!is_gxp_mode("a=b"));
        assert!(!is_gxp_mode("level=ERROR"));
        // `==` 有意分歧操作符 → 仍 gxp 模式（非数值 RHS 编译期 D5 拒绝）
        assert!(is_gxp_mode("a==b"));
        // 字符串等值：双引号规则命中（不受本前瞻影响）
        assert!(is_gxp_mode(r#"level="ERROR""#));
    }

    #[test]
    fn test_has_quoted_wrong_op_scan() {
        // 误用形态：非等值操作符 + 引号 RHS
        assert!(has_quoted_wrong_op(r#"level>"ERROR""#));
        assert!(has_quoted_wrong_op(r#"level!="ERROR""#));
        assert!(has_quoted_wrong_op(r#"cost_ms>="800""#));
        // 等值操作符带引号 = 合法脱糖，不误报
        assert!(!has_quoted_wrong_op(r#"level="ERROR""#));
        assert!(!has_quoted_wrong_op(r#"level=="ERROR""#));
        // 无操作符邻接的引号（字面量并列 / ast 模式）
        assert!(!has_quoted_wrong_op(r#""a" && "b""#));
        assert!(!has_quoted_wrong_op(r#"ast(ts, "useQuery($A)")"#));
    }

    #[test]
    fn test_is_gxp_mode_pipeline() {
        assert!(is_gxp_mode("ERROR | context 3"));
        assert!(is_gxp_mode("ERROR | count"));
    }

    #[test]
    fn test_is_gxp_mode_field_predicate() {
        assert!(is_gxp_mode("cost_ms>800"));
        assert!(is_gxp_mode("user_id exists"));
        assert!(is_gxp_mode("tag in [a, b]"));
        assert!(is_gxp_mode("cost between 100 and 200"));
    }

    // ast()/capture() 关键字 → gxp 模式（fail-loud，不走 BARE 兜底）

    #[test]
    fn test_is_gxp_mode_ast_keyword() {
        // ast( 开头 → gxp 模式
        assert!(is_gxp_mode("ast(java, \"pattern\")"));
        assert!(is_gxp_mode("ast(c++, \"pattern\")"));
        assert!(is_gxp_mode("ast(Java, \"pattern\")"));
        // ast ( 开头（带空格）→ gxp 模式
        assert!(is_gxp_mode("ast (java, \"pattern\")"));
    }

    #[test]
    fn test_is_gxp_mode_ast_not_triggered_by_prefix() {
        // asthma / astute 不以 ast( 开头 → BARE 模式
        assert!(!is_gxp_mode("asthma"));
        assert!(!is_gxp_mode("astute"));
        assert!(!is_gxp_mode("ast "));
    }

    #[test]
    fn test_is_gxp_mode_capture_keyword() {
        // capture( 开头 → gxp 模式
        assert!(is_gxp_mode("capture(cost_ms, /(\\d+)/)"));
        assert!(is_gxp_mode("capture (cost_ms, /(\\d+)/)"));
    }

    // --- has_any_predicate ---

    #[test]
    fn test_has_any_predicate_pure_literal_false() {
        let expr = Expr::Prim(Prim::String("error".to_string()));
        assert!(!has_any_predicate(&expr));
    }

    #[test]
    fn test_has_any_predicate_compare_true() {
        let expr = Expr::Prim(Prim::Pred(Pred::Compare {
            name: "cost_ms".to_string(),
            op: CmpOp::Gt,
            value: "800".to_string(),
        }));
        assert!(has_any_predicate(&expr));
    }

    #[test]
    fn test_has_any_predicate_not_wrapping_compare_true() {
        // Not 内部的 Compare 也要识别（L1 粗筛字段前缀仍要下推）
        let expr = Expr::Not(Box::new(Expr::Prim(Prim::Pred(Pred::Compare {
            name: "cost_ms".to_string(),
            op: CmpOp::Gt,
            value: "800".to_string(),
        }))));
        assert!(has_any_predicate(&expr));
    }

    // Not(String/Bare/Regex) 必须返回 true（L1 不下推 Not 字面量）
    #[test]
    fn test_has_any_predicate_not_wrapping_string_true() {
        // ! "ERROR" → L1 无必选组（Not 保守不下推）→ 需要 L2 实际校验
        let expr = Expr::Not(Box::new(Expr::Prim(Prim::String("ERROR".to_string()))));
        assert!(has_any_predicate(&expr), "Not(String) 必须返回 true");
    }

    #[test]
    fn test_has_any_predicate_not_wrapping_bare_true() {
        // ! ERROR → 同 Not(String)（BARE 词在 gxp 模式下被识别为 Prim::Bare）
        let expr = Expr::Not(Box::new(Expr::Prim(Prim::Bare("ERROR".to_string()))));
        assert!(has_any_predicate(&expr), "Not(Bare) 必须返回 true");
    }

    #[test]
    fn test_has_any_predicate_not_wrapping_regex_true() {
        // ! /ERROR/ → 同 Not(String)
        let expr = Expr::Not(Box::new(Expr::Prim(Prim::Regex("ERROR".to_string()))));
        assert!(has_any_predicate(&expr), "Not(Regex) 必须返回 true");
    }

    #[test]
    fn test_has_any_predicate_in_true() {
        let expr = Expr::Prim(Prim::Pred(Pred::In {
            name: "tag".to_string(),
            values: vec!["a".to_string()],
        }));
        assert!(has_any_predicate(&expr));
    }

    #[test]
    fn test_has_any_predicate_between_true() {
        let expr = Expr::Prim(Prim::Pred(Pred::Between {
            name: "cost".to_string(),
            lo: "100".to_string(),
            hi: "200".to_string(),
        }));
        assert!(has_any_predicate(&expr));
    }

    #[test]
    fn test_has_any_predicate_exists_true() {
        let expr = Expr::Prim(Prim::Pred(Pred::Exists {
            name: "user_id".to_string(),
        }));
        assert!(has_any_predicate(&expr));
    }

    #[test]
    fn test_has_any_predicate_and_with_one_pred_true() {
        // "timeout" && cost_ms>800：一边字面量一边谓词
        let expr = Expr::And(
            Box::new(Expr::Prim(Prim::String("timeout".to_string()))),
            Box::new(Expr::Prim(Prim::Pred(Pred::Compare {
                name: "cost_ms".to_string(),
                op: CmpOp::Gt,
                value: "800".to_string(),
            }))),
        );
        assert!(has_any_predicate(&expr));
    }

    // --- B14：l1_exact 判定 + Or(And, Prim) 全链不得 match-all ---

    #[test]
    fn test_l1_exact_pure_literals_true() {
        // "a" && "b" / "a" || "b" / ("a" || "b") && "c"：L1 组精确编码
        for q in [
            r#""trace" && "json""#,
            r#""json" || "stdin""#,
            r#""trace" && ("json" || "stdin")"#,
            r#"("json" || "stdin") && "trace""#,
            r#""a" || "b" || "c""#,
        ] {
            let expr = crate::dsl::parser::parse_query(q).unwrap().expr;
            assert!(l1_exact(&expr), "应为 l1_exact: {q}");
        }
    }

    #[test]
    fn test_l1_exact_regex_not_or_mixed_false() {
        // Regex / 混合 Or / Not / 谓词：L1 组不完整，必须走 L2
        for q in [
            r#""json" && /onl/"#,
            r#""json" || /neith/"#,
            r#""trace" && "json" || "stdin""#,
            r#""json" || "trace" && "stdin""#,
            r#"("trace" && "json") || "stdin""#,
            r#"!("trace")"#,
            r#"cost_ms>800"#,
        ] {
            let expr = crate::dsl::parser::parse_query(q).unwrap().expr;
            assert!(!l1_exact(&expr), "不应为 l1_exact: {q}");
        }
    }

    #[test]
    fn test_b14_scan_and_collect_mixed_and_or() {
        // B14 核心回归：Or(And("trace","json"), "stdin") 不得 match-all
        let expr = crate::dsl::parser::parse_query(r#""trace" && "json" || "stdin""#)
            .unwrap()
            .expr;
        assert!(!l1_exact(&expr), "混合 Or 必须 l1_exact=false");
        let compiled = crate::l1::compiler::compile(&expr).unwrap();
        let scan = PreparedScan::build(&expr, &[], "", &[]).unwrap();
        let bytes = b"trace json\ntrace stdin\nneither\njson only\nstdin only\n";
        let matched = scan_and_collect(&compiled, &scan, bytes);
        let line_nos: Vec<usize> = matched.iter().map(|m| m.line_no).collect();
        assert_eq!(line_nos, vec![1, 2, 5], "期望 (t∧j)∨s = 行 1/2/5");
    }

    #[test]
    fn test_b14_scan_and_collect_regex_in_bool() {
        // B14 Regex 家族回归：正则右支必须被 L2 检查
        let expr = crate::dsl::parser::parse_query(r#""json" && /onl/"#)
            .unwrap()
            .expr;
        let compiled = crate::l1::compiler::compile(&expr).unwrap();
        let scan = PreparedScan::build(&expr, &[], "", &[]).unwrap();
        let bytes = b"trace json\njson only\n";
        let matched = scan_and_collect(&compiled, &scan, bytes);
        assert_eq!(matched.len(), 1, "只有 json only 同时满足字面量与正则");
        assert_eq!(matched[0].line_no, 2);
    }

    // --- V1: PreparedScan Send+Sync 编译断言 ---
    //
    // 路线图 §11.2 V1 要求验证 PreparedScan（含 ast-grep 产物）是否 Send+Sync，
    // 这是 rayon 并行跨文件扫描的前提。
    //
    // 交叉验证结论：
    // 1. gxp BARE 搜 `AstGrep` → 只在 src/l3/structural.rs，从不在 detect.rs
    // 2. grep -E 对账一致
    // 3. Read 源码：PreparedScan 字段 = PreparedExpr + bool + Option<AstLineSet>
    //    AstLineSet = HashMap<String, HashSet<usize>>（纯标准集合）
    //    ast-grep 的 AstGrep/Pattern/root/Match 全是 run_all_ast 函数局部变量
    // 4. 本编译断言：类型层正式证明

    #[test]
    fn test_prepared_scan_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PreparedScan>();
    }

    #[cfg(feature = "l3-structural")]
    #[test]
    fn test_ast_line_set_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::l3::structural::AstLineSet>();
    }

    #[test]
    fn test_compiled_l1_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::l1::compiler::CompiledL1>();
    }

    #[test]
    fn test_prepared_expr_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::mode::prepared::PreparedExpr>();
    }

    #[test]
    fn test_matched_line_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::pipeline::MatchedLine>();
    }

    // --- no-feature 构建下 ast() 必须 fail-loud ---

    #[cfg(not(feature = "l3-structural"))]
    #[test]
    fn test_has_any_ast_nofeature_detects_ast_node() {
        use crate::dsl::ast::{Expr, Prim};
        // ast(LANG, PATTERN) 节点 → true
        let ast_expr = Expr::Prim(Prim::Ast {
            lang: "rs".to_string(),
            pattern: "fn main".to_string(),
        });
        assert!(has_any_ast_nofeature(&ast_expr));
        // 普通字面量 → false
        let lit_expr = Expr::Prim(Prim::String("ERROR".to_string()));
        assert!(!has_any_ast_nofeature(&lit_expr));
        // 复合：ast || "ERROR" → true
        let or_expr = Expr::Or(
            Box::new(ast_expr),
            Box::new(Expr::Prim(Prim::String("ERROR".to_string()))),
        );
        assert!(has_any_ast_nofeature(&or_expr));
        // Not(ast(...)) → true
        let not_expr = Expr::Not(Box::new(Expr::Prim(Prim::Ast {
            lang: "rs".to_string(),
            pattern: "fn main".to_string(),
        })));
        assert!(has_any_ast_nofeature(&not_expr));
    }

    #[cfg(not(feature = "l3-structural"))]
    #[test]
    fn test_prepared_scan_build_rejects_ast_when_no_feature() {
        use crate::dsl::parser::parse_query;
        // ast(rs, "fn main") → PreparedScan::build 应报错（fail-loud，引导重建）
        let q = parse_query(r#"ast(rs, "fn main")"#).expect("parse 应成功");
        let r = PreparedScan::build(&q.expr, b"", "", &[]);
        let err = match r {
            Ok(_) => panic!("应 fail-loud 报错，但 build 成功返回"),
            Err(e) => format!("{e}"),
        };
        assert!(err.contains("ast(LANG, PATTERN)"), "得到: {err}");
        assert!(err.contains("l3-structural"), "应给重建提示: {err}");
        assert!(err.contains("--features l3-structural"), "得到: {err}");
    }

    #[cfg(not(feature = "l3-structural"))]
    #[test]
    fn test_prepared_scan_build_passes_non_ast_query() {
        // 非 ast 查询在无 l3-structural 下应正常 build
        let q = crate::dsl::parser::parse_query(r#""ERROR""#).expect("parse 应成功");
        let r = PreparedScan::build(&q.expr, b"", "", &[]);
        assert!(r.is_ok(), "非 ast 查询应正常 build");
    }

    // l3-structural 开启下 PreparedScan::build 不该报 ast 错误（直接走 prepare_l3）
    #[cfg(feature = "l3-structural")]
    #[test]
    fn test_prepared_scan_build_accepts_ast_when_feature_enabled() {
        let q = crate::dsl::parser::parse_query(r#"ast(rs, "fn main")"#).expect("parse 应成功");
        // 空 bytes → ast-grep find_all 在空字符串上跑（rs 文件不能为空字符串，
        // 但函数应不报"feature 未编译"错误，而是返回 Result<Ok 或 L3 错误>）
        let r = PreparedScan::build(&q.expr, b"fn main() {}", "rs", &[]);
        // 不应报 l3-structural 未编译错误
        match r {
            Ok(_) => {} // 正常
            Err(e) => {
                let s = format!("{e}");
                assert!(!s.contains("未编译 l3-structural feature"), "得到: {s}");
            }
        }
    }
}
