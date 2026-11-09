//! ExecutionPlan：`--explain` 输出（Rust 方案 §14、§11.2 验收）。
//!
//! 消费 [`crate::dsl::ast::Query`] + [`crate::l1::compiler::CompiledL1`]，
//! 产出执行计划视图。阈值/边界按 11 维契约（D5）预解析为 f64。
//!
//! 输出格式：
//! - 默认文本（人读）：L1 必选组 + L2 字段谓词 + 管道动词摘要
//! - `--json`：结构化 JSON（机器可读）
//!
//! 设计文档 §2.4 示例 3：`gxp --explain --verify '<query>' app.log`。

use serde::Serialize;

use crate::dsl::ast::{CmpOp, Query, Stage};
use crate::l1::compiler::{CompiledL1, FieldPredicate, PredicateKind};
use crate::l2::{build_between_bounds, parse_numeric_value, parse_timestamp};

/// `--explain` 输出：执行计划（L1 必选组 + L2 字段谓词 + 管道动词摘要）。
///
/// 设计文档 §2.4 示例 3 + Rust 方案 §11.2 "执行计划输出"验收：
/// - 列出 L1 必选组（字面量）
/// - 列出 L2 抽取字段
/// - 列出 L2 数值谓词（阈值已按 11 维契约预解析）
#[derive(Debug, Default, Serialize)]
pub struct ExecutionPlan {
    /// L1 必选组：组间 AND，组内 OR。
    pub required_groups: Vec<LiteralGroupView>,
    /// L2 字段谓词（阈值/边界已按 11 维契约预解析为 f64）。
    pub field_predicates: Vec<FieldPredicateView>,
    /// 管道动词摘要。
    pub stages: Vec<StageView>,
}

/// L1 必选组视图（组内 OR）。
#[derive(Debug, Serialize)]
pub struct LiteralGroupView {
    /// 组内字面量列表（OR 语义）。
    pub literals: Vec<String>,
}

/// L2 字段谓词视图（对应 [`crate::l1::compiler::PredicateKind`]）。
#[derive(Debug, Serialize)]
pub struct FieldPredicateView {
    /// 字段名。
    pub field: String,
    /// 谓词种类（阈值/边界已预解析为 f64）。
    pub kind: PredicateKindView,
}

/// 谓词种类视图（11 维契约预解析后的形态）。
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum PredicateKindView {
    /// `NAME OP VALUE`：数值/字符串比较。
    Compare {
        /// 操作符（如 `>` / `>=` / `==`）。
        op: String,
        /// 阈值（已按 11 维契约解析为 f64）。
        threshold: f64,
    },
    /// `NAME in [v1, v2, ...]`：集合成员判定。
    In {
        /// 候选值列表（字节字面量）。
        values: Vec<String>,
    },
    /// `NAME exists`：字段存在判定。
    Exists,
    /// `NAME between LO and HI`：闭区间数值判定。
    Between {
        /// 下界（已预解析为 f64）。
        lo: f64,
        /// 上界（已预解析为 f64）。
        hi: f64,
    },
    /// `_ts between "ISO" and "ISO"`：时间戳区间判定。
    TimeBetween {
        /// 下界（epoch seconds as f64）。
        lo: f64,
        /// 上界（epoch seconds as f64）。
        hi: f64,
    },
}

/// `sample` 抽取规格视图（对应 [`crate::dsl::ast::SampleSpec`]）。
#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
pub enum SampleSpecView {
    /// `sample N`：抽 N 条。
    Count {
        /// 抽取条数。
        n: usize,
    },
    /// `sample P%`：抽 `ceil(len * p)` 条。
    Ratio {
        /// 比例（0.0..=1.0）。
        p: f64,
    },
}

