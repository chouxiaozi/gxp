//! L3 结构层实现：ast-grep 接入（设计文档 §2.2 + §8.2）。
//!
//! 支持范围（九语言穷举测试，大小写不敏感 + 别名）：
//! - TypeScript（`ast(ts, ...)` / `ast(TS, ...)` / `ast(typescript, ...)`，PoC 通路验证）
//! - Rust（`ast(rs, ...)` / `ast(RS, ...)` / `ast(rust, ...)`，PoC 通路验证）
//! - Python（`ast(py, ...)` / `ast(PY, ...)` / `ast(python, ...)`）
//! - JavaScript（`ast(js, ...)` / `ast(JS, ...)` / `ast(javascript, ...)`）
//! - Go（`ast(go, ...)` / `ast(GO, ...)` / `ast(golang, ...)`）
//! - TSX（`ast(tsx, ...)` / `ast(TSX, ...)`）
//! - Java（`ast(java, ...)` / `ast(Java, ...)` / `ast(JAVA, ...)`）
//! - C（`ast(c, ...)` / `ast(C, ...)`）
//! - C++（`ast(cpp, ...)` / `ast(CPP, ...)` / `ast(c++, ...)` / `ast(C++, ...)` / `ast(cxx, ...)` / `ast(cc, ...)`）
//!
//! 求值模型：
//! - L1 视 ast() 为"可能命中"（不淘汰候选，见 `l1::compiler::collect_prim_literals`）
//! - L2 阶段 `PreparedPrim::Ast` 查询 [`AstLineSet`] 行号集合（由本模块预计算）
//! - 不变式 `L1 ⊇ L2 ⊇ L3`：L1 不淘汰 → L2 查集合 → L3 精判 ast-grep find_all
//!
//! 行号映射：ast-grep `find_all` 返回 AST 节点 `range.start.byte`（字节偏移），
//! 用 memchr 计数 `\n` 得到 1-based 行号（与 gxp `MatchedLine.line_no` 对齐）。

use ast_grep_core::{AstGrep, Pattern};
use ast_grep_language::SupportLang;
use std::collections::HashMap;
use std::path::Path;

/// L3 查询：ast-grep pattern + 语言。
///
/// 六语言穷举测试（py/js/ts/rs/go/tsx）；扩展到九语言（+java/c/cpp）；大小写不敏感 + 别名。
#[derive(Debug, Clone)]
pub struct L3Query {
    /// ast-grep `SupportLang`（如 `SupportLang::TypeScript`）。
    pub language: SupportLang,
    /// ast-grep pattern（如 `useQuery($A)`）。
    pub pattern: String,
}

/// ast() 命中行号集合（多 ast 节点合并）。
///
/// key = (lang, pattern) 元组的拼接（PoC 简化：同一文件内同一 pattern 不重复求值），
/// value = 命中行号集合（1-based，对齐 gxp `MatchedLine.line_no`）。
pub type AstLineSet = HashMap<String, std::collections::HashSet<usize>>;

/// L3 错误类型。
#[derive(Debug, thiserror::Error)]
pub enum L3Error {
    /// 文件内容不是合法 UTF-8。
    #[error("L3 文件内容不是合法 UTF-8：{0}")]
    InvalidUtf8(String),
    /// ast-grep pattern 解析失败。
    #[error("ast-grep pattern 解析失败：{0}")]
    PatternParse(String),
    /// 语言不支持。
    #[error("L3 语言不支持：{lang}（支持：ts/typescript/rs/rust/py/python/js/javascript/go/golang/tsx/java/c/cpp/c++/cxx/cc，大小写不敏感）")]
    UnsupportedLang {
        /// 用户指定的语言名。
        lang: String,
    },
}

