//! gxp 模式执行：DSL parse → compile → scan_lines → L2 精判 → 输出。
//!
//! # 谓词支持矩阵
//!
//! | 谓词 | 状态 |
//! |---|---|
//! | `Compare`（`cost_ms>800`） | ✅ L2 精判 |
//! | `Not`（`!(cost>800)`） | ✅ 三态取反（§3.3 字段不存在语义） |
//! | `In`（`tag in [a,b]`） | ✅ 字节字面量集合判定 |
//! | `Between`（`cost between 100 and 200`） | ✅ 闭区间数值判定 |
//! | `Exists`（`user_id exists`） | ✅ 字段存在性 |
//!
//! 管道动词（`|`）→ [`crate::pipeline::execute_stages`] 执行链。

use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use crate::cli::RunOpts;
use crate::dsl::parser::parse_query;
use crate::io::file::read_for_scan;
use crate::l1::compiler::compile;
use crate::mode::detect::{scan_and_collect, scan_and_collect_stream, PreparedScan};
use crate::mode::strict::strict_field_probe;
use crate::mode::verify::{run_verify, run_verify_directory};
use crate::pipeline::execute_stages;
use crate::pipeline::execute_stages_streaming;

/// DRY helper: parse_query + compile（gxp 模式两处调用点统一）。
///
/// `run_gxp_mode_directory` / `run_gxp_mode_with_content` 均需先 parse 再 compile，
/// 错误映射一致（Parse → `GxpError::Parse`，编译失败 → `GxpError::Runtime`）。
/// v0.48（③b hint）：解析失败且查询含 `NAME <非等值操作符> "STRING"` 形态时，
/// 在 Parse 错误 detail 上附加定向 hint（`level>"ERROR"` / `level!="ERROR"` 类误写）。
///
/// 宽松扫描（`has_quoted_wrong_op`）只影响错误文案；语义拦截在 parser 层
/// （`quoted_wrong_op_shape` fail-loud，不落 BARE 兜底）。
fn enrich_string_eq_hint(pattern: &str, e: crate::GxpError) -> crate::GxpError {
    if !crate::mode::detect::has_quoted_wrong_op(pattern) {
        return e;
    }
    let crate::GxpError::Parse { detail } = e else {
        return e;
    };
    crate::GxpError::Parse {
        detail: format!(
            "{detail}\nhint: 双引号 RHS（字符串值）只支持等值操作符 `=` / `==`：\n  \
             - 字符串等值：level=\"ERROR\"（≡ level in [ERROR]）\n  \
             - 不等值：!(level=\"ERROR\")（D6 强制括号）\n  \
             - 数值比较请去引号（D5 11 维契约）：cost_ms>800"
        ),
    }
}

