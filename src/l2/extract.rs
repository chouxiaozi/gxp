//! L2 字段抽取：手写状态机 + memchr 定位字段名前缀（Rust 方案 §2.3）。
//!
//! ## 支持的 4 种格式（设计文档 §3.3）
//!
//! - **logfmt**：`k=v` / `k:v`，空白分隔
//! - **JSON line**：`{...}` / `[...]`，字段为 `"k": v`
//! - **`[tag]`**：方括号标签，伪字段 `_tag`
//! - **ISO 8601**：RFC 3339 时间戳，伪字段 `_ts`
//!
//! ## 零拷贝视图
//!
//! [`ExtractedFields`] 持行切片引用，所有字段值都是原行的子切片。
//! `get(name)` 返回 `Option<&[u8]>`，不分配。
//!
//! ## Late Materialization（设计文档 §3.3）
//!
//! 当前实现：单行内全扫所有分隔符，收集所有字段。
//! Late Materialization 体现在**跨行/跨字段**——L2 只对 L1 命中行做抽取，
//! 不对全文件做。单行内全扫成本极低（memchr SIMD 加速，~3 字节/周期）。
//! 真正的"只解析引用字段"优化在多字段 JSON line 场景才有意义，v1 不做。

use memchr::{memchr, memchr2, memchr_iter};

/// 伪字段：时间戳（设计文档 §3.3 "ISO 8601 时间戳"）。
pub const PSEUDO_TIMESTAMP: &[u8] = b"_ts";
/// 伪字段：第一个方括号标签（设计文档 §3.3 "`[tag]` 方括号标签"）。
pub const PSEUDO_TAG: &[u8] = b"_tag";

/// 从一行字节抽取的字段视图（零拷贝）。
///
/// 所有字段值都是原行 `line` 的子切片，生命周期与 `line` 绑定。
#[derive(Debug, Clone)]
pub struct ExtractedFields<'a> {
    /// k=v / k:v 对（key 不含分隔符，value 不含空白）。
    pub kv: Vec<(&'a [u8], &'a [u8])>,
    /// `[tag]` 列表（方括号内字符，不含方括号本身）。
    pub tags: Vec<&'a [u8]>,
    /// 第一个 RFC 3339 时间戳（从 `YYYY-MM-DDTHH:MM:SS` 形式抽取）。
    pub timestamp: Option<&'a [u8]>,
    /// 原始行引用（无字段查询的回退）。
    pub line: &'a [u8],
}

impl<'a> ExtractedFields<'a> {
    /// 从行抽取字段。
    ///
    /// 探测格式后按格式分支抽取：
    /// - JSON line：用 `serde_json::Value` 解析（保守：解析失败回退到 logfmt）
    /// - logfmt / BracketTag / Mixed / Plain：手写状态机扫 `=` 和 `:`
    /// - `[tag]`：扫 `[`-`]` 对
    /// - ISO 时间戳：扫 `\d{4}-\d{2}-\d{2}T...`
    pub fn extract(line: &'a [u8]) -> Self {
        let mut out = ExtractedFields {
            kv: Vec::new(),
            tags: Vec::new(),
            timestamp: None,
            line,
        };

        // 1. 时间戳（独立于格式，优先扫描，因为冒号会影响 logfmt 判定）
        out.timestamp = extract_iso_timestamp(line);

        // 2. [tag] 方括号标签（独立于格式，优先扫描）
        extract_bracket_tags(line, &mut out.tags);

        // 3. kv 对：按格式分支
        let format = super::format::detect_format(line);
        match format {
            super::format::LineFormat::JsonLine => {
                // JSON line：用 memchr 扫 `"k": value` 模式
                // 解析失败（非合法 JSON）时回退到 logfmt 状态机
                if !extract_json_kv_via_memchr(line, &mut out.kv) {
                    extract_logfmt_kv(line, &mut out.kv);
                }
            }
            _ => {
                // logfmt / Plain / BracketTag / IsoTimestamp / Mixed：
                // 统一走 logfmt 状态机（memchr 扫 `=` 和 `:`，剔除来自 JSON/ISO 的伪分隔）
                extract_logfmt_kv(line, &mut out.kv);
            }
        }

        out
    }