/// 把 DSL `ast(NAME, STRING)` 的 NAME 映射到 ast-grep `SupportLang`。
///
/// 九语言：ts/rs/py/js/go/tsx/java/c/cpp。
/// LLM 友好性：大小写不敏感（`Java`/`java`/`JAVA` 都映射）+ `c++`/`C++` 别名。
/// 不在列表中的语言 → `L3Error::UnsupportedLang`（fail-loud）。
pub fn lang_from_str(lang: &str) -> Result<SupportLang, L3Error> {
    match lang.to_lowercase().as_str() {
        "ts" | "typescript" => Ok(SupportLang::TypeScript),
        "rs" | "rust" => Ok(SupportLang::Rust),
        "py" | "python" => Ok(SupportLang::Python),
        "js" | "javascript" => Ok(SupportLang::JavaScript),
        "go" | "golang" => Ok(SupportLang::Go),
        "tsx" => Ok(SupportLang::Tsx),
        "java" => Ok(SupportLang::Java),
        "c" => Ok(SupportLang::C),
        "cpp" | "c++" | "cxx" | "cc" => Ok(SupportLang::Cpp),
        other => Err(L3Error::UnsupportedLang {
            lang: other.to_string(),
        }),
    }
}

/// 判断文件扩展名是否与给定 `SupportLang` 匹配。
///
/// 用于 L3 预计算跳过非目标语言文件——`ast(ts, ...)` 只需解析 `.ts`/`.mts`/`.tsx` 文件，
/// 不再对 `.json`/`.md`/`.css` 等无谓跑 ast-grep parser。
///
/// **.vue 例外**：Vue SFC 内嵌 `<script lang="ts">`，TypeScript parser 能解析其中函数声明。
/// `.vue` 同时匹配 `TypeScript` 和 `JavaScript`（ast-grep 容错解析 `<script>` 内嵌代码）。
///
/// **.jsx 例外**：JSX 语法同时可被 ast-grep `Tsx` parser 和 `JavaScript` parser 解析，
/// `.jsx` 同时匹配 `Tsx` 和 `JavaScript`。
pub(crate) fn lang_matches_ext(lang: SupportLang, ext: &str) -> bool {
    let ext = ext.to_lowercase();
    match lang {
        SupportLang::TypeScript => matches!(ext.as_str(), "ts" | "mts" | "tsx" | "vue"),
        SupportLang::JavaScript => matches!(ext.as_str(), "js" | "mjs" | "cjs" | "jsx" | "vue"),
        SupportLang::Tsx => matches!(ext.as_str(), "tsx" | "jsx"),
        SupportLang::Rust => matches!(ext.as_str(), "rs"),
        SupportLang::Python => matches!(ext.as_str(), "py" | "pyw"),
        SupportLang::Go => matches!(ext.as_str(), "go"),
        SupportLang::Java => matches!(ext.as_str(), "java"),
        SupportLang::C => matches!(ext.as_str(), "c" | "h"),
        SupportLang::Cpp => matches!(ext.as_str(), "cpp" | "cxx" | "cc" | "hpp" | "hxx" | "hh"),
        // ast-grep 支持但 gxp 未映射的语言（CSharp/Css/Dart/Elixir 等）
        // — gxp 的 lang_from_str 不会返回这些，但 SupportLang 枚举包含它们
        _ => false,
    }
}

/// 递归检查 `PreparedExpr` 中是否有任何 `ast(LANG, ...)` 节点的语言匹配给定扩展名。
///
/// 用于 `prepare_l3` 跳过非目标语言文件——如果查询中所有 ast() 语言都不匹配文件扩展名，
/// 则该文件无需跑 ast-grep，直接返回空集合。
pub(crate) fn expr_has_matching_ast_lang(
    expr: &crate::mode::prepared::PreparedExpr,
    ext: &str,
) -> bool {
    use crate::mode::prepared::{PreparedExpr, PreparedPrim};
    match expr {
        PreparedExpr::Or(l, r) | PreparedExpr::And(l, r) => {
            expr_has_matching_ast_lang(l, ext) || expr_has_matching_ast_lang(r, ext)
        }
        PreparedExpr::Not(e) => expr_has_matching_ast_lang(e, ext),
        PreparedExpr::Prim(PreparedPrim::Paren(e)) => expr_has_matching_ast_lang(e, ext),
        PreparedExpr::Prim(PreparedPrim::Ast { lang, .. }) => lang_from_str(lang)
            .map(|l| lang_matches_ext(l, ext))
            .unwrap_or(false),
        PreparedExpr::Prim(_) => false,
    }
}

