//! gxp DSL 解析器集成测试。
//!
//! 对应设计文档 §2.4 的 11 个示例查询（示例 11 是 CLI 层段落式报错，已在
//! `src/mode/reject_grep_opts.rs` 涵盖，本文件只测表达式 + 管道）。
//!
//! 断言策略：不断言"解析成功"，每个示例都断言 AST 结构（Expr 变体、Stage 字段）。
use gxp::dsl::ast::*;
use gxp::dsl::parser::parse_query;

/// 断言 `parse_query(input)` 返回 Err 且错误消息含 `substr`。
fn assert_parse_fails(input: &str, substr: &str) {
    let err = parse_query(input).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains(substr),
        "期望错误消息含 {:?}，得到 {:?}",
        substr,
        msg
    );
}

// ===== 示例 1：纯 grep 兼容（BARE 模式）=====

#[test]
fn test_example_1_bare_simple() {
    // gxp 'ERROR' app.log ≡ grep -E 'ERROR' app.log
    let q = parse_query("ERROR").unwrap();
    assert_eq!(q.stages, vec![]);
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "ERROR"),
        other => panic!("期望 Prim::Bare，得到 {:?}", other),
    }
}

#[test]
fn test_example_1_bare_with_space() {
    // gxp 'ERROR timeout' app.log ≡ grep -E 'ERROR timeout' app.log
    // 含空格的 BARE 匹配连续子串（不拆成多个 token）
    let q = parse_query("ERROR timeout").unwrap();
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "ERROR timeout"),
        other => panic!("期望 Prim::Bare，得到 {:?}", other),
    }
}

#[test]
fn test_example_1_bare_ere_or() {
    // gxp 'a|b' app.log ≡ grep -E 'a|b' app.log（ERE 的 OR）
    // 注意：单 | 在 BARE 内是 ERE OR；引号外的 | 才是管道
    let q = parse_query("a|b").unwrap();
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "a|b"),
        other => panic!("期望 Prim::Bare，得到 {:?}", other),
    }
}

// ===== 示例 2：布尔 + 字段作用域 + 数值域 + Top-K =====

#[test]
fn test_example_2_full_query_with_pipeline() {
    // gxp '("timeout" || "latency") && _tag in [checkout,billing] && cost_ms>800
    //      | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms' app.log
    let input = r#"("timeout" || "latency") && _tag in [checkout,billing] && cost_ms>800 | rank top 20 by cost_ms desc | fields _ts,_tag,cost_ms"#;
    let q = parse_query(input).unwrap();
    // 三段 pipeline
    assert_eq!(q.stages.len(), 2);
    // 第一段：rank top 20 by cost_ms desc
    assert_eq!(
        q.stages[0],
        Stage::Rank {
            top: 20,
            by: "cost_ms".into(),
            desc: true,
        }
    );
    // 第二段：fields _ts,_tag,cost_ms
    assert_eq!(
        q.stages[1],
        Stage::Fields(vec!["_ts".into(), "_tag".into(), "cost_ms".into()])
    );
    // expr 结构：And(And(Or(String,String), In), Compare)
    match q.expr {
        Expr::And(outer_l, outer_r) => {
            // 右枝：cost_ms>800
            match *outer_r {
                Expr::Prim(Prim::Pred(Pred::Compare { name, op, value })) => {
                    assert_eq!(name, "cost_ms");
                    assert_eq!(op, CmpOp::Gt);
                    assert_eq!(value, "800");
                }
                other => panic!("右枝应为 Compare，得到 {:?}", other),
            }
            // 左枝：And(Or, In)
            match *outer_l {
                Expr::And(inner_l, inner_r) => {
                    // inner_r: _tag in [checkout,billing]
                    match *inner_r {
                        Expr::Prim(Prim::Pred(Pred::In { name, values })) => {
                            assert_eq!(name, "_tag");
                            assert_eq!(values, vec!["checkout".to_string(), "billing".into()]);
                        }
                        other => panic!("应为 In，得到 {:?}", other),
                    }
                    // inner_l: ("timeout" || "latency")
                    match *inner_l {
                        Expr::Prim(Prim::Paren(p)) => match *p {
                            Expr::Or(l, r) => {
                                assert!(
                                    matches!(*l, Expr::Prim(Prim::String(s)) if s == "timeout")
                                );
                                assert!(
                                    matches!(*r, Expr::Prim(Prim::String(s)) if s == "latency")
                                );
                            }
                            other => panic!("Paren 内应为 Or，得到 {:?}", other),
                        },
                        other => panic!("应为 Paren，得到 {:?}", other),
                    }
                }
                other => panic!("左枝应为 And，得到 {:?}", other),
            }
        }
        other => panic!("顶层应为 And，得到 {:?}", other),
    }
}