/// 管道动词摘要（对应 [`crate::dsl::ast::Stage`]）。
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum StageView {
    /// `where EXPR`：对上游结果再过滤。
    Where,
    /// `context N` / `context before:N after:M`。
    Context {
        /// 前文行数。
        before: usize,
        /// 后文行数。
        after: usize,
    },
    /// `count`：计数。
    Count,
    /// `group by FIELD`：按字段分组计数。
    GroupBy {
        /// 分组字段名。
        field: String,
    },
    /// `rank top K by FIELD [desc|asc]`。
    Rank {
        /// Top-K。
        top: usize,
        /// 排序字段。
        by: String,
        /// `desc` 为 true。
        desc: bool,
    },
    /// `head N`：限制输出行数。
    Head {
        /// 行数。
        n: usize,
    },
    /// `sample N` / `sample P%`：随机抽样（固定种子 0）。
    Sample {
        /// `Count(N)` / `Ratio(p)`——ratio 在执行时按命中数换算。
        spec: SampleSpecView,
    },
    /// `fields a, b, ...`：字段投影。
    Fields {
        /// 字段名列表。
        names: Vec<String>,
    },
    /// `files` / `files --invert`。
    Files {
        /// `--invert` 为 true。
        invert: bool,
    },
    /// `stats FIELD`：流式聚合 min/max/avg。
    Stats {
        /// 聚合字段名。
        field: String,
    },
    /// `percentiles FIELD [p50,p95,p99]`：非流式百分位。
    Percentiles {
        /// 聚合字段名。
        field: String,
        /// 百分位列表（如 `[50.0, 95.0, 99.0]`）。
        percentiles: Vec<f64>,
    },
    /// `sort [by FIELD] [desc|asc]`：全量稳定排序。
    Sort {
        /// 排序字段名。`None` 按整行字节序。
        by: Option<String>,
        /// `desc` 为 true。
        desc: bool,
    },
    /// `distinct [by FIELD]`：稳定去重（保留首次出现的行）。
    Distinct {
        /// 去重字段名。`None` 按整行字节。
        by: Option<String>,
    },
    /// `trace [LANG]`：多行 stack trace 捕获。
    Trace {
        /// 语言名。`None` 自动检测；`Some(name)` 指定语言规则。
        lang: Option<String>,
    },
}

/// 从 `Query` + `CompiledL1` 构造 [`ExecutionPlan`]。
///
/// 阈值/边界按 11 维契约（D5）解析为 f64；非法数值 fail-loud（返回 `Err`）。
pub fn build_plan(q: &Query, compiled: &CompiledL1) -> crate::Result<ExecutionPlan> {
    let required_groups: Vec<LiteralGroupView> = compiled
        .required_groups
        .iter()
        .map(|g| LiteralGroupView {
            literals: g
                .literals
                .iter()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect(),
        })
        .collect();

    let mut field_predicates: Vec<FieldPredicateView> =
        Vec::with_capacity(compiled.field_predicates.len());
    for fp in &compiled.field_predicates {
        field_predicates.push(build_predicate_view(fp)?);
    }

    let stages: Vec<StageView> = q.stages.iter().map(stage_view).collect();

    Ok(ExecutionPlan {
        required_groups,
        field_predicates,
        stages,
    })
}

/// 构造单个 [`FieldPredicateView`]（阈值/边界预解析）。
fn build_predicate_view(fp: &FieldPredicate) -> crate::Result<FieldPredicateView> {
    let kind = match &fp.kind {
        PredicateKind::Compare { op, value } => {
            let threshold =
                parse_numeric_value(value.as_bytes()).ok_or_else(|| crate::GxpError::Runtime {
                    detail: format!("阈值 '{value}' 不是合法数值（11 维契约拒绝）"),
                })?;
            PredicateKindView::Compare {
                op: cmp_op_str(*op),
                threshold,
            }
        }
        PredicateKind::In { values } => PredicateKindView::In {
            values: values.clone(),
        },
        PredicateKind::Exists => PredicateKindView::Exists,
        PredicateKind::Between { lo, hi } => {
            // _ts 伪字段特例——时间戳 between 走 speedate（D5 不适用）
            if fp.field == "_ts" {
                let lo_ts = parse_timestamp(lo).ok_or_else(|| crate::GxpError::Runtime {
                    detail: format!("_ts between 下界 '{lo}' 不是合法 ISO 8601 时间戳"),
                })?;
                let hi_ts = parse_timestamp(hi).ok_or_else(|| crate::GxpError::Runtime {
                    detail: format!("_ts between 上界 '{hi}' 不是合法 ISO 8601 时间戳"),
                })?;
                PredicateKindView::TimeBetween {
                    lo: lo_ts as f64,
                    hi: hi_ts as f64,
                }
            } else {
                let (lo_f, hi_f) =
                    build_between_bounds(lo, hi).map_err(|e| crate::GxpError::Runtime {
                        detail: format!("{e}"),
                    })?;
                PredicateKindView::Between { lo: lo_f, hi: hi_f }
            }
        }
    };
    Ok(FieldPredicateView {
        field: fp.field.clone(),
        kind,
    })
}

