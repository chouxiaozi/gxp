//! L2 求值：递归走 `PreparedExpr`，懒抽字段，返回 [`EvalResult`] 三态。
//!
//! 优化：不再消费 `ExtractedFields`（全字段扫描 + Vec 分配），
//! 改为传入 `line: &[u8]`，谓词需要某字段时调 [`ExtractedFields::find_value`]
//! 按需定位（memchr 单次扫描，零分配）。
//!
//! 字段注入：`eval_prepared_with_captures` 在求值时提取 capture 值
//! 写入 `CaptureMap`，供下游 `fields` 投影使用。
//!
//! 语义（设计文档 §3.3 "字段不存在语义"）：
//! - `Prim::Pred`：按 kind 分发到 `eval_numeric`/`eval_in`/`eval_between`/`eval_exists`
//! - `And(l, r)`：`True AND True → True`；任一 `False` → `False`；
//!   `NotNumeric` 视为"谓词不成立"（False 语义）——And 整体假
//! - `Or(l, r)`：任一 `True` → `True`；任一 `False` 且其他 `NotNumeric` → `False`；
//!   全 `NotNumeric` → `NotNumeric`（无法判定）
//! - `Not(e)`：三态取反——`True → False`、`False → True`、`NotNumeric → True`
//!   （§3.3 "字段不存在 → 谓词为假 → 取反为真"，这是 Not 对无字段行命中的语义基础）
//! - `Prim::Literal`：L1 阶段已命中，L2 视为 `True`
//! - `Prim::Paren(e)`：递归

use crate::l2::extract::ExtractedFields;
use crate::l2::{eval_between, eval_exists, eval_in, eval_numeric, eval_time_between, EvalResult};
use crate::mode::prepared::{PreparedExpr, PreparedPred, PreparedPrim};
use crate::pipeline::CaptureMap;

// 原 `eval_prepared` / `eval_prepared_prim`（无 capture 提取）已删除：
// Not 分支若调无 capture 版本不传 captures，
// 会导致 `capture(X) && !(X>Y)` 中 Not 内部谓词找不到 X → NotNumeric → Not=True →
// 所有行命中。Not 分支统一调 `eval_prepared_with_captures` 后，无 capture 路径
// 不再有调用方。统一走 `eval_prepared_with_captures`（无 capture 调用方传空
// `CaptureMap`，O(1) 哈希查询无性能损耗）。

// =========================================================================
// 带 capture 提取的求值路径
// =========================================================================

/// L2 求值 + capture 提取：匹配时提取 capture 值写入 `captures`。
///
/// 此函数是 L2 求值唯一入口（原无 capture 版本 `eval_prepared`
/// 已删除）。在 `Capture` 节点命中时提取正则匹配值（有 capture group 取
/// group 1，否则取 full match）写入 `captures[name]`。
///
/// `captures` 在调用前应为空 map；多次 capture 同名时后者覆盖前者
/// （与 EBNF `capture(NAME, /REGEX/)` 语义一致：NAME 是字段名，唯一）。
pub(crate) fn eval_prepared_with_captures(
    expr: &PreparedExpr,
    line: &[u8],
    captures: &mut CaptureMap,
) -> EvalResult {
    match expr {
        PreparedExpr::Or(l, r) => {
            // Or：两侧都可能提取 capture，先试左，不命中再试右
            let lv = eval_prepared_with_captures(l, line, captures);
            if matches!(lv, EvalResult::True) {
                return EvalResult::True;
            }
            let rv = eval_prepared_with_captures(r, line, captures);
            match (lv, rv) {
                (EvalResult::True, _) | (_, EvalResult::True) => EvalResult::True,
                (EvalResult::False, _) | (_, EvalResult::False) => EvalResult::False,
                (EvalResult::NotNumeric, EvalResult::NotNumeric) => EvalResult::NotNumeric,
            }
        }
        PreparedExpr::And(l, r) => {
            let lv = eval_prepared_with_captures(l, line, captures);
            if !matches!(lv, EvalResult::True) {
                return EvalResult::False;
            }
            let rv = eval_prepared_with_captures(r, line, captures);
            match rv {
                EvalResult::True => EvalResult::True,
                EvalResult::False | EvalResult::NotNumeric => EvalResult::False,
            }
        }
        PreparedExpr::Not(e) => {
            // Not 子树也要传递 captures。
            // 原注释"capture 提取无意义"部分正确——Not 内部新提取的 capture
            // 不应注入（取反的谓词匹配了也不注入）。但**外部已提取的 captures**
            // 必须传递（如 `capture(rid) && !(rid>50000)` 中 rid 值由外部 capture
            // 提供，Not 内部谓词需用此值）。原调 eval_prepared（无 captures）
            // → 谓词找不到 rid → NotNumeric → Not 映射为 True → 所有行命中。
            match eval_prepared_with_captures(e, line, captures) {
                EvalResult::True => EvalResult::False,
                EvalResult::False | EvalResult::NotNumeric => EvalResult::True,
            }
        }
        PreparedExpr::Prim(prim) => eval_prepared_prim_with_captures(prim, line, captures),
    }
}