// ===== 示例 3：聚合与可解释执行计划 =====

#[test]
fn test_example_3_group_by() {
    // gxp '"timeout" && cost_ms>900 | group by _tag' app.log
    let input = r#""timeout" && cost_ms>900 | group by _tag"#;
    let q = parse_query(input).unwrap();
    assert_eq!(q.stages.len(), 1);
    assert_eq!(q.stages[0], Stage::GroupBy("_tag".into()));
    match q.expr {
        Expr::And(l, r) => {
            assert!(matches!(*l, Expr::Prim(Prim::String(s)) if s == "timeout"));
            match *r {
                Expr::Prim(Prim::Pred(Pred::Compare {
                    name, op, value, ..
                })) => {
                    assert_eq!(name, "cost_ms");
                    assert_eq!(op, CmpOp::Gt);
                    assert_eq!(value, "900");
                }
                other => panic!("右枝应为 Compare，得到 {:?}", other),
            }
        }
        other => panic!("顶层应为 And，得到 {:?}", other),
    }
}

// ===== 示例 4：代码结构模式（ast-grep 对标）=====
// 注：`ast(...)` 和 `sym ref(...)` 是 L3 结构层功能，v1 骨架未实现。
// 测试可验证字符串解析阶段不 crash——L3 留给后续模块。
#[test]
fn test_example_4_structural_pattern_fails_loud() {
    // gxp 'ast(python, "useQuery($A)") && !"test" | sym ref(getUserById) | head 30' src/
    // v1 骨架：ast(...) 和 sym ref(...) 不实现为 pred，只作为 BARE/管道处理。
    // 验证：解析不 panic，且管道 head 30 被识别。
    let input = r#"ast(python, "useQuery($A)") && !"test" | sym ref(getUserById) | head 30"#;
    // 注意：当前 parser 不支持 `ast(...)` pred，解析会失败。
    // 这个测试验证 fail-loud（设计文档 §2.1 原则 4）。
    let result = parse_query(input);
    assert!(result.is_err(), "ast(...) v1 未实现，应 fail-loud 报错");
}

// ===== 示例 5：fail-loud（字段写错直接报错）=====
// 这是运行时 fail-loud（strict 模式探活字段），不是解析期。
#[test]
fn test_example_5_strict_field_check_is_runtime_not_parse() {
    // 解析期不报错；strict 检查在执行期做
    let q = parse_query("ERROR && http_status>400").unwrap();
    match q.expr {
        Expr::And(l, r) => {
            assert!(matches!(*l, Expr::Prim(Prim::Bare(s)) if s == "ERROR"));
            match *r {
                Expr::Prim(Prim::Pred(Pred::Compare {
                    name, op, value, ..
                })) => {
                    assert_eq!(name, "http_status");
                    assert_eq!(op, CmpOp::Gt);
                    assert_eq!(value, "400");
                }
                other => panic!("右枝应为 Compare，得到 {:?}", other),
            }
        }
        other => panic!("顶层应为 And，得到 {:?}", other),
    }
}

// ===== 示例 6：引号与转义 =====

#[test]
fn test_example_6_zero_escape() {
    // gxp 'cost_ms>800 && _tag in [checkout,billing]' app.log —— 零转义
    let q = parse_query("cost_ms>800 && _tag in [checkout,billing]").unwrap();
    match q.expr {
        Expr::And(l, r) => {
            assert!(
                matches!(*l, Expr::Prim(Prim::Pred(Pred::Compare { name, .. })) if name == "cost_ms")
            );
            assert!(matches!(*r, Expr::Prim(Prim::Pred(Pred::In { name, .. })) if name == "_tag"));
        }
        other => panic!("顶层应为 And，得到 {:?}", other),
    }
}

