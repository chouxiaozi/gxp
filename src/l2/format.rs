//! L2 格式探测：从一行字节判定它的字段格式（Rust 方案 §2.3）。
//!
//! 支持的格式（设计文档 §3.3）：
//! - [`LineFormat::Logfmt`]：`k=v` / `k:v` 密集
//! - [`LineFormat::JsonLine`]：首字符 `{` 或 `[`，JSON line 风格（`"k": v`）
//! - [`LineFormat::BracketTag`]：含 `[tag]` 子串（如 `[INFO] started`）
//! - [`LineFormat::IsoTimestamp`]：含 RFC 3339 / ISO 8601 时间戳
//! - [`LineFormat::Plain`]：无字段特征，纯文本
//! - [`LineFormat::Mixed`]：多种格式特征并存（保守返回，L2 抽取分支逐一尝试）
//!
//! 探测策略（Rust 方案 §2.3 `detect_format`）：
//! 1. 首字符 `{` 或 `[` 且尾字符匹配对应闭合花括号 → `JsonLine`
//!    （`[` 的歧义由"尾字符是否 `]` + 内部是否含 `":` " 进一步判定，避免与 `[tag]` 混淆）
//! 2. 用 `memchr` 一次性扫描 `=` `:` `[` 三种分隔符字节，统计频次
//!    - `=` 计数 ≥ 1 且（`=` + `:`）密度 > 2/行 → `Logfmt`
//!    - 含 `[` 子串但不是 JSON 数组 → `BracketTag`
//! 3. RFC 3339 时间戳探测：4 位数字 + `-` + 2 位 + `-` + 2 位 + `T`
//!    → `IsoTimestamp`
//! 4. 以上都未命中 → `Plain`
//! 5. 多种特征同时存在 → `Mixed`（保守）
//!
//! 注：本探测是启发式粗判，用于 `--explain` 展示和未来的格式分支调度。
//! `extract_field` 对单行的抽取会回退到 `memchr` 全分隔符扫描，
//! 即使探测错误也能正确抽取（只是性能略差）。

use memchr::memchr;

/// L2 行格式探测结果（Rust 方案 §2.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFormat {
    /// 纯文本，无字段特征。
    Plain,
    /// logfmt：`k=v` / `k:v` 密集。
    Logfmt,
    /// JSON line：`{...}` 或 `[...]`，字段为 `"k": v`。
    JsonLine,
    /// 方括号标签：`[INFO]` / `[tag]`。
    BracketTag,
    /// RFC 3339 / ISO 8601 时间戳。
    IsoTimestamp,
    /// 多种格式特征并存（保守返回）。
    Mixed,
}