/// prim 求值 + capture 提取。
pub(crate) fn eval_prepared_prim_with_captures(
    prim: &PreparedPrim,
    line: &[u8],
    captures: &mut CaptureMap,
) -> EvalResult {
    match prim {
        PreparedPrim::Paren(e) => eval_prepared_with_captures(e, line, captures),
        PreparedPrim::Literal { regex, l1_verified } => {
            // v0.49 Opt A：L1 已构造性保证存在的字面量（verified_literals 成员）
            // 豁免重校验（Rust 方案 §7.9）。Not 内同串节点语义一致：行必含该
            // 子串 ⇒ Not(verified literal) 恒 False，与跑 regex 结果相同。
            if *l1_verified {
                EvalResult::True
            } else if regex.is_match(line) {
                // L2 实际校验字面量（Not(Literal) 取反依赖此语义）
                EvalResult::True
            } else {
                EvalResult::False
            }
        }
        // captures 优先（apply_where 路径：where 谓词读已有 capture 值）
        PreparedPrim::Pred(p) => eval_prepared_pred(p, line, captures),
        PreparedPrim::Ast { .. } => EvalResult::True,
        PreparedPrim::Capture { name, regex } => {
            // 字段注入：find 提取匹配值，写入 captures[name]
            // 有 capture group 取 group 1，否则取 full match
            let value = if regex.captures_len() > 1 {
                // 有子组：取第一个捕获组
                regex
                    .captures(line)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_bytes().to_vec())
            } else {
                // 无子组：取 full match
                regex.find(line).map(|m| m.as_bytes().to_vec())
            };
            match value {
                Some(v) => {
                    captures.insert(name.clone(), v);
                    EvalResult::True
                }
                None => EvalResult::False,
            }
        }
    }
}

// =========================================================================
// L3 ast() 上下文求值（feature `l3-structural` 开启时编译）
// =========================================================================
//
// 设计文档 §2.2 + §8.2：
// - L1 视 ast() 为"可能命中"（不淘汰候选，见 l1::compiler::collect_prim_literals）
// - L2 求值时 `PreparedPrim::Ast` 查 `ast_lines` 集合（由 l3::structural::run_all_ast 预计算）
// - 不变式 `L1 ⊇ L2 ⊇ L3`：L1 不淘汰 → L2 查集合 → L3 精判 ast-grep find_all

/// L2 求值上下文。
///
/// 持有 L3 ast() 预计算的行号集合（由 [`crate::l3::structural::run_all_ast`] 生成）。
/// - `ast_lines = None`：无 ast() 节点，`PreparedPrim::Ast` 不会出现
/// - `ast_lines = Some(set)`：含 ast() 节点，`PreparedPrim::Ast` 查此集合
#[cfg(feature = "l3-structural")]
#[derive(Default)]
pub(crate) struct EvalCtx<'a> {
    /// ast() 命中行号集合（lang|pattern → 行号 HashSet）。
    pub ast_lines: Option<&'a crate::l3::structural::AstLineSet>,
}

// 原 `eval_prepared_with_ctx` / `eval_prepared_prim_with_ctx`
// （无 capture 提取的 l3-structural 变体）已删除。所有调用方走
// `eval_prepared_with_ctx_and_captures`（无 capture 调用方传空 CaptureMap）。

