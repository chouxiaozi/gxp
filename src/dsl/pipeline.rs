//! gxp 管道 tokenizer（手写，设计文档 §2.2 + Rust 方案 §7.6）。
//!
//! 管道动词（`stage` 规则）：
//! - `where EXPR`：对上游结果再过滤
//! - `rank top K by FIELD [desc|asc]`：BM25 Top-K 排序
//! - `head N`：限制输出行数
//! - `fields NAME (, NAME)*`：字段投影
//! - `count`：计数
//! - `group by NAME`：按字段分组
//! - `context N` / `context before:N after:M`：上下文行（等价 `grep -C` / `-B` / `-A`）
//! - `files [--invert]`：只输出文件名（等价 `grep -l` / `-L`）
//! - `stats FIELD`：流式聚合 min/max/avg
//! - `sort [by FIELD] [desc|asc]`：全量稳定排序（对齐 SQL ORDER BY / Unix sort）
//! - `distinct [by FIELD]`：稳定去重（对齐 SQL DISTINCT，保留首次出现的行）
//!
//! `stage` 规则解析失败必须 fail-loud（设计文档 §2.1 原则 4）：
//! - 未知动词 → `DslError::UnknownStage`
//! - `rank` 缺 `top`/`by` → `MalformedStage`
//! - `head N` / `context N` 的 N 非正整数 → `InvalidNumber`
//! - `files --invert` 以外的 flag → `UnknownFlag`
//!
//! `|` 歧义（Rust 方案 §7.5 风险表）：引号外 `|` 是管道；引号内 `|` 是 ERE OR。
//! 引号内的拆分由上层 [`parse_query`](super::parser::parse_query) 处理，这里只接收已切好的 token 列表。

use crate::dsl::ast::{SampleSpec, Stage};
use crate::error::{GxpError, Result};

/// 解析管道动词序列。
///
/// 入参 `tokens` 是按 `|` 切分后的每段（首段为表达式，不在此函数处理）。
/// 每个 token 已按空白切分（由上层负责；引号内空格不拆分）。
///
/// # 示例
///
/// ```
/// use gxp::dsl::pipeline::parse_pipeline;
/// use gxp::dsl::ast::Stage;
///
/// let tokens = vec!["context".to_string(), "3".to_string()];
/// let stages = parse_pipeline(&tokens).unwrap();
/// assert_eq!(stages, vec![Stage::Context { before: 3, after: 3 }]);
/// ```
pub fn parse_pipeline(tokens: &[String]) -> Result<Vec<Stage>> {
    let mut stages = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let stage = parse_one_stage(tokens, &mut i)?;
        stages.push(stage);
    }
    Ok(stages)
}