/// 从一行字节探测格式。
///
/// 详见模块级注释的探测策略。空行返回 [`LineFormat::Plain`]。
///
/// JSON 探测先 trim 首尾 ASCII 空白（空格/制表符/CR/LF）。
/// 原实现用 `line[0]` / `line[line.len()-1]` 判首尾字符，对尾随空格（`{"k":1} `）、
/// CRLF（`{"k":1}\r\n`）、行首空格（` {"k":1}`）的 JSONL 行误判为非 JSON →
/// `find_value` 走不到 JSON 兜底 → 0 命中（正确性 bug，违反"单行 JSONL 顶层字段过滤"
/// 的能力声明）。trim 后所有调用方（find_value / extract / --explain）自动受益。
pub fn detect_format(line: &[u8]) -> LineFormat {
    // trim 首尾 ASCII 空白（空格/制表符/CR/LF），不影响后续 Logfmt/Bracket/ISO 探测
    // （这些探测基于行内字节扫描，不依赖首尾字符）。
    let line = trim_ascii_whitespace(line);
    if line.is_empty() {
        return LineFormat::Plain;
    }

    let mut flags: u8 = 0;
    const F_LOGFMT: u8 = 1 << 0;
    const F_BRACKET: u8 = 1 << 2;
    const F_ISO: u8 = 1 << 3;

    // 1. JSON line：首字符 `{` 或 `[`，且行尾是对应闭合
    //    JSON 对象/数组必含 `":`（`"k":v`），与 `[tag]` 区分
    //    JSON 一旦命中就不再叠加其他标志（`:` `=` 都来自 JSON 语法）
    let first = line[0];
    let last = line[line.len() - 1];
    let is_json = if (first == b'{' && last == b'}') || (first == b'[' && last == b']') {
        // 必须同时含 `"` 和 `:`（`"k": v` 的最小特征）
        memchr(b'"', line).is_some() && memchr(b':', line).is_some()
    } else {
        false
    };
    if is_json {
        return LineFormat::JsonLine;
    }

    // 2. [tag] 探测（非 JSON 场景下）
    //    - 行首 [tag]：`[` 开头 + 字母 + `]`
    //    - 行内 [tag]：`[` 后紧跟字母（如 `INFO [auth] ok`）
    if first == b'[' {
        // 行首 `[`，检查紧跟是否字母
        if line.get(1).is_some_and(|b| b.is_ascii_alphabetic()) {
            flags |= F_BRACKET;
        }
    } else if let Some(pos) = memchr(b'[', line) {
        // 行内 `[`
        if line.get(pos + 1).is_some_and(|b| b.is_ascii_alphabetic()) {
            flags |= F_BRACKET;
        }
    }

    // 3. Logfmt：统计 `=` 和 `:` 频次
    //    扣除 ISO 时间戳贡献的 `:`，避免把时间戳误判为 logfmt `k:v`
    let eq_count = byte_count(line, b'=');
    let colon_count = byte_count(line, b':');
    let iso_colon_count = iso_timestamp_colon_count(line);
    let logfmt_colon_count = colon_count.saturating_sub(iso_colon_count);
    if eq_count >= 1 || logfmt_colon_count >= 1 {
        let total = eq_count + logfmt_colon_count;
        // 密度阈值：每 30 字节 1 个分隔符
        let threshold = (line.len() / 30).max(1);
        if total >= threshold {
            flags |= F_LOGFMT;
        }
    }

    // 4. ISO 8601 / RFC 3339：`\d{4}-\d{2}-\d{2}T`
    if has_iso_timestamp(line) {
        flags |= F_ISO;
    }

    // 5. 汇总
    match flags.count_ones() {
        0 => LineFormat::Plain,
        1 => match flags {
            f if f == F_LOGFMT => LineFormat::Logfmt,
            f if f == F_BRACKET => LineFormat::BracketTag,
            f if f == F_ISO => LineFormat::IsoTimestamp,
            _ => LineFormat::Plain,
        },
        _ => LineFormat::Mixed,
    }
}

/// trim 首尾 ASCII 空白（空格 `0x20` / 制表符 `\t` / CR `\r` / LF `\n`）。
///
/// JSONL 行可能带尾随空格 / CRLF / 行首空格，原 `detect_format` 用
/// `line[0]` / `line[len-1]` 判 JSON 首尾字符，这些边角行被误判为非 JSON。
fn trim_ascii_whitespace(line: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = line.len();
    while start < end && matches!(line[start], b' ' | b'\t' | b'\r' | b'\n') {
        start += 1;
    }
    while end > start && matches!(line[end - 1], b' ' | b'\t' | b'\r' | b'\n') {
        end -= 1;
    }
    &line[start..end]
}

/// 统计行内某字节出现次数。
fn byte_count(line: &[u8], target: u8) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(offset) = memchr(target, &line[start..]) {
        count += 1;
        start += offset + 1;
        if start > line.len() {
            break;
        }
    }
    count
}