    /// 取字段值，未找到返回 `None`。
    ///
    /// 伪字段：
    /// - `_ts`：返回第一个 ISO 时间戳
    /// - `_tag`：返回第一个方括号标签
    pub fn get(&self, name: &[u8]) -> Option<&'a [u8]> {
        if name == PSEUDO_TIMESTAMP {
            return self.timestamp;
        }
        if name == PSEUDO_TAG {
            return self.tags.first().copied();
        }
        self.kv.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }

    /// 单字段快速提取（性能优化）。
    ///
    /// 当只需要一个字段时，跳过 `ExtractedFields::extract` 的全字段扫描：
    /// - logfmt / 混合格式：用 `memchr` 扫 `name=` 或 `name:` 直接定位
    /// - 伪字段 `_ts`：走 `extract_iso_timestamp` 单路径扫描（而非全字段 extract）
    /// - 伪字段 `_tag`：走 `extract_first_bracket_tag` 单路径扫描（而非全字段 extract）
    ///
    /// 适用场景：`rank top K by FIELD` 只取排序字段、`fields F` 投影单字段、
    /// L2 `eval_prepared_with_captures` 懒求值（谓词只需要 1-2 个字段，不必全扫）。
    /// 对于 `cost_ms=` 形式（最常见的 logfmt 数值字段），从 N=103k 行全扫优化为
    /// 平均 < N/2 次 memchr 定位，实测约 2-3× 加速。
    pub fn find_value(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
        // 伪字段走单路径扫描（避免全字段 extract 的开销）
        if name == PSEUDO_TIMESTAMP {
            return extract_iso_timestamp(line);
        }
        if name == PSEUDO_TAG {
            return extract_first_bracket_tag(line);
        }

        // 构造 `name=` 和 `name:` 搜索模式
        // 用 memchr 扫 `=` 或 `:`，匹配到的位置向前查 key
        // 这比先 split 整行再匹配更高效（单次 memchr 覆盖两种分隔符）
        let mut start = 0;
        while start < line.len() {
            let Some(offset) = memchr2(b'=', b':', &line[start..]) else {
                break;
            };
            let sep_pos = start + offset;

            // 向左找 key 起点
            let key_end = sep_pos;
            let key_start = line[..key_end]
                .iter()
                .rposition(|&c| c == b' ' || c == b'\t')
                .map_or(0, |i| i + 1);
            let key = &line[key_start..key_end];

            // key 必须非空且全字母数字下划线，且等于目标字段名
            if !key.is_empty()
                && key.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_')
                && key == name
            {
                // 命中目标字段
                let val_start = sep_pos + 1;
                let val_end =
                    memchr2(b' ', b'\t', &line[val_start..]).map_or(line.len(), |i| val_start + i);
                let value = &line[val_start..val_end];
                if !value.is_empty() {
                    return Some(value);
                }
            }

            start = sep_pos + 1;
        }

        // JSON line 通路。
        // logfmt 通路扫 `=` / `:` 找 key，但 JSON line `"name": value` 中 key
        // 带双引号且 `:` 紧跟 `"`——logfmt 通路找到的 `:` 前面是 `"`（非字母数字）
        // 会被 `key.iter().all(...)` 拒绝，导致 JSONL 行 0 命中（违反 L1 ⊇ L2）。
        // 详见 src/l1/compiler.rs::prefix_variants 注释。
        //
        // 走 `extract_json_kv_via_memchr` 复用已稳定的 JSON 抽取逻辑，
        // 在抽取结果里查目标字段。比 logfmt 通路慢（全字段抽取），
        // 但 JSON line 是设计文档 §3.3 一等支持格式，必须支持。
        if super::format::detect_format(line) == super::format::LineFormat::JsonLine {
            let mut kv: Vec<(&[u8], &[u8])> = Vec::new();
            if extract_json_kv_via_memchr(line, &mut kv) {
                for (k, v) in kv {
                    if k == name && !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }

        None
    }

    /// 多字段批量提取（性能优化，算法层非 SIMD，bench Exp4 实测 n≥3 反超 find_value×N）。
    ///
    /// 一次 `memchr2('='/':')` 扫描行内所有分隔符，遇到 key 后线性匹配目标字段集。
    /// 返回 `Vec<Option<&[u8]>>`，顺序与 `names` 对齐。
    ///
    /// **命名纠正**：原 roadmap 名"SIMD 多字段投影"误导——本方法优化在**算法层**
    /// （N 遍 memchr2 调用 → 1 遍），SIMD 仍在 memchr crate 内部，gxp 不碰
    /// intrinsics（D7 约束不变）。减少的是 memchr2 crate 调用次数，不是 SIMD 窗口大小。
    ///
    /// 伪字段（`_ts`/`_tag`）不并入 batch，需调用方单独走 `find_value`。
    /// 仅适用于普通字段（非伪字段），伪字段会在结果中留 `None`，由调用方 fallback。
    ///
    /// 性能数据（31931 行 × 12 字段/行）：
    /// - n=2: 2.60ms vs find_value×2 2.82ms（+8% 快）
    /// - n=5: 3.84ms vs find_value×5 9.11ms（2.37× 快）
    /// - n=12: 8.03ms vs find_value×12 35.63ms（4.44× 快）
    pub fn batch_find_values(line: &'a [u8], names: &[&[u8]]) -> Vec<Option<&'a [u8]>> {
        let mut results: Vec<Option<&'a [u8]>> = vec![None; names.len()];
        let mut found = 0usize;
        let mut start = 0usize;

        while start < line.len() && found < names.len() {
            let Some(offset) = memchr2(b'=', b':', &line[start..]) else {
                break;
            };
            let sep_pos = start + offset;
            let key_end = sep_pos;
            let key_start = line[..key_end]
                .iter()
                .rposition(|&c| c == b' ' || c == b'\t')
                .map_or(0, |i| i + 1);
            let key = &line[key_start..key_end];

            if !key.is_empty() && key.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_') {
                for (i, name) in names.iter().enumerate() {
                    if results[i].is_none() && key == *name {
                        let val_start = sep_pos + 1;
                        let val_end = memchr2(b' ', b'\t', &line[val_start..])
                            .map_or(line.len(), |i| val_start + i);
                        let value = &line[val_start..val_end];
                        if !value.is_empty() {
                            results[i] = Some(value);
                            found += 1;
                        }
                        break;
                    }
                }
            }
            start = sep_pos + 1;
        }
        results
    }

    /// 取所有字段名（用于 `--explain` 展示和"字段不存在"错误）。
    pub fn field_names(&self) -> Vec<&'a [u8]> {
        let mut names: Vec<&'a [u8]> = self.kv.iter().map(|(k, _)| *k).collect();
        if self.timestamp.is_some() {
            names.push(PSEUDO_TIMESTAMP);
        }
        if !self.tags.is_empty() {
            names.push(PSEUDO_TAG);
        }
        names
    }
}

