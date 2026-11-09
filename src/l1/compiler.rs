//! L1 编译器：必选字面量组 + 字段名前缀下推（方向 B，设计文档 §3.2）。
//!
//! 不变式 `L1(粗筛) ⊇ L2(精判)`：
//! - 必选组组间 AND：每个组都必须至少命中一个字面量。
//! - 组内 OR：OR 分支的字面量合并到同一组。
//! - `Expr::Not` 不下推字面量：保守不漏，L2 精判时承担取反。
//! - 字段名前缀（如 `cost_ms=`/`cost_ms:`）只做存在性粗筛，不做数值比较。
//!
//! digit-DP 已废弃（设计文档 §3.2）：正则表达力是 CFG 子集，无法覆盖任意数值比较
//! （9 位以上大数、前导零定宽、浮点、负数都漏匹配）。数值比较交给 L2。

use aho_corasick::AhoCorasick;
use regex::bytes::Regex;
use std::sync::Arc;

use crate::dsl::ast::{CmpOp, Expr, Pred, Prim};

/// L1 编译产物：必选字面量组 + 字段名前缀粗筛。
///
/// 设计文档 §3.2 方向 B：
/// - `required_groups` 组间 AND：每个组都必须命中，否则行被淘汰。
/// - `field_prefix_ac` 把所有字段名前缀变体（`name=` 和 `name:`）打包成单个
///   aho-corasick 实例做并行存在性粗筛。`field_prefix_pattern_to_field`
///   维护 pattern ordinal → 字段索引映射，用于"每个字段至少 1 个变体命中"检查。
/// - `field_predicates` 携带原始谓词描述，待 L2 精判消费（L1 不做数值比较）。
#[derive(Debug)]
pub struct CompiledL1 {
    /// 必选组：组间 AND。空 Vec 表示"无字面量约束"（如纯字段谓词或 `.*`），
    /// 此时 L1 只看字段名前缀粗筛。
    pub required_groups: Vec<LiteralGroup>,
    /// 所有字段名前缀变体打包成单个 aho-corasick 实例。
    /// 若查询没有字段谓词，此为 `None`（不做前缀粗筛）。
    pub field_prefix_ac: Option<AhoCorasick>,
    /// pattern ordinal → 字段索引（每个字段 2 个变体，故 len = 2 * field_count）。
    /// `field_prefix_ac` 命中 pattern ordinal `p` 时，`field_prefix_pattern_to_field[p]`
    /// 给出对应字段索引（`field_predicates` 的下标）。
    pub field_prefix_pattern_to_field: Vec<usize>,
    /// 字段谓词描述符（L2 消费）：字段名 + 操作符 + 阈值字节。
    /// L1 不解析阈值，原样透传给 L2（11 维格式契约，D5）。
    pub field_predicates: Vec<FieldPredicate>,
    /// **L1 已构造性保证存在的字面量**（v0.49 Opt A，Rust 方案 §7.9）：
    /// 单元素纯字面量必选组的成员集合。L1 幸存行必含该子串
    /// （组间 AND 全命中 + 单元素组 any-hit ≡ 该字面量 is_match），
    /// L2 对树中同串 `PreparedPrim::Literal` 节点可豁免 `regex.is_match` 重校验。
    ///
    /// 健全性边界（三条，详见 Rust 方案 §7.9）：
    /// - Or 分支组永不单元素（每个 complete 叶子至少推进 1 字面量，Or 两支 ≥2）
    /// - Paren(And) 组因 B14 完整性规则整组放弃 → 其字面量不入集
    /// - 同串多出现：某串入集 ⟺ 行必含该子串 ⟺ 树中任意位置同串节点求值恒 True
    pub verified_literals: Vec<Vec<u8>>,
}

/// 必选组：一组字面量候选 + 精判正则。
///
/// - `fast`：aho-corasick 多模式并行匹配，任一命中即通过粗筛。
/// - `verify`：完整正则精判（仅在 `fast` 命中后调用，可选）。
///   对 `Prim::String`/`Bare` 用"子串包含"精判（pattern 已 `regex::escape` 转义）；
///   对 `Prim::Regex` 用正则 `is_match`。
/// - `is_pure_literal`：若组内全部为 `Prim::String`/`Bare`（无 `Prim::Regex`），
///   aho-corasick 命中即等价 regex 命中，`l1_match` 跳过 `verify.is_match` 第二遍。
#[derive(Debug)]
pub struct LiteralGroup {
    /// 粗筛：aho-corasick 多模式字面量。
    pub fast: AhoCorasick,
    /// 精判：完整正则（`is_match`）。
    pub verify: Regex,
    /// 字面量列表（用于 `--explain` 展示与调试 + `find_match_bytes` 快速路径）。
    /// `Arc<[u8]>`（v0.49 Opt B）：单元素纯字面量组命中即确定 `_match` 值，
    /// `find_match_bytes` 直接 Arc clone 免 aho 二次查找 + 每行 Vec 分配。
    pub literals: Vec<Arc<[u8]>>,
    /// 是否纯字面量组（无 `Prim::Regex` 成员）。
    /// true 时 aho-corasick 命中即等价 regex 命中，跳过 verify 第二遍。
    pub is_pure_literal: bool,
}

/// 字段谓词描述符（L1 透传给 L2）。
///
/// L1 不解析 `value`，原样保留字节，L2 按 11 维格式契约（D5）解析。
#[derive(Debug, Clone)]
pub struct FieldPredicate {
    /// 字段名（如 `cost_ms`）。
    pub field: String,
    /// 谓词种类（对应 AST `Pred`）。
    pub kind: PredicateKind,
}

/// L1 透传给 L2 的谓词种类（对应 [`crate::dsl::ast::Pred`]）。
#[derive(Debug, Clone)]
pub enum PredicateKind {
    /// `NAME OP VALUE`：数值/字符串比较。
    Compare {
        /// 比较操作符。
        op: CmpOp,
        /// 阈值字节字面量（原样透传，L2 解析）。
        value: String,
    },
    /// `NAME in [v1, v2, ...]`：集合成员判定。
    In {
        /// 候选值列表（字节字面量）。
        values: Vec<String>,
    },
    /// `NAME exists`：字段存在判定。
    Exists,
    /// `NAME between LO and HI`：区间判定。
    Between {
        /// 下界（字节字面量）。
        lo: String,
        /// 上界（字节字面量）。
        hi: String,
    },
}