pub(crate) fn parse_and_compile(
    pattern: &str,
) -> crate::Result<(crate::dsl::ast::Query, crate::l1::compiler::CompiledL1)> {
    // parse_query 已返回 GxpError::Parse（detail 不含 "gxp: error: parse:" 前缀，
    // 前缀由 error.rs Display 模板加一次）。直接透传避免双重前缀。
    let q = parse_query(pattern).map_err(|e| enrich_string_eq_hint(pattern, e))?;
    // v0.42 修复：gxp 模式静默吞未闭合字段谓词 bug。
    // `is_gxp_mode=true` 表示用户意图 gxp DSL（含 ` in [`/` between `/`(`/`&&`/`||`/`!`），
    // 但 parse_query 的 prim() 失败时 fall through 到 BARE 兜底，导致 `_tag in [a, b`
    // （缺 `]`）静默解析为 `Prim::Bare("_tag in [a, b")` 字面量搜索。
    // 修复：is_gxp_mode=true 但解析结果整体是 `Prim::Bare`（无 stages）→ fail-loud。
    // 例外：合法 BARE 在 gxp 模式判定下不应触发——is_gxp_mode 已过滤纯 BARE。
    // 例：`"hello"`（含双引号）→ is_gxp_mode=true，parse 为 Prim::String，不触发
    //   `cost_ms>800` → is_gxp_mode=true，parse 为 Prim::Pred，不触发
    //   `_tag in [a, b]` → is_gxp_mode=true，parse 为 Prim::Pred，不触发
    //   `_tag in [a, b`（缺 `]`）→ is_gxp_mode=true，parse 为 Prim::Bare，触发 fail-loud
    if q.stages.is_empty()
        && crate::mode::detect::is_gxp_mode(pattern)
        && matches!(
            q.expr,
            crate::dsl::ast::Expr::Prim(crate::dsl::ast::Prim::Bare(_))
        )
    {
        return Err(crate::GxpError::Parse {
            detail: format!(
                "查询被识别为 gxp 模式（含 ` in [`/` between `/`(`/`&&`/`||`/`!` 等特征），\
                 但解析失败回退为字面量搜索。这通常表示查询有语法错误（如未闭合的 `]`/`)`）。\
                 \n输入：{pattern}\n\
                 请检查：\n  - `in [...]` 列表是否闭合 `]`\n  - `between X and Y` 是否完整\n  \
                 - 括号是否成对 `()`\n  - 字符串是否用双引号 `\"...\"`\n  \
                 参见 gxp --help"
            ),
        });
    }
    let compiled = compile(&q.expr).map_err(|e| crate::GxpError::Runtime {
        detail: format!("L1 编译失败: {e}"),
    })?;
    Ok((q, compiled))
}

/// gxp 模式执行：DSL parse → compile → scan_lines → L2 精判 → 输出。
pub(crate) fn run_gxp_mode<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    opts: RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let content = crate::mode::detect::read_input(file_path).map_err(|e| {
        // 包装 io::Error 加 file_path 上下文，让用户看到具体哪个路径出错
        // （std::io::Error Display 只含 "系统找不到指定的文件" 不含路径）
        crate::GxpError::Runtime {
            detail: format!("读取 '{file_path}' 失败: {e}"),
        }
    })?;
    run_gxp_mode_with_content(pattern, file_path, opts, content, out)
}

/// gxp 模式核心（用已读取的 content 执行，被 `run_gxp_mode` 和 `dispatch_with_content` 复用）。
pub(crate) fn run_gxp_mode_with_content<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    opts: RunOpts,
    content: crate::io::file::FileContent,
    out: &mut W,
) -> crate::Result<()> {
    let (q, compiled) = parse_and_compile(pattern)?;

    // 文件 > 512MB 走流式路径
    if content.is_stream() {
        return run_gxp_mode_streaming(pattern, file_path, opts, content, out);
    }

    let bytes = content.as_bytes();

    // --explain：输出执行计划，不执行扫描
    if opts.explain {
        let plan = crate::plan::build_plan(&q, &compiled)?;
        let text = if opts.json {
            plan.render_json()?
        } else {
            plan.render_text()
        };
        writeln!(out, "{text}")?;
        // --explain --verify 联动：计划末尾追加 verify 摘要
        if opts.verify {
            let ext = crate::mode::detect::ext_from_path(file_path);
            let verify_summary = run_verify(&q, &compiled, bytes, &ext)?;
            writeln!(out, "verify: {verify_summary}")?;
        }
        return Ok(());
    }

    // --verify 单独使用：暴力对账与优化路径 diff
    if opts.verify {
        let ext = crate::mode::detect::ext_from_path(file_path);
        let summary = run_verify(&q, &compiled, bytes, &ext)?;
        writeln!(out, "verify: {summary}")?;
        return Ok(());
    }

    // L2 预编译 + L3 预计算（DRY：PreparedScan::build 统一封装）
    // verified_literals：L1 已保证存在的字面量 → L2 豁免重校验（v0.49 Opt A）
    let file_ext = crate::mode::detect::ext_from_path(file_path);
    let scan = PreparedScan::build(&q.expr, bytes, &file_ext, &compiled.verified_literals)?;

    // --strict：字段探活（设计文档 §2.4 示例 5）
    // 查询引用的字段名在输入中从未出现 → fail-loud 报错
    if opts.strict {
        strict_field_probe(&scan.prepared, &[bytes], file_path)?;
    }

    // L1 扫描 + L2 精判 → Vec<MatchedLine>（DRY: scan_and_collect）
    let mut matched = scan_and_collect(&compiled, &scan, bytes);

    // 单文件模式填充 path（与流式路径同构：与 execute_stages 入口对齐，
    // 供 `_file` 伪字段提取；文本输出由 emit_remaining show_path_prefix=false 抑制前缀）
    // Opt C：一次 Arc 构造，逐行 refcount clone（原每行 String 堆分配 ~6.5ms/44MB）
    let normalized_path: Arc<str> = Arc::from(file_path.replace('\\', "/").as_str());
    for m in &mut matched {
        if m.path.is_none() {
            m.path = Some(Arc::clone(&normalized_path));
        }
    }

    // 管道执行链：按语义顺序执行所有 stage，输出写到 out
    execute_stages(&q.stages, matched, bytes, file_path, out, opts.json)
}