/// 解析单个 stage，从 `tokens[i]` 起，成功后推进 `i`。
fn parse_one_stage(tokens: &[String], i: &mut usize) -> Result<Stage> {
    let verb = tokens[*i].as_str();
    *i += 1;
    match verb {
        "where" => {
            // `where EXPR`：EXPR 由上层表达式解析器处理。
            // 注意：原实现只取单个 token，多 token 谓词（如 `cost between 100 and 500`）
            // 被切断，剩余 token 被当作下一个 stage 动词。
            // 修复：收集 where 后所有 token 直到遇到下一个 stage 动词或 token 列表结束，
            // 用空格重组为完整 EXPR 字符串，再调 parse_query_inline。
            if *i >= tokens.len() {
                return Err(GxpError::Parse {
                    detail: format!("'where' 缺少表达式（token 位置 {}）", *i),
                });
            }
            let mut expr_parts: Vec<&str> = Vec::new();
            while *i < tokens.len() {
                let tok = tokens[*i].as_str();
                if is_verb(tok) {
                    break;
                }
                expr_parts.push(tok);
                *i += 1;
            }
            if expr_parts.is_empty() {
                return Err(GxpError::Parse {
                    detail: format!("'where' 缺少表达式（token 位置 {}）", *i),
                });
            }
            let expr_str = expr_parts.join(" ");
            Ok(Stage::Where(crate::dsl::parser::parse_query_inline(
                &expr_str,
            )?))
        }
        "rank" => {
            // rank top K by FIELD [desc|asc]
            expect_kw(tokens, i, "top")?;
            let top = parse_usize(tokens, i, "top")?;
            expect_kw(tokens, i, "by")?;
            let by = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'rank top K by' 缺少字段名".into(),
                })?
                .clone();
            *i += 1;
            // 可选 desc|asc（默认 desc，Rust 方案 §7.6）
            let desc = match tokens.get(*i).map(|s| s.as_str()) {
                Some("desc") => {
                    *i += 1;
                    true
                }
                Some("asc") => {
                    *i += 1;
                    false
                }
                _ => true, // 默认降序
            };
            Ok(Stage::Rank { top, by, desc })
        }
        "head" => {
            let n = parse_usize(tokens, i, "head")?;
            Ok(Stage::Head(n))
        }
        "sample" => {
            // sample N（整数） / sample P%（P ∈ 0..=100）
            // 形式不可混：50 是条数，50% 是比例
            let raw = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'sample' 缺少参数（N 或 P%，N 为非负整数，P ∈ 0..=100）".into(),
                })?
                .clone();
            *i += 1;

            // % 后缀 → ratio
            if let Some(stripped) = raw.strip_suffix('%') {
                let p: u32 = stripped.parse::<u32>().map_err(|_| GxpError::Parse {
                    detail: format!(
                        "'sample P%' 的 P 需 0..=100 整数，得到 '{}'",
                        stripped
                    ),
                })?;
                if p > 100 {
                    return Err(GxpError::Parse {
                        detail: format!(
                            "'sample P%' 的 P 需 0..=100，得到 {}",
                            p
                        ),
                    });
                }
                let ratio = p as f64 / 100.0;
                Ok(Stage::Sample {
                    spec: SampleSpec::Ratio(ratio),
                })
            } else {
                // 纯整数 → count
                let n = raw.parse::<usize>().map_err(|_| GxpError::Parse {
                    detail: format!(
                        "'sample N' 的 N 需非负整数（或 P% 形式的百分比），得到 '{}'",
                        raw
                    ),
                })?;
                Ok(Stage::Sample {
                    spec: SampleSpec::Count(n),
                })
            }
        }
        "sort" => {
            // sort [by KEY] [desc|asc]
            // - 省略 by → 按整行字节序（聚集相同错误，对齐 Unix sort 默认）
            // - 默认 asc（对齐 SQL ORDER BY / Unix sort / Python sorted）
            // - desc/asc 可选
            let mut by: Option<String> = None;
            // peek：by 是可选关键字
            if tokens.get(*i).map(|s| s.as_str()) == Some("by") {
                *i += 1;
                by = Some(
                    tokens
                        .get(*i)
                        .ok_or_else(|| GxpError::Parse {
                            detail: "'sort by' 缺少字段名".into(),
                        })?
                        .clone(),
                );
                *i += 1;
            }
            // 可选 desc|asc（默认 asc）
            let desc = match tokens.get(*i).map(|s| s.as_str()) {
                Some("desc") => {
                    *i += 1;
                    true
                }
                Some("asc") => {
                    *i += 1;
                    false
                }
                _ => false, // 默认升序
            };
            Ok(Stage::Sort { by, desc })
        }
        "distinct" => {
            // distinct [by FIELD]
            // - 省略 by → 整行字节去重（对齐 Unix `sort | uniq`，但不排序）
            // - by FIELD → 按字段值去重（对齐 SQL `SELECT DISTINCT ON (field) *`）
            let mut by: Option<String> = None;
            if tokens.get(*i).map(|s| s.as_str()) == Some("by") {
                *i += 1;
                by = Some(
                    tokens
                        .get(*i)
                        .ok_or_else(|| GxpError::Parse {
                            detail: "'distinct by' 缺少字段名".into(),
                        })?
                        .clone(),
                );
                *i += 1;
            }
            // 拒绝 distinct 后的未知 flag（fail-loud，避免静默吞参数）
            if let Some(extra) = tokens.get(*i) {
                if extra.starts_with("--") {
                    return Err(GxpError::Parse {
                        detail: format!("'distinct' 不接受 flag '{}'", extra),
                    });
                }
            }
            Ok(Stage::Distinct { by })
        }
        "fields" => {
            // fields a, b, c 或 fields a,b,c（允许逗号粘在一起）
            let raw: Vec<String> = tokens[*i..]
                .iter()
                .take_while(|t| !is_verb(t))
                .cloned()
                .collect();
            if raw.is_empty() {
                return Err(GxpError::Parse {
                    detail: "'fields' 至少需要 1 个字段名".into(),
                });
            }
            *i += raw.len();
            // 拆 a,b,c
            let mut fields = Vec::new();
            for tok in raw {
                for part in tok.split(',') {
                    let t = part.trim();
                    if !t.is_empty() {
                        fields.push(t.to_string());
                    }
                }
            }
            if fields.is_empty() {
                return Err(GxpError::Parse {
                    detail: "'fields' 解析后无有效字段名".into(),
                });
            }
            Ok(Stage::Fields(fields))
        }
        "count" => Ok(Stage::Count),
        "group" => {
            expect_kw(tokens, i, "by")?;
            let name = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'group by' 缺少字段名".into(),
                })?
                .clone();
            *i += 1;
            Ok(Stage::GroupBy(name))
        }
        "context" => {
            // context N → before=after=N
            // context before:N after:M / context after:M before:N（顺序不固定）
            let next = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'context' 缺少参数（N 或 before:N after:M）".into(),
                })?
                .clone();
            if next.starts_with("before:") || next.starts_with("after:") {
                let mut before = 0usize;
                let mut after = 0usize;
                while *i < tokens.len() {
                    let tok = &tokens[*i];
                    if let Some(n) = tok.strip_prefix("before:") {
                        before = n.parse::<usize>().map_err(|_| GxpError::Parse {
                            detail: format!("'before:' 后需非负整数，得到 '{}'", n),
                        })?;
                    } else if let Some(n) = tok.strip_prefix("after:") {
                        after = n.parse::<usize>().map_err(|_| GxpError::Parse {
                            detail: format!("'after:' 后需非负整数，得到 '{}'", n),
                        })?;
                    } else {
                        break;
                    }
                    *i += 1;
                }
                Ok(Stage::Context { before, after })
            } else {
                let n = next.parse::<usize>().map_err(|_| GxpError::Parse {
                    detail: format!("'context' 后需非负整数，得到 '{}'", next),
                })?;
                *i += 1;
                Ok(Stage::Context {
                    before: n,
                    after: n,
                })
            }
        }
        "files" => {
            let invert = tokens.get(*i).map(|s| s.as_str()) == Some("--invert");
            if invert {
                *i += 1;
            }
            // 拒绝 files 上的未知 flag（fail-loud）
            if let Some(extra) = tokens.get(*i) {
                if extra.starts_with("--") {
                    return Err(GxpError::Parse {
                        detail: format!("'files' 不接受 flag '{}'", extra),
                    });
                }
            }
            Ok(Stage::Files { invert })
        }
        "stats" => {
            // stats FIELD：流式聚合 min/max/avg
            let field = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'stats' 缺少字段名".into(),
                })?
                .clone();
            *i += 1;
            Ok(Stage::Stats { field })
        }
        "percentiles" => {
            // percentiles FIELD [p50,p95,p99]
            // - FIELD 必填
            // - 百分位列表可选（逗号分隔，如 p50,p95,p99），默认 [50, 95, 99]
            let field = tokens
                .get(*i)
                .ok_or_else(|| GxpError::Parse {
                    detail: "'percentiles' 缺少字段名".into(),
                })?
                .clone();
            *i += 1;
            // 可选百分位列表：peek 下一个 token，如果以 p 开头则解析
            let mut percentiles = vec![50.0, 95.0, 99.0]; // 默认
            if let Some(next) = tokens.get(*i) {
                if !is_verb(next) && !next.starts_with("--") {
                    // 解析 p50,p95,p99 格式
                    let parts: Vec<&str> = next.split(',').collect();
                    let mut parsed = Vec::with_capacity(parts.len());
                    for part in parts {
                        let trimmed = part.trim();
                        if let Some(num_str) = trimmed.strip_prefix('p') {
                            let p = num_str.parse::<f64>().map_err(|_| GxpError::Parse {
                                detail: format!(
                                    "百分位 '{}' 需 p 后跟数字（如 p50），得到 '{}'",
                                    trimmed, num_str
                                ),
                            })?;
                            if !(0.0..=100.0).contains(&p) {
                                return Err(GxpError::Parse {
                                    detail: format!("百分位需 0-100，得到 {p}"),
                                });
                            }
                            parsed.push(p);
                        } else {
                            return Err(GxpError::Parse {
                                detail: format!(
                                    "百分位需 p 前缀（如 p50,p95,p99），得到 '{trimmed}'"
                                ),
                            });
                        }
                    }
                    if parsed.is_empty() {
                        return Err(GxpError::Parse {
                            detail: "百分位列表不能为空".into(),
                        });
                    }
                    percentiles = parsed;
                    *i += 1;
                }
            }
            Ok(Stage::Percentiles { field, percentiles })
        }
        "trace" => {
            // trace [LANG]
            // - LANG 可选：java/python/rust/js/go/csharp（大小写不敏感）
            // - 省略则自动检测（pipeline 阶段按命中首行特征识别）
            let lang = if let Some(next) = tokens.get(*i) {
                if is_verb(next) || next.starts_with("--") {
                    None
                } else {
                    let lang = next.to_lowercase();
                    let valid = matches!(
                        lang.as_str(),
                        "java" | "python" | "rust" | "js" | "go" | "csharp"
                    );
                    if !valid {
                        return Err(GxpError::Parse {
                            detail: format!(
                                "trace 不支持语言 '{}'（合法：java/python/rust/js/go/csharp）",
                                next
                            ),
                        });
                    }
                    *i += 1;
                    Some(lang)
                }
            } else {
                None
            };
            Ok(Stage::Trace { lang })
        }
        other => Err(GxpError::Parse {
            detail: format!(
                "未知管道动词 '{}'（合法：where/rank/sort/distinct/head/sample/fields/count/group/context/files/stats/percentiles/trace）",
                other
            ),
        }),
    }
}