impl CompiledL1 {
    /// L1 行级粗筛：所有必选组命中 + 所有引用字段名前缀存在。
    ///
    /// 不变式：返回 `true` 是 L2 精判的必要不充分条件——
    /// L1 命中集 ⊇ L2 精判命中集（绝不漏答案，可能多送候选）。
    #[inline]
    pub fn l1_match(&self, line: &[u8]) -> bool {
        // 1. 必选组组间 AND：每个组都必须至少命中一个字面量。
        //    纯字面量组（is_pure_literal）：aho-corasick 命中即等价 regex 命中，跳过 verify。
        //    含 Regex 组：aho-corasick 粗筛命中后，仍需 regex.is_match 精判（regex 可能有元字符）。
        for grp in &self.required_groups {
            if grp.fast.find(line).is_none() {
                return false;
            }
            if !grp.is_pure_literal && !grp.verify.is_match(line) {
                return false;
            }
        }
        // 2. 字段名前缀存在性粗筛：每个引用字段至少 1 个变体（`=` 或 `:`）命中。
        //    **伪字段豁免**：`_ts` / `_tag` 不参与前缀粗筛（见 compile 函数注释）。
        if let Some(ac) = &self.field_prefix_ac {
            let n_fields = self.field_predicates.len();
            // 每个字段 N 个变体（= / : / `"`name`":` JSON），命中任一即满足该字段存在性。
            let mut field_hit = vec![false; n_fields];
            for m in ac.find_iter(line) {
                let p = m.pattern().as_usize();
                // pattern ordinal 映射到字段索引（见 field_prefix_pattern_to_field）
                if let Some(&field_idx) = self.field_prefix_pattern_to_field.get(p) {
                    if field_idx < n_fields {
                        field_hit[field_idx] = true;
                    }
                }
            }
            // 伪字段（_ts / _tag）跳过存在性检查——L1 不做前缀粗筛，L2 抽取后判定。
            for (idx, hit) in field_hit.iter().enumerate() {
                if !hit {
                    let Some(fp) = self.field_predicates.get(idx) else {
                        continue;
                    };
                    if is_pseudo_field(&fp.field) {
                        continue;
                    }
                    return false;
                }
            }
        }
        true
    }
}

/// 判定字段名是否为伪字段（`_ts` / `_tag`，设计文档 §3.3）。
///
/// 伪字段不参与 L1 前缀粗筛——它们不是 logfmt kv 字段，
/// 是 L2 抽取器从行结构（ISO 时间戳、方括号标签）推断的伪字段，
/// 行里没有 `_ts=` / `_tag=` 子串可粗筛。
fn is_pseudo_field(name: &str) -> bool {
    name == "_ts" || name == "_tag"
}

/// 生成字段名前缀的字节变体，L1 aho-corasick 多模式粗筛用。
///
/// 方向 B（设计文档 §3.2）：L1 只检查字段名前缀是否存在，不做数值比较。
/// 覆盖 4 种常见格式（设计文档 §3.3）：
/// - logfmt：`name=` / `name:`
/// - JSON line：`"name":`（key 带双引号，§3.3 JSON line 格式）
///
/// 原只生成 `name=` / `name:`，对 JSON line `"name": value`
/// 会漏命中——aho-corasick 找 `name:` 但行内是 `"name":`（前有 `"` 阻断），
/// 导致 L1 误淘汰所有 JSONL 行，违反不变式 L1 ⊇ L2。补 `"name":` 变体修复。
///
/// 返回 Vec 而非固定数组：变体数量可能 >2（JSON 引号变体是第 3 个）。
fn prefix_variants(name: &str) -> Vec<Vec<u8>> {
    let mut eq = name.as_bytes().to_vec();
    eq.push(b'=');

    let mut colon = name.as_bytes().to_vec();
    colon.push(b':');

    // JSON line：`"name":`（key 双引号 + 冒号，无空格分隔）
    let mut json_colon = vec![b'"'];
    json_colon.extend_from_slice(name.as_bytes());
    json_colon.push(b'"');
    json_colon.push(b':');

    vec![eq, colon, json_colon]
}

/// 把 EBNF 表达式编译成 L1 产物（必选组 + 字段名前缀）。
///
/// 遍历 `Expr` 树：
/// - `Expr::And(l, r)`：左右进入不同必选组（组间 AND）。
/// - `Expr::Or(l, r)`：左右字面量合并到同一组（组内 OR）。
/// - `Expr::Not(e)`：不下推字面量（保守不漏，L2 承担取反）。
/// - `Expr::Prim`：按 prim 类型处理（见下）。
/// - `Prim::String(s)`/`Bare(s)`：作为字面量进当前组。
/// - `Prim::Regex(r)`：编译为正则精判；保守不提取字面量（后续优化点）。
/// - `Prim::Pred(p)`：不进字面量组，但字段名前缀进 `field_prefixes`。
/// - `Prim::Paren(e)`：递归处理内部 expr。
pub fn compile(expr: &Expr) -> Result<CompiledL1, CompileError> {
    // 1. 收集必选组（同时产出 verified_literals：单元素纯字面量组成员）
    let mut required_groups: Vec<LiteralGroup> = Vec::new();
    let mut verified_literals: Vec<Vec<u8>> = Vec::new();
    collect_required_groups(expr, &mut required_groups, &mut verified_literals)?;

    // 2. 收集字段谓词
    let mut field_predicates: Vec<FieldPredicate> = Vec::new();
    collect_field_predicates(expr, &mut field_predicates);

    // 3. 字段名前缀变体打包成单个 aho-corasick
    //    pattern ordinal 顺序：field 0 的 N 个变体在前，field 1 的 N 个变体在后，依此类推。
    //    **伪字段豁免**：`_ts` / `_tag` 不走前缀粗筛——它们不是 logfmt kv 字段，
    //    是抽取器从行结构（ISO 时间戳、方括号标签）推断的伪字段，行里没有
    //    `_ts=` / `_tag=` 子串可粗筛。若下推，L1 会淘汰所有含真实时间戳/标签的行，
    //    破坏不变式 L1 ⊇ L2。
    let mut all_prefixes: Vec<Vec<u8>> = Vec::with_capacity(field_predicates.len() * 3);
    let mut pattern_to_field: Vec<usize> = Vec::with_capacity(field_predicates.len() * 3);
    for (field_idx, p) in field_predicates.iter().enumerate() {
        if is_pseudo_field(&p.field) {
            continue;
        }
        for variant in prefix_variants(&p.field) {
            pattern_to_field.push(field_idx);
            all_prefixes.push(variant);
        }
    }
    let field_prefix_ac = if all_prefixes.is_empty() {
        None
    } else {
        Some(
            AhoCorasick::new(all_prefixes.iter().map(|v| v.as_slice()))
                .map_err(|e| CompileError::AhoCorasickBuild(e.to_string()))?,
        )
    };

    Ok(CompiledL1 {
        required_groups,
        field_prefix_ac,
        field_prefix_pattern_to_field: pattern_to_field,
        field_predicates,
        verified_literals,
    })
}