#[test]
fn test_example_6_string_with_not() {
    // gxp '"error" && ! timeout' app.log —— 字面量用双引号 + 否定 BARE
    // 设计文档 §3.4：`!` 是操作符当且仅当前后都是边界（空白/(/[ 等）；
    // `! timeout`（带空格）是操作符；`!timeout`（不带空格）是字面。
    let q = parse_query(r#""error" && ! timeout"#).unwrap();
    match q.expr {
        Expr::And(l, r) => {
            assert!(matches!(*l, Expr::Prim(Prim::String(s)) if s == "error"));
            // 右枝：! timeout —— ! 前后都是空白，是操作符
            match *r {
                Expr::Not(inner) => {
                    // inner 应为 BARE "timeout"
                    assert!(matches!(*inner, Expr::Prim(Prim::Bare(s)) if s == "timeout"));
                }
                other => panic!("右枝应为 Not，得到 {:?}", other),
            }
        }
        other => panic!("顶层应为 And，得到 {:?}", other),
    }
}

#[test]
fn test_example_6_string_containing_logical_operators() {
    // gxp '"line with && in it"' app.log —— 字面 "&&" 在 STRING 内
    let q = parse_query(r#""line with && in it""#).unwrap();
    match q.expr {
        Expr::Prim(Prim::String(s)) => assert_eq!(s, "line with && in it"),
        other => panic!("期望 Prim::String，得到 {:?}", other),
    }
}

// ===== 示例 7：LLM 谚记引号的 fail-loud =====
// 这是 shell 层 fail-loud（bash/PS 解析期报错），gxp parser 看不到。
// 这里测试：即使 gxp 收到无引号输入，也不应静默默认接受为合法 BARE。
#[test]
fn test_example_7_unquoted_ampersand_accepted_by_parser() {
    // `ERROR && timeout` 若 shell 未拦截，到 gxp parser 层是合法 DSL。
    // 设计文档 §2.1 原则 5：fail-loud 在 shell 层，不在 gxp parser 层。
    // 所以这里只验证：gxp parser 解析 `ERROR && timeout` 成功（合法 DSL）。
    let q = parse_query("ERROR && timeout").unwrap();
    assert!(matches!(q.expr, Expr::And(_, _)));
}

// ===== 示例 8：上下文行（管道动词 context）=====

#[test]
fn test_example_8_context_single_n() {
    // gxp 'ERROR | context 3' app.log ≡ grep -C 3 ERROR app.log
    let q = parse_query("ERROR | context 3").unwrap();
    assert_eq!(
        q.stages,
        vec![Stage::Context {
            before: 3,
            after: 3
        }]
    );
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "ERROR"),
        other => panic!("期望 Prim::Bare，得到 {:?}", other),
    }
}

#[test]
fn test_example_8_context_before_after_explicit() {
    // gxp 'ERROR | context before:1 after:3' app.log ≡ grep -B 1 -A 3 ERROR app.log
    let q = parse_query("ERROR | context before:1 after:3").unwrap();
    assert_eq!(
        q.stages,
        vec![Stage::Context {
            before: 1,
            after: 3
        }]
    );
}

#[test]
fn test_example_8_combined_predicate_context_fields() {
    // gxp 'cost_ms>800 | context 5 | fields _ts,_tag,cost_ms' app.log
    let q = parse_query("cost_ms>800 | context 5 | fields _ts,_tag,cost_ms").unwrap();
    assert_eq!(q.stages.len(), 2);
    assert_eq!(
        q.stages[0],
        Stage::Context {
            before: 5,
            after: 5
        }
    );
    assert_eq!(
        q.stages[1],
        Stage::Fields(vec!["_ts".into(), "_tag".into(), "cost_ms".into()])
    );
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare {
            name, op, value, ..
        })) => {
            assert_eq!(name, "cost_ms");
            assert_eq!(op, CmpOp::Gt);
            assert_eq!(value, "800");
        }
        other => panic!("期望 Compare，得到 {:?}", other),
    }
}

// ===== 示例 9：只列文件名 / 计数 =====

#[test]
fn test_example_9_files() {
    // gxp 'ERROR *.log | files' ≡ grep -l ERROR *.log
    // *.log 是 paths，不在 query 内；这里只测 ERROR | files
    let q = parse_query("ERROR | files").unwrap();
    assert_eq!(q.stages, vec![Stage::Files { invert: false }]);
}