/// [`CmpOp`] → 字符串（与 EBNF 语法一致）。
fn cmp_op_str(op: CmpOp) -> String {
    match op {
        CmpOp::Gt => ">".to_string(),
        CmpOp::Ge => ">=".to_string(),
        CmpOp::Lt => "<".to_string(),
        CmpOp::Le => "<=".to_string(),
        CmpOp::Eq => "==".to_string(),
        CmpOp::Ne => "!=".to_string(),
        CmpOp::Assign => "=".to_string(),
    }
}

/// [`Stage`] → [`StageView`]。
fn stage_view(s: &Stage) -> StageView {
    match s {
        Stage::Where(_) => StageView::Where,
        Stage::Context { before, after } => StageView::Context {
            before: *before,
            after: *after,
        },
        Stage::Count => StageView::Count,
        Stage::GroupBy(field) => StageView::GroupBy {
            field: field.clone(),
        },
        Stage::Rank { top, by, desc } => StageView::Rank {
            top: *top,
            by: by.clone(),
            desc: *desc,
        },
        Stage::Head(n) => StageView::Head { n: *n },
        Stage::Sample { spec } => StageView::Sample {
            spec: match spec {
                crate::dsl::ast::SampleSpec::Count(n) => SampleSpecView::Count { n: *n },
                crate::dsl::ast::SampleSpec::Ratio(p) => SampleSpecView::Ratio { p: *p },
            },
        },
        Stage::Fields(names) => StageView::Fields {
            names: names.clone(),
        },
        Stage::Files { invert } => StageView::Files { invert: *invert },
        Stage::Stats { field } => StageView::Stats {
            field: field.clone(),
        },
        Stage::Percentiles { field, percentiles } => StageView::Percentiles {
            field: field.clone(),
            percentiles: percentiles.clone(),
        },
        Stage::Sort { by, desc } => StageView::Sort {
            by: by.clone(),
            desc: *desc,
        },
        Stage::Distinct { by } => StageView::Distinct { by: by.clone() },
        Stage::Trace { lang } => StageView::Trace { lang: lang.clone() },
    }
}

impl ExecutionPlan {
    /// 文本渲染（默认输出）。
    ///
    /// 格式：
    /// ```text
    /// L1 必选组（组间 AND）:
    ///   组 1 (OR): "error", "timeout"
    ///   组 2 (OR): "fail"
    /// L2 字段谓词:
    ///   cost_ms > 800
    ///   tag in [info, error]
    /// 管道动词:
    ///   rank top 20 by cost_ms desc
    ///   fields _ts, _tag, cost_ms
    /// ```
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("L1 必选组（组间 AND）:\n");
        if self.required_groups.is_empty() {
            out.push_str(
                "  (无字面量约束——纯字段谓词 / .* / Or 混合非字面量分支 [Regex/And/Not，B14])\n",
            );
        } else {
            for (i, g) in self.required_groups.iter().enumerate() {
                let lits: Vec<String> = g.literals.iter().map(|s| format!("{s:?}")).collect();
                out.push_str(&format!("  组 {} (OR): {}\n", i + 1, lits.join(", ")));
            }
        }

        out.push_str("L2 字段谓词:\n");
        if self.field_predicates.is_empty() {
            out.push_str("  (无字段谓词)\n");
        } else {
            for fp in &self.field_predicates {
                out.push_str("  ");
                out.push_str(&render_predicate_text(fp));
                out.push('\n');
            }
        }

        out.push_str("管道动词:\n");
        if self.stages.is_empty() {
            out.push_str("  (无管道动词)\n");
        } else {
            for s in &self.stages {
                out.push_str("  ");
                out.push_str(&render_stage_text(s));
                out.push('\n');
            }
        }
        out
    }

    /// JSON 渲染（`--json` 输出）。
    pub fn render_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::GxpError::Runtime {
            detail: format!("执行计划序列化失败: {e}"),
        })
    }
}