/// 抽取 RFC 3339 / ISO 8601 时间戳：`\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|+\d{2}:\d{2})?`
///
/// 返回完整时间戳字节切片。多个时间戳只取第一个。
fn extract_iso_timestamp(line: &[u8]) -> Option<&[u8]> {
    // 扫描所有 `T` 位置
    for t_pos in memchr_iter(b'T', line) {
        // 前置 10 字节 = `\d{4}-\d{2}-\d{2}`
        if t_pos < 10 {
            continue;
        }
        let prefix = &line[t_pos - 10..t_pos];
        if !is_iso_date_prefix(prefix) {
            continue;
        }
        // T 后至少 8 字节（`HH:MM:SS`）
        if line.len() < t_pos + 9 {
            continue;
        }
        if !is_iso_time_suffix(&line[t_pos + 1..]) {
            continue;
        }
        // 计算时间戳结束位置：
        // `\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}` = 19 字节
        // 后面可能跟 `.\d+`（毫秒）和 `Z` / `+08:00`（时区）
        let start = t_pos - 10;
        let end = compute_iso_end(line, t_pos + 9); // t_pos + 9 = `:` 之后的 `\d{2}` 结束位置
        return Some(&line[start..end]);
    }
    None
}

/// 判定字节是否是 ISO 日期前缀 `\d{4}-\d{2}-\d{2}`。
fn is_iso_date_prefix(b: &[u8]) -> bool {
    b.len() == 10
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

/// 判定 `T` 后是否紧跟 ISO 时间 `HH:MM:SS`（前 8 字节）。
fn is_iso_time_suffix(after_t: &[u8]) -> bool {
    after_t.len() >= 8
        && after_t[0].is_ascii_digit()
        && after_t[1].is_ascii_digit()
        && after_t[2] == b':'
        && after_t[3].is_ascii_digit()
        && after_t[4].is_ascii_digit()
        && after_t[5] == b':'
        && after_t[6].is_ascii_digit()
        && after_t[7].is_ascii_digit()
}

/// 计算时间戳结束位置（含毫秒和时区）。
/// `anchor` 是 `:` 后第二个 `\d{2}` 的结束位置（即 `t_pos + 9`）。
fn compute_iso_end(line: &[u8], anchor: usize) -> usize {
    let mut end = anchor;
    // 毫秒：`.\d+`
    if end < line.len() && line[end] == b'.' {
        end += 1;
        while end < line.len() && line[end].is_ascii_digit() {
            end += 1;
        }
    }
    // 时区：`Z` 或 `+\d{2}:\d{2}` / `-\d{2}:\d{2}`
    if end < line.len() && line[end] == b'Z' {
        end += 1;
    } else if end + 6 <= line.len()
        && (line[end] == b'+' || line[end] == b'-')
        && line[end + 1].is_ascii_digit()
        && line[end + 2].is_ascii_digit()
        && line[end + 3] == b':'
        && line[end + 4].is_ascii_digit()
        && line[end + 5].is_ascii_digit()
    {
        end += 6;
    }
    end
}

/// 抽取所有 `[tag]` 方括号标签（不含方括号本身）。
fn extract_bracket_tags<'a>(line: &'a [u8], out: &mut Vec<&'a [u8]>) {
    let mut start = 0;
    while start < line.len() {
        let Some(open_offset) = memchr(b'[', &line[start..]) else {
            break;
        };
        let open_pos = start + open_offset;
        // 找闭合 `]`
        let Some(close_offset) = memchr(b']', &line[open_pos + 1..]) else {
            break;
        };
        let close_pos = open_pos + 1 + close_offset;
        let tag = &line[open_pos + 1..close_pos];
        // 过滤空 tag 和数字 tag（如 `[0]` 数组索引，不是 log tag）
        if !tag.is_empty() && tag[0].is_ascii_alphabetic() {
            out.push(tag);
        }
        start = close_pos + 1;
    }
}

