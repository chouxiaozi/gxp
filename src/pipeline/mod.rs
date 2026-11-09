//! Pipeline：查询后动词。
//!
//! 子模块：`rank` / `sort` / `distinct` / `project` / `head` / `count` / `context` / `files` / `stats` / `sample` / `percentiles`
//! （设计文档 §2.2 + Rust 方案 §7.6）
//!
//! # 执行模型（Rust 方案 §7.7）
//!
//! 执行顺序：`where → context → group/count/stats/percentiles → rank → sort → distinct → head → sample → fields → files`。
//! 扫描阶段产出 `Vec<MatchedLine>`（命中行 + 行号），各 stage 依次变换。
//!
//! 本模块提供：
//! - [`MatchedLine`]：命中行统一结构（拥有数据，跨 stage 传递）
//! - [`execute_stages`]：按语义顺序执行所有 stage，输出写到一个 writer

pub mod context;
pub mod count;
pub mod distinct;
pub mod files;
pub mod head;
pub mod percentiles;
pub mod project;
pub mod rank;
pub mod sample;
pub mod sort;
pub mod stats;
pub mod trace;

use std::io::Write;
use std::sync::Arc;

use crate::dsl::ast::{Expr, Pred, Prim, Stage};
use crate::error::Result;

/// 捕获字段集合：`capture(NAME, /REGEX/)` 提取的值，供下游 `fields` 使用。
///
/// 字段注入：capture 谓词匹配行时提取正则匹配值写入此 map，
/// `apply_projection` 优先查此 map，fallback 到 `ExtractedFields::find_value`。
pub type CaptureMap = std::collections::HashMap<String, Vec<u8>>;

/// 命中行：行号（1-based）+ 行内容（拥有数据，跨 stage 可排序/投影）。
///
/// `captures` 携带 `capture(NAME, /REGEX/)` 提取的字段值，
/// 供 `fields` 投影使用。无 capture 的行 captures 为空。
///
/// `path` 携带文件路径（目录模式 `Some(rel_path)`，单文件/stdin `Some(file_path)`/`Some(<stdin>)`）。
/// `emit_remaining` 在 JSON 中输出 path 字段，在非 JSON 目录模式加路径前缀。
///
/// `line` 改为 `Arc<[u8]>`——clone 从深拷贝变引用计数（O(1)），
/// 消除 collect 阶段 7MB `Vec<u8>` 深拷贝（bench 12.47ms / 15%）。
/// `Arc<[u8]>: Send+Sync`，不破坏 V1 rayon 并行（Rust 方案 §11.2 V1 编译断言保持通过）。
///
/// `extra_lines` 由 `trace` 动词填充（§9.1 候选 #2）：多行 stack trace 的后续帧。
/// 默认空 Vec（非 trace 路径无开销）。`emit_remaining` 输出时若非空，
/// 在 JSON 加 `extra_lines` 字段（数组），在终端文本用 `  → ` 前缀视觉区分。
#[derive(Debug, Clone)]
pub struct MatchedLine {
    /// 行号（1-based，与 grep -n 一致）。
    pub line_no: usize,
    /// 行内容（不含末尾换行符）。`Arc<[u8]>` 引用计数 clone。
    pub line: Arc<[u8]>,
    /// capture 提取的字段值（字段注入）。
    pub captures: CaptureMap,
    /// 文件路径（目录模式填充，供 JSON 输出 + 非 JSON 路径前缀）。
    /// `Arc<str>`（v0.49 Opt C）：单文件/目录模式所有命中行共享同一路径，
    /// clone 从每行堆分配 String 变引用计数（103607 行 × ~40ns → ~0.3ms，
    /// 原实测增量 ~6.5ms，Rust 方案 §7.9）。
    pub path: Option<Arc<str>>,
    /// L1 匹配的字节范围内容（伪字段 `_match` 使用）。
    /// BARE 模式：正则 `find` 的匹配字节。
    /// gxp 模式：第一个必选组第一个命中的字面量（aho-corasick find）。
    /// 无字面量组（纯字段谓词）或 BARE 无 match 时为 None。
    /// `Arc<[u8]>`（v0.49 Opt B）：字面量是编译期常量，clone 引用计数化
    /// （原每行 Vec 分配 ~3.6ms，Rust 方案 §7.9）。
    pub match_bytes: Option<Arc<[u8]>>,
    /// 多行 stack trace 的后续行（由 `trace` 动词填充）。
    /// 默认空 Vec——非 trace 路径无开销。
    pub extra_lines: Vec<Arc<[u8]>>,
}

impl MatchedLine {
    /// 从行号 + 行字节构造（无 captures, 无 path, 无 match_bytes, 无 extra_lines）。
    pub fn new(line_no: usize, line: &[u8]) -> Self {
        MatchedLine {
            line_no,
            line: Arc::from(line),
            captures: CaptureMap::new(),
            path: None,
            match_bytes: None,
            extra_lines: Vec::new(),
        }
    }

    /// 从行号 + 行字节 + captures 构造（无 path, 无 match_bytes, 无 extra_lines）。
    pub fn with_captures(line_no: usize, line: &[u8], captures: CaptureMap) -> Self {
        MatchedLine {
            line_no,
            line: Arc::from(line),
            captures,
            path: None,
            match_bytes: None,
            extra_lines: Vec::new(),
        }
    }
}