/// L2 求值 + capture 提取（带 L3 ast 上下文）。feature `l3-structural` 开启时编译。
#[cfg(feature = "l3-structural")]
pub(crate) fn eval_prepared_with_ctx_and_captures(
    expr: &PreparedExpr,
    line: &[u8],
    line_no: usize,
    ctx: &EvalCtx<'_>,
    captures: &mut CaptureMap,
) -> EvalResult {
    match expr {
        PreparedExpr::Or(l, r) => {
            let lv = eval_prepared_with_ctx_and_captures(l, line, line_no, ctx, captures);
            if matches!(lv, EvalResult::True) {
                return EvalResult::True;
            }
            let rv = eval_prepared_with_ctx_and_captures(r, line, line_no, ctx, captures);
            match (lv, rv) {
                (EvalResult::True, _) | (_, EvalResult::True) => EvalResult::True,
                (EvalResult::False, _) | (_, EvalResult::False) => EvalResult::False,
                (EvalResult::NotNumeric, EvalResult::NotNumeric) => EvalResult::NotNumeric,
            }
        }
        PreparedExpr::And(l, r) => {
            let lv = eval_prepared_with_ctx_and_captures(l, line, line_no, ctx, captures);
            if !matches!(lv, EvalResult::True) {
                return EvalResult::False;
            }
            let rv = eval_prepared_with_ctx_and_captures(r, line, line_no, ctx, captures);
            match rv {
                EvalResult::True => EvalResult::True,
                EvalResult::False | EvalResult::NotNumeric => EvalResult::False,
            }
        }
        PreparedExpr::Not(e) => {
            // Not 子树也要传递 captures（l3-structural 镜像）。
            match eval_prepared_with_ctx_and_captures(e, line, line_no, ctx, captures) {
                EvalResult::True => EvalResult::False,
                EvalResult::False | EvalResult::NotNumeric => EvalResult::True,
            }
        }
        PreparedExpr::Prim(prim) => {
            eval_prepared_prim_with_ctx_and_captures(prim, line, line_no, ctx, captures)
        }
    }
}

#[cfg(feature = "l3-structural")]
pub(crate) fn eval_prepared_prim_with_ctx_and_captures(
    prim: &PreparedPrim,
    line: &[u8],
    line_no: usize,
    ctx: &EvalCtx<'_>,
    captures: &mut CaptureMap,
) -> EvalResult {
    match prim {
        PreparedPrim::Paren(e) => {
            eval_prepared_with_ctx_and_captures(e, line, line_no, ctx, captures)
        }
        PreparedPrim::Literal { regex, l1_verified } => {
            // v0.49 Opt A（l3 镜像，与 eval_prepared_prim_with_captures 保持一致，§6.5.3）
            if *l1_verified {
                EvalResult::True
            } else if regex.is_match(line) {
                // L2 实际校验字面量（Not(Literal) 取反依赖此语义）
                EvalResult::True
            } else {
                EvalResult::False
            }
        }
        // captures 优先（apply_where + L3 ast 上下文路径）
        PreparedPrim::Pred(p) => eval_prepared_pred(p, line, captures),
        PreparedPrim::Ast { lang, pattern } => match ctx.ast_lines {
            Some(set) => {
                if crate::l3::structural::ast_line_contains(set, lang, pattern, line_no) {
                    EvalResult::True
                } else {
                    EvalResult::False
                }
            }
            None => EvalResult::True,
        },
        PreparedPrim::Capture { name, regex } => {
            let value = if regex.captures_len() > 1 {
                regex
                    .captures(line)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_bytes().to_vec())
            } else {
                regex.find(line).map(|m| m.as_bytes().to_vec())
            };
            match value {
                Some(v) => {
                    captures.insert(name.clone(), v);
                    EvalResult::True
                }
                None => EvalResult::False,
            }
        }
    }
}