/// 递归收集必选组。
///
/// `verified`：单元素纯字面量必选组成员收集（v0.49 Opt A）——仅在 Prim 分支
/// 推组处判定（Or 分支组 ≥2 元素永不单元素，见 Rust 方案 §7.9 健全性论证）。
fn collect_required_groups(
    expr: &Expr,
    out: &mut Vec<LiteralGroup>,
    verified: &mut Vec<Vec<u8>>,
) -> Result<(), CompileError> {
    match expr {
        Expr::And(l, r) => {
            // 左右独立组：先建左组，再建右组。
            collect_required_groups(l, out, verified)?;
            collect_required_groups(r, out, verified)?;
        }
        Expr::Or(l, r) => {
            // 合并到同一组：收集 OR 两边字面量到同一组。
            //
            // B14 修复：组内 any-hit 语义只在「每个分支都推进了字面量」时
            // 才与析取语义一致。任一分支不推进字面量（And/Not 嵌套、Regex、
            // Pred/Ast/Capture），部分字面量组成的必选组会误杀其余分支的
            // 合法候选（违反 L1 ⊇ L2，如 `"a" && "b" || "c"` 丢掉只含 a∧b
            // 的行、`"a" || /re/` 丢掉只匹配正则的行）。此时整组放弃
            // （无字面量约束），交给 L2 全树精判。
            let mut group_literals: Vec<Vec<u8>> = Vec::new();
            let mut group_verify_patterns: Vec<String> = Vec::new();
            let mut group_has_regex = false;
            let l_complete = collect_or_literals(
                l,
                &mut group_literals,
                &mut group_verify_patterns,
                &mut group_has_regex,
            )?;
            let r_complete = collect_or_literals(
                r,
                &mut group_literals,
                &mut group_verify_patterns,
                &mut group_has_regex,
            )?;
            if l_complete && r_complete && !group_literals.is_empty() {
                out.push(build_literal_group(
                    &group_literals,
                    &group_verify_patterns,
                    group_has_regex,
                )?);
            }
        }
        Expr::Not(e) => {
            // 保守不漏：NOT 不下推字面量。
            // NOT 内部的 Prim::Pred 仍要收集字段名前缀（外部 collect_field_predicates 处理）。
            let _ = e;
        }
        Expr::Prim(prim) => {
            // 单 prim 独立成组（仅当推进了字面量，B14 完整性规则）。
            let mut group_literals: Vec<Vec<u8>> = Vec::new();
            let mut group_verify_patterns: Vec<String> = Vec::new();
            let mut group_has_regex = false;
            let complete = collect_prim_literals(
                prim,
                &mut group_literals,
                &mut group_verify_patterns,
                &mut group_has_regex,
            )?;
            if complete && !group_literals.is_empty() {
                // v0.49 Opt A：单元素纯字面量组 → 成员入 verified_literals。
                // 单元素组 any-hit ≡ 该字面量子串存在（构造性，Rust 方案 §7.9）。
                // Paren(Or) 推多字面量 → len ≥2 不入；含 Regex → has_regex 不入。
                if group_literals.len() == 1 && !group_has_regex {
                    verified.push(group_literals[0].clone());
                }
                out.push(build_literal_group(
                    &group_literals,
                    &group_verify_patterns,
                    group_has_regex,
                )?);
            }
        }
    }
    Ok(())
}

/// 从 OR 分支收集字面量到同一组。
///
/// 返回 `bool`：`true` = 所有分支都推进了字面量（组完整，any-hit 语义成立）；
/// `false` = 存在不推进字面量的分支（B14），调用方必须放弃整组。
fn collect_or_literals(
    expr: &Expr,
    literals: &mut Vec<Vec<u8>>,
    verify_patterns: &mut Vec<String>,
    has_regex: &mut bool,
) -> Result<bool, CompileError> {
    match expr {
        Expr::Or(l, r) => {
            let l_complete = collect_or_literals(l, literals, verify_patterns, has_regex)?;
            let r_complete = collect_or_literals(r, literals, verify_patterns, has_regex)?;
            Ok(l_complete && r_complete)
        }
        Expr::Prim(prim) => collect_prim_literals(prim, literals, verify_patterns, has_regex),
        // AND / NOT 嵌套在 OR 里：不提取字面量，且使所在 Or 组失效（B14）——
        // 部分字面量组的 any-hit 语义与 `(A ∧ B) ∨ …` 不一致，会误杀候选。
        Expr::And(_, _) | Expr::Not(_) => Ok(false),
    }
}