/// 执行管道动词链，输出写到一个 writer。
///
/// 语义顺序（Rust 方案 §7.7）：
/// `where → context → group/count → rank → head → fields → files`。
///
/// - `count` / `group by`：输出数字/分组计数，终止链
/// - `files` / `files --invert`：输出文件名（单文件场景），终止链
/// - 其余：变换命中行集合后，最终按 `行号:行内容` 格式输出
///
/// # 参数
///
/// - `stages`：DSL 解析出的管道动词序列（已解析，未重排）
/// - `matched`：扫描阶段（L1+L2 过滤后）的命中行
/// - `content`：整个文件内容（`context` 需要访问命中行以外的行）
/// - `file_path`：当前文件路径（`files` 输出用）
/// - `out`：输出 writer
pub fn execute_stages<W: Write>(
    stages: &[Stage],
    matched: Vec<MatchedLine>,
    content: &[u8],
    file_path: &str,
    out: &mut W,
    json: bool,
) -> Result<()> {
    // 现在：单文件模式补填 path（与目录模式 mod.rs:347 对齐）。
    // 漏填导致 `distinct by _file` / `group by _file` 单文件输入返回空键
    // （`: N` 或只保留首行），跨输入源契约违反（§6.4 第 3 条 + 第 1 条）。
    // 文本输出由 `emit_remaining` 的 `show_path_prefix = false` 抑制路径前缀。
    let mut lines = matched;
    if !lines.is_empty() {
        // Opt C：一次构造 Arc，逐行 refcount clone（原每行 String 堆分配）
        let shared_path: Arc<str> = Arc::from(file_path);
        for m in &mut lines {
            if m.path.is_none() {
                m.path = Some(Arc::clone(&shared_path));
            }
        }
    }
    let mut emitted = false;

    let ordered = reorder_stages(stages);
    check_terminal_stages(&ordered)?;

    for stage in ordered {
        match stage {
            Stage::Where(expr) => {
                lines = apply_where(lines, expr)?;
            }
            Stage::Context { before, after } => {
                lines = context::apply_context(&lines, content, *before, *after);
            }
            Stage::Count => {
                let n = count::apply_count(&lines);
                writeln!(out, "{n}")?;
                emitted = true;
            }
            Stage::GroupBy(field) => {
                let groups = count::apply_count_group_by(&lines, field.as_bytes());
                for (k, v) in groups {
                    writeln!(out, "{}: {v}", String::from_utf8_lossy(&k))?;
                }
                emitted = true;
            }
            Stage::Rank { top, by, desc } => {
                lines = rank::apply_rank(&lines, by.as_bytes(), *top, *desc);
            }
            Stage::Sort { by, desc } => {
                lines = sort::apply_sort(lines, by.as_deref().map(|s| s.as_bytes()), *desc);
            }
            Stage::Distinct { by } => {
                lines = distinct::apply_distinct(lines, by.as_deref().map(|s| s.as_bytes()));
            }
            Stage::Head(n) => {
                lines.truncate(*n);
            }
            Stage::Sample { spec } => {
                lines = sample::apply_sample(lines, spec);
            }
            Stage::Fields(fields) => {
                let names: Vec<&[u8]> = fields.iter().map(|s| s.as_bytes()).collect();
                lines = project::apply_projection(&lines, &names);
            }
            Stage::Files { invert } => {
                files::apply_files(&lines, *invert, file_path, out)?;
                emitted = true;
            }
            Stage::Stats { field } => {
                let summary = stats::apply_stats(&lines, field.as_bytes());
                writeln!(out, "{summary}")?;
                emitted = true;
            }
            Stage::Percentiles { field, percentiles } => {
                let summary = percentiles::apply_percentiles(&lines, field.as_bytes(), percentiles);
                writeln!(out, "{summary}")?;
                emitted = true;
            }
            Stage::Trace { lang } => {
                lines = trace::apply_trace(&lines, content, lang.as_deref());
                // trace 输出多行记录，后续 stage 受限（fail-loud 由 check_terminal_stages 处理）
            }
        }
        if emitted {
            break;
        }
    }

    // 单文件模式：文本输出不带路径前缀（与历史行为一致）；
    // JSON 输出仍含 path（下游 LLM 需要）。
    emit_remaining(&lines, emitted, json, false, out)
}