#[test]
fn test_example_9_files_invert() {
    // gxp 'ERROR *.log | files --invert' ≡ grep -L ERROR *.log
    let q = parse_query("ERROR | files --invert").unwrap();
    assert_eq!(q.stages, vec![Stage::Files { invert: true }]);
}

#[test]
fn test_example_9_count() {
    // gxp 'ERROR app.log | count' ≡ grep -c ERROR app.log（单文件）
    let q = parse_query("ERROR | count").unwrap();
    assert_eq!(q.stages, vec![Stage::Count]);
}

// ===== 示例 10：字面 > < 的表达 =====

#[test]
fn test_example_10_string_literal_greater_than() {
    // gxp '"cost>800"' app.log —— STRING 字面量
    let q = parse_query(r#""cost>800""#).unwrap();
    match q.expr {
        Expr::Prim(Prim::String(s)) => assert_eq!(s, "cost>800"),
        other => panic!("期望 Prim::String，得到 {:?}", other),
    }
}

#[test]
fn test_example_10_regex_literal_greater_than() {
    // gxp '/cost>800/' app.log —— REGEX 字面量
    let q = parse_query(r"/cost>800/").unwrap();
    match q.expr {
        Expr::Prim(Prim::Regex(s)) => assert_eq!(s, "cost>800"),
        other => panic!("期望 Prim::Regex，得到 {:?}", other),
    }
}

#[test]
fn test_example_10_ere_escape_greater_than() {
    // gxp 'cost\>800' app.log —— ERE 转义（grep 习惯）
    // 当前 BARE 兼容不处理转义，应原样保留 `cost\>800`
    // 注意：\> 不是谓词操作符（被 \ 转义），应整体作为一个 BARE
    let q = parse_query(r"cost\>800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, r"cost\>800"),
        other => panic!("期望 Prim::Bare，得到 {:?}", other),
    }
}

// ===== ! 词法规则（设计文档 §3.4）=====

#[test]
fn test_bang_rule_bang_error_is_literal() {
    // gxp '!ERROR' —— ! 前是行首但后是字母 → 字面 "!ERROR"
    let q = parse_query("!ERROR").unwrap();
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "!ERROR"),
        other => panic!("期望 Prim::Bare（!ERROR 字面），得到 {:?}", other),
    }
}

#[test]
fn test_bang_rule_bang_space_is_operator() {
    // gxp '! ERROR' —— ! 前行首，后空白 → 操作符
    let q = parse_query("! ERROR").unwrap();
    match q.expr {
        Expr::Not(inner) => {
            assert!(matches!(*inner, Expr::Prim(Prim::Bare(s)) if s == "ERROR"));
        }
        other => panic!("期望 Not，得到 {:?}", other),
    }
}