/// 对整个文件内容跑 ast-grep `find_all`，返回命中行号集合。
///
/// 步骤：
/// 1. 内容转 `&str`（非 UTF-8 → `L3Error::InvalidUtf8`，fail-loud）
/// 2. `Pattern::try_new(pattern, language)` 编译 pattern
///    （pattern 语法错误 → `L3Error::PatternParse`，fail-loud）
/// 3. `AstGrep::new(src, language)` 解析源码
/// 4. `root().find_all(pattern)` 枚举所有 AST 匹配
/// 5. 每个匹配的 `range.start.byte` → memchr 计数 `\n` → 1-based 行号
///
/// 单 pattern 公开 API（`run_file` + 测试使用）。
/// 多 pattern 场景由 `run_all_ast` 按语言分组复用 `AstGrep` root。
pub fn run_single(
    query: &L3Query,
    content: &[u8],
) -> Result<std::collections::HashSet<usize>, L3Error> {
    let src = std::str::from_utf8(content).map_err(|e| L3Error::InvalidUtf8(e.to_string()))?;
    // fail-loud：pattern 编译失败时返回错误，不静默返回空集合
    let pattern = Pattern::try_new(query.pattern.as_str(), query.language)
        .map_err(|e| L3Error::PatternParse(format!("{e}")))?;
    let grep = AstGrep::new(src, query.language);
    let root = grep.root();
    let mut matches = std::collections::HashSet::new();
    // ast-grep 的 find_all 返回 Match 抽象，range() 给 `Range<usize>`（字节偏移）
    for nm in root.find_all(pattern) {
        let range = nm.range();
        let line_no = count_lines(&content[..range.start]) + 1; // 1-based
        matches.insert(line_no);
    }
    Ok(matches)
}

/// 对整个文件内容跑所有 `ast()` 节点，返回 `(lang|pattern) → 行号集合` 映射。
///
/// 用于 `run_gxp_mode`：扫描前对整个文件预计算 ast 命中行号，
/// L2 求值时 `PreparedPrim::Ast` 查此集合。
///
/// key 格式：`"{lang}\x00{pattern}"`。
/// 同语言多 pattern 时复用同一 `AstGrep` root（单次查询内缓存，
/// 避免同文件多次 tree-sitter 解析）。
pub(crate) fn run_all_ast(
    prepared: &crate::mode::prepared::PreparedExpr,
    content: &[u8],
) -> Result<AstLineSet, L3Error> {
    let src = std::str::from_utf8(content).map_err(|e| L3Error::InvalidUtf8(e.to_string()))?;

    // 收集所有 ast() 查询
    let mut ast_queries: Vec<(String, SupportLang, String)> = Vec::new();
    collect_ast_queries(prepared, &mut ast_queries)?;

    // 按语言分组 + 去重 pattern
    let mut by_lang: HashMap<SupportLang, Vec<(String, String)>> = HashMap::new();
    for (lang_str, language, pattern) in ast_queries {
        let entry = by_lang.entry(language).or_default();
        if !entry.iter().any(|(_, p)| p == &pattern) {
            entry.push((lang_str, pattern));
        }
    }

    let mut out: AstLineSet = HashMap::new();
    for (language, queries) in by_lang {
        // 每语言只解析一次 AST，多 pattern 复用同一 root
        let grep = AstGrep::new(src, language);
        let root = grep.root();
        for (lang_str, pattern) in queries {
            let key = format!("{lang_str}\x00{pattern}");
            if out.contains_key(&key) {
                continue;
            }
            let pattern = Pattern::try_new(pattern.as_str(), language)
                .map_err(|e| L3Error::PatternParse(format!("{e}")))?;
            let mut lines = std::collections::HashSet::new();
            for nm in root.find_all(pattern) {
                let range = nm.range();
                let line_no = count_lines(&content[..range.start]) + 1;
                lines.insert(line_no);
            }
            out.insert(key, lines);
        }
    }
    Ok(out)
}