/// gxp 模式流式执行：文件 > 512MB 时走此路径。
///
/// 流式 fallback：
/// - `scan_and_collect_stream` 逐行扫描（BufReader 8MB buffer）
/// - `execute_stages_streaming` 管道执行，`context N` 二次遍历文件
/// - `--verify` / `--strict` / `--explain` 在流式模式下不支持（需完整 content），
///   返回友好错误
fn run_gxp_mode_streaming<W: std::io::Write>(
    pattern: &str,
    file_path: &str,
    opts: RunOpts,
    content: crate::io::file::FileContent,
    out: &mut W,
) -> crate::Result<()> {
    let (q, compiled) = parse_and_compile(pattern)?;
    let path = content
        .stream_path()
        .ok_or_else(|| crate::GxpError::Runtime {
            detail: "流式模式缺少文件路径".to_string(),
        })?;

    // --explain / --verify / --strict 需要完整 content，流式模式不支持
    if opts.explain {
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "文件 {} 超过 512MB 流式阈值，--explain 不支持大文件。\
                 请减小文件或去掉 --explain。",
                path.display()
            ),
        });
    }
    if opts.verify {
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "文件 {} 超过 512MB 流式阈值，--verify 不支持大文件。\
                 请减小文件或去掉 --verify。",
                path.display()
            ),
        });
    }
    if opts.strict {
        return Err(crate::GxpError::Runtime {
            detail: format!(
                "文件 {} 超过 512MB 流式阈值，--strict 不支持大文件。\
                 请减小文件或去掉 --strict。",
                path.display()
            ),
        });
    }

    // L2 预编译（L3 不支持流式，scan_and_collect_stream 会报错）
    let file_ext = crate::mode::detect::ext_from_path(file_path);
    let scan = PreparedScan::build(&q.expr, &[], &file_ext, &compiled.verified_literals)?;

    // 流式 L1 扫描 + L2 精判 → Vec<MatchedLine>
    let mut matched = scan_and_collect_stream(&compiled, &scan, path)?;

    // 流式单文件也填充 path（：与 execute_stages 入口对齐）
    // 之前只在 opts.json=true 时填，文本模式漏填 →
    // `group by _file` / `distinct by _file` 大文件流式返回空键
    // Opt C：Arc 共享（与 run_gxp_mode_with_content 同构）
    let normalized_path: Arc<str> = Arc::from(file_path.replace('\\', "/").as_str());
    for m in &mut matched {
        if m.path.is_none() {
            m.path = Some(Arc::clone(&normalized_path));
        }
    }

    // 流式管道执行链
    execute_stages_streaming(&q.stages, matched, path, file_path, out, opts.json)
}