/// 单个字段谓词的文本渲染。
fn render_predicate_text(fp: &FieldPredicateView) -> String {
    match &fp.kind {
        PredicateKindView::Compare { op, threshold } => {
            format!("{} {op} {}", fp.field, format_threshold(*threshold))
        }
        PredicateKindView::In { values } => {
            format!("{} in [{}]", fp.field, values.join(", "))
        }
        PredicateKindView::Exists => format!("{} exists", fp.field),
        PredicateKindView::Between { lo, hi } => {
            format!(
                "{} between {} and {}",
                fp.field,
                format_threshold(*lo),
                format_threshold(*hi)
            )
        }
        PredicateKindView::TimeBetween { lo, hi } => {
            format!("{} between (epoch {} and {})", fp.field, lo, hi)
        }
    }
}

/// 单个管道动词的文本渲染。
fn render_stage_text(s: &StageView) -> String {
    match s {
        StageView::Where => "where <expr>".to_string(),
        StageView::Context { before, after } => {
            if before == after {
                format!("context {before}")
            } else {
                format!("context before:{before} after:{after}")
            }
        }
        StageView::Count => "count".to_string(),
        StageView::GroupBy { field } => format!("group by {field}"),
        StageView::Rank { top, by, desc } => {
            let dir = if *desc { "desc" } else { "asc" };
            format!("rank top {top} by {by} {dir}")
        }
        StageView::Head { n } => format!("head {n}"),
        StageView::Sample { spec } => match spec {
            SampleSpecView::Count { n } => format!("sample {n} (seed=0)"),
            SampleSpecView::Ratio { p } => {
                let pct = p * 100.0;
                if pct.fract() == 0.0 {
                    format!("sample {}% (seed=0)", pct as u32)
                } else {
                    format!("sample {:.2}% (seed=0)", pct)
                }
            }
        },
        StageView::Fields { names } => format!("fields {}", names.join(", ")),
        StageView::Files { invert } => {
            if *invert {
                "files --invert".to_string()
            } else {
                "files".to_string()
            }
        }
        StageView::Stats { field } => format!("stats {field} (min/max/avg)"),
        StageView::Percentiles { field, percentiles } => {
            let ps: Vec<String> = percentiles
                .iter()
                .map(|p| format!("p{}", *p as u64))
                .collect();
            format!("percentiles {field} {}", ps.join(","))
        }
        StageView::Sort { by, desc } => {
            let dir = if *desc { "desc" } else { "asc" };
            match by {
                Some(f) => format!("sort by {f} {dir}"),
                None => format!("sort {dir}"),
            }
        }
        StageView::Distinct { by } => match by {
            Some(f) => format!("distinct by {f}"),
            None => "distinct".to_string(),
        },
        StageView::Trace { lang } => match lang {
            Some(l) => format!("trace {l} (multi-line stack trace)"),
            None => "trace (auto-detect, multi-line stack trace)".to_string(),
        },
    }
}