/// 递归收集 `PreparedExpr` 中所有 `PreparedPrim::Ast` 节点的 (lang, SupportLang, pattern)。
///
/// fail-loud：遇到不支持的 lang 立即返回 `L3Error::UnsupportedLang`，
/// 不静默跳过（否则 `ast(swift, ...)` 会静默返回空集合，破坏诚实 > 承诺）。
fn collect_ast_queries(
    expr: &crate::mode::prepared::PreparedExpr,
    out: &mut Vec<(String, SupportLang, String)>,
) -> Result<(), L3Error> {
    use crate::mode::prepared::PreparedExpr;
    match expr {
        PreparedExpr::Or(l, r) | PreparedExpr::And(l, r) => {
            collect_ast_queries(l, out)?;
            collect_ast_queries(r, out)?;
        }
        PreparedExpr::Not(e) => collect_ast_queries(e, out)?,
        PreparedExpr::Prim(crate::mode::prepared::PreparedPrim::Paren(e)) => {
            collect_ast_queries(e, out)?
        }
        PreparedExpr::Prim(crate::mode::prepared::PreparedPrim::Ast { lang, pattern }) => {
            let language = lang_from_str(lang)?;
            out.push((lang.clone(), language, pattern.clone()));
        }
        PreparedExpr::Prim(_) => {}
    }
    Ok(())
}

/// 查询 `ast_lines` 集合中是否含 `(lang, pattern)` 对应的行号。
///
/// `eval_prepared_prim_with_ctx_and_captures` 调此函数判断 `PreparedPrim::Ast` 的求值结果。
/// 返回 true 当且仅当 ast-grep find_all 在此行命中过。
pub fn ast_line_contains(
    ast_lines: &AstLineSet,
    lang: &str,
    pattern: &str,
    line_no: usize,
) -> bool {
    let key = format!("{lang}\x00{pattern}");
    ast_lines
        .get(&key)
        .map(|set| set.contains(&line_no))
        .unwrap_or(false)
}

/// 计数 `bytes` 中 `\n` 出现次数（用于 ast-grep byte offset → 1-based 行号映射）。
#[inline]
fn count_lines(bytes: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', bytes).count()
}