/// 检测行内是否含 RFC 3339 / ISO 8601 时间戳前缀：`\d{4}-\d{2}-\d{2}T`。
///
/// 这是 ISO 时间戳的最小锚定特征：4 位年 + `-` + 2 位月 + `-` + 2 位日 + `T`。
/// 时间戳可能出现在行首（`2026-01-02T03:04:05 ...`）或 JSON 字段中
/// （`"ts":"2026-01-02T03:04:05"`），无论位置都该识别。
fn has_iso_timestamp(line: &[u8]) -> bool {
    // 扫描所有 `T` 位置，回看前 10 字节是否匹配 `\d{4}-\d{2}-\d{2}`
    let mut start = 0;
    while let Some(offset) = memchr(b'T', &line[start..]) {
        let t_pos = start + offset;
        // 前置 10 字节 = `\d{4}-\d{2}-\d{2}`
        if t_pos >= 10 {
            let prefix = &line[t_pos - 10..t_pos];
            // 格式：dddd-dd-dd
            if prefix.len() == 10
                && prefix[0].is_ascii_digit()
                && prefix[1].is_ascii_digit()
                && prefix[2].is_ascii_digit()
                && prefix[3].is_ascii_digit()
                && prefix[4] == b'-'
                && prefix[5].is_ascii_digit()
                && prefix[6].is_ascii_digit()
                && prefix[7] == b'-'
                && prefix[8].is_ascii_digit()
                && prefix[9].is_ascii_digit()
            {
                return true;
            }
        }
        start = t_pos + 1;
        if start >= line.len() {
            break;
        }
    }
    false
}