/// 期望当前 token 等于 `kw`，不匹配则报错并返回 Err。
fn expect_kw(tokens: &[String], i: &mut usize, kw: &str) -> Result<()> {
    let got = tokens.get(*i).map(|s| s.as_str());
    if got != Some(kw) {
        return Err(GxpError::Parse {
            detail: format!("期望关键字 '{}'，得到 {:?}", kw, got),
        });
    }
    *i += 1;
    Ok(())
}

/// 解析非负整数 token，失败则 fail-loud。
fn parse_usize(tokens: &[String], i: &mut usize, ctx: &str) -> Result<usize> {
    let s = tokens
        .get(*i)
        .ok_or_else(|| GxpError::Parse {
            detail: format!("'{}' 缺少正整数参数", ctx),
        })?
        .as_str();
    let n = s.parse::<usize>().map_err(|_| GxpError::Parse {
        detail: format!("'{}' 后需非负整数，得到 '{}'", ctx, s),
    })?;
    *i += 1;
    Ok(n)
}

/// 判断 token 是否是已知管道动词（用于 `fields` 的贪婪消费终止条件）。
fn is_verb(s: &str) -> bool {
    matches!(
        s,
        "where"
            | "rank"
            | "sort"
            | "distinct"
            | "head"
            | "sample"
            | "fields"
            | "count"
            | "group"
            | "context"
            | "files"
            | "stats"
            | "percentiles"
            | "trace"
    )
}