#[test]
fn test_bang_rule_bang_paren_is_operator() {
    // gxp '!("ERROR")' —— ! 后是 ( → 操作符
    let q = parse_query(r#"!("ERROR")"#).unwrap();
    match q.expr {
        Expr::Not(inner) => {
            assert!(matches!(*inner, Expr::Prim(Prim::Paren(_))));
        }
        other => panic!("期望 Not，得到 {:?}", other),
    }
}

#[test]
fn test_bang_rule_a_bang_b_is_literal() {
    // gxp 'a!b' —— ! 前后均是字母 → 字面 "a!b"
    let q = parse_query("a!b").unwrap();
    match q.expr {
        Expr::Prim(Prim::Bare(s)) => assert_eq!(s, "a!b"),
        other => panic!("期望 Prim::Bare（a!b 字面），得到 {:?}", other),
    }
}

// ===== fail-loud 场景（设计文档 §2.1 原则 4）=====

#[test]
fn test_fail_loud_unterminated_string() {
    // gxp '"unterminated' —— 未闭双引号应报错
    assert_parse_fails(r#""unterminated"#, "parse");
}

#[test]
fn test_fail_loud_unknown_pipeline_verb() {
    // gxp 'ERROR | foo' —— foo 不是已知动词
    assert_parse_fails("ERROR | foo", "未知管道动词");
}

#[test]
fn test_fail_loud_rank_without_top() {
    // gxp 'ERROR | rank 20 by x' —— 缺 top 关键字
    assert_parse_fails("ERROR | rank 20 by x", "top");
}

#[test]
fn test_fail_loud_head_non_integer() {
    // gxp 'ERROR | head abc' —— abc 非整数
    assert_parse_fails("ERROR | head abc", "head");
}

#[test]
fn test_sample_count_parses_to_count_spec() {
    // gxp 'ERROR | sample 30'
    let q = parse_query("ERROR | sample 30").unwrap();
    assert_eq!(q.stages.len(), 1);
    assert_eq!(
        q.stages[0],
        Stage::Sample {
            spec: SampleSpec::Count(30)
        }
    );
}

#[test]
fn test_sample_ratio_parses_to_ratio_spec() {
    // gxp 'ERROR | sample 50%' —— 50% 换算成 0.5
    let q = parse_query("ERROR | sample 50%").unwrap();
    assert_eq!(q.stages.len(), 1);
    assert_eq!(
        q.stages[0],
        Stage::Sample {
            spec: SampleSpec::Ratio(0.5)
        }
    );
}

#[test]
fn test_sample_zero_percent_parses() {
    // gxp 'ERROR | sample 0%' —— 边界：0%
    let q = parse_query("ERROR | sample 0%").unwrap();
    assert_eq!(
        q.stages[0],
        Stage::Sample {
            spec: SampleSpec::Ratio(0.0)
        }
    );
}

#[test]
fn test_sample_hundred_percent_parses() {
    // gxp 'ERROR | sample 100%' —— 边界：100%
    let q = parse_query("ERROR | sample 100%").unwrap();
    assert_eq!(
        q.stages[0],
        Stage::Sample {
            spec: SampleSpec::Ratio(1.0)
        }
    );
}

#[test]
fn test_sample_count_zero_parses() {
    // gxp 'ERROR | sample 0' —— 边界：0 条
    let q = parse_query("ERROR | sample 0").unwrap();
    assert_eq!(
        q.stages[0],
        Stage::Sample {
            spec: SampleSpec::Count(0)
        }
    );
}

#[test]
fn test_fail_loud_sample_over_100_percent() {
    // gxp 'ERROR | sample 150%' —— 150% > 100，拒绝
    assert_parse_fails("ERROR | sample 150%", "0..=100");
}

#[test]
fn test_fail_loud_sample_non_integer() {
    // gxp 'ERROR | sample abc' —— 非整数也非 %
    assert_parse_fails("ERROR | sample abc", "sample");
}

#[test]
fn test_fail_loud_sample_missing_arg() {
    // gxp 'ERROR | sample' —— 缺参数
    assert_parse_fails("ERROR | sample", "sample");
}

#[test]
fn test_fail_loud_sample_fractional_percent() {
    // gxp 'ERROR | sample 50.5%' —— P 须整数（0..=100），50.5% 拒绝
    assert_parse_fails("ERROR | sample 50.5%", "0..=100");
}

#[test]
fn test_fail_loud_files_unknown_flag() {
    // gxp 'ERROR | files --unknown' —— 未知 flag
    assert_parse_fails("ERROR | files --unknown", "--unknown");
}

#[test]
fn test_fail_loud_group_without_by() {
    // gxp 'ERROR | group _tag' —— 缺 by 关键字
    assert_parse_fails("ERROR | group _tag", "by");
}

// ===== `!` 作用于 pred 必须用括号（设计文档 §3.4 方向 C）=====

#[test]
fn test_bang_pred_with_paren_is_valid() {
    // !(cost>800) —— ! 绑定括号内整个 pred
    let q = parse_query("!(cost>800)").unwrap();
    match q.expr {
        Expr::Not(inner) => {
            // inner 应为 Paren(Expr::Prim(Prim::Pred(Compare)))
            assert!(matches!(*inner, Expr::Prim(Prim::Paren(_))));
        }
        other => panic!("期望 Not，得到 {:?}", other),
    }
}

// ===== pred 四种形式全测 =====

#[test]
fn test_pred_compare_ge() {
    let q = parse_query("cost_ms>=800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare {
            name, op, value, ..
        })) => {
            assert_eq!(name, "cost_ms");
            assert_eq!(op, CmpOp::Ge);
            assert_eq!(value, "800");
        }
        other => panic!("期望 Compare Ge，得到 {:?}", other),
    }
}