/// 简单的端到端 API：给定文件路径 + ast() 查询，返回命中行号列表。
///
/// 测试用便利函数（`tests/cli_e2e.rs` 使用）。
#[allow(dead_code)]
pub fn run_file(query: &L3Query, path: &Path) -> Result<Vec<usize>, L3Error> {
    let content = std::fs::read(path).map_err(|e| L3Error::InvalidUtf8(e.to_string()))?;
    let lines = run_single(query, &content)?;
    let mut v: Vec<usize> = lines.into_iter().collect();
    v.sort_unstable();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PoC 通路验证：TypeScript `useQuery($$$A)` 在 TS 调用上命中。
    ///
    /// 注：
    /// - 测试源代码用纯 TS（非 JSX），因为 `SupportLang::TypeScript` 不解析 JSX。
    ///   含 JSX 的代码应走 `SupportLang::Tsx`（`ast(tsx, ...)` 语法）。
    /// - pattern 用 `useQuery($$$A)` 而非 `useQuery($A)`：前者匹配任意参数列表，
    ///   后者只匹配单参数调用（设计文档 §2.2 + Rust 方案 §3.1 PoC 选用更通用的 $$$A）。
    #[test]
    fn test_ts_usequery_matches_react_component() {
        let src = b"import { useQuery } from 'react-query';\n\
                    function MyComponent() {\n\
                    const data = useQuery(['key'], fetcher);\n\
                    return data;\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::TypeScript,
            pattern: "useQuery($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep ts useQuery 应解析");
        // useQuery 调用在第 3 行
        assert!(
            lines.contains(&3),
            "useQuery($$$A) 应命中第 3 行（实际命中行号: {lines:?}）"
        );
    }

    /// 不命中的 pattern 返回空集合（fail-loud 的反例：空结果不报错）。
    #[test]
    fn test_ts_no_match_returns_empty() {
        let src = b"const x = 1;\n";
        let q = L3Query {
            language: SupportLang::TypeScript,
            pattern: "useQuery($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep ts useQuery 应解析");
        assert!(lines.is_empty(), "不命中的 pattern 应返回空集合");
    }

    /// 不支持的语言 fail-loud 报 `UnsupportedLang`。
    #[test]
    fn test_unsupported_lang_fail_loud() {
        let result = lang_from_str("swift");
        let Err(L3Error::UnsupportedLang { lang }) = result else {
            panic!("swift 应报 UnsupportedLang，实际: {result:?}");
        };
        assert_eq!(lang, "swift");
    }

    /// ts → SupportLang::TypeScript 映射正确。
    #[test]
    fn test_ts_lang_maps_to_typescript() {
        let lang = lang_from_str("ts").expect("\"ts\" 应映射到 SupportLang::TypeScript");
        assert_eq!(lang, SupportLang::TypeScript);
    }

    /// Rust 通路验证：`$OBJ.unwrap()` 方法调用在 Rust 代码上命中。
    ///
    /// ast-grep pattern `$OBJ.unwrap()` 匹配 method call 表达式：
    /// `$OBJ` 是单节点通配，匹配接收者表达式。
    /// 测试源用典型 Rust：Option/Result 的 unwrap 调用。
    #[test]
    fn test_rs_unwrap_matches_method_call() {
        let src = b"fn main() {\n\
                    let x = Some(1).unwrap();\n\
                    let y = Ok(2).unwrap();\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::Rust,
            pattern: "$OBJ.unwrap()".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep rs unwrap 应解析");
        // 两个 unwrap 调用分别在第 2、3 行
        assert!(
            lines.contains(&2),
            "$OBJ.unwrap() 应命中第 2 行（实际命中行号: {lines:?}）"
        );
        assert!(
            lines.contains(&3),
            "$OBJ.unwrap() 应命中第 3 行（实际命中行号: {lines:?}）"
        );
    }

    /// Rust 不命中的 pattern 返回空集合。
    #[test]
    fn test_rs_no_match_returns_empty() {
        let src = b"fn main() {}\n";
        let q = L3Query {
            language: SupportLang::Rust,
            pattern: "println!($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep rs println 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// Rust println! 宏调用匹配。
    #[test]
    fn test_rs_println_macro_matches() {
        let src = b"fn main() {\n\
                    println!(\"hello\");\n\
                    println!(\"{}\", 42);\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::Rust,
            pattern: "println!($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep rs println 应解析");
        assert!(
            lines.contains(&2),
            "println! 应命中第 2 行（实际: {lines:?}）"
        );
        assert!(
            lines.contains(&3),
            "println! 应命中第 3 行（实际: {lines:?}）"
        );
    }

    /// rs → SupportLang::Rust 映射正确。
    #[test]
    fn test_rs_lang_maps_to_rust() {
        let lang = lang_from_str("rs").expect("\"rs\" 应映射到 SupportLang::Rust");
        assert_eq!(lang, SupportLang::Rust);
    }

    // ===== Python 通路验证 =====

    /// Python `requests.get($$$A)` 在 Python 调用上命中。
    #[test]
    fn test_py_requests_get_matches() {
        let src = b"import requests\n\
                    resp = requests.get('https://api.example.com')\n\
                    print(resp.status_code)\n";
        let q = L3Query {
            language: SupportLang::Python,
            pattern: "requests.get($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep py requests.get 应解析");
        assert!(
            lines.contains(&2),
            "requests.get 应命中第 2 行（实际命中行号: {lines:?}）"
        );
    }

    /// Python 不命中的 pattern 返回空集合。
    #[test]
    fn test_py_no_match_returns_empty() {
        let src = b"x = 1\ny = 2\n";
        let q = L3Query {
            language: SupportLang::Python,
            pattern: "requests.get($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep py 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// py → SupportLang::Python 映射正确。
    #[test]
    fn test_py_lang_maps_to_python() {
        let lang = lang_from_str("py").expect("\"py\" 应映射到 SupportLang::Python");
        assert_eq!(lang, SupportLang::Python);
    }

    // ===== JavaScript 通路验证 =====

    /// JavaScript `fetch($$$A)` 在 JS 调用上命中。
    #[test]
    fn test_js_fetch_matches() {
        let src = b"async function loadData() {\n\
                    const res = await fetch('/api/data');\n\
                    return res.json();\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::JavaScript,
            pattern: "fetch($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep js fetch 应解析");
        assert!(
            lines.contains(&2),
            "fetch 应命中第 2 行（实际命中行号: {lines:?}）"
        );
    }

    /// JavaScript 不命中的 pattern 返回空集合。
    #[test]
    fn test_js_no_match_returns_empty() {
        let src = b"const x = 1;\n";
        let q = L3Query {
            language: SupportLang::JavaScript,
            pattern: "fetch($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep js 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// js → SupportLang::JavaScript 映射正确。
    #[test]
    fn test_js_lang_maps_to_javascript() {
        let lang = lang_from_str("js").expect("\"js\" 应映射到 SupportLang::JavaScript");
        assert_eq!(lang, SupportLang::JavaScript);
    }

    // ===== Go 通路验证 =====

    /// Go `func main() { $$$B }` 在 Go 函数声明上命中。
    ///
    /// 注：Go tree-sitter grammar 中 expression statement 需匹配完整
    /// statement node（含行尾），selector expression pattern 不单独命中。
    /// 用 func declaration pattern 避开此问题。
    #[test]
    fn test_go_func_declaration_matches() {
        let src = b"package main\n\
                    import \"fmt\"\n\
                    func main() {\n\tfmt.Println(\"hello\")\n}\n";
        let q = L3Query {
            language: SupportLang::Go,
            pattern: "func main() { $$$B }".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep go func 应解析");
        // func main 在第 3 行
        assert!(
            lines.contains(&3),
            "func main 应命中第 3 行（实际命中行号: {lines:?}）"
        );
    }

    /// Go 不命中的 pattern 返回空集合。
    #[test]
    fn test_go_no_match_returns_empty() {
        let src = b"package main\nfunc main() {}\n";
        let q = L3Query {
            language: SupportLang::Go,
            pattern: "fmt.Println($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep go 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// go → SupportLang::Go 映射正确。
    #[test]
    fn test_go_lang_maps_to_go() {
        let lang = lang_from_str("go").expect("\"go\" 应映射到 SupportLang::Go");
        assert_eq!(lang, SupportLang::Go);
    }

    /// 无效 ast-grep pattern fail-loud 报 `PatternParse`，不静默返回空集合。
    ///
    /// 修复前：`run_single` 用 `root.find_all(pattern.as_str())`，
    /// `str: Matcher` 内部调 `Pattern::new` → `try_new().unwrap()`，
    /// pattern 语法错误时 panic 或静默返回空。
    /// 修复后：用 `Pattern::try_new` 显式传播 `PatternError` → `L3Error::PatternParse`。
    #[test]
    fn test_invalid_pattern_fail_loud() {
        let src = b"const x = 1;\n";
        // `$$$A` 独立使用是非法 pattern → `PatternError::RootMultiMetaVar`
        // （ast-grep 要求 $$$ 不能作为 pattern 根节点）
        let q = L3Query {
            language: SupportLang::TypeScript,
            pattern: "$$$A".to_string(),
        };
        let result = run_single(&q, src);
        let Err(L3Error::PatternParse(msg)) = result else {
            panic!("无效 pattern 应报 PatternParse，实际: {result:?}");
        };
        assert!(!msg.is_empty(), "PatternParse 错误消息不应为空");
    }

    // ===== TSX 通路验证 =====

    /// TSX `useQuery($$$A)` 在含 JSX 的 TSX 组件上命中。
    ///
    /// `SupportLang::Tsx` 解析 JSX 语法（`<div>...</div>`），
    /// 而 `SupportLang::TypeScript` 不解析 JSX。
    /// pattern `useQuery($$$A)` 匹配任意参数列表的 useQuery 调用。
    #[test]
    fn test_tsx_usequery_matches_jsx_component() {
        let src = b"import { useQuery } from 'react-query';\n\
                    function MyComponent() {\n\
                    const data = useQuery(['key'], fetcher);\n\
                    return <div>{data}</div>;\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::Tsx,
            pattern: "useQuery($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep tsx useQuery 应解析");
        assert!(
            lines.contains(&3),
            "useQuery($$$A) 应命中第 3 行（实际命中行号: {lines:?}）"
        );
    }

    /// TSX 不命中的 pattern 返回空集合。
    #[test]
    fn test_tsx_no_match_returns_empty() {
        let src = b"const x = 1;\nconst y = 2;\n";
        let q = L3Query {
            language: SupportLang::Tsx,
            pattern: "useQuery($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep tsx 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// tsx → SupportLang::Tsx 映射正确。
    #[test]
    fn test_tsx_lang_maps_to_tsx() {
        let lang = lang_from_str("tsx").expect("\"tsx\" 应映射到 SupportLang::Tsx");
        assert_eq!(lang, SupportLang::Tsx);
    }

    // ===== Java 通路验证 =====

    /// Java `System.out.println($$$A)` 在 Java 方法调用上命中。
    #[test]
    fn test_java_println_matches() {
        let src = b"public class Main {\n\
                    public static void main(String[] args) {\n\
                    System.out.println(\"hello\");\n\
                    }\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::Java,
            pattern: "System.out.println($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep java println 应解析");
        assert!(
            lines.contains(&3),
            "System.out.println 应命中第 3 行（实际命中行号: {lines:?}）"
        );
    }

    /// Java 不命中的 pattern 返回空集合。
    #[test]
    fn test_java_no_match_returns_empty() {
        let src = b"public class Main {\n}\n";
        let q = L3Query {
            language: SupportLang::Java,
            pattern: "System.out.println($$$A)".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep java 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// java → SupportLang::Java 映射正确。
    #[test]
    fn test_java_lang_maps_to_java() {
        let lang = lang_from_str("java").expect("\"java\" 应映射到 SupportLang::Java");
        assert_eq!(lang, SupportLang::Java);
    }

    // ===== C 通路验证 =====

    /// C `return $X;` 在 C 函数上命中。
    ///
    /// V5 验证结论（2026-10-23）：ast-grep 对 C/C++ 不做预处理
    ///（不展开 #include/#define/#ifdef），AST 行号准确对应源码行。
    #[test]
    fn test_c_return_matches() {
        let src = b"#include <stdio.h>\n\
                    int main() {\n\
                    printf(\"hello\");\n\
                    return 0;\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::C,
            pattern: "return $X;".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep c return 应解析");
        assert!(
            lines.contains(&4),
            "return 应命中第 4 行（实际命中行号: {lines:?}）"
        );
    }

    /// C 不命中的 pattern 返回空集合。
    #[test]
    fn test_c_no_match_returns_empty() {
        let src = b"int main() {\n    int x = 1;\n}\n";
        let q = L3Query {
            language: SupportLang::C,
            pattern: "return $X;".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep c 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// c → SupportLang::C 映射正确。
    #[test]
    fn test_c_lang_maps_to_c() {
        let lang = lang_from_str("c").expect("\"c\" 应映射到 SupportLang::C");
        assert_eq!(lang, SupportLang::C);
    }

    // ===== C++ 通路验证 =====

    /// C++ `namespace $NS { $$$B }` 在 C++ namespace 声明上命中。
    #[test]
    fn test_cpp_namespace_matches() {
        let src = b"#include <iostream>\n\
                    namespace mylib {\n\
                    int add(int a, int b) { return a + b; }\n\
                    }\n";
        let q = L3Query {
            language: SupportLang::Cpp,
            pattern: "namespace $NS { $$$B }".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep cpp namespace 应解析");
        assert!(
            lines.contains(&2),
            "namespace 应命中第 2 行（实际命中行号: {lines:?}）"
        );
    }

    /// C++ 不命中的 pattern 返回空集合。
    #[test]
    fn test_cpp_no_match_returns_empty() {
        let src = b"int main() {\n    int x = 1;\n}\n";
        let q = L3Query {
            language: SupportLang::Cpp,
            pattern: "namespace $NS { $$$B }".to_string(),
        };
        let lines = run_single(&q, src).expect("ast-grep cpp 应解析");
        assert!(
            lines.is_empty(),
            "不命中的 pattern 应返回空集合（实际: {lines:?}）"
        );
    }

    /// cpp → SupportLang::Cpp 映射正确。
    #[test]
    fn test_cpp_lang_maps_to_cpp() {
        let lang = lang_from_str("cpp").expect("\"cpp\" 应映射到 SupportLang::Cpp");
        assert_eq!(lang, SupportLang::Cpp);
    }
}
