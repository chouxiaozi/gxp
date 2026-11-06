//! `--strict` 字段探活（设计文档 §2.4 示例 5）。
//!
//! 查询引用的字段名在输入中从未出现 → fail-loud 报错。

use crate::l2::extract::ExtractedFields;
use crate::mode::prepared::{PreparedExpr, PreparedPred, PreparedPrim};

/// 从 `PreparedExpr` 收集所有字段谓词引用的字段名。
///
/// 递归遍历 Or/And/Not/Paren，在 `PreparedPred` 节点取 `field`。
/// 用于 `--strict` 模式：检查这些字段是否在输入中出现过。
pub(crate) fn collect_field_names(expr: &PreparedExpr) -> Vec<String> {
    let mut names = Vec::new();
    collect_field_names_inner(expr, &mut names);
    names
}

pub(crate) fn collect_field_names_inner(expr: &PreparedExpr, out: &mut Vec<String>) {
    match expr {
        PreparedExpr::Or(l, r) | PreparedExpr::And(l, r) => {
            collect_field_names_inner(l, out);
            collect_field_names_inner(r, out);
        }
        PreparedExpr::Not(e) => collect_field_names_inner(e, out),
        PreparedExpr::Prim(PreparedPrim::Paren(e)) => collect_field_names_inner(e, out),
        PreparedExpr::Prim(PreparedPrim::Pred(p)) => match p {
            PreparedPred::Compare { field, .. }
            | PreparedPred::In { field, .. }
            | PreparedPred::Exists { field, .. }
            | PreparedPred::Between { field, .. }
            | PreparedPred::TimeBetween { field, .. } => {
                out.push(field.clone());
            }
        },
        // capture() 的 name 是提取结果字段名，不是输入字段——不进 strict 探活
        PreparedExpr::Prim(
            PreparedPrim::Literal { .. } | PreparedPrim::Ast { .. } | PreparedPrim::Capture { .. },
        ) => {}
    }
}

/// `--strict` 字段探活：扫描输入，检查查询引用的字段是否出现过。
///
/// 设计文档 §2.4 示例 5：
/// ```text
/// gxp --strict 'ERROR && http_status>400' app.log
/// # 错误：字段 ['http_status'] 在 app.log 的候选集中从未出现；实际探测到的字段：['cost_ms', 'msg', 'req']
/// ```
///
/// 逻辑：
/// 1. 从 `PreparedExpr` 提取所有字段名
/// 2. 逐行扫描整个输入，用 `ExtractedFields::find_value` 检测字段是否出现
/// 3. 未出现的字段 → 返回错误（列出缺失字段 + 实际探测到的字段）
///
/// 返回 `Ok(())` 如果所有字段都出现过，或字段列表为空（无字段谓词）。
/// 扫描 `bytes`，返回 `(found, missing)`。
///
/// 核心扫描逻辑：逐行检测 `query_fields_unique` 中哪些字段出现过。
/// 找到所有字段后提前退出。未找到的字段放入 `missing`。
pub(crate) fn scan_fields_in_bytes(
    bytes: &[u8],
    query_fields_unique: &[String],
) -> std::collections::HashSet<String> {
    let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut start = 0;
    while start < bytes.len() {
        let end = memchr::memchr(b'\n', &bytes[start..])
            .map(|i| start + i)
            .unwrap_or(bytes.len());
        let line = &bytes[start..end];
        for field in query_fields_unique {
            if !found.contains(field)
                && ExtractedFields::find_value(line, field.as_bytes()).is_some()
            {
                found.insert(field.clone());
            }
        }
        if found.len() == query_fields_unique.len() {
            break;
        }
        start = end + 1;
        if end == bytes.len() {
            break;
        }
    }
    found
}

/// 收集 `bytes` 中实际出现的字段名（最多 20 个），用于错误信息。
pub(crate) fn collect_detected_fields(bytes: &[u8]) -> Vec<String> {
    let mut detected: Vec<String> = Vec::new();
    let mut detected_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scan_start = 0;
    while scan_start < bytes.len() && detected.len() < 20 {
        let end = memchr::memchr(b'\n', &bytes[scan_start..])
            .map(|i| scan_start + i)
            .unwrap_or(bytes.len());
        let line = &bytes[scan_start..end];
        let fields = ExtractedFields::extract(line);
        for (k, _v) in &fields.kv {
            let key_str = String::from_utf8_lossy(k).into_owned();
            if detected_set.insert(key_str.clone()) {
                detected.push(key_str);
            }
        }
        scan_start = end + 1;
        if end == bytes.len() {
            break;
        }
    }
    detected
}

/// 从 `PreparedExpr` 收集字段名并去重，返回 `Vec<String>`。
pub(crate) fn collect_unique_field_names(prepared: &PreparedExpr) -> Vec<String> {
    let query_fields = collect_field_names(prepared);
    let mut seen = std::collections::HashSet::new();
    query_fields
        .into_iter()
        .filter(|f| seen.insert(f.clone()))
        .collect()
}

/// 构造 `--strict` 缺失字段错误。
///
/// 列出缺失字段 + 实际探测到的字段（取自 `sources`，跨所有文件并集）。
/// `location` 为单文件路径或目录路径，用于错误信息上下文。
pub(crate) fn strict_missing_error(
    missing: Vec<&String>,
    sources: &[&[u8]],
    location: &str,
) -> crate::GxpError {
    // 跨所有源收集检测到的字段（并集，最多 20 个）
    let mut detected: Vec<String> = Vec::new();
    let mut detected_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for bytes in sources {
        if detected.len() >= 20 {
            break;
        }
        let file_detected = collect_detected_fields(bytes);
        for f in file_detected {
            if detected_set.insert(f.clone()) {
                detected.push(f);
            }
        }
    }
    let missing_fmt: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
    crate::GxpError::Runtime {
        detail: format!(
            "字段 {} 在{}的候选集中从未出现；实际探测到的字段：{}",
            format_field_list(&missing_fmt),
            location,
            format_field_list(&detected.iter().map(|s| s.as_str()).collect::<Vec<&str>>())
        ),
    }
}

/// `--strict` field probe（设计文档 §2.4 示例 5）。
///
/// 查询引用的字段名在输入中从未出现 → fail-loud 报错。
///
/// `sources` 为一个或多个输入源的字节切片（单文件 1 个，目录多个文件）。
/// `location` 为错误信息中的位置描述（如 `""` 或 `"目录 ./src/ "`）。
pub(crate) fn strict_field_probe(
    prepared: &PreparedExpr,
    sources: &[&[u8]],
    location: &str,
) -> crate::Result<()> {
    let query_fields_unique = collect_unique_field_names(prepared);
    if query_fields_unique.is_empty() {
        return Ok(());
    }
    let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();
    for bytes in sources {
        if found.len() == query_fields_unique.len() {
            break;
        }
        let file_found = scan_fields_in_bytes(bytes, &query_fields_unique);
        found.extend(file_found);
    }
    let missing: Vec<&String> = query_fields_unique
        .iter()
        .filter(|f| !found.contains(*f))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(strict_missing_error(missing, sources, location))
    }
}

/// 格式化字段列表为 `['field1', 'field2']` 风格。
pub(crate) fn format_field_list(fields: &[&str]) -> String {
    let quoted: Vec<String> = fields.iter().map(|f| format!("'{f}'")).collect();
    format!("[{}]", quoted.join(", "))
}