/// gxp 模式目录递归执行（Rust 方案 §7.6 目录多文件聚合）。
///
/// 工作流程：
/// 1. PathPlan::default() walk 目录枚举所有文件
/// 2. 逐文件 read_for_scan → L1 scan_lines_cb + L2 eval_prepared_with_captures 收集 MatchedLine
/// 3. 所有命中文件进 `Vec<(matched, content, path)>` → `execute_stages_directory`
///    （每文件独立调 apply_context + 全局 rank 归并）
///
/// `--verify` 目录场景：逐文件跑 `run_verify` + 末尾汇总（Rust 方案 §7.6 + §11.2 V1）。
/// `--explain` 目录场景：输出 walk 计划（Rust 方案 §7.6）。
pub(crate) fn run_gxp_mode_directory<W: std::io::Write>(
    pattern: &str,
    dir: &Path,
    opts: RunOpts,
    out: &mut W,
) -> crate::Result<()> {
    let (q, compiled) = parse_and_compile(pattern)?;

    // --explain：输出 walk 计划（Rust 方案 §7.6），不执行扫描
    if opts.explain {
        let plan_text = crate::mode::verify::format_directory_explain(dir, &q, &compiled, &opts)?;
        if opts.json {
            // 目录场景 --explain --json：输出 walk 计划 + 单文件执行计划
            let plan = crate::plan::build_plan(&q, &compiled)?;
            writeln!(out, "{plan_text}")?;
            writeln!(out, "plan_json: {}", plan.render_json()?)?;
        } else {
            writeln!(out, "{plan_text}")?;
        }
        if opts.verify {
            // --explain --verify 目录场景：逐文件跑 verify 汇总
            let summary = run_verify_directory(&q, &compiled, dir, &opts)?;
            writeln!(out, "verify: {summary}")?;
        }
        return Ok(());
    }

    // --verify 目录场景：逐文件对账（Rust 方案 §7.6 + §11.2 V1）
    if opts.verify {
        let summary = run_verify_directory(&q, &compiled, dir, &opts)?;
        writeln!(out, "verify: {summary}")?;
        return Ok(());
    }

    // L2 预编译（顶层算一次，每文件复用 prepared；ast_lines 每文件单独算）
    // 顶层 build 无文件路径 → file_ext="" → 不做扩展名过滤（每文件 build_per_file 再过滤）
    let scan = PreparedScan::build(&q.expr, &[], "", &compiled.verified_literals)?;

    // --strict：字段探活（设计文档 §2.4 示例 5）
    // 目录模式：逐文件读入 → strict_field_probe 跨文件并集检测（DRY：与单文件统一）
    if opts.strict {
        let plan = crate::mode::bare::build_pathplan_from_opts(&opts);
        let (files, _stats) = plan.walk(dir);
        let mut file_bytes: Vec<Vec<u8>> = Vec::new();
        for fp in &files {
            if let Ok(content) = read_for_scan(fp) {
                file_bytes.push(content.as_bytes().to_vec());
            }
        }
        let sources: Vec<&[u8]> = file_bytes.iter().map(|v| v.as_slice()).collect();
        strict_field_probe(
            &scan.prepared,
            &sources,
            &format!("目录 {} ", dir.display()),
        )?;
    }

    // 从 RunOpts 构建 PathPlan（glob + gitignore）
    let plan = crate::mode::bare::build_pathplan_from_opts(&opts);
    let (files, _stats) = plan.walk(dir);

    // rayon 并行跨文件扫描
    // 文件 > 512MB 走流式扫描路径，≤ 512MB 走 read_to_end
    // 结果按原始文件顺序排列（par_iter 保序），read 失败的文件跳过（与 grep -r 一致）
    let files_input: Vec<(
        Vec<crate::pipeline::MatchedLine>,
        crate::io::file::FileContent,
        String,
    )> = files
        .par_iter()
        .map(|file_path| -> crate::Result<Option<_>> {
            let content = match read_for_scan(file_path) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            let path_str = file_path.to_string_lossy().replace('\\', "/");

            // 流式路径（文件 > 512MB）
            if content.is_stream() {
                let stream_path = content.stream_path().unwrap_or(Path::new(file_path));
                let ext = crate::mode::detect::ext_from_path(&path_str);
                let per_file_scan = PreparedScan::build_per_file(&q.expr, &scan, &[], &ext)?;
                let matched = scan_and_collect_stream(&compiled, &per_file_scan, stream_path)?;
                return Ok(Some((matched, content, path_str)));
            }

            let bytes: &[u8] = content.as_bytes();

            // L2/L3 求值上下文（每文件 build_per_file：复用 prepared，重算 ast_lines）
            let ext = crate::mode::detect::ext_from_path(&path_str);
            let per_file_scan = PreparedScan::build_per_file(&q.expr, &scan, bytes, &ext)?;

            // L1 扫描 + L2 精判 → Vec<MatchedLine>（DRY: scan_and_collect）
            let matched = scan_and_collect(&compiled, &per_file_scan, bytes);

            // 目录场景：所有 walk 到的文件都进 files_input（files stage 需要全文件列表）
            Ok(Some((matched, content, path_str)))
        })
        .collect::<crate::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();

    // 分离 Vec 和 Stream 文件
    // Vec 文件走 execute_stages_directory（content 可用）
    // Stream 文件：context stage 已预计算（二次遍历），matched 行自带数据
    let mut vec_inputs: Vec<(Vec<crate::pipeline::MatchedLine>, Vec<u8>, String)> = Vec::new();
    let mut stream_inputs: Vec<(
        Vec<crate::pipeline::MatchedLine>,
        std::path::PathBuf,
        String,
    )> = Vec::new();

    for (m, c, p) in files_input {
        match c {
            crate::io::file::FileContent::Vec(v) => vec_inputs.push((m, v, p)),
            crate::io::file::FileContent::Stream(path) => stream_inputs.push((m, path, p)),
        }
    }

    // 流式文件：检查是否有 context stage
    let context_stage: Option<(usize, usize)> = q.stages.iter().find_map(|s| match s {
        crate::dsl::ast::Stage::Context { before, after } => Some((*before, *after)),
        _ => None,
    });

    if !stream_inputs.is_empty() {
        if let Some((before, after)) = context_stage {
            // 有 context：对流式文件二次遍历扩展 context 窗口
            for (matched, path, path_str) in stream_inputs {
                let ctx_lines = crate::pipeline::context::apply_context_streaming(
                    &matched, &path, before, after,
                )
                .map_err(|e| crate::GxpError::Runtime {
                    detail: format!("流式 context 失败: {e}"),
                })?;
                // context 已扩展，用空 content 占位（MatchedLine 自带 line 数据）
                vec_inputs.push((ctx_lines, Vec::new(), path_str));
            }
        } else {
            // 无 context：流式文件的 matched 直接合并
            for (matched, _path, path_str) in stream_inputs {
                vec_inputs.push((matched, Vec::new(), path_str));
            }
        }
    }

    crate::pipeline::execute_stages_directory(&q.stages, vec_inputs, out, opts.json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrich_string_eq_hint_appends_on_wrong_op() {
        let base = crate::GxpError::Parse {
            detail: "DSL 解析失败".to_string(),
        };
        let enriched = enrich_string_eq_hint(r#"level>"ERROR""#, base);
        let crate::GxpError::Parse { detail } = enriched else {
            panic!("期望 Parse")
        };
        assert!(detail.contains("DSL 解析失败"), "应保留原 detail：{detail}");
        assert!(detail.contains("level=\"ERROR\""), "应含等值写法：{detail}");
        assert!(
            detail.contains("!(level=\"ERROR\")"),
            "应含否定写法：{detail}"
        );
    }

    #[test]
    fn test_enrich_string_eq_hint_noop_on_legal_or_other_error() {
        // 合法脱糖形态：不加 hint
        let base = crate::GxpError::Parse {
            detail: "x".to_string(),
        };
        match enrich_string_eq_hint(r#"level="ERROR""#, base) {
            crate::GxpError::Parse { detail } => assert_eq!(detail, "x"),
            _ => panic!("期望 Parse"),
        }
        // 非 Parse 错误：原样返回
        let runtime = crate::GxpError::Runtime {
            detail: "r".to_string(),
        };
        assert!(matches!(
            enrich_string_eq_hint(r#"level>"ERROR""#, runtime),
            crate::GxpError::Runtime { .. }
        ));
    }
}