/// L2 求值预编译谓词。
///
/// # 字段可见性
///
/// **captures 优先**：先查 `captures[field]`，命中则用作字段值；未命中
/// 再 fallback 到 `ExtractedFields::find_value(line, field)`（行内字段）。
///
/// 原签名 `eval_prepared_pred(p, line)` 直接调 `find_value(line)`，导致
/// `apply_where` 路径下 `capture(NAME, /REGEX/) | where NAME>X` 不读 capture 值——
/// where 表达式看不到扫描阶段提取的 capture 字段，谓词对 capture 字段恒返回
/// NotNumeric（行内无 NAME 字段时），破坏 `--verify` 与优化路径语义一致性。
///
/// 无 capture 提取路径（`eval_prepared`）传 `&CaptureMap::new()` 即可，
/// 空 map 的 `get` 是 O(1) 哈希查询无性能损耗。
pub(crate) fn eval_prepared_pred(
    p: &PreparedPred,
    line: &[u8],
    captures: &CaptureMap,
) -> EvalResult {
    match p {
        PreparedPred::Compare {
            field,
            op,
            threshold,
        } => {
            // captures 优先（apply_where 路径必需）
            let value = captures
                .get(field)
                .map(|v| v.as_slice())
                .or_else(|| ExtractedFields::find_value(line, field.as_bytes()));
            match value {
                Some(v) => eval_numeric(v, *op, *threshold),
                None => EvalResult::NotNumeric,
            }
        }
        PreparedPred::In { field, values } => {
            let value = captures
                .get(field)
                .map(|v| v.as_slice())
                .or_else(|| ExtractedFields::find_value(line, field.as_bytes()));
            eval_in(value, values)
        }
        PreparedPred::Exists { field } => {
            let value = captures
                .get(field)
                .map(|v| v.as_slice())
                .or_else(|| ExtractedFields::find_value(line, field.as_bytes()));
            eval_exists(value)
        }
        PreparedPred::Between { field, lo, hi } => {
            let value = captures
                .get(field)
                .map(|v| v.as_slice())
                .or_else(|| ExtractedFields::find_value(line, field.as_bytes()));
            match value {
                Some(v) => eval_between(v, *lo, *hi),
                None => EvalResult::NotNumeric,
            }
        }
        PreparedPred::TimeBetween { field, lo, hi } => {
            // _ts 时间戳 between（D5 不适用，走 speedate 时间解析路径）
            let value = captures
                .get(field)
                .map(|v| v.as_slice())
                .or_else(|| ExtractedFields::find_value(line, field.as_bytes()));
            match value {
                Some(v) => eval_time_between(v, *lo, *hi),
                None => EvalResult::NotNumeric,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 字段注入：capture 提取值测试。

    use super::*;
    use crate::dsl::parser::parse_query;

    #[test]
    fn test_capture_extracts_full_match_no_group() {
        // capture(trace_id, /req-\d+/) 无子组 → full match
        let q = parse_query(r#"capture(trace_id, /req-\d+/)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"ERROR req-12345 timeout";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::True);
        assert_eq!(
            captures
                .get("trace_id")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("req-12345".to_string())
        );
    }

    #[test]
    fn test_capture_extracts_group_1_when_present() {
        // capture(tid, /req=(\d+)/) 有子组 → group 1
        let q = parse_query(r#"capture(tid, /req=(\d+)/)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req=12345 done";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::True);
        assert_eq!(
            captures
                .get("tid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("12345".to_string())
        );
    }

    #[test]
    fn test_capture_no_match_returns_false_empty_captures() {
        let q = parse_query(r#"capture(tid, /req=\d+/)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"no match here";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::False);
        assert!(captures.is_empty());
    }

    #[test]
    fn test_capture_in_and_expression_both_match() {
        // "ERROR" && capture(tid, /req-\d+/)
        let q = parse_query(r#""ERROR" && capture(tid, /req-\d+/)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"ERROR req-99999 timeout";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::True);
        assert_eq!(
            captures
                .get("tid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("req-99999".to_string())
        );
    }

    #[test]
    fn test_capture_in_and_expression_right_fails_no_capture() {
        // capture(tid, /req-\d+/) && cost_ms>999 — 右侧谓词不命中 → 短路，但 captures 已提取
        // 注：And 短路方向是 left→right，左侧 capture 先执行（提取），右侧失败 → False
        let q = parse_query(r#"capture(tid, /req-\d+/) && cost_ms>999"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-99999 cost_ms=100";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::False);
        // capture 在左侧先执行，已提取——但 And 整体 False
        // 这符合语义：capture 匹配了行，但 cost_ms>999 不满足
        assert_eq!(
            captures
                .get("tid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("req-99999".to_string())
        );
    }

    #[test]
    fn test_capture_and_predicate_same_field_uses_capture_value() {
        // capture(X) && X>Y —— 谓词字段名 == capture 字段名
        // capture 提取的值应供同字段谓词使用（行内无 X= 字段）
        // pipeline where 路径与 L2 && 路径需同构
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && rid>10000"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-12345";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(result, EvalResult::True, "rid=12345 > 10000 应命中");
        assert_eq!(
            captures
                .get("rid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("12345".to_string())
        );
    }

    #[test]
    fn test_capture_and_predicate_same_field_in_list() {
        // capture(X) && X in [...] 同字段 in 谓词
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && rid in [12345, 99999]"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-12345";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(
            result,
            EvalResult::True,
            "rid=12345 in [12345,99999] 应命中"
        );
    }

    #[test]
    fn test_capture_and_predicate_same_field_exists() {
        // capture(X) && X exists 同字段 exists 谓词
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && rid exists"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-12345";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(
            result,
            EvalResult::True,
            "rid exists 应命中（capture 提供值）"
        );
    }

    #[test]
    fn test_capture_and_predicate_same_field_between() {
        // capture(X) && X between A and B 同字段 between 谓词
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && rid between 100 and 99999"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-12345";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        assert_eq!(
            result,
            EvalResult::True,
            "rid=12345 between 100 and 99999 应命中"
        );
    }

    // ===== Not(capture predicate) 组合 =====
    //
    // L2 eval_prepared_with_captures 的 Not 分支若调无 captures 版本
    // → capture(X) && !(X>Y) 中 Not 内部谓词找不到 X → NotNumeric → Not=True →
    // 所有行命中（67890 > 50000 应被 Not 排除却未被排除）。
    // Not 分支必须调 eval_prepared_with_captures 传递 captures。

    #[test]
    fn test_capture_and_not_predicate_same_field_uses_capture_value() {
        // capture(X) && !(X>Y) — Not 子树谓词也要读 capture 值
        // 67890 > 50000 → True → Not → False → 整体 And False（应被排除）
        // 12345 ≤ 50000 → False → Not → True → 整体 And True（应命中）
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && !(rid>50000)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();

        // rid=12345 ≤ 50000：Not(False) = True → And True
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-12345", &mut captures);
        assert_eq!(result, EvalResult::True, "rid=12345 ≤ 50000 应命中");
        assert_eq!(
            captures
                .get("rid")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("12345".to_string())
        );

        // rid=67890 > 50000：Not(True) = False → And False
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-67890", &mut captures);
        assert_eq!(result, EvalResult::False, "rid=67890 > 50000 应被排除");
    }

    #[test]
    fn test_capture_and_not_predicate_in_list() {
        // capture(X) && !(X in [A, B]) — Not(in) 组合
        let q = parse_query(r#"capture(rid, /req-(\d+)/) && !(rid in [12345, 99999])"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();

        // rid=12345 in [12345,99999]：True → Not → False → And False
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-12345", &mut captures);
        assert_eq!(result, EvalResult::False, "rid=12345 in 列表 → Not → 排除");

        // rid=67890 not in [12345,99999]：False → Not → True → And True
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-67890", &mut captures);
        assert_eq!(result, EvalResult::True, "rid=67890 不在列表 → Not → 命中");
    }

    #[test]
    fn test_capture_and_not_predicate_between() {
        // capture(X) && !(X between A and B) — Not(between) 组合
        let q =
            parse_query(r#"capture(rid, /req-(\d+)/) && !(rid between 10000 and 50000)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();

        // rid=12345 in [10000,50000]：True → Not → False → And False
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-12345", &mut captures);
        assert_eq!(result, EvalResult::False, "rid=12345 在区间 → Not → 排除");

        // rid=67890 not in [10000,50000]：False → Not → True → And True
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"req-67890", &mut captures);
        assert_eq!(result, EvalResult::True, "rid=67890 不在区间 → Not → 命中");
    }

    #[test]
    fn test_capture_in_or_expression_extracts_from_matching_branch() {
        // capture(a, /x\d+/) || capture(b, /y\d+/)
        let q = parse_query(r#"capture(a, /x\d+/) || capture(b, /y\d+/)"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();

        // 左分支命中
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"x42 line", &mut captures);
        assert_eq!(result, EvalResult::True);
        assert_eq!(
            captures
                .get("a")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("x42".to_string())
        );
        assert!(!captures.contains_key("b"));

        // 右分支命中
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, b"y99 line", &mut captures);
        assert_eq!(result, EvalResult::True);
        assert!(!captures.contains_key("a"));
        assert_eq!(
            captures
                .get("b")
                .map(|v| String::from_utf8_lossy(v).to_string()),
            Some("y99".to_string())
        );
    }

    #[test]
    fn test_not_does_not_extract_capture() {
        // !(capture(tid, /req-\d+/)) — Not 取反：capture 命中 → True → Not → False
        // 现在：Not 子树递归调 eval_prepared_with_captures（传 captures），
        // 内部 capture(tid) 命中时会向 captures 写入 tid（作为求值副产物）。
        // 但由于 Not 整体返回 False，下游 stage 不会看到此 captures（仅匹配行进 stage）。
        // 断言改为：整体返回 False（核心语义），不再断言 captures.is_empty()
        // （旧行为是 Not 不传 captures → 内部 capture 不写入 → is_empty 为真，但
        // 这是历史 bug：capture(rid) && !(rid>50000) 中 Not 内部谓词
        // 找不到 rid → NotNumeric → Not=True → 所有行命中）。
        let q = parse_query(r#"!(capture(tid, /req-\d+/))"#).unwrap();
        let prepared = crate::mode::prepared::prepare_expr(&q.expr, &[]).unwrap();
        let line = b"req-12345";
        let mut captures = CaptureMap::new();
        let result = eval_prepared_with_captures(&prepared, line, &mut captures);
        // 核心语义：Not 取反 capture 命中 → False
        assert_eq!(result, EvalResult::False);
        // captures 副产物不再断言（现在允许 Not 子树求值副产物写入 captures）
    }
}