/// 流式管道执行链：文件 > 512MB 时使用此函数。
///
/// 流式 fallback：与 `execute_stages` 语义对齐，但 `context` stage
/// 通过 `apply_context_streaming` 二次遍历文件实现（而非持有完整 `content`）。
///
/// - `stages`：DSL 解析出的管道动词序列
/// - `matched`：流式扫描阶段（`scan_and_collect_stream`）产出的命中行
/// - `stream_path`：文件路径（供 `context` 二次遍历使用）
/// - `file_path`：输出用的文件路径标识
/// - `out`：输出 writer
pub fn execute_stages_streaming<W: Write>(
    stages: &[Stage],
    matched: Vec<MatchedLine>,
    stream_path: &std::path::Path,
    file_path: &str,
    out: &mut W,
    json: bool,
) -> Result<()> {
    let mut lines = matched;
    let mut emitted = false;

    let ordered = reorder_stages(stages);
    check_terminal_stages(&ordered)?;

    for stage in ordered {
        match stage {
            Stage::Where(expr) => {
                lines = apply_where(lines, expr)?;
            }
            Stage::Context { before, after } => {
                lines = context::apply_context_streaming(&lines, stream_path, *before, *after)
                    .map_err(|e| crate::GxpError::Runtime {
                        detail: format!("流式 context 失败: {e}"),
                    })?;
            }
            Stage::Count => {
                let n = count::apply_count(&lines);
                writeln!(out, "{n}")?;
                emitted = true;
            }
            Stage::GroupBy(field) => {
                let groups = count::apply_count_group_by(&lines, field.as_bytes());
                for (k, v) in groups {
                    writeln!(out, "{}: {v}", String::from_utf8_lossy(&k))?;
                }
                emitted = true;
            }
            Stage::Rank { top, by, desc } => {
                lines = rank::apply_rank(&lines, by.as_bytes(), *top, *desc);
            }
            Stage::Sort { by, desc } => {
                lines = sort::apply_sort(lines, by.as_deref().map(|s| s.as_bytes()), *desc);
            }
            Stage::Distinct { by } => {
                lines = distinct::apply_distinct(lines, by.as_deref().map(|s| s.as_bytes()));
            }
            Stage::Head(n) => {
                lines.truncate(*n);
            }
            Stage::Sample { spec } => {
                lines = sample::apply_sample(lines, spec);
            }
            Stage::Fields(fields) => {
                let names: Vec<&[u8]> = fields.iter().map(|s| s.as_bytes()).collect();
                lines = project::apply_projection(&lines, &names);
            }
            Stage::Files { invert } => {
                files::apply_files(&lines, *invert, file_path, out)?;
                emitted = true;
            }
            Stage::Stats { field } => {
                let summary = stats::apply_stats(&lines, field.as_bytes());
                writeln!(out, "{summary}")?;
                emitted = true;
            }
            Stage::Percentiles { field, percentiles } => {
                let summary = percentiles::apply_percentiles(&lines, field.as_bytes(), percentiles);
                writeln!(out, "{summary}")?;
                emitted = true;
            }
            Stage::Trace { lang } => {
                // 流式路径不支持 trace：跨行边界判定需要"看到下一行才能决定上一行是续行"，
                // 但 BufReader 按行回调时下一行还没读进来。
                // fail-loud 提示用 `context N` 替代（拉固定窗口的近似）
                let hint = match lang {
                    Some(l) => format!("'{l}'"),
                    None => "".to_string(),
                };
                return Err(crate::GxpError::Runtime {
                    detail: format!(
                        "trace {}不支持流式扫描（文件 > 512MB）。\n\
                         trace 需要跨行边界判定，但流式路径按行回调时下一行未读入。\n\
                         修复方案（任选其一）：\n\
                         1. 减小文件或预切分（让单文件 ≤ 512MB，走非流式路径）\n\
                         2. 用 `context N` 近似替代：`<pattern> | context 20`（拉固定 20 行窗口，每行独立输出）",
                        hint
                    ),
                });
            }
        }
        if emitted {
            break;
        }
    }

    emit_remaining(&lines, emitted, json, false, out)
}

/// 目录场景管道执行链（Rust 方案 §7.6 目录多文件聚合）。
///
/// 与 [`execute_stages`] 的区别：多文件输入，`context` 每文件独立扩展
/// （不拼跨文件 context），所有命中行聚合到全局 Vec 后跑剩余 stages
/// （`where → count/group → rank → head → fields → files`）。
///
/// - `files_input`：每文件一组 `(matched, content, file_path)`，按 walk 顺序
/// - `stages`：DSL 解析出的管道动词序列（未重排）
/// - `out`：输出 writer
///
/// # 全局归并语义（Rust 方案 §7.6）
///
/// `rank top K` 在所有命中行进同一 Vec 后 partial-sort，与单文件语义自然延伸。
/// `context N` 每文件独立扩展窗口，文件 A 末行 + 文件 B 首行不拼成跨文件 context。
pub fn execute_stages_directory<W: Write>(
    stages: &[Stage],
    files_input: Vec<(Vec<MatchedLine>, Vec<u8>, String)>,
    out: &mut W,
    json: bool,
) -> Result<()> {
    // 分离 context stage（每文件独立执行）与后续 stages（全局执行）
    let context_stage: Option<(usize, usize)> = stages.iter().find_map(|s| match s {
        Stage::Context { before, after } => Some((*before, *after)),
        _ => None,
    });
    // 分离 trace stage（每文件独立执行——trace 跨文件不连续，参照 context 跨文件规则）
    let trace_stage: Option<Option<String>> = stages.iter().find_map(|s| match s {
        Stage::Trace { lang } => Some(lang.clone()),
        _ => None,
    });

    // 每文件独立处理 context + trace + 聚合到全局 Vec
    let mut global_lines: Vec<MatchedLine> = Vec::new();
    let mut file_match_counts: Vec<(String, usize)> = Vec::new();

    for (matched, content, path) in files_input {
        let hit_count = matched.len();
        file_match_counts.push((path.clone(), hit_count));

        // Opt C：每文件一次 Arc 构造，行级 clone 引用计数化
        let path_arc: Arc<str> = Arc::from(path.as_str());

        // 给每行填充 path（目录模式特有）
        let mut matched = matched;
        for m in &mut matched {
            m.path = Some(Arc::clone(&path_arc));
        }

        // content 为空 → 流式文件：context/trace 均不支持流式（trace 流式 fail-loud 在外层）
        let mut lines = if content.is_empty() {
            matched
        } else if let Some((before, after)) = context_stage {
            context::apply_context(&matched, &content, before, after)
        } else {
            matched
        };
        // context 扩展行继承命中行 path（同文件内 context 行 path 相同）
        for m in &mut lines {
            if m.path.is_none() {
                m.path = Some(Arc::clone(&path_arc));
            }
        }
        // trace 在每文件独立执行（跨文件不连续，参照 context 跨文件规则）
        if let Some(ref lang) = trace_stage {
            if !content.is_empty() {
                lines = trace::apply_trace(&lines, &content, lang.as_deref());
            }
        }
        global_lines.extend(lines);
    }

    // 跑剩余 stages（不含 context，已处理）
    let mut lines = global_lines;
    let mut emitted = false;

    let ordered = reorder_stages(stages);
    check_terminal_stages(&ordered)?;

    for stage in ordered {
        match stage {
            Stage::Where(expr) => {
                lines = apply_where(lines, expr)?;
            }
            Stage::Context { .. } => {} // 已在聚合阶段处理
            Stage::Trace { .. } => {}   // 已在聚合阶段每文件独立处理
            Stage::Count => {
                let n = count::apply_count(&lines);
                writeln!(out, "{n}")?;
                emitted = true;
            }
            Stage::GroupBy(field) => {
                let groups = count::apply_count_group_by(&lines, field.as_bytes());
                for (k, v) in groups {
                    writeln!(out, "{}: {v}", String::from_utf8_lossy(&k))?;
                }
                emitted = true;
            }
            Stage::Rank { top, by, desc } => {
                lines = rank::apply_rank(&lines, by.as_bytes(), *top, *desc);
            }
            Stage::Sort { by, desc } => {
                lines = sort::apply_sort(lines, by.as_deref().map(|s| s.as_bytes()), *desc);
            }
            Stage::Distinct { by } => {
                lines = distinct::apply_distinct(lines, by.as_deref().map(|s| s.as_bytes()));
            }
            Stage::Head(n) => {
                lines.truncate(*n);
            }
            Stage::Sample { spec } => {
                lines = sample::apply_sample(lines, spec);
            }
            Stage::Fields(fields) => {
                let names: Vec<&[u8]> = fields.iter().map(|s| s.as_bytes()).collect();
                lines = project::apply_projection(&lines, &names);
            }
            Stage::Files { invert } => {
                // 目录场景：输出命中（或未命中，--invert）的文件名列表
                for (path, count) in &file_match_counts {
                    if *invert {
                        if *count == 0 {
                            writeln!(out, "{}", normalize_path(path))?;
                        }
                    } else if *count > 0 {
                        writeln!(out, "{}", normalize_path(path))?;
                    }
                }
                emitted = true;
            }
            Stage::Stats { field } => {
                let summary = stats::apply_stats(&lines, field.as_bytes());
                writeln!(out, "{summary}")?;
                emitted = true;
            }
            Stage::Percentiles { field, percentiles } => {
                let summary = percentiles::apply_percentiles(&lines, field.as_bytes(), percentiles);
                writeln!(out, "{summary}")?;
                emitted = true;
            }
        }
        if emitted {
            break;
        }
    }

    emit_remaining(&lines, emitted, json, true, out)
}