/// 从 prim 收集字面量 + 精判 pattern。
///
/// 返回 `bool`：`true` = 该 prim 推进了字面量（String/Bare，或括号包裹的
/// 完整字面量组合）；`false` = 未推进（Regex/Pred/Ast/Capture）。
///
/// `has_regex` 被置 true 当且仅当遇到 `Prim::Regex`——
/// 用于 `build_literal_group` 设置 `is_pure_literal` 标记。
fn collect_prim_literals(
    prim: &Prim,
    literals: &mut Vec<Vec<u8>>,
    verify_patterns: &mut Vec<String>,
    has_regex: &mut bool,
) -> Result<bool, CompileError> {
    match prim {
        Prim::String(s) | Prim::Bare(s) => {
            // 字面量子串匹配：用 regex::escape 转义后作为 pattern。
            let pattern = regex::escape(s);
            literals.push(s.as_bytes().to_vec());
            verify_patterns.push(pattern);
            Ok(true)
        }
        Prim::Regex(r) => {
            // REGEX 字面量：用 pattern 本身作为精判。
            // 保守：不做字面量提取，整个 regex 作为 verify。
            // 不 push literals 且返回 false——regex 无法进 aho-corasick
            // 字面量组，若 Or 其余分支的字面量仍组成必选组，会误杀只匹配
            // regex 的行（B14，L1 ⊇ L2 违反）。
            *has_regex = true;
            verify_patterns.push(r.clone());
            Ok(false)
        }
        Prim::Paren(e) => {
            // 括号分组：递归处理内部 expr（完整性由内层判定传导）。
            collect_or_literals(e, literals, verify_patterns, has_regex)
        }
        Prim::Pred(_) => {
            // 字段谓词不进字面量组（交给 field_prefixes），并使所在 Or 组失效。
            Ok(false)
        }
        Prim::Ast { .. } => {
            // ast() 结构层查询无字节级下推（设计文档 §2.2 + §8.2）：
            // L1 视 ast() 为"可能命中"——不提取字面量，不淘汰候选。
            // 同时使所在 Or 组失效（B14 完整性规则）。
            Ok(false)
        }
        Prim::Capture { .. } => {
            // capture() 正则捕获组无字节级字面量可提取：
            // L1 视 capture() 为"可能命中"——不提取字面量，不淘汰候选。
            // 同时使所在 Or 组失效（B14 完整性规则）。
            Ok(false)
        }
    }
}

/// 构造单个必选组。
///
/// 若 `literals` 为空（纯 Regex 或纯字段谓词），返回 `Err(CompileError::EmptyGroup)`
/// 调用方应跳过该组（不进 required_groups）。
///
/// `has_regex`：组内是否含 `Prim::Regex` 成员。false 时 `is_pure_literal = true`，
/// `l1_match` 跳过 `verify.is_match` 第二遍（aho-corasick 命中即等价 regex 命中）。
fn build_literal_group(
    literals: &[Vec<u8>],
    verify_patterns: &[String],
    has_regex: bool,
) -> Result<LiteralGroup, CompileError> {
    if literals.is_empty() {
        return Err(CompileError::EmptyGroup);
    }
    let fast = AhoCorasick::new(literals.iter().map(|v| v.as_slice()))
        .map_err(|e| CompileError::AhoCorasickBuild(e.to_string()))?;
    // 精判 pattern：OR 合并所有 verify_patterns。
    let verify_pattern = if verify_patterns.len() == 1 {
        verify_patterns[0].clone()
    } else {
        let combined = verify_patterns
            .iter()
            .map(|p| format!("(?:{p})"))
            .collect::<Vec<_>>()
            .join("|");
        format!("(?:{combined})")
    };
    let verify =
        Regex::new(&verify_pattern).map_err(|e| CompileError::RegexBuild(e.to_string()))?;
    Ok(LiteralGroup {
        fast,
        verify,
        literals: literals.iter().map(|v| Arc::from(v.as_slice())).collect(),
        is_pure_literal: !has_regex,
    })
}

/// 递归收集字段谓词（所有 Prim::Pred，**Not 子树豁免**）。
///
/// # Not 子树豁免（不变式 `L1 ⊇ L2`）
///
/// 设计文档 §3.3 "字段不存在语义"：`!(cost>800)` 对无 cost 字段的行视为
/// 谓词假→取反为真→命中。若 L1 对 Not 内部的 cost>800 下推 `cost=` 前缀粗筛，
/// 会淘汰所有无 cost 字段的行，破坏 `L1 ⊇ L2`（L1 漏候选，L2 倒希望命中）。
///
/// 故 Not 子树完全不收集字段谓词——L1 对 Not 内部不做字段前缀粗筛，
/// 把 Not 子树的判定全部交给 L2 递归求值。
///
/// capture 同字段豁免——`capture(cost, ...) && cost>1000`
/// 中 `cost` 字段值由 capture 提供，行内无 `cost=` 前缀。若 L1 下推 `cost=`
/// 前缀粗筛，会淘汰所有行（行里只有 `cost_ms=`），破坏 `L1 ⊇ L2`。
/// 先收集所有 `Prim::Capture` 的 name，谓词字段名 == capture name 时
/// 跳过前缀收集（与 pipeline `apply_where` 的 captures 优先逻辑同构）。
fn collect_field_predicates(expr: &Expr, out: &mut Vec<FieldPredicate>) {
    // 先收集所有 capture 字段名（用于谓词同字段豁免）
    let mut capture_names: Vec<String> = Vec::new();
    collect_capture_names(expr, &mut capture_names);
    collect_field_predicates_inner(expr, &capture_names, out);
}

/// 递归收集所有 `Prim::Capture` 的 name（用于谓词同字段豁免）。
fn collect_capture_names(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::And(l, r) | Expr::Or(l, r) => {
            collect_capture_names(l, out);
            collect_capture_names(r, out);
        }
        Expr::Not(_) => {
            // Not 子树内的 capture 不影响外部谓词（Not 子树语义独立）
        }
        Expr::Prim(Prim::Capture { name, .. }) => {
            out.push(name.clone());
        }
        Expr::Prim(Prim::Paren(e)) => {
            collect_capture_names(e, out);
        }
        _ => {}
    }
}