#[test]
fn test_pred_compare_le() {
    let q = parse_query("cost_ms<=800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare { op, .. })) => {
            assert_eq!(op, CmpOp::Le);
        }
        other => panic!("期望 Compare Le，得到 {:?}", other),
    }
}

#[test]
fn test_pred_compare_eq_double_eq() {
    let q = parse_query("cost_ms==800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare { op, .. })) => {
            assert_eq!(op, CmpOp::Eq);
        }
        other => panic!("期望 Compare Eq，得到 {:?}", other),
    }
}

#[test]
fn test_pred_compare_ne() {
    let q = parse_query("cost_ms!=800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare { op, .. })) => {
            assert_eq!(op, CmpOp::Ne);
        }
        other => panic!("期望 Compare Ne，得到 {:?}", other),
    }
}

#[test]
fn test_pred_compare_assign_eq() {
    // gxp 'cost_ms=800' —— EBNF 接受 = 作为相等
    let q = parse_query("cost_ms=800").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Compare { op, .. })) => {
            assert_eq!(op, CmpOp::Assign);
        }
        other => panic!("期望 Compare Assign，得到 {:?}", other),
    }
}

#[test]
fn test_pred_exists() {
    let q = parse_query("cost_ms exists").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Exists { name })) => assert_eq!(name, "cost_ms"),
        other => panic!("期望 Exists，得到 {:?}", other),
    }
}

#[test]
fn test_pred_between() {
    let q = parse_query("cost_ms between 100 and 200").unwrap();
    match q.expr {
        Expr::Prim(Prim::Pred(Pred::Between { name, lo, hi })) => {
            assert_eq!(name, "cost_ms");
            assert_eq!(lo, "100");
            assert_eq!(hi, "200");
        }
        other => panic!("期望 Between，得到 {:?}", other),
    }
}

#[test]
fn test_pred_between_combined_with_and_uses_paren() {
    // 设计文档 §2.2：between 的 and 是固定关键字；与外部布尔组合需括号
    let q = parse_query("(cost_ms between 100 and 200) && _tag in [checkout]").unwrap();
    match q.expr {
        Expr::And(l, r) => {
            assert!(matches!(*l, Expr::Prim(Prim::Paren(_))));
            assert!(matches!(*r, Expr::Prim(Prim::Pred(Pred::In { .. }))));
        }
        other => panic!("期望 And，得到 {:?}", other),
    }
}

// ===== pipeline 切分歧义（Rust 方案 §7.5 风险表）=====

#[test]
fn test_pipe_in_string_is_ere_or_not_pipeline() {
    // gxp '"a|b"' —— | 在 STRING 内，不是管道
    let q = parse_query(r#""a|b""#).unwrap();
    match q.expr {
        Expr::Prim(Prim::String(s)) => assert_eq!(s, "a|b"),
        other => panic!("期望 Prim::String，得到 {:?}", other),
    }
    assert!(q.stages.is_empty());
}

#[test]
fn test_pipe_in_regex_is_ere_or_not_pipeline() {
    // gxp '/a\|b/' —— | 在 REGEX 内且被转义，不是管道
    let q = parse_query(r"/a\|b/").unwrap();
    match q.expr {
        Expr::Prim(Prim::Regex(s)) => assert_eq!(s, r"a\|b"),
        other => panic!("期望 Prim::Regex，得到 {:?}", other),
    }
    assert!(q.stages.is_empty());
}

#[test]
fn test_pipe_in_paren_is_ere_or_not_pipeline() {
    // gxp '(a | b) && c | count' —— 括号内的 | 不是管道
    let q = parse_query("(a | b) && c | count").unwrap();
    assert_eq!(q.stages, vec![Stage::Count]);
    // expr: And(Paren(Or(Bare,Bare)), Bare("c"))
    match q.expr {
        Expr::And(l, r) => {
            assert!(matches!(*l, Expr::Prim(Prim::Paren(_))));
            assert!(matches!(*r, Expr::Prim(Prim::Bare(s)) if s == "c"));
        }
        other => panic!("期望 And，得到 {:?}", other),
    }
}