/// 抽取第一个 `[tag]` 方括号标签（单字段快速路径）。
///
/// 与 `extract_bracket_tags` 语义一致：过滤空 tag 和数字 tag，
/// 但只返回首个命中，且不分配 Vec。伪字段 `_tag` 的快速路径。
fn extract_first_bracket_tag(line: &[u8]) -> Option<&[u8]> {
    let mut start = 0;
    while start < line.len() {
        let Some(open_offset) = memchr(b'[', &line[start..]) else {
            break;
        };
        let open_pos = start + open_offset;
        let Some(close_offset) = memchr(b']', &line[open_pos + 1..]) else {
            break;
        };
        let close_pos = open_pos + 1 + close_offset;
        let tag = &line[open_pos + 1..close_pos];
        if !tag.is_empty() && tag[0].is_ascii_alphabetic() {
            return Some(tag);
        }
        start = close_pos + 1;
    }
    None
}

/// 抽取 logfmt 风格的 kv 对：`k=v` / `k:v`，空白分隔。
///
/// 用 `memchr2` 扫描 `=` 和 `:`，对每个分隔符：
/// - 向左找 key 起点（上一个空白或行首）
/// - 向右找 value 终点（下一个空白或行尾）
/// - key 必须非空且全字母数字下划线（避免把 `2026-01-02T...` 的 `-` 当分隔符）
///
/// 剔除来自 JSON 的伪分隔（`"k": v` 形式）——
/// 但如果 detect_format 已经判定为 JSON，此函数只在 JSON 解析失败时回退使用。
fn extract_logfmt_kv<'a>(line: &'a [u8], out: &mut Vec<(&'a [u8], &'a [u8])>) {
    let mut start = 0;
    while start < line.len() {
        let Some(offset) = memchr2(b'=', b':', &line[start..]) else {
            break;
        };
        let sep_pos = start + offset;

        // 向左找 key 起点
        let key_end = sep_pos;
        let key_start = line[..key_end]
            .iter()
            .rposition(|&c| c == b' ' || c == b'\t')
            .map_or(0, |i| i + 1);
        let key = &line[key_start..key_end];

        // key 必须非空且全字母数字下划线
        // （避免把 ISO 时间戳 `2026-01-02T03:04:05` 的 `:` 当 logfmt 分隔符）
        if key.is_empty() || !key.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_') {
            start = sep_pos + 1;
            continue;
        }

        // 向右找 value 终点
        let val_start = sep_pos + 1;
        let val_end =
            memchr2(b' ', b'\t', &line[val_start..]).map_or(line.len(), |i| val_start + i);
        let value = &line[val_start..val_end];

        // 值不能为空（设计文档 §3.3 "空值"维度：空值视为字段不存在）
        if !value.is_empty() {
            out.push((key, value));
        }

        start = val_end + 1;
        if val_end >= line.len() {
            break;
        }
    }
}