fn collect_field_predicates_inner(
    expr: &Expr,
    capture_names: &[String],
    out: &mut Vec<FieldPredicate>,
) {
    match expr {
        Expr::And(l, r) => {
            collect_field_predicates_inner(l, capture_names, out);
            collect_field_predicates_inner(r, capture_names, out);
        }
        // B15（v0.48 修复）：Or 子树豁免——L1 前缀粗筛是「每个引用字段前缀必须存在」
        // 的 AND 语义（l1_match 步骤 2），而 Or 分支的谓词只需任一分支成立。
        // 若下推 Or 内部的字段前缀，只满足另一分支的行会被 L1 误杀——
        // `level in [ERROR] || "plain"` 丢掉 plain-only 行，`--verify` 报
        // 「暴力独有」（L1 ⊇ L2 违反，v0.47 前装，B14 只审了字面量组完整性
        // 未审前缀收集的 Or 豁免，与 Not 子树豁免同构的镜像遗漏）。
        // 代价：含谓词的 Or 查询退化为字面量组约束（纯谓词 Or 无约束），候选
        // 增多但语义正确；「纯谓词 Or 前缀 any-hit 下推」列为性能候选（§11.3）。
        // L2 不受影响：prepare_expr 独立走全树（Or/Not 分支递归求值）。
        Expr::Or(..) => {}
        Expr::Not(_) => {
            // Not 子树豁免：L1 不下推 Not 内部的字段前缀（不变式 L1 ⊇ L2）。
            // L2 阶段 eval_prepared_with_captures 走 Expr::Not 分支递归求值，承担全部语义。
        }
        Expr::Prim(Prim::Pred(p)) => {
            let field = p.field_name();
            // capture 同字段豁免——capture 提供的字段值
            // 不在行内（无 `field=` 前缀），L1 前缀粗筛会误淘汰。
            // 与 pipeline apply_where 的 captures 优先逻辑同构（captures_has_field 跳过 L1）。
            if capture_names.iter().any(|n| n == field) {
                return;
            }
            out.push(FieldPredicate {
                field: field.to_string(),
                kind: p.to_predicate_kind(),
            });
        }
        Expr::Prim(Prim::Paren(e)) => {
            collect_field_predicates_inner(e, capture_names, out);
        }
        _ => {}
    }
}

/// Pred 扩展：获取字段名 + 转换为 L1 透传的 PredicateKind。
trait PredExt {
    fn field_name(&self) -> &str;
    fn to_predicate_kind(&self) -> PredicateKind;
}

impl PredExt for Pred {
    fn field_name(&self) -> &str {
        match self {
            Pred::Compare { name, .. } => name,
            Pred::In { name, .. } => name,
            Pred::Exists { name } => name,
            Pred::Between { name, .. } => name,
        }
    }

    fn to_predicate_kind(&self) -> PredicateKind {
        match self {
            Pred::Compare { op, value, .. } => PredicateKind::Compare {
                op: *op,
                value: value.clone(),
            },
            Pred::In { values, .. } => PredicateKind::In {
                values: values.clone(),
            },
            Pred::Exists { .. } => PredicateKind::Exists,
            Pred::Between { lo, hi, .. } => PredicateKind::Between {
                lo: lo.clone(),
                hi: hi.clone(),
            },
        }
    }
}