#[cfg(test)]
mod tests {
    //! 管道 tokenizer 单测：覆盖 8 种动词 + fail-loud 场景。
    //! AST 结构断言放集成测试 `tests/dsl_parser.rs`，这里只测 tokenizer 行为。

    use super::*;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_context_single_n_sets_before_after_equal() {
        let s = parse_pipeline(&toks(&["context", "3"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Context {
                before: 3,
                after: 3
            }]
        );
    }

    #[test]
    fn test_context_before_after_explicit() {
        let s = parse_pipeline(&toks(&["context", "before:1", "after:3"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Context {
                before: 1,
                after: 3
            }]
        );
    }

    #[test]
    fn test_context_after_before_order_independent() {
        let s = parse_pipeline(&toks(&["context", "after:5", "before:2"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Context {
                before: 2,
                after: 5
            }]
        );
    }

    #[test]
    fn test_files_invert_flag_recognized() {
        let s = parse_pipeline(&toks(&["files", "--invert"])).unwrap();
        assert_eq!(s, vec![Stage::Files { invert: true }]);
    }

    #[test]
    fn test_files_no_flag_defaults_to_match_only() {
        let s = parse_pipeline(&toks(&["files"])).unwrap();
        assert_eq!(s, vec![Stage::Files { invert: false }]);
    }

    #[test]
    fn test_files_rejects_unknown_flag_fail_loud() {
        // fail-loud（设计文档 §2.1 原则 4）：未知 flag 不静默忽略
        let err = parse_pipeline(&toks(&["files", "--foo"])).unwrap_err();
        assert!(matches!(err, GxpError::Parse { .. }));
        assert!(format!("{err}").contains("--foo"));
    }

    #[test]
    fn test_rank_top_k_by_field_desc_default() {
        let s = parse_pipeline(&toks(&["rank", "top", "20", "by", "cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Rank {
                top: 20,
                by: "cost_ms".into(),
                desc: true
            }]
        );
    }

    #[test]
    fn test_rank_explicit_asc_overrides_default() {
        let s = parse_pipeline(&toks(&["rank", "top", "10", "by", "_ts", "asc"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Rank {
                top: 10,
                by: "_ts".into(),
                desc: false
            }]
        );
    }

    #[test]
    fn test_rank_missing_top_keyword_fails_loud() {
        let err = parse_pipeline(&toks(&["rank", "20", "by", "x"])).unwrap_err();
        assert!(format!("{err}").contains("top"));
    }

    #[test]
    fn test_head_parses_usize() {
        let s = parse_pipeline(&toks(&["head", "30"])).unwrap();
        assert_eq!(s, vec![Stage::Head(30)]);
    }

    #[test]
    fn test_head_non_integer_fails_loud() {
        let err = parse_pipeline(&toks(&["head", "abc"])).unwrap_err();
        assert!(format!("{err}").contains("head"));
    }

    #[test]
    fn test_count_no_args() {
        let s = parse_pipeline(&toks(&["count"])).unwrap();
        assert_eq!(s, vec![Stage::Count]);
    }

    #[test]
    fn test_group_by_field() {
        let s = parse_pipeline(&toks(&["group", "by", "_tag"])).unwrap();
        assert_eq!(s, vec![Stage::GroupBy("_tag".into())]);
    }

    #[test]
    fn test_group_missing_by_fails_loud() {
        let err = parse_pipeline(&toks(&["group", "_tag"])).unwrap_err();
        assert!(format!("{err}").contains("by"));
    }

    #[test]
    fn test_fields_space_separated() {
        let s = parse_pipeline(&toks(&["fields", "_ts", "_tag", "cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Fields(vec![
                "_ts".into(),
                "_tag".into(),
                "cost_ms".into()
            ])]
        );
    }

    #[test]
    fn test_fields_comma_separated_single_token() {
        let s = parse_pipeline(&toks(&["fields", "_ts,_tag,cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Fields(vec![
                "_ts".into(),
                "_tag".into(),
                "cost_ms".into()
            ])]
        );
    }

    #[test]
    fn test_fields_mixed_space_and_comma() {
        let s = parse_pipeline(&toks(&["fields", "_ts,_tag", "cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Fields(vec![
                "_ts".into(),
                "_tag".into(),
                "cost_ms".into()
            ])]
        );
    }

    #[test]
    fn test_unknown_verb_fails_loud() {
        let err = parse_pipeline(&toks(&["foo", "3"])).unwrap_err();
        assert!(format!("{err}").contains("未知管道动词"));
    }

    #[test]
    fn test_multiple_stages_in_sequence() {
        // ERROR | context 3 | count 的管道段（不含 expr 部分）
        let s = parse_pipeline(&toks(&["context", "3", "count"])).unwrap();
        assert_eq!(
            s,
            vec![
                Stage::Context {
                    before: 3,
                    after: 3
                },
                Stage::Count
            ]
        );
    }

    // stats

    #[test]
    fn test_stats_parses_field_name() {
        let s = parse_pipeline(&toks(&["stats", "cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Stats {
                field: "cost_ms".into()
            }]
        );
    }

    #[test]
    fn test_stats_missing_field_fails_loud() {
        let err = parse_pipeline(&toks(&["stats"])).unwrap_err();
        assert!(format!("{err}").contains("stats"));
        assert!(format!("{err}").contains("字段名"));
    }

    #[test]
    fn test_stats_is_recognized_as_verb() {
        // stats 应该被 is_verb 识别（用于 fields 贪婪消费终止条件）
        assert!(is_verb("stats"));
    }

    #[test]
    fn test_stats_in_sequence_with_head() {
        // stats cost_ms | head 20
        let s = parse_pipeline(&toks(&["stats", "cost_ms", "head", "20"])).unwrap();
        assert_eq!(
            s,
            vec![
                Stage::Stats {
                    field: "cost_ms".into()
                },
                Stage::Head(20)
            ]
        );
    }

    // sort

    #[test]
    fn test_sort_no_args_defaults_asc_by_line() {
        // sort → by=None, desc=false（默认升序、按整行字节序）
        let s = parse_pipeline(&toks(&["sort"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: None,
                desc: false
            }]
        );
    }

    #[test]
    fn test_sort_desc_only_sets_desc_true() {
        // sort desc → by=None, desc=true
        let s = parse_pipeline(&toks(&["sort", "desc"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: None,
                desc: true
            }]
        );
    }

    #[test]
    fn test_sort_asc_only_explicit_asc() {
        // sort asc → by=None, desc=false
        let s = parse_pipeline(&toks(&["sort", "asc"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: None,
                desc: false
            }]
        );
    }

    #[test]
    fn test_sort_by_field_defaults_asc() {
        // sort by cost → by=Some, desc=false（默认升序）
        let s = parse_pipeline(&toks(&["sort", "by", "cost_ms"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: Some("cost_ms".into()),
                desc: false
            }]
        );
    }

    #[test]
    fn test_sort_by_field_desc() {
        let s = parse_pipeline(&toks(&["sort", "by", "cost_ms", "desc"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: Some("cost_ms".into()),
                desc: true
            }]
        );
    }

    #[test]
    fn test_sort_by_field_asc_explicit() {
        let s = parse_pipeline(&toks(&["sort", "by", "_ts", "asc"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Sort {
                by: Some("_ts".into()),
                desc: false
            }]
        );
    }

    #[test]
    fn test_sort_by_missing_field_fails_loud() {
        // sort by 缺字段名 → fail-loud
        let err = parse_pipeline(&toks(&["sort", "by"])).unwrap_err();
        assert!(format!("{err}").contains("sort by"));
        assert!(format!("{err}").contains("字段名"));
    }

    #[test]
    fn test_sort_is_recognized_as_verb() {
        // sort 应该被 is_verb 识别（用于 fields 贪婪消费终止条件）
        assert!(is_verb("sort"));
    }

    #[test]
    fn test_sort_in_sequence_with_head() {
        // sort by cost_ms desc | head 20
        let s = parse_pipeline(&toks(&["sort", "by", "cost_ms", "desc", "head", "20"])).unwrap();
        assert_eq!(
            s,
            vec![
                Stage::Sort {
                    by: Some("cost_ms".into()),
                    desc: true
                },
                Stage::Head(20)
            ]
        );
    }

    // distinct

    #[test]
    fn test_distinct_no_args_defaults_to_whole_line() {
        // distinct → by=None
        let s = parse_pipeline(&toks(&["distinct"])).unwrap();
        assert_eq!(s, vec![Stage::Distinct { by: None }]);
    }

    #[test]
    fn test_distinct_by_field() {
        // distinct by user → by=Some("user")
        let s = parse_pipeline(&toks(&["distinct", "by", "user"])).unwrap();
        assert_eq!(
            s,
            vec![Stage::Distinct {
                by: Some("user".into())
            }]
        );
    }

    #[test]
    fn test_distinct_by_missing_field_fails_loud() {
        // distinct by 缺字段名 → fail-loud
        let err = parse_pipeline(&toks(&["distinct", "by"])).unwrap_err();
        assert!(format!("{err}").contains("distinct by"));
        assert!(format!("{err}").contains("字段名"));
    }

    #[test]
    fn test_distinct_rejects_unknown_flag() {
        // fail-loud：未知 flag 不静默忽略
        let err = parse_pipeline(&toks(&["distinct", "--foo"])).unwrap_err();
        assert!(format!("{err}").contains("--foo"));
    }

    #[test]
    fn test_distinct_is_recognized_as_verb() {
        // distinct 应被 is_verb 识别（用于 fields 贪婪消费终止条件）
        assert!(is_verb("distinct"));
    }

    #[test]
    fn test_distinct_in_sequence_with_head() {
        // distinct by user | head 5
        let s = parse_pipeline(&toks(&["distinct", "by", "user", "head", "5"])).unwrap();
        assert_eq!(
            s,
            vec![
                Stage::Distinct {
                    by: Some("user".into())
                },
                Stage::Head(5)
            ]
        );
    }
}
