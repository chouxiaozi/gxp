//! `fields a,b`：字段投影（等价 `awk '{print $1,$2}'`）。
//!
//! 设计文档 §2.2 EBNF `stage` 规则 + §2.4 示例 2：
//! `| fields _ts,_tag,cost_ms` 把每行命中行替换为指定字段值序列。
//!
//! 语义（v1）：
//! - 每行用 [`ExtractedFields::extract`] 抽字段，按 `names` 顺序取字段值
//! - 字段缺失 → 空串占位（与 `group by` 的缺失归空一致）
//! - 值以单个空格拼接（等价 `awk '{print $1,$2}'` 的输出风格）
//! - 行号保留（输出仍为 `行号:投影值` 格式）
//! - 伪字段 `_ts` / `_tag` 由 `ExtractedFields::get` 支持

use super::MatchedLine;
use crate::l2::ExtractedFields;

/// 执行 `fields` 投影：把每行替换为指定字段值序列（空格拼接）。
///
/// - `names`：字段名列表（按查询书写顺序）
/// - 缺失字段 → 空串占位
/// - 行号不变
///
/// 优先查 `MatchedLine::captures`（capture 提取值），
/// 再 fallback 到 `ExtractedFields::find_value`（行内字段）。
///
/// 性能优化（算法层，非 SIMD intrinsics）：n≥3 时行内字段走 `batch_find_values`
/// 一次 memchr2 扫描，n≤2 仍走 `find_value` 单路径（bench 实测阈值：n=2 batch +8%、
/// n=5 +137%、n=12 +344%；详见 `docs/performance-baseline.md §4.4 SIMD 配置结论`）。
/// **命名纠正**：原 roadmap 名"SIMD 多字段投影"误导——优化在算法层（N 遍 memchr2
/// 调用 → 1 遍），SIMD 仍在 memchr crate 内部，gxp 不碰 intrinsics（D7 约束不变）。
pub fn apply_projection(matched: &[MatchedLine], names: &[&[u8]]) -> Vec<MatchedLine> {
    // 预先判断：n≥3 且有非伪字段时启用 batch 路径
    const BATCH_THRESHOLD: usize = 3;
    let use_batch = names.len() >= BATCH_THRESHOLD;

    matched
        .iter()
        .map(|m| {
            let mut projected: Vec<Vec<u8>> = Vec::with_capacity(names.len());

            // batch 路径：对未命中 captures/伪字段的"剩余"字段做一次 batch 扫描
            // 收集需要从行内提取的字段索引 + name 引用
            let mut pending: Vec<(usize, &[u8])> = Vec::new();
            for (idx, name) in names.iter().enumerate() {
                let name_str = String::from_utf8_lossy(name).into_owned();
                let val = match name_str.as_str() {
                    // v0.42 DRY：伪字段统一走 pseudo_field_value
                    // （与 sort/rank 语义对齐——sort/rank 用同 helper 提取排序键）
                    "_match" | "_line" | "_file" | "_ts" | "_tag" => {
                        super::pseudo_field_value(m, name).unwrap_or_default()
                    }
                    // 普通字段：captures 优先，命中则直接用；未命中暂存待 batch
                    _ => {
                        if let Some(v) = m.captures.get(&name_str).cloned() {
                            v
                        } else if use_batch {
                            // 延迟到 batch_find_values 统一提取
                            pending.push((idx, name));
                            continue;
                        } else {
                            ExtractedFields::find_value(&m.line, name)
                                .map(|v| v.to_vec())
                                .unwrap_or_default()
                        }
                    }
                };
                projected.push(val);
            }

            // batch 路径：一次 memchr2 扫描提取所有 pending 字段
            if use_batch && !pending.is_empty() {
                let pending_names: Vec<&[u8]> = pending.iter().map(|(_, n)| *n).collect();
                let batch_results = ExtractedFields::batch_find_values(&m.line, &pending_names);
                // 按 pending 顺序写回 projected 的对应索引位置
                // 注意 projected 当前长度可能 < names.len()，需先补齐
                while projected.len() < names.len() {
                    projected.push(Vec::new());
                }
                for (batch_idx, (proj_idx, _)) in pending.iter().enumerate() {
                    if let Some(v) = batch_results[batch_idx] {
                        projected[*proj_idx] = v.to_vec();
                    }
                    // 未命中保持空串（已由补齐步骤初始化）
                }
            }

            let line: Vec<u8> = projected.join(&b' ');
            let mut ml = MatchedLine::new(m.line_no, &line);
            ml.captures = m.captures.clone();
            ml.path = m.path.clone();
            ml.match_bytes = m.match_bytes.clone();
            ml
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MatchedLine;

    fn lines(rows: &[&str]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, s)| MatchedLine::new(i + 1, s.as_bytes()))
            .collect()
    }

    fn texts(lines: &[MatchedLine]) -> Vec<String> {
        lines
            .iter()
            .map(|m| String::from_utf8_lossy(&m.line).to_string())
            .collect()
    }

    #[test]
    fn test_projection_selects_fields_in_order() {
        let m = lines(&["[T] cost_ms=900 user=alice"]);
        let out = apply_projection(&m, &[b"_tag", b"cost_ms", b"user"]);
        assert_eq!(texts(&out), vec!["T 900 alice"]);
        // 行号保留
        assert_eq!(out[0].line_no, 1);
    }

    #[test]
    fn test_projection_missing_field_empty_placeholder() {
        // 缺失字段 → 空串占位（与 group by 一致）
        let m = lines(&["cost_ms=900 user=alice"]);
        let out = apply_projection(&m, &[b"cost_ms", b"missing"]);
        assert_eq!(texts(&out), vec!["900 "]);
    }

    #[test]
    fn test_projection_multiple_lines() {
        let m = lines(&["cost_ms=900 user=alice", "cost_ms=100 user=bob"]);
        let out = apply_projection(&m, &[b"user", b"cost_ms"]);
        assert_eq!(texts(&out), vec!["alice 900", "bob 100"]);
    }

    #[test]
    fn test_projection_empty_input() {
        let out = apply_projection(&[], &[b"cost_ms"]);
        assert!(out.is_empty());
    }

    #[test]
    fn test_projection_pseudo_fields() {
        // 伪字段 _ts / _tag 由 ExtractedFields::get 支持
        let m = lines(&["2026-01-02T03:04:05Z [INFO] msg=ok"]);
        let out = apply_projection(&m, &[b"_ts", b"_tag"]);
        assert_eq!(texts(&out), vec!["2026-01-02T03:04:05Z INFO"]);
    }

    // 字段注入

    #[test]
    fn test_projection_uses_capture_value_over_line_field() {
        // capture 提取的值优先于行内同名字段
        let mut m = MatchedLine::new(1, b"req-12345 log");
        m.captures
            .insert("trace_id".to_string(), b"req-12345".to_vec());
        let out = apply_projection(&[m], &[b"trace_id"]);
        assert_eq!(texts(&out), vec!["req-12345"]);
    }

    #[test]
    fn test_projection_capture_value_when_no_line_field() {
        // 行内无 trace_id 字段，capture 提取的值仍可用
        let mut m = MatchedLine::new(1, b"ERROR req-99999 timeout");
        m.captures
            .insert("trace_id".to_string(), b"req-99999".to_vec());
        let out = apply_projection(&[m], &[b"trace_id"]);
        assert_eq!(texts(&out), vec!["req-99999"]);
    }

    #[test]
    fn test_projection_capture_missing_falls_back_to_line_field() {
        // 无 capture 时 fallback 到行内字段
        let m = MatchedLine::new(1, b"cost_ms=900");
        let out = apply_projection(&[m], &[b"cost_ms"]);
        assert_eq!(texts(&out), vec!["900"]);
    }

    #[test]
    fn test_projection_capture_with_regular_field() {
        // capture 字段 + 行内字段混合投影
        let mut m = MatchedLine::new(1, b"cost_ms=900 req-12345");
        m.captures
            .insert("trace_id".to_string(), b"req-12345".to_vec());
        let out = apply_projection(&[m], &[b"trace_id", b"cost_ms"]);
        assert_eq!(texts(&out), vec!["req-12345 900"]);
    }

    // 伪字段 _match / _line / _file

    #[test]
    fn test_projection_match_returns_match_bytes() {
        // _match = L1 匹配的字节（等价 grep -o）
        let mut m = MatchedLine::new(1, b"ERROR cost_ms=900");
        m.match_bytes = Some(std::sync::Arc::from(b"ERROR".as_ref()));
        let out = apply_projection(&[m], &[b"_match"]);
        assert_eq!(texts(&out), vec!["ERROR"]);
    }

    #[test]
    fn test_projection_match_empty_when_no_match_bytes() {
        // 无 match_bytes（纯字段谓词查询）→ 空串
        let m = MatchedLine::new(1, b"cost_ms=900");
        let out = apply_projection(&[m], &[b"_match"]);
        assert_eq!(texts(&out), vec![""]);
    }

    #[test]
    fn test_projection_line_returns_full_line() {
        // _line = 整行内容
        let m = MatchedLine::new(1, b"ERROR cost_ms=900");
        let out = apply_projection(&[m], &[b"_line"]);
        assert_eq!(texts(&out), vec!["ERROR cost_ms=900"]);
    }

    #[test]
    fn test_projection_file_returns_path() {
        // _file = 文件路径
        let mut m = MatchedLine::new(1, b"ERROR");
        m.path = Some(std::sync::Arc::from("src/main.rs"));
        let out = apply_projection(&[m], &[b"_file"]);
        assert_eq!(texts(&out), vec!["src/main.rs"]);
    }

    #[test]
    fn test_projection_file_empty_when_no_path() {
        // 无 path → 空串
        let m = MatchedLine::new(1, b"ERROR");
        let out = apply_projection(&[m], &[b"_file"]);
        assert_eq!(texts(&out), vec![""]);
    }

    #[test]
    fn test_projection_preserves_captures_and_path() {
        // 投影后 captures 和 path 保留
        let mut m = MatchedLine::new(1, b"ERROR cost_ms=900");
        m.captures.insert("uid".to_string(), b"user=alice".to_vec());
        m.path = Some(std::sync::Arc::from("app.log"));
        m.match_bytes = Some(std::sync::Arc::from(b"ERROR".as_ref()));
        let out = apply_projection(&[m], &[b"cost_ms"]);
        assert_eq!(out.len(), 1);
        assert_eq!(texts(&out), vec!["900"]);
        // captures 保留
        assert_eq!(
            out[0]
                .captures
                .get("uid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("user=alice".to_string()),
            "captures 应保留"
        );
        // path 保留
        assert_eq!(
            out[0].path,
            Some(std::sync::Arc::from("app.log")),
            "path 应保留"
        );
        // match_bytes 保留
        assert_eq!(
            out[0]
                .match_bytes
                .as_ref()
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("ERROR".to_string()),
            "match_bytes 应保留"
        );
    }
}