/// f64 阈值渲染：整数无小数尾巴，浮点保留原样。
fn format_threshold(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::ast::{CmpOp, Expr, Pred, Prim};
    use crate::l1::compiler::compile;

    /// 构造 `"error" && cost_ms>800` 的 Query。
    fn sample_query() -> Query {
        Query {
            expr: Expr::And(
                Box::new(Expr::Prim(Prim::String("error".to_string()))),
                Box::new(Expr::Prim(Prim::Pred(Pred::Compare {
                    name: "cost_ms".to_string(),
                    op: CmpOp::Gt,
                    value: "800".to_string(),
                }))),
            ),
            stages: vec![Stage::Head(10)],
        }
    }

    #[test]
    fn test_build_plan_single_literal_and_compare() {
        let q = sample_query();
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        assert_eq!(plan.required_groups.len(), 1);
        assert_eq!(plan.required_groups[0].literals, vec!["error".to_string()]);
        assert_eq!(plan.field_predicates.len(), 1);
        match &plan.field_predicates[0].kind {
            PredicateKindView::Compare { op, threshold } => {
                assert_eq!(op, ">");
                assert!((threshold - 800.0).abs() < f64::EPSILON);
            }
            other => panic!("期望 Compare，得到 {:?}", other),
        }
        assert_eq!(plan.stages.len(), 1);
        match &plan.stages[0] {
            StageView::Head { n } => assert_eq!(*n, 10),
            other => panic!("期望 Head，得到 {:?}", other),
        }
    }

    #[test]
    fn test_render_text_shows_required_groups_and_predicate() {
        let q = sample_query();
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        let text = plan.render_text();
        assert!(
            text.contains("L1 必选组"),
            "文本应含 L1 必选组标题，实际: {text}"
        );
        assert!(
            text.contains("\"error\""),
            "文本应含字面量 error，实际: {text}"
        );
        assert!(
            text.contains("cost_ms > 800"),
            "文本应含字段谓词，实际: {text}"
        );
        assert!(
            text.contains("管道动词"),
            "文本应含管道动词标题，实际: {text}"
        );
        assert!(text.contains("head 10"), "文本应含 head 10，实际: {text}");
    }

    #[test]
    fn test_render_json_contains_expected_fields() {
        let q = sample_query();
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        let json = plan.render_json().expect("render_json 应成功");
        assert!(
            json.contains("\"required_groups\""),
            "JSON 应含 required_groups: {json}"
        );
        assert!(json.contains("\"error\""), "JSON 应含字面量 error: {json}");
        assert!(
            json.contains("\"field_predicates\""),
            "JSON 应含 field_predicates: {json}"
        );
        assert!(json.contains("\"cost_ms\""), "JSON 应含 cost_ms: {json}");
        assert!(
            json.contains("\"threshold\": 800.0"),
            "JSON 应含阈值 800.0: {json}"
        );
        assert!(json.contains("\"stages\""), "JSON 应含 stages: {json}");
    }

    #[test]
    fn test_build_plan_rejects_invalid_threshold() {
        // 阈值 "NaN" 不是合法数值（11 维契约拒绝）
        let q = Query {
            expr: Expr::Prim(Prim::Pred(Pred::Compare {
                name: "cost_ms".to_string(),
                op: CmpOp::Gt,
                value: "NaN".to_string(),
            })),
            stages: vec![],
        };
        let compiled = compile(&q.expr).expect("编译应成功");
        let result = build_plan(&q, &compiled);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("不是合法数值"),
            "错误应含'不是合法数值'，实际: {err}"
        );
        assert!(err.contains("NaN"), "错误应含 NaN，实际: {err}");
    }

    #[test]
    fn test_build_plan_between_bounds_parsed() {
        let q = Query {
            expr: Expr::Prim(Prim::Pred(Pred::Between {
                name: "cost".to_string(),
                lo: "100".to_string(),
                hi: "200".to_string(),
            })),
            stages: vec![],
        };
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        assert_eq!(plan.field_predicates.len(), 1);
        match &plan.field_predicates[0].kind {
            PredicateKindView::Between { lo, hi } => {
                assert!((lo - 100.0).abs() < f64::EPSILON);
                assert!((hi - 200.0).abs() < f64::EPSILON);
            }
            other => panic!("期望 Between，得到 {:?}", other),
        }
    }

    #[test]
    fn test_build_plan_between_rejects_invalid_bound() {
        let q = Query {
            expr: Expr::Prim(Prim::Pred(Pred::Between {
                name: "cost".to_string(),
                lo: "abc".to_string(),
                hi: "200".to_string(),
            })),
            stages: vec![],
        };
        let compiled = compile(&q.expr).expect("编译应成功");
        let result = build_plan(&q, &compiled);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("下界") || err.contains("上界") || err.contains("abc"),
            "错误应含下界/上界/abc，实际: {err}"
        );
    }

    #[test]
    fn test_render_text_empty_query() {
        // 纯字面量查询，无字段谓词，无管道动词
        let q = Query {
            expr: Expr::Prim(Prim::String("error".to_string())),
            stages: vec![],
        };
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        let text = plan.render_text();
        assert!(
            text.contains("(无字段谓词)"),
            "文本应含'无字段谓词'，实际: {text}"
        );
        assert!(
            text.contains("(无管道动词)"),
            "文本应含'无管道动词'，实际: {text}"
        );
    }

    #[test]
    fn test_render_text_pure_predicate_query() {
        // 纯字段谓词查询，无字面量
        let q = Query {
            expr: Expr::Prim(Prim::Pred(Pred::Compare {
                name: "cost_ms".to_string(),
                op: CmpOp::Gt,
                value: "800".to_string(),
            })),
            stages: vec![],
        };
        let compiled = compile(&q.expr).expect("编译应成功");
        let plan = build_plan(&q, &compiled).expect("build_plan 应成功");
        let text = plan.render_text();
        assert!(
            text.contains("(无字面量约束"),
            "文本应含'无字面量约束'，实际: {text}"
        );
    }
}