/// 统计 ISO 时间戳贡献的冒号数（`2026-01-02T03:04:05` 贡献 2 个 `:`）。
///
/// 用于从 `colon_count` 中扣除，避免把 ISO 时间戳误判为 logfmt `k:v`。
fn iso_timestamp_colon_count(line: &[u8]) -> usize {
    // 简化：每个 ISO 时间戳前缀贡献 2 个 `:`
    if has_iso_timestamp(line) {
        // 扫描所有 `T` 后是否紧跟 `dd:dd:dd` 模式
        let mut count = 0;
        let mut start = 0;
        while let Some(offset) = memchr(b'T', &line[start..]) {
            let t_pos = start + offset;
            // T 后至少 8 字节（`HH:MM:SS`）
            if line.len() >= t_pos + 9
                && line[t_pos + 1].is_ascii_digit()
                && line[t_pos + 2].is_ascii_digit()
                && line[t_pos + 3] == b':'
                && line[t_pos + 4].is_ascii_digit()
                && line[t_pos + 5].is_ascii_digit()
                && line[t_pos + 6] == b':'
                && line[t_pos + 7].is_ascii_digit()
                && line[t_pos + 8].is_ascii_digit()
            {
                count += 2;
            }
            start = t_pos + 1;
            if start >= line.len() {
                break;
            }
        }
        count
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Plain ---

    #[test]
    fn test_detect_format_empty_line() {
        assert_eq!(detect_format(b""), LineFormat::Plain);
    }

    #[test]
    fn test_detect_format_plain_text() {
        assert_eq!(detect_format(b"started"), LineFormat::Plain);
        assert_eq!(detect_format(b"the quick brown fox"), LineFormat::Plain);
    }

    // --- Logfmt ---

    #[test]
    fn test_detect_format_logfmt_eq() {
        // 典型 logfmt：3 个 k=v 对
        let line = b"level=info msg=started user=alice";
        assert_eq!(detect_format(line), LineFormat::Logfmt);
    }

    #[test]
    fn test_detect_format_logfmt_colon() {
        // k:v 形式：常见于 syslog 风格
        // 长度 35，阈值 35/30=2，4 个 `:` 满足
        let line = b"level:info msg:started user:alice ts:now";
        assert_eq!(detect_format(line), LineFormat::Logfmt);
    }

    #[test]
    fn test_detect_format_logfmt_single_kv() {
        // 单个 k=v 且行短：阈值 1，满足
        assert_eq!(detect_format(b"cost_ms=900"), LineFormat::Logfmt);
    }

    #[test]
    fn test_detect_format_single_eq_long_line_not_logfmt() {
        // 长行只含 1 个 `=`：阈值 30/30=1，但密度太低
        // 长度 50，1 个 `=`，阈值 50/30=2，不满足
        let line = b"this is a long sentence with only one equals sign in it";
        assert_eq!(detect_format(line), LineFormat::Plain);
    }

    // --- JSON line ---

    #[test]
    fn test_detect_format_json_object() {
        let line = br#"{"level":"info","cost_ms":900}"#;
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    #[test]
    fn test_detect_format_json_array() {
        let line = br#"[{"k":1},{"k":2}]"#;
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    // JSONL 行带首尾空白的边角场景（原 detect_format 不 trim → 误判非 JSON）

    #[test]
    fn test_detect_format_json_trailing_space() {
        let line = br#"{"cost_ms":900} "#;
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    #[test]
    fn test_detect_format_json_crlf() {
        let line = b"{\"cost_ms\":900}\r\n";
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    #[test]
    fn test_detect_format_json_leading_space() {
        let line = br#" {"cost_ms":900}"#;
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    #[test]
    fn test_detect_format_json_both_sides_whitespace() {
        let line = b"  {\"k\":1}  \r\n";
        assert_eq!(detect_format(line), LineFormat::JsonLine);
    }

    #[test]
    fn test_detect_format_bracket_not_misjudged_as_json_after_trim() {
        // `[INFO] started` 首字符 `[` 但尾字符 `d` 不是 `]` → 不是 JSON
        // （trim 后仍是 `[INFO] started`，不应误判）
        let line = b" [INFO] started ";
        assert_eq!(detect_format(line), LineFormat::BracketTag);
    }

    // --- BracketTag ---

    #[test]
    fn test_detect_format_bracket_tag_leading() {
        // 行首 [tag]
        assert_eq!(detect_format(b"[INFO] started"), LineFormat::BracketTag);
        assert_eq!(
            detect_format(b"[ERROR] connection refused"),
            LineFormat::BracketTag
        );
    }

    #[test]
    fn test_detect_format_bracket_tag_inline() {
        // 行内 [tag]（非行首）+ 含 logfmt 特征 → Mixed
        // 纯 [tag]（无 k=v）才判为 BracketTag
        assert_eq!(detect_format(b"INFO [auth]"), LineFormat::BracketTag);
        assert_eq!(detect_format(b"INFO [auth] user=alice"), LineFormat::Mixed);
    }

    // --- ISO timestamp ---

    #[test]
    fn test_detect_format_iso_leading() {
        let line = b"2026-01-02T03:04:05Z started";
        assert_eq!(detect_format(line), LineFormat::IsoTimestamp);
    }

    #[test]
    fn test_detect_format_iso_inline() {
        let line = b"ts=2026-01-02T03:04:05 level=info";
        // 既有 ISO 时间戳又有 logfmt 特征 → Mixed
        assert_eq!(detect_format(line), LineFormat::Mixed);
    }

    // --- Mixed ---

    #[test]
    fn test_detect_format_mixed_logfmt_and_iso() {
        let line = b"2026-01-02T03:04:05 level=info msg=started";
        assert_eq!(detect_format(line), LineFormat::Mixed);
    }

    #[test]
    fn test_detect_format_mixed_bracket_and_logfmt() {
        // [INFO] + 多个 k=v
        let line = b"[INFO] level=info msg=started user=alice";
        assert_eq!(detect_format(line), LineFormat::Mixed);
    }

    // --- 边界 ---

    #[test]
    fn test_detect_format_single_char() {
        assert_eq!(detect_format(b"x"), LineFormat::Plain);
    }

    #[test]
    fn test_detect_format_only_equals() {
        // 只有 `=`，无字母数字 key：不视为 logfmt
        // （首字符 `=`，非 `{` `[`，故进入 logfmt 判定分支）
        // 1 个 `=`，长度 1，阈值 1，满足
        assert_eq!(detect_format(b"="), LineFormat::Logfmt);
    }

    #[test]
    fn test_detect_format_bracket_without_alpha() {
        // `[123]`：方括号内非字母，不视为 BracketTag
        assert_eq!(detect_format(b"[123] numbers"), LineFormat::Plain);
    }
}