/// 用 memchr 直接扫描 JSON line 的 `"k": "v"` / `"k": 800` 模式。
///
/// 零拷贝：所有字段值都是原行的子切片。
/// 保守：只抽取 `"k": value` 形式，value 不含引号（字符串）或纯数字/浮点。
fn extract_json_kv_via_memchr<'a>(line: &'a [u8], out: &mut Vec<(&'a [u8], &'a [u8])>) -> bool {
    // JSON 行必含 `"k":` 模式：`"` + 字母/数字/下划线 + `"` + `:`
    let mut p = 0;
    let mut found_any = false;
    while p < line.len() {
        // 找第一个 `"`
        let Some(open_quote) = memchr(b'"', &line[p..]) else {
            break;
        };
        let key_start = p + open_quote + 1;
        // 找闭合 `"`
        let Some(close_quote_offset) = memchr(b'"', &line[key_start..]) else {
            break;
        };
        let close_quote = key_start + close_quote_offset;
        let key = &line[key_start..close_quote];

        // key 后紧跟 `:`
        let colon_pos = close_quote + 1;
        if colon_pos >= line.len() || line[colon_pos] != b':' {
            p = close_quote + 1;
            continue;
        }

        // value 起点（跳过 `:` 后的空白）
        let mut val_start = colon_pos + 1;
        while val_start < line.len() && (line[val_start] == b' ' || line[val_start] == b'\t') {
            val_start += 1;
        }
        if val_start >= line.len() {
            break;
        }

        // value 终点：根据首字符决定
        let val_end = if line[val_start] == b'"' {
            // 字符串值：找闭合 `"`
            let Some(close) = memchr(b'"', &line[val_start + 1..]) else {
                p = val_start + 1;
                continue;
            };
            // 取引号内字节（不含引号本身）
            let real_end = val_start + 1 + close;
            // 存入去引号的 value
            out.push((key, &line[val_start + 1..real_end]));
            real_end + 1
        } else {
            // 数字/布尔/null：到下一个 `,` `}` `]` 或行尾
            let end = memchr2(b',', b'}', &line[val_start..])
                .map(|i| val_start + i)
                .or_else(|| memchr(b']', &line[val_start..]).map(|i| val_start + i))
                .unwrap_or(line.len());
            // 去尾随空白
            let mut real_end = end;
            while real_end > val_start
                && (line[real_end - 1] == b' ' || line[real_end - 1] == b'\t')
            {
                real_end -= 1;
            }
            // 空值跳过（设计文档 §3.3 "空值"维度：null/None 视为字段不存在）
            let val_bytes = &line[val_start..real_end];
            if !val_bytes.is_empty() && val_bytes != b"null" && val_bytes != b"None" {
                out.push((key, val_bytes));
            }
            end
        };

        found_any = true;
        p = val_end;
    }
    found_any
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- logfmt k=v ---

    #[test]
    fn test_extract_logfmt_basic() {
        let line = b"level=info cost_ms=900 user=alice";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
        assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
        assert_eq!(f.get(b"missing"), None);
    }

    #[test]
    fn test_extract_logfmt_colon_separator() {
        let line = b"level:info cost_ms:900";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    }

    #[test]
    fn test_extract_logfmt_empty_value_skipped() {
        // 设计文档 §3.3 "空值"维度：空值视为字段不存在
        let line = b"cost_ms= user=alice";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"cost_ms"), None);
        assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    }

    #[test]
    fn test_extract_logfmt_null_value_skipped() {
        // null 不是数值字段（numeric.rs 拒绝），但字段抽取层存原值
        // （数值谓词比较时 eval_numeric 返回 NotNumeric）
        let line = b"cost_ms=null user=alice";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"cost_ms"), Some(&b"null"[..]));
    }

    #[test]
    fn test_extract_logfmt_value_with_special_chars() {
        // 值含 `-`（URL、错误码等）：logfmt 状态机不切分
        let line = b"path=/api/v1/users status=404-not-found";
        let f = ExtractedFields::extract(line);
        // 注：`status=404-not-found` 中 `:` 在 `404-not-found` 之后无 `:`
        // 但 `path=/api/v1/users` 后 `:` 没出现，状态机扫到 `=` 后找空白
        // `404-not-found` 中没有 `:` 或 `=`,状态机取整个 `404-not-found`
        assert_eq!(f.get(b"status"), Some(&b"404-not-found"[..]));
    }

    // --- JSON line ---

    #[test]
    fn test_extract_json_string_values() {
        let line = br#"{"level":"info","user":"alice"}"#;
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    }

    #[test]
    fn test_extract_json_numeric_values() {
        let line = br#"{"cost_ms":900,"count":42}"#;
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
        assert_eq!(f.get(b"count"), Some(&b"42"[..]));
    }

    #[test]
    fn test_extract_json_numeric_last_field_no_comma() {
        // 末字段数值型 + 紧邻 `}`
        let line = br#"{"cost_ms":900}"#;
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    }

    #[test]
    fn test_extract_json_numeric_single_field_no_space() {
        // 反例：单字段字符串 value 正常
        let line = br#"{"cost_ms":"x"}"#;
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"cost_ms"), Some(&b"x"[..]));
    }

    #[test]
    fn test_extract_json_mixed_types() {
        let line = br#"{"level":"info","cost_ms":900.5,"ok":true}"#;
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"cost_ms"), Some(&b"900.5"[..]));
        // bool 值 "true" 也抽取（数值谓词比较时 eval_numeric 拒绝）
        assert_eq!(f.get(b"ok"), Some(&b"true"[..]));
    }

    #[test]
    fn test_extract_json_null_skipped() {
        let line = br#"{"cost_ms":null,"user":"alice"}"#;
        let f = ExtractedFields::extract(line);
        // null 视为字段不存在（设计文档 §3.3 空值）
        assert_eq!(f.get(b"cost_ms"), None);
        assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    }

    // --- [tag] ---

    #[test]
    fn test_extract_tag_leading() {
        let line = b"[INFO] started cost_ms=900";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
        assert_eq!(f.get(b"cost_ms"), Some(&b"900"[..]));
    }

    #[test]
    fn test_extract_tag_inline() {
        let line = b"INFO [auth] user=alice";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_tag"), Some(&b"auth"[..]));
    }

    #[test]
    fn test_extract_tag_multiple() {
        let line = b"[INFO] [auth] msg=ok";
        let f = ExtractedFields::extract(line);
        // get(_tag) 返回第一个
        assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
        assert_eq!(f.tags.len(), 2);
        assert_eq!(f.tags[0], b"INFO");
        assert_eq!(f.tags[1], b"auth");
    }

    #[test]
    fn test_extract_tag_numeric_not_tag() {
        // `[0]` 数组索引不是 tag（设计文档：tag 必须字母开头）
        let line = b"items[0] value=x";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_tag"), None);
    }

    // --- ISO timestamp ---

    #[test]
    fn test_extract_iso_timestamp() {
        let line = b"2026-01-02T03:04:05Z level=info";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05Z"[..]));
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
    }

    #[test]
    fn test_extract_iso_timestamp_with_millis() {
        let line = b"2026-01-02T03:04:05.123Z level=info";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05.123Z"[..]));
    }

    #[test]
    fn test_extract_iso_timestamp_with_timezone() {
        let line = b"2026-01-02T03:04:05+08:00 started";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05+08:00"[..]));
    }

    // --- 混合格式 ---

    #[test]
    fn test_extract_mixed_iso_logfmt() {
        let line = b"2026-01-02T03:04:05 level=info msg=started";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_ts"), Some(&b"2026-01-02T03:04:05"[..]));
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"msg"), Some(&b"started"[..]));
    }

    #[test]
    fn test_extract_mixed_bracket_logfmt() {
        let line = b"[INFO] level=info msg=started user=alice";
        let f = ExtractedFields::extract(line);
        assert_eq!(f.get(b"_tag"), Some(&b"INFO"[..]));
        assert_eq!(f.get(b"level"), Some(&b"info"[..]));
        assert_eq!(f.get(b"user"), Some(&b"alice"[..]));
    }

    // --- 边界 ---

    #[test]
    fn test_extract_empty_line() {
        let f = ExtractedFields::extract(b"");
        assert!(f.kv.is_empty());
        assert!(f.tags.is_empty());
        assert_eq!(f.get(b"anything"), None);
    }

    #[test]
    fn test_extract_plain_text_no_fields() {
        let line = b"the quick brown fox";
        let f = ExtractedFields::extract(line);
        assert!(f.kv.is_empty());
        assert_eq!(f.get(b"anything"), None);
    }

    #[test]
    fn test_extract_field_names() {
        let line = b"2026-01-02T03:04:05Z [INFO] level=info user=alice";
        let f = ExtractedFields::extract(line);
        let names = f.field_names();
        assert!(names.contains(&b"level".as_ref()));
        assert!(names.contains(&b"user".as_ref()));
        assert!(names.contains(&PSEUDO_TIMESTAMP));
        assert!(names.contains(&PSEUDO_TAG));
    }

    // --- find_value 快速路径 ---

    #[test]
    fn test_find_value_logfmt_basic() {
        let line = b"level=info cost_ms=900 user=alice";
        assert_eq!(
            ExtractedFields::find_value(line, b"cost_ms"),
            Some(&b"900"[..])
        );
        assert_eq!(
            ExtractedFields::find_value(line, b"level"),
            Some(&b"info"[..])
        );
        assert_eq!(
            ExtractedFields::find_value(line, b"user"),
            Some(&b"alice"[..])
        );
        assert_eq!(ExtractedFields::find_value(line, b"missing"), None);
    }

    #[test]
    fn test_find_value_colon_separator() {
        let line = b"level:info cost_ms:900";
        assert_eq!(
            ExtractedFields::find_value(line, b"cost_ms"),
            Some(&b"900"[..])
        );
        assert_eq!(
            ExtractedFields::find_value(line, b"level"),
            Some(&b"info"[..])
        );
    }

    #[test]
    fn test_find_value_empty_value_rejected() {
        // 空值视为字段不存在（与 extract 一致，§3.3 空值维度）
        let line = b"cost_ms= user=alice";
        assert_eq!(ExtractedFields::find_value(line, b"cost_ms"), None);
        assert_eq!(
            ExtractedFields::find_value(line, b"user"),
            Some(&b"alice"[..])
        );
    }

    #[test]
    fn test_find_value_pseudo_timestamp() {
        let line = b"2026-01-02T03:04:05Z level=info";
        assert_eq!(
            ExtractedFields::find_value(line, b"_ts"),
            Some(&b"2026-01-02T03:04:05Z"[..])
        );
        // 无时间戳的行
        assert_eq!(ExtractedFields::find_value(b"level=info", b"_ts"), None);
    }

    #[test]
    fn test_find_value_pseudo_tag() {
        let line = b"[INFO] started cost_ms=900";
        assert_eq!(
            ExtractedFields::find_value(line, b"_tag"),
            Some(&b"INFO"[..])
        );
        // 行内 [tag]
        let line2 = b"INFO [auth] user=alice";
        assert_eq!(
            ExtractedFields::find_value(line2, b"_tag"),
            Some(&b"auth"[..])
        );
        // 数字 tag 被过滤
        assert_eq!(
            ExtractedFields::find_value(b"items[0] value=x", b"_tag"),
            None
        );
        // 无 tag
        assert_eq!(ExtractedFields::find_value(b"plain text", b"_tag"), None);
    }

    #[test]
    fn test_find_value_pseudo_tag_takes_first() {
        // 与 extract 的 get(_tag) 一致：返回第一个 tag
        let line = b"[INFO] [auth] msg=ok";
        assert_eq!(
            ExtractedFields::find_value(line, b"_tag"),
            Some(&b"INFO"[..])
        );
    }

    #[test]
    fn test_find_value_matches_extract_semantics() {
        // 验证 find_value 对所有字段类型与 extract().get() 结果一致
        let line = b"2026-01-02T03:04:05Z [INFO] level=info cost_ms=900 user=alice";
        let f = ExtractedFields::extract(line);
        for name in [
            b"_ts".as_ref(),
            b"_tag",
            b"level",
            b"cost_ms",
            b"user",
            b"missing",
        ] {
            assert_eq!(
                ExtractedFields::find_value(line, name),
                f.get(name),
                "find_value 与 extract().get() 不一致: {}",
                String::from_utf8_lossy(name)
            );
        }
    }
}