/// 按语义顺序重排 stage（Rust 方案 §7.6：先扩行再投影/排序）。
/// 返回 `Vec<&Stage>` 引用，不消费原 slice。
fn reorder_stages(stages: &[Stage]) -> Vec<&Stage> {
    let mut ordered = Vec::with_capacity(stages.len());
    for s in stages {
        match s {
            Stage::Where(_) => ordered.push(s),
            Stage::Context { .. } => ordered.push(s),
            Stage::Count
            | Stage::GroupBy(_)
            | Stage::Stats { .. }
            | Stage::Percentiles { .. }
            | Stage::Trace { .. } => ordered.push(s),
            Stage::Rank { .. } => ordered.push(s),
            Stage::Sort { .. } => ordered.push(s),
            Stage::Distinct { .. } => ordered.push(s),
            Stage::Head(_) => ordered.push(s),
            Stage::Sample { .. } => ordered.push(s),
            Stage::Fields(_) => ordered.push(s),
            Stage::Files { .. } => ordered.push(s),
        }
    }
    ordered
}

/// 终止 stage：执行后不再处理后续 stage（输出结果即终止链）。
///
/// `count` / `group by` / `files` / `stats` 都是终止 stage——它们输出
/// 聚合结果（计数/分组计数/文件名/统计摘要），不再传递命中行给后续 stage。
/// B1 修复（§0.1 LLM 友好原则）：终止 stage 后还有非空 stage → fail-loud 报错，
/// 而非静默吞掉（静默吞掉导致 `count group by FIELD` 只输出总数）。
fn is_terminal(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::Count
            | Stage::GroupBy(_)
            | Stage::Files { .. }
            | Stage::Stats { .. }
            | Stage::Percentiles { .. }
            | Stage::Trace { .. }
    )
}

/// 检查终止 stage 后无其他 stage（B1：fail-loud）。
///
/// 终止 stage 执行后链终止，后续 stage 会被静默跳过。LLM 友好原则要求
/// fail-loud：如果用户写了 `count | head 3` 或 `count group by FIELD`，
/// 报错告知终止 stage 后不允许其他 stage。
fn check_terminal_stages(ordered: &[&Stage]) -> Result<()> {
    for i in 0..ordered.len() {
        if is_terminal(ordered[i]) {
            let remaining: &[&Stage] = &ordered[i + 1..];
            if !remaining.is_empty() {
                let terminal_name = stage_name(ordered[i]);
                let remaining_names: Vec<String> =
                    remaining.iter().map(|s| stage_name(s)).collect();
                return Err(crate::GxpError::Runtime {
                    detail: format!(
                        "终止动词 `{}` 后不允许其他管道动词（{}），\
                         `{}` 是终止动词——输出结果后链即终止。\n\
                         修复：去掉 `{}` 后的动词，或调整查询逻辑。",
                        terminal_name,
                        remaining_names.join(", "),
                        terminal_name,
                        terminal_name
                    ),
                });
            }
        }
    }
    Ok(())
}