/// L1 编译错误。
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// aho-corasick 构造失败（通常因 pattern 为空或冲突）。
    #[error("aho-corasick build failed: {0}")]
    AhoCorasickBuild(String),
    /// regex 构造失败（语法错误）。
    #[error("regex build failed: {0}")]
    RegexBuild(String),
    /// 必选组为空（无可下推字面量，调用方应跳过）。
    #[error("empty literal group")]
    EmptyGroup,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::ast::{CmpOp, Expr, Pred, Prim};

    /// 构造 `Prim::String("error")` 表达式。
    fn expr_str(s: &str) -> Expr {
        Expr::Prim(Prim::String(s.to_string()))
    }

    /// 构造 `Prim::Bare("ERROR")` 表达式。
    fn expr_bare(s: &str) -> Expr {
        Expr::Prim(Prim::Bare(s.to_string()))
    }

    /// 构造 `Prim::Pred(Compare { name, op, value })`。
    fn expr_compare(name: &str, op: CmpOp, value: &str) -> Expr {
        Expr::Prim(Prim::Pred(Pred::Compare {
            name: name.to_string(),
            op,
            value: value.to_string(),
        }))
    }

    // --- compile 基本场景 ---

    #[test]
    fn test_compile_single_string_produces_one_group() {
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译单 String 应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        assert!(compiled.field_prefix_ac.is_none());
        assert!(compiled.field_predicates.is_empty());
        assert_eq!(
            compiled.required_groups[0].literals,
            vec![std::sync::Arc::from(b"error".as_ref())]
        );
    }

    // --- B15（v0.48）：Or 子树前缀豁免 ---

    #[test]
    fn test_compile_or_with_pred_exempts_prefix_b15() {
        // `level in [ERROR] || "plain"`：Or 内谓词不收集前缀——前缀粗筛是 AND
        // 语义（l1_match 步骤 2 要求每个字段前缀存在），下推会误杀 plain-only
        // 行（L1 ⊇ L2 违反，v0.47 前装，--verify 报「暴力独有」）
        let expr = Expr::Or(
            Box::new(Expr::Prim(Prim::Pred(Pred::In {
                name: "level".to_string(),
                values: vec!["ERROR".to_string()],
            }))),
            Box::new(expr_str("plain")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.field_predicates.is_empty(),
            "Or 内谓词不应收集前缀（B15 豁免）"
        );
        assert!(compiled.field_prefix_ac.is_none());
        // B14：Or 混非字面量分支 → 整组放弃 → 无必选组
        assert!(compiled.required_groups.is_empty());
    }

    #[test]
    fn test_compile_or_numeric_pred_exempts_prefix_b15() {
        // 同构镜像：数值谓词在 Or 内同样豁免
        let expr = Expr::Or(
            Box::new(expr_compare("cost_ms", CmpOp::Gt, "800")),
            Box::new(expr_str("plain")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.field_predicates.is_empty(),
            "B15：Or 数值谓词同豁免"
        );
    }

    #[test]
    fn test_compile_and_with_pred_still_collects_prefix() {
        // 对照：And 位置的谓词照常收集（AND 语义下前缀必选成立，优化保留）
        let expr = Expr::And(
            Box::new(expr_compare("cost_ms", CmpOp::Gt, "800")),
            Box::new(expr_str("timeout")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert_eq!(compiled.field_predicates.len(), 1);
        assert_eq!(compiled.field_predicates[0].field, "cost_ms");
        assert!(compiled.field_prefix_ac.is_some());
    }

    #[test]
    fn test_compile_bare_treated_as_literal() {
        let expr = expr_bare("ERROR");
        let compiled = compile(&expr).expect("编译 Bare 应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        assert_eq!(
            compiled.required_groups[0].literals,
            vec![std::sync::Arc::from(b"ERROR".as_ref())]
        );
    }

    #[test]
    fn test_compile_and_produces_two_groups() {
        // "error" && "timeout"
        let expr = Expr::And(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译 AND 应成功");
        assert_eq!(compiled.required_groups.len(), 2);
        assert_eq!(
            compiled.required_groups[0].literals,
            vec![std::sync::Arc::from(b"error".as_ref())]
        );
        assert_eq!(
            compiled.required_groups[1].literals,
            vec![std::sync::Arc::from(b"timeout".as_ref())]
        );
    }

    #[test]
    fn test_compile_or_merges_into_one_group() {
        // "error" || "timeout"
        let expr = Expr::Or(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译 OR 应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        let lits = &compiled.required_groups[0].literals;
        assert_eq!(lits.len(), 2);
        assert!(lits.contains(&std::sync::Arc::from(b"error".as_ref())));
        assert!(lits.contains(&std::sync::Arc::from(b"timeout".as_ref())));
    }

    // --- B14：Or 组完整性规则 ---

    #[test]
    fn test_compile_or_with_and_branch_drops_group() {
        // ("a" && "b") || "c"：And 分支不推进字面量 → 整组放弃（部分组会误杀 a∧b 候选）
        let expr = Expr::Or(
            Box::new(Expr::And(Box::new(expr_str("a")), Box::new(expr_str("b")))),
            Box::new(expr_str("c")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups.is_empty(),
            "Or 含 And 分支不得产生部分字面量组（B14），实际: {:?}",
            compiled.required_groups
        );
    }

    #[test]
    fn test_compile_or_with_regex_branch_drops_group() {
        // "a" || /re/：Regex 不推进字面量 → 整组放弃（部分组会误杀只匹配正则的候选）
        let expr = Expr::Or(
            Box::new(expr_str("a")),
            Box::new(Expr::Prim(Prim::Regex("re".to_string()))),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups.is_empty(),
            "Or 含 Regex 分支不得产生部分字面量组（B14），实际: {:?}",
            compiled.required_groups
        );
    }

    #[test]
    fn test_compile_nested_or_literals_still_merge() {
        // ("a" || "b") || "c" → 嵌套 Or 全字面量仍合并单组（回归）
        let expr = Expr::Or(
            Box::new(Expr::Or(Box::new(expr_str("a")), Box::new(expr_str("b")))),
            Box::new(expr_str("c")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        assert_eq!(compiled.required_groups[0].literals.len(), 3);
    }

    #[test]
    fn test_compile_and_with_regex_keeps_string_group() {
        // "a" && /re/：String 侧独立成组（作为粗筛 sound），Regex 组跳过（回归）
        let expr = Expr::And(
            Box::new(expr_str("a")),
            Box::new(Expr::Prim(Prim::Regex("re".to_string()))),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        assert_eq!(
            compiled.required_groups[0].literals,
            vec![std::sync::Arc::from(b"a".as_ref())]
        );
    }

    // --- compile 字段谓词 ---

    #[test]
    fn test_compile_compare_produces_field_prefix() {
        // cost_ms > 800
        let expr = expr_compare("cost_ms", CmpOp::Gt, "800");
        let compiled = compile(&expr).expect("编译 Compare 应成功");
        // 无必选组（字段谓词不进字面量组）
        assert!(compiled.required_groups.is_empty());
        // 1 个字段谓词
        assert_eq!(compiled.field_predicates.len(), 1);
        assert_eq!(compiled.field_predicates[0].field, "cost_ms");
        // field_prefix_ac 应构造（含 cost_ms= / cost_ms: / `"cost_ms":` 三个前缀）
        assert!(compiled.field_prefix_ac.is_some());
        // pattern_to_field 映射：3 个变体都指向 field 0
        // 补 JSON line `"name":` 变体
        assert_eq!(compiled.field_prefix_pattern_to_field, vec![0, 0, 0]);
    }

    #[test]
    fn test_compile_exists_produces_exists_kind() {
        // user_id exists
        let expr = Expr::Prim(Prim::Pred(Pred::Exists {
            name: "user_id".to_string(),
        }));
        let compiled = compile(&expr).expect("编译 Exists 应成功");
        assert_eq!(compiled.field_predicates.len(), 1);
        match &compiled.field_predicates[0].kind {
            PredicateKind::Exists => {}
            other => panic!("期望 Exists，得到 {:?}", other),
        }
    }

    #[test]
    fn test_compile_between_produces_between_kind() {
        // cost between 100 and 200
        let expr = Expr::Prim(Prim::Pred(Pred::Between {
            name: "cost".to_string(),
            lo: "100".to_string(),
            hi: "200".to_string(),
        }));
        let compiled = compile(&expr).expect("编译 Between 应成功");
        assert_eq!(compiled.field_predicates.len(), 1);
        match &compiled.field_predicates[0].kind {
            PredicateKind::Between { lo, hi } => {
                assert_eq!(lo, "100");
                assert_eq!(hi, "200");
            }
            other => panic!("期望 Between，得到 {:?}", other),
        }
    }

    #[test]
    fn test_compile_in_produces_in_kind() {
        // tag in [a, b]
        let expr = Expr::Prim(Prim::Pred(Pred::In {
            name: "tag".to_string(),
            values: vec!["a".to_string(), "b".to_string()],
        }));
        let compiled = compile(&expr).expect("编译 In 应成功");
        assert_eq!(compiled.field_predicates.len(), 1);
        match &compiled.field_predicates[0].kind {
            PredicateKind::In { values } => {
                assert_eq!(values, &vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("期望 In，得到 {:?}", other),
        }
    }

    // --- compile NOT ---

    #[test]
    fn test_compile_not_does_not_push_literals() {
        // !"error"（NOT 不下推字面量）
        let expr = Expr::Not(Box::new(expr_str("error")));
        let compiled = compile(&expr).expect("编译 NOT 应成功");
        // 无必选组（NOT 不下推字面量，保守不漏）
        assert!(compiled.required_groups.is_empty());
    }

    // --- l1_match 行级粗筛 ---

    #[test]
    fn test_l1_match_single_string_hit() {
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        assert!(compiled.l1_match(b"this is an error log"));
        assert!(!compiled.l1_match(b"this is a warning log"));
    }

    #[test]
    fn test_l1_match_and_requires_both_groups() {
        // "error" && "timeout"：行必须同时含 "error" 和 "timeout" 子串
        let expr = Expr::And(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译应成功");
        // 同时含两者
        assert!(compiled.l1_match(b"error: timeout occurred"));
        assert!(compiled.l1_match(b"timeout occurred without error word"));
        // 只含 error
        assert!(!compiled.l1_match(b"error: something else"));
        // 只含 timeout
        assert!(!compiled.l1_match(b"timeout occurred with no e-r-r-o-r"));
    }

    #[test]
    fn test_l1_match_or_matches_either() {
        // "error" || "timeout"
        let expr = Expr::Or(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译应成功");
        assert!(compiled.l1_match(b"error log"));
        assert!(compiled.l1_match(b"timeout log"));
        assert!(!compiled.l1_match(b"info log"));
    }

    #[test]
    fn test_l1_match_field_prefix_filters_no_field_line() {
        // cost_ms > 800
        let expr = expr_compare("cost_ms", CmpOp::Gt, "800");
        let compiled = compile(&expr).expect("编译应成功");
        // 含字段名前缀：通过 L1 粗筛（数值比较交给 L2）
        assert!(compiled.l1_match(b"cost_ms=900 user=test"));
        assert!(compiled.l1_match(b"cost_ms:900 user=test"));
        assert!(compiled.l1_match(b"cost_ms=100 user=test")); // 数值不满足，但 L1 只看前缀
                                                              // 不含字段名前缀：L1 淘汰
        assert!(!compiled.l1_match(b"latency=900 user=test"));
        assert!(!compiled.l1_match(b"no fields here"));
    }

    #[test]
    fn test_l1_match_combined_literal_and_field() {
        // "timeout" && cost_ms > 800
        let expr = Expr::And(
            Box::new(expr_str("timeout")),
            Box::new(expr_compare("cost_ms", CmpOp::Gt, "800")),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(compiled.l1_match(b"timeout cost_ms=900"));
        assert!(!compiled.l1_match(b"cost_ms=900")); // 缺字面量
        assert!(!compiled.l1_match(b"timeout occurred")); // 缺字段前缀
    }

    #[test]
    fn test_l1_match_multiple_fields_all_must_exist() {
        // cost_ms > 800 && user_id exists
        let expr = Expr::And(
            Box::new(expr_compare("cost_ms", CmpOp::Gt, "800")),
            Box::new(Expr::Prim(Prim::Pred(Pred::Exists {
                name: "user_id".to_string(),
            }))),
        );
        let compiled = compile(&expr).expect("编译应成功");
        // 两字段都存在
        assert!(compiled.l1_match(b"cost_ms=900 user_id=abc"));
        assert!(compiled.l1_match(b"cost_ms:900 user_id:abc"));
        // 只含一个字段
        assert!(!compiled.l1_match(b"cost_ms=900 no_user"));
        assert!(!compiled.l1_match(b"user_id=abc no_cost"));
    }

    // --- 不变式 L1 ⊇ L2 ---

    #[test]
    fn test_l1_superset_invariant_numeric_predicate() {
        // cost_ms > 800
        // L1 粗筛命中集 = 所有含 cost_ms= 或 cost_ms: 的行
        // L2 精判命中集 = cost_ms 值 > 800 的行（子集）
        let expr = expr_compare("cost_ms", CmpOp::Gt, "800");
        let compiled = compile(&expr).expect("编译应成功");

        let lines: Vec<&[u8]> = vec![
            b"cost_ms=900 user=test", // L2 命中（900 > 800）
            b"cost_ms=100 user=test", // L2 不命中（100 <= 800），但 L1 命中（前缀存在）
            b"latency=900 user=test", // L1 不命中（无 cost_ms 前缀）
            b"no fields here",        // L1 不命中
        ];
        let l1_hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| compiled.l1_match(line).then_some(i))
            .collect();

        // L1 命中集应包含所有含 cost_ms= 的行（索引 0 和 1）
        assert!(l1_hits.contains(&0));
        assert!(l1_hits.contains(&1));
        // 不含字段前缀的行不命中（索引 2 和 3）
        assert!(!l1_hits.contains(&2));
        assert!(!l1_hits.contains(&3));

        // 模拟 L2 精判：只保留 cost_ms 值 > 800 的行
        let l2_hits: Vec<usize> = l1_hits
            .iter()
            .copied()
            .filter(|&i| {
                let line = lines[i];
                let key_eq = b"cost_ms=";
                let key_colon = b"cost_ms:";
                let val_start = line
                    .windows(key_eq.len())
                    .position(|w| w == key_eq)
                    .map(|p| p + key_eq.len())
                    .or_else(|| {
                        line.windows(key_colon.len())
                            .position(|w| w == key_colon)
                            .map(|p| p + key_colon.len())
                    });
                val_start
                    .map(|start| {
                        let end = line[start..]
                            .iter()
                            .position(|&b| !b.is_ascii_digit() && b != b'.')
                            .map(|p| start + p)
                            .unwrap_or(line.len());
                        std::str::from_utf8(&line[start..end])
                            .ok()
                            .and_then(|s| s.parse::<f64>().ok())
                            .map(|v| v > 800.0)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .collect();

        // L2 只命中索引 0
        assert_eq!(l2_hits, vec![0]);
        // 不变式：L2 命中集 ⊆ L1 命中集
        for l2_hit in &l2_hits {
            assert!(
                l1_hits.contains(l2_hit),
                "L2 命中 {} 不在 L1 命中集中",
                l2_hit
            );
        }
    }

    // --- is_pure_literal 标记正确性 ---

    #[test]
    fn test_pure_literal_flag_set_for_string_prim() {
        let expr = expr_str("error");
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups[0].is_pure_literal,
            "Prim::String 组应为纯字面量"
        );
    }

    #[test]
    fn test_pure_literal_flag_set_for_bare_prim() {
        let expr = expr_bare("ERROR");
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups[0].is_pure_literal,
            "Prim::Bare 组应为纯字面量"
        );
    }

    #[test]
    fn test_pure_literal_flag_set_for_or_of_strings() {
        // "error" || "timeout" → 同一组，两者都是 String
        let expr = Expr::Or(Box::new(expr_str("error")), Box::new(expr_str("timeout")));
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups[0].is_pure_literal,
            "OR 两个 String 应为纯字面量组"
        );
    }

    #[test]
    fn test_pure_literal_flag_cleared_for_regex_prim() {
        // v0.42 版本此测试断言 `"error" || /time./` 产生 1 个 has_regex 组
        // （literals=[error]）——B14 判定该形态违反 L1 ⊇ L2：只匹配 /time./
        // 而不含 "error" 的行会被部分组的 fast(aho-corasick) 误杀。
        // v0.47 起 Or 含 Regex 分支整组放弃（无字面量约束），交 L2 全树精判。
        let expr = Expr::Or(
            Box::new(expr_str("error")),
            Box::new(Expr::Prim(Prim::Regex("time.".to_string()))),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert!(
            compiled.required_groups.is_empty(),
            "Or 含 Regex 分支不得产生部分字面量组（B14），实际: {:?}",
            compiled.required_groups
        );
        // l1_match 无组 → 恒 true（粗筛不淘汰，L1 ⊇ L2 成立）
        assert!(compiled.l1_match(b"this is a time.x log"));
        assert!(compiled.l1_match(b"totally unrelated"));
    }

    #[test]
    fn test_pure_literal_flag_cleared_for_and_mixed_regex() {
        // "error" && /time./ → 两个独立组：
        // - 组 0：Prim::String("error") → is_pure_literal=true
        // - 组 1：Prim::Regex("time.") → 无 literals → EmptyGroup → 跳过
        // 故 required_groups 只含组 0。
        let expr = Expr::And(
            Box::new(expr_str("error")),
            Box::new(Expr::Prim(Prim::Regex("time.".to_string()))),
        );
        let compiled = compile(&expr).expect("编译应成功");
        assert_eq!(compiled.required_groups.len(), 1);
        assert!(
            compiled.required_groups[0].is_pure_literal,
            "String 组应为纯字面量（Regex 组被跳过）"
        );
    }

    // --- verified_literals（v0.49 Opt A，Rust 方案 §7.9）---

    fn verified_of(expr: &Expr) -> Vec<Vec<u8>> {
        compile(expr).expect("编译应成功").verified_literals
    }

    #[test]
    fn test_verified_literals_and_side_singleton() {
        // "a" && cost>5 → a 是单元素纯字面量必选组成员 → 入集
        let expr = Expr::And(
            Box::new(expr_str("a")),
            Box::new(expr_compare("cost", CmpOp::Gt, "5")),
        );
        assert_eq!(verified_of(&expr), vec![b"a".to_vec()]);
    }

    #[test]
    fn test_verified_literals_or_group_not_singleton() {
        // ("a" || "b") && cost>5 → Or 合组 [a,b] ≥2 元素 → 都不入集
        let expr = Expr::And(
            Box::new(Expr::Or(Box::new(expr_str("a")), Box::new(expr_str("b")))),
            Box::new(expr_compare("cost", CmpOp::Gt, "5")),
        );
        assert!(verified_of(&expr).is_empty(), "Or 组成员不得入 verified");
    }

    #[test]
    fn test_verified_literals_paren_and_trap() {
        // "a" && ("b" && "c")：Paren(And) 组因 B14 完整性规则整组放弃
        // → 只有 a 入集（朴素 And/Paren 路径规则会把 b/c 误标，健全性陷阱）
        let expr = Expr::And(
            Box::new(expr_str("a")),
            Box::new(Expr::Prim(Prim::Paren(Box::new(Expr::And(
                Box::new(expr_str("b")),
                Box::new(expr_str("c")),
            ))))),
        );
        assert_eq!(verified_of(&expr), vec![b"a".to_vec()]);
    }

    #[test]
    fn test_verified_literals_not_side_excluded() {
        // !("a") && "b"：Not 子树不下推 → a 不入集；b 入集
        let expr = Expr::And(
            Box::new(Expr::Not(Box::new(expr_str("a")))),
            Box::new(expr_str("b")),
        );
        assert_eq!(verified_of(&expr), vec![b"b".to_vec()]);
    }

    #[test]
    fn test_verified_literals_same_string_dominates() {
        // "a" && ("a" || "b")：And 侧单元素组保证 a 存在 → a 入集
        //（Or 侧同串节点豁免后恒 True，与 And 侧保证一致，Rust 方案 §7.9）
        let expr = Expr::And(
            Box::new(expr_str("a")),
            Box::new(Expr::Prim(Prim::Paren(Box::new(Expr::Or(
                Box::new(expr_str("a")),
                Box::new(expr_str("b")),
            ))))),
        );
        assert_eq!(
            verified_of(&expr),
            vec![b"a".to_vec()],
            "And 侧单元素组成员应入集（可能重复）"
        );
    }

    #[test]
    fn test_verified_literals_pure_string_query() {
        // 纯 "a"（无谓词）：单元素组 → 入集（虽然该查询本身走 l1_exact 捷径）
        assert_eq!(verified_of(&expr_str("a")), vec![b"a".to_vec()]);
    }
}