/// 返回 stage 的人类可读名称（用于错误消息）。
fn stage_name(stage: &Stage) -> String {
    match stage {
        Stage::Where(_) => "where".to_string(),
        Stage::Context { .. } => "context".to_string(),
        Stage::Count => "count".to_string(),
        Stage::GroupBy(f) => format!("group by {f}"),
        Stage::Stats { field } => format!("stats {field}"),
        Stage::Percentiles { field, .. } => format!("percentiles {field}"),
        Stage::Trace { lang } => match lang {
            Some(l) => format!("trace {l}"),
            None => "trace".to_string(),
        },
        Stage::Rank { .. } => "rank".to_string(),
        Stage::Sort { .. } => "sort".to_string(),
        Stage::Distinct { .. } => "distinct".to_string(),
        Stage::Head(_) => "head".to_string(),
        Stage::Sample { .. } => "sample".to_string(),
        Stage::Fields(_) => "fields".to_string(),
        Stage::Files { .. } => "files".to_string(),
    }
}

/// 最终输出：如果未提前终止（count/files），按 `行号:行内容` 格式输出所有命中行。
///
/// `json = true` 时，每行输出为 NDJSON，含 `path`（当 `MatchedLine::path` 为 Some）和 `captures`（当非空）。
/// 非 JSON 目录模式：path 为 Some 时加 `path:` 前缀（与 `grep -r` 一致）。
/// `show_path_prefix = false`（单文件文本模式）：抑制 path 前缀，输出 `line_no:line`。
/// 现在：单文件模式现在也填充 `m.path`（供 `_file` 伪字段提取），
/// 但文本输出仍不带路径前缀（保持历史 UX）。
fn emit_remaining<W: Write>(
    lines: &[MatchedLine],
    emitted: bool,
    json: bool,
    show_path_prefix: bool,
    out: &mut W,
) -> Result<()> {
    if !emitted {
        if json {
            for m in lines {
                let line_str = String::from_utf8_lossy(&m.line);
                let mut obj = serde_json::json!({
                    "line_no": m.line_no,
                    "line": line_str,
                });
                if let Some(ref p) = m.path {
                    obj["path"] = serde_json::Value::String(normalize_path(p));
                }
                // v0.42 修复：gxp 模式 JSON 缺 `match` 字段——与 BARE 模式 schema 不一致。
                // BARE 模式 JSON 含 "match"（line 117-118），gxp 模式 emit_remaining
                // 原本不输出 → 下游 LLM 用 `--json` 时无法判断字面量命中位置。
                // `_match` 伪字段（等价 grep -o）在 scan_and_collect 中填充。
                if let Some(ref mb) = m.match_bytes {
                    let mb_str = String::from_utf8_lossy(mb).into_owned();
                    obj["match"] = serde_json::Value::String(mb_str);
                }
                if !m.captures.is_empty() {
                    let caps_obj: serde_json::Map<String, serde_json::Value> = m
                        .captures
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                serde_json::Value::String(String::from_utf8_lossy(v).into_owned()),
                            )
                        })
                        .collect();
                    obj["captures"] = serde_json::Value::Object(caps_obj);
                }
                // extra_lines（trace 动词填充）：仅非空时输出数组字段
                if !m.extra_lines.is_empty() {
                    let extras: Vec<serde_json::Value> = m
                        .extra_lines
                        .iter()
                        .map(|l| serde_json::Value::String(String::from_utf8_lossy(l).into_owned()))
                        .collect();
                    obj["extra_lines"] = serde_json::Value::Array(extras);
                }
                writeln!(out, "{obj}")?;
            }
        } else {
            for m in lines {
                // 现在：show_path_prefix = false（单文件模式）抑制路径前缀
                if show_path_prefix {
                    if let Some(p) = &m.path {
                        writeln!(
                            out,
                            "{}:{}:{}",
                            normalize_path(p),
                            m.line_no,
                            String::from_utf8_lossy(&m.line)
                        )?;
                    } else {
                        writeln!(out, "{}:{}", m.line_no, String::from_utf8_lossy(&m.line))?;
                    }
                } else {
                    writeln!(out, "{}:{}", m.line_no, String::from_utf8_lossy(&m.line))?;
                }
                // extra_lines（trace 动词填充）：每行用 `  → ` 前缀视觉区分
                for extra in &m.extra_lines {
                    if show_path_prefix {
                        if let Some(p) = &m.path {
                            writeln!(
                                out,
                                "{}:  → {}",
                                normalize_path(p),
                                String::from_utf8_lossy(extra)
                            )?;
                        } else {
                            writeln!(out, "  → {}", String::from_utf8_lossy(extra))?;
                        }
                    } else {
                        writeln!(out, "  → {}", String::from_utf8_lossy(extra))?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// 统一路径分隔符为正斜杠（跨平台一致性，LLM 解析友好）。
#[inline]
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/")
}

/// 伪字段值提取：`_match`/`_line`/`_file`/`_ts`/`_tag` 统一入口。
///
/// **v0.42 修复**：原 `sort` / `rank` 直接调 `ExtractedFields::find_value`，
/// 但 `find_value` 只特判 `_ts`/`_tag`（extract.rs:114-119），对 `_match`/`_line`/`_file`
/// 走 logfmt 提取（在行内找 `_match=...`），显然找不到 → 返回 `None` → `RankKey::Bytes`
/// → 全部同键 → 稳定排序保持原序。`project.rs` 已正确处理这三个伪字段（line 45-52），
/// 但 sort/rank 缺失特判，导致 `sort by _match asc` / `rank top K by _match desc` 静默
/// 不排序（数据正确性 bug，但隐蔽——LLM 写 `| sort by _match asc` 期望按匹配字节排序）。
///
/// 修复：抽 helper，与 `project.rs::apply_projection` 伪字段分支语义对齐：
/// - `_match` → `MatchedLine::match_bytes`（L1 命中字节，与 grep -o 等价）
/// - `_line`  → `MatchedLine::line`（整行内容）
/// - `_file`  → `MatchedLine::path`（文件路径，跨平台规范化）
/// - `_ts`/`_tag` → `ExtractedFields::find_value`（单路径专用扫描）
/// - 普通字段 → `ExtractedFields::find_value`（logfmt/JSON/bracket 抽取）
///
/// 返回 `Option<Vec<u8>>`（owned）。`sort`/`rank` 仅在键提取阶段调用一次/行，
/// clone 开销可接受（project.rs 也是 owned `Vec<u8>`）。
///
/// **DRY**：消除 `project.rs`/`sort.rs`/`rank.rs` 三处伪字段分支复制。
pub(crate) fn pseudo_field_value(m: &MatchedLine, name: &[u8]) -> Option<Vec<u8>> {
    if name == b"_match" {
        // match_bytes 已 Arc 化（v0.49 Opt B），此处按接口契约转 owned
        return m.match_bytes.as_ref().map(|b| b.to_vec());
    }
    if name == b"_line" {
        return Some(m.line.to_vec());
    }
    if name == b"_file" {
        // 现在：execute_stages 入口已补填 m.path（单文件模式）。
        // 之前单文件模式 m.path = None → _file 返回 None → 空键 bug。
        return m.path.as_ref().map(|p| normalize_path(p).into_bytes());
    }
    // _ts / _tag / 普通字段 → find_value（已含 _ts/_tag 特判）
    crate::l2::ExtractedFields::find_value(&m.line, name).map(|v| v.to_vec())
}

/// `where EXPR`：对命中行重新抽字段 + 递归求值，过滤不满足的行。
///
/// 如果 where 的 EXPR 含 capture，提取的值合并到 `MatchedLine::captures`。
///
/// # 字段可见性（见下）
///
/// **captures 优先**：先查 `MatchedLine::captures[field]`，命中则用作字段值；
/// 未命中再 fallback 到 `ExtractedFields::find_value(line, field)`（行内字段）。
///
/// 原实现直接调 `compiled.l1_match(line)` 做字段名前缀粗筛，但 L1 前缀粗筛
/// 只看行内 `field=` / `field:` 子串——若上游 capture 已提取该字段值（行内
/// 无该字段），L1 粗筛淘汰 → where 谓词永远看不到 capture 值，破坏
/// `capture(NAME, /REGEX/) | where NAME>X` 语义。
///
/// 修复：L1 粗筛前先查 captures——若该行 captures 已含谓词引用字段，
/// 跳过 L1 前缀粗筛直接走 L2（L2 会 captures 优先 + 行内 fallback）。
fn apply_where(lines: Vec<MatchedLine>, expr: &Expr) -> Result<Vec<MatchedLine>> {
    // where 的 EXPR 是独立表达式，走 L1 编译 + L2 精判（复用 mode::detect 的 L2 求值链）
    let compiled = crate::l1::compiler::compile(expr).map_err(|e| crate::GxpError::Runtime {
        detail: format!("L1 编译失败: {e}"),
    })?;
    // where 的 L1 预筛可被 captures 旁路（captures_has_field 时跳过 l1_match），
    // 同一 prepared 无法对两种路径给不同豁免 → 保守传空集，不豁免（v0.49 Opt A）
    let prepared = crate::mode::prepared::prepare_expr(expr, &[])?;
    // B14：where 的 EXPR 同样只在 l1_exact（纯字面量且 L1 组精确编码语义）
    // 时可走「L1 命中即通过」捷径；Regex / 混合 Or 必须走 L2 全树精判
    // （原 `!has_any_predicate` 门槛对 `"a" && /re/`、`"a" && "b" || "c"`
    // 等 where 表达式不健全——正则右支从不检查、And 分支被部分组误杀）。
    let where_l1_exact = crate::mode::detect::l1_exact(expr);
    // 收集 where EXPR 引用的所有字段名（用于判断 captures 是否已含某字段值，
    // 若含则跳过 L1 前缀粗筛——L1 粗筛只对行内字段有意义）。
    let pred_fields = collect_pred_fields(expr);

    let mut out = Vec::new();
    for m in lines {
        // 注意：若 captures 已含谓词引用的任一字段，跳过 L1 前缀粗筛
        // （L1 粗筛只看行内 field= / field:，对 captures 提供的字段值无效）
        let captures_has_field = pred_fields
            .iter()
            .any(|f| m.captures.contains_key(f.as_str()));
        if !captures_has_field && !compiled.l1_match(&m.line) {
            continue;
        }
        if where_l1_exact {
            out.push(m);
            continue;
        }
        let mut captures = m.captures.clone();
        if matches!(
            crate::mode::eval::eval_prepared_with_captures(&prepared, &m.line, &mut captures),
            crate::l2::EvalResult::True
        ) {
            let mut ml = MatchedLine::with_captures(m.line_no, &m.line, captures);
            ml.path = m.path.clone();
            ml.match_bytes = m.match_bytes.clone();
            out.push(ml);
        }
    }
    Ok(out)
}

/// 递归收集 EXPR 中所有 `Prim::Pred` 引用的字段名（`Compare`/`In`/`Exists`/`Between`）。
///
/// 用于 `apply_where` 判断上游 captures 是否已含谓词字段——若含则跳过 L1 前缀粗筛。
fn collect_pred_fields(expr: &Expr) -> Vec<String> {
    fn walk(e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::And(l, r) | Expr::Or(l, r) => {
                walk(l, out);
                walk(r, out);
            }
            Expr::Not(inner) => walk(inner, out),
            Expr::Prim(Prim::Paren(inner)) => walk(inner, out),
            Expr::Prim(Prim::Pred(Pred::Compare { name, .. })) => out.push(name.clone()),
            Expr::Prim(Prim::Pred(Pred::In { name, .. })) => out.push(name.clone()),
            Expr::Prim(Prim::Pred(Pred::Exists { name })) => out.push(name.clone()),
            Expr::Prim(Prim::Pred(Pred::Between { name, .. })) => out.push(name.clone()),
            Expr::Prim(
                Prim::String(_)
                | Prim::Regex(_)
                | Prim::Bare(_)
                | Prim::Ast { .. }
                | Prim::Capture { .. },
            ) => {}
        }
    }
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_line(line_no: usize, line: &str) -> MatchedLine {
        MatchedLine::new(line_no, line.as_bytes())
    }

    fn emit_json(lines: &[MatchedLine]) -> String {
        let mut buf = Vec::new();
        emit_remaining(lines, false, true, false, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn emit_text(lines: &[MatchedLine]) -> String {
        // 默认单文件模式：不带 path 前缀
        let mut buf = Vec::new();
        emit_remaining(lines, false, false, false, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn emit_text_dir(lines: &[MatchedLine]) -> String {
        // 目录模式：path=Some 时加 path: 前缀
        let mut buf = Vec::new();
        emit_remaining(lines, false, false, true, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_emit_json_includes_path_when_some() {
        let mut m = make_line(1, "ERROR");
        m.path = Some(std::sync::Arc::from("src/main.rs"));
        let json = emit_json(&[m]);
        assert!(
            json.contains("\"path\":\"src/main.rs\""),
            "JSON 应含 path: {json}"
        );
    }

    #[test]
    fn test_emit_json_omits_path_when_none() {
        let m = make_line(1, "ERROR");
        let json = emit_json(&[m]);
        assert!(!json.contains("\"path\""), "JSON 不应含 path 字段: {json}");
    }

    #[test]
    fn test_emit_json_includes_captures_when_non_empty() {
        let mut m = make_line(1, "ERROR user=alice");
        m.captures.insert("uid".to_string(), b"user=alice".to_vec());
        let json = emit_json(&[m]);
        assert!(
            json.contains("\"captures\":{\"uid\":\"user=alice\"}"),
            "JSON 应含 captures: {json}"
        );
    }

    #[test]
    fn test_emit_json_omits_captures_when_empty() {
        let m = make_line(1, "ERROR");
        let json = emit_json(&[m]);
        assert!(!json.contains("captures"), "JSON 不应含 captures: {json}");
    }

    // v0.42 修复：gxp 模式 JSON 应含 `match` 字段（与 BARE 模式 schema 一致）
    #[test]
    fn test_emit_json_includes_match_when_some() {
        let mut m = make_line(1, "ERROR here");
        m.match_bytes = Some(std::sync::Arc::from(b"ERROR".as_ref()));
        let json = emit_json(&[m]);
        assert!(
            json.contains("\"match\":\"ERROR\""),
            "JSON 应含 match 字段: {json}"
        );
    }

    #[test]
    fn test_emit_json_omits_match_when_none() {
        // 纯字段谓词（无字面量约束）→ match_bytes=None → 不输出 match
        let m = make_line(1, "ERROR");
        let json = emit_json(&[m]);
        assert!(
            !json.contains("\"match\""),
            "JSON 不应含 match 字段: {json}"
        );
    }

    #[test]
    fn test_emit_text_no_path_no_prefix() {
        // 单文件模式 path=None → 行号:行内容（无路径前缀）
        let m = make_line(1, "ERROR");
        let text = emit_text(&[m]);
        assert_eq!(text, "1:ERROR\n");
    }

    #[test]
    fn test_emit_text_with_path_adds_prefix() {
        // 目录模式 path=Some + show_path_prefix=true → path:行号:行内容
        let mut m = make_line(1, "ERROR");
        m.path = Some(std::sync::Arc::from("src/main.rs"));
        let text = emit_text_dir(&[m]);
        assert_eq!(text, "src/main.rs:1:ERROR\n");
    }

    #[test]
    fn test_emit_text_normalizes_backslash_path() {
        // 路径分隔符统一为正斜杠（目录模式）
        let mut m = make_line(1, "ERROR");
        m.path = Some(std::sync::Arc::from("src\\pipeline\\mod.rs"));
        let text = emit_text_dir(&[m]);
        assert_eq!(text, "src/pipeline/mod.rs:1:ERROR\n");
    }

    #[test]
    fn test_normalize_path_replaces_backslashes() {
        assert_eq!(
            normalize_path("src\\pipeline\\mod.rs"),
            "src/pipeline/mod.rs"
        );
        assert_eq!(normalize_path("src/pipeline/mod.rs"), "src/pipeline/mod.rs");
        assert_eq!(normalize_path("a\\b\\c"), "a/b/c");
    }

    // B1 修复：终止 stage 后非空 stage → fail-loud

    #[test]
    fn test_check_terminal_count_then_head_errors() {
        // count | head 3 → count 是终止动词，head 在后面 → 报错
        let stages = vec![Stage::Count, Stage::Head(3)];
        let ordered = reorder_stages(&stages);
        let Err(e) = check_terminal_stages(&ordered) else {
            panic!("count 后跟 head 应报错");
        };
        let msg = format!("{e}");
        assert!(msg.contains("count"), "错误消息应含 count: {msg}");
        assert!(msg.contains("head"), "错误消息应含 head: {msg}");
        assert!(msg.contains("终止动词"), "应说明是终止动词: {msg}");
    }

    #[test]
    fn test_check_terminal_count_alone_ok() {
        // count 单独使用 → 无错
        let stages = vec![Stage::Count];
        let ordered = reorder_stages(&stages);
        assert!(check_terminal_stages(&ordered).is_ok());
    }

    #[test]
    fn test_check_terminal_group_by_alone_ok() {
        // group by FIELD 单独使用 → 无错
        let stages = vec![Stage::GroupBy("_tag".into())];
        let ordered = reorder_stages(&stages);
        assert!(check_terminal_stages(&ordered).is_ok());
    }

    #[test]
    fn test_check_terminal_stats_then_fields_errors() {
        // stats cost_ms | fields _ts → stats 是终止动词 → 报错
        let stages = vec![
            Stage::Stats {
                field: "cost_ms".into(),
            },
            Stage::Fields(vec!["_ts".into()]),
        ];
        let ordered = reorder_stages(&stages);
        assert!(check_terminal_stages(&ordered).is_err());
    }

    #[test]
    fn test_check_terminal_non_terminal_chain_ok() {
        // 非终止动词链：rank | sort | distinct | head → 无错
        let stages = vec![
            Stage::Rank {
                top: 20,
                by: "cost_ms".into(),
                desc: true,
            },
            Stage::Sort {
                by: Some("cost_ms".into()),
                desc: false,
            },
            Stage::Distinct { by: None },
            Stage::Head(10),
        ];
        let ordered = reorder_stages(&stages);
        assert!(check_terminal_stages(&ordered).is_ok());
    }

    // --- apply_where + captures 优先（见下）---

    use crate::dsl::ast::CmpOp;

    fn where_pred(field: &str, op: CmpOp, value: &str) -> Expr {
        Expr::Prim(Prim::Pred(Pred::Compare {
            name: field.to_string(),
            op,
            value: value.to_string(),
        }))
    }

    #[test]
    fn test_apply_where_reads_capture_when_no_line_field() {
        // capture 提取的值供 where 谓词使用（行内无 cost 字段）
        // 修复前：eval_prepared_pred 不读 captures → NotNumeric → 0 行
        // 修复后：captures["cost"] 优先 → 2 行命中
        let mut m1 = MatchedLine::new(1, b"ERROR req-67890");
        m1.captures.insert("cost".to_string(), b"67890".to_vec());
        let mut m2 = MatchedLine::new(2, b"ERROR req-100");
        m2.captures.insert("cost".to_string(), b"100".to_vec());
        // where cost>500 → 只有 m1 命中
        let out = apply_where(vec![m1, m2], &where_pred("cost", CmpOp::Gt, "500")).unwrap();
        assert_eq!(out.len(), 1, "只有 m1 cost=67890>500 应命中");
        assert_eq!(out[0].line_no, 1);
    }

    #[test]
    fn test_apply_where_capture_preferred_over_line_field() {
        // 同名字段：capture 值优先于行内字段
        // m1 行内 cost=10，capture cost=1000 → 1000>500 命中
        // m2 行内 cost=1000，无 capture → 行内 1000>500 命中
        let mut m1 = MatchedLine::new(1, b"cost=10");
        m1.captures.insert("cost".to_string(), b"1000".to_vec());
        let m2 = MatchedLine::new(2, b"cost=1000");
        let out = apply_where(vec![m1, m2], &where_pred("cost", CmpOp::Gt, "500")).unwrap();
        assert_eq!(out.len(), 2, "m1 capture=1000 优先, m2 行内=1000, 都应命中");
        assert_eq!(out[0].line_no, 1);
        assert_eq!(out[1].line_no, 2);
    }

    #[test]
    fn test_apply_where_falls_back_to_line_field_when_no_capture() {
        // 无 capture 时回退到行内字段
        let m1 = MatchedLine::new(1, b"cost=100");
        let m2 = MatchedLine::new(2, b"cost=1000");
        let out = apply_where(vec![m1, m2], &where_pred("cost", CmpOp::Gt, "500")).unwrap();
        assert_eq!(out.len(), 1, "只有 m2 行内 cost=1000>500 命中");
        assert_eq!(out[0].line_no, 2);
    }

    #[test]
    fn test_apply_where_capture_missing_field_returns_not_numeric() {
        // capture 不命中且行内无字段 → NotNumeric → 不命中
        let m1 = MatchedLine::new(1, b"ERROR no field");
        let out = apply_where(vec![m1], &where_pred("cost", CmpOp::Gt, "500")).unwrap();
        assert!(out.is_empty(), "无字段 → NotNumeric → 0 命中");
    }

    #[test]
    fn test_apply_where_preserves_captures_through_l2_eval() {
        // 验证 where 求值后 captures 仍可被下游 stage 使用
        // m1 capture cost=1000, m2 capture cost=100，where cost>500 → 只 m1
        // m1 的 captures 应保留（含 cost=1000）
        let mut m1 = MatchedLine::new(1, b"req-a");
        m1.captures.insert("cost".to_string(), b"1000".to_vec());
        let mut m2 = MatchedLine::new(2, b"req-b");
        m2.captures.insert("cost".to_string(), b"100".to_vec());
        let out = apply_where(vec![m1, m2], &where_pred("cost", CmpOp::Gt, "500")).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].captures.get("cost").map(|v| v.as_slice()),
            Some(b"1000".as_ref()),
            "where 命中后 captures 应保留"
        );
    }
}
