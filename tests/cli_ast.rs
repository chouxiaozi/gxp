//! L3 ast() 结构模式匹配端到端集成测试（所有语言 + verify 覆盖矩阵）。
//!
//! 这些测试只在 `l3-structural` feature 开启时编译（ast-grep crate 依赖）。
//! feature 关闭时 gxp 会解析 ast() 但运行时 `PreparedPrim::Ast` 视为永真（不漏候选）。
//!
//! 覆盖语言：ts / rs / py / js / go / tsx / java / c / cpp（九语言穷举）。
//! 覆盖场景：单文件命中/不命中/字面量组合 / 目录模式 / --verify 覆盖矩阵 A-G /
//! 暴力路径假阳性回归。

mod common;
#[cfg(feature = "l3-structural")]
use common::*;

#[cfg(feature = "l3-structural")]
use gxp::cli::RunOpts;
#[cfg(feature = "l3-structural")]
use gxp::io::file::FileContent;
#[cfg(feature = "l3-structural")]
use gxp::mode::detect::{dispatch_with_content, run_with_writer};
#[cfg(feature = "l3-structural")]
use std::fs;
#[cfg(feature = "l3-structural")]
use tempfile::tempdir;

// ===== L3 ast() 结构模式匹配（设计文档 §2.2 + §8.2）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_ts_usequery_matches() {
    // gxp 'ast(ts, "useQuery($$$A)")' file.ts
    // 命中含 useQuery 调用的行（ast-grep find_all → 行号映射）
    let src = "import { useQuery } from 'react-query';\n\
               function MyComponent() {\n\
               const data = useQuery(['key'], fetcher);\n\
               return data;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(ts, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(ts, ...) 应执行成功");
    assert!(
        out.contains("useQuery(['key'], fetcher)"),
        "应含 useQuery 调用行，实际: {out}"
    );
    assert!(
        !out.contains("import { useQuery }"),
        "import 行不应命中（pattern 是 useQuery($$$A) 调用形式，非 import），实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_ts_no_match_returns_empty() {
    // 无 useQuery 调用的文件 → 空输出
    let src = "const x = 1;\nconst y = 2;\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(ts, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(ts, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_ts_combined_with_literal_filter() {
    // gxp '"data" && ast(ts, "useQuery($$$A)")' file.ts
    // 行含 "data" 且 ast-grep 命中 useQuery 调用 → 单行命中
    let src = "import { useQuery } from 'react-query';\n\
               function MyComponent() {\n\
               const data = useQuery(['key'], fetcher);\n\
               return data;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"data\" && ast(ts, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 3 行含 data + useQuery → 命中
    assert!(
        out.contains("useQuery(['key'], fetcher)"),
        "第 3 行应命中（data + useQuery），实际: {out}"
    );
    // 第 4 行含 data 但无 useQuery 调用 → 不命中
    assert!(
        !out.contains("return data;\n"),
        "第 4 行 data 但无 useQuery 不应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_unsupported_lang_fail_loud() {
    // gxp 'ast(swift, "...")' file：swift 语言 PoC 不支持 → fail-loud
    let f = write_temp("any content\n");
    let err = run(&args(
        "ast(swift, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect_err("swift 语言应 fail-loud 报错");
    assert!(
        err.contains("L3 语言不支持") || err.contains("UnsupportedLang") || err.contains("swift"),
        "错误消息应含 swift 不支持，实际: {err}"
    );
}

// ===== L3 Rust 通路 e2e（扩展第二语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_unwrap_matches() {
    // gxp 'ast(rs, "$OBJ.unwrap()")' file.rs
    // Rust method call pattern：$OBJ.unwrap() 匹配 Option/Result 的 unwrap 调用
    let src = "fn main() {\n\
               let x = Some(1).unwrap();\n\
               let y = Ok(2).unwrap();\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(rs, \"$OBJ.unwrap()\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(rs, ...) unwrap 应执行成功");
    // 两个 unwrap 调用分别在第 2、3 行
    assert!(
        out.contains("2:let x = Some(1).unwrap();"),
        "第 2 行 unwrap 应命中，实际: {out}"
    );
    assert!(
        out.contains("3:let y = Ok(2).unwrap();"),
        "第 3 行 unwrap 应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_no_match_returns_empty() {
    // 无 unwrap 调用的 Rust 文件 → 空输出
    let src = "fn main() {\n    let x = 1;\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(rs, \"$OBJ.unwrap()\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(rs, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_combined_with_literal_filter() {
    // gxp '"hello" && ast(rs, "println!($$$A)")' file.rs
    // 行含 "hello" 且 ast-grep 命中 println! 调用
    let src = "fn main() {\n\
               println!(\"hello world\");\n\
               println!(\"other\");\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"hello\" && ast(rs, \"println!($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 2 行含 hello + println! → 命中
    assert!(
        out.contains("2:println!(\"hello world\");"),
        "第 2 行应命中（hello + println!），实际: {out}"
    );
    // 第 3 行有 println! 但无 "hello" → 不命中
    assert!(
        !out.contains("3:println!(\"other\");"),
        "第 3 行无 hello 不应命中，实际: {out}"
    );
}

// ===== L3 Python 通路 e2e（扩展第三语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_py_requests_get_matches() {
    // gxp 'ast(py, "requests.get($$$A)")' file.py
    let src = "import requests\n\
               resp = requests.get('https://api.example.com')\n\
               print(resp.status_code)\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(py, \"requests.get($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(py, ...) 应执行成功");
    assert!(
        out.contains("2:resp = requests.get"),
        "第 2 行 requests.get 应命中，实际: {out}"
    );
    assert!(
        !out.contains("import requests"),
        "import 行不应命中（pattern 是调用形式），实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_py_no_match_returns_empty() {
    let src = "x = 1\ny = 2\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(py, \"requests.get($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(py, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_py_combined_with_literal_filter() {
    // gxp '"api" && ast(py, "requests.get($$$A)")' file.py
    let src = "import requests\n\
               resp = requests.get('https://api.example.com')\n\
               base = 'https://api.example.com'\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"api\" && ast(py, \"requests.get($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 2 行含 api + requests.get → 命中
    assert!(
        out.contains("2:resp = requests.get"),
        "第 2 行应命中（api + requests.get），实际: {out}"
    );
    // 第 3 行含 api 但无 requests.get 调用 → 不命中
    assert!(
        !out.contains("base = "),
        "第 3 行无 requests.get 不应命中，实际: {out}"
    );
}

// ===== L3 JavaScript 通路 e2e（扩展第四语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_js_fetch_matches() {
    // gxp 'ast(js, "fetch($$$A)")' file.js
    let src = "async function loadData() {\n\
               const res = await fetch('/api/data');\n\
               return res.json();\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(js, \"fetch($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(js, ...) 应执行成功");
    assert!(
        out.contains("2:const res = await fetch"),
        "第 2 行 fetch 应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_js_no_match_returns_empty() {
    let src = "const x = 1;\nconst y = 2;\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(js, \"fetch($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(js, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_js_combined_with_literal_filter() {
    // gxp '"data" && ast(js, "fetch($$$A)")' file.js
    let src = "async function loadData() {\n\
               const res = await fetch('/api/data');\n\
               const url = '/api/data';\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"data\" && ast(js, \"fetch($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 2 行含 data + fetch → 命中
    assert!(
        out.contains("2:const res = await fetch"),
        "第 2 行应命中（data + fetch），实际: {out}"
    );
    // 第 3 行含 data 但无 fetch → 不命中
    assert!(
        !out.contains("const url = "),
        "第 3 行无 fetch 不应命中，实际: {out}"
    );
}

// ===== L3 Go 通路 e2e（扩展第五语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_go_func_declaration_matches() {
    // gxp 'ast(go, "func main() { $$$B }")' file.go
    let src = "package main\n\
               import \"fmt\"\n\
               func main() {\n\tfmt.Println(\"hello\")\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(go, \"func main() { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(go, ...) 应执行成功");
    assert!(
        out.contains("3:func main()"),
        "第 3 行 func main 应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_go_no_match_returns_empty() {
    let src = "package main\nfunc helper() {}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(go, \"func main() { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(go, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_go_combined_with_literal_filter() {
    // gxp '"main" && ast(go, "func main() { $$$B }")' file.go
    // 行级模型：ast() 命中 func 声明行（第 3 行），"main" 也在第 3 行
    let src = "package main\n\
               import \"fmt\"\n\
               func main() {\n\tfmt.Println(\"hello\")\n}\n\
               func helper() {}\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"main\" && ast(go, \"func main() { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 3 行含 main + func main 声明 → 命中
    assert!(
        out.contains("3:func main()"),
        "第 3 行应命中（main + func main），实际: {out}"
    );
    // 第 5 行 func helper 无 main → 不命中
    assert!(
        !out.contains("func helper"),
        "func helper 不含 main 不应命中，实际: {out}"
    );
}

// ===== L3 TSX 通路 e2e（扩展第六语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_tsx_usequery_matches_jsx_component() {
    // gxp 'ast(tsx, "useQuery($$$A)")' file.tsx
    // 命中含 JSX 语法的 TSX 组件中的 useQuery 调用
    let src = "import { useQuery } from 'react-query';\n\
               function MyComponent() {\n\
               const data = useQuery(['key'], fetcher);\n\
               return <div>{data}</div>;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(tsx, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(tsx, ...) 应执行成功");
    assert!(
        out.contains("useQuery(['key'], fetcher)"),
        "应含 useQuery 调用行，实际: {out}"
    );
    assert!(
        !out.contains("import { useQuery }"),
        "import 行不应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_tsx_no_match_returns_empty() {
    // 无 useQuery 调用的 TSX 文件 → 空输出
    let src = "const x = 1;\nconst y = 2;\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(tsx, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(tsx, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_tsx_combined_with_literal_filter() {
    // gxp '"data" && ast(tsx, "useQuery($$$A)")' file.tsx
    // 行含 "data" 且 ast-grep 命中 useQuery 调用 → 单行命中
    let src = "import { useQuery } from 'react-query';\n\
               function MyComponent() {\n\
               const data = useQuery(['key'], fetcher);\n\
               return <div>{data}</div>;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"data\" && ast(tsx, \"useQuery($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 3 行含 data + useQuery → 命中
    assert!(
        out.contains("useQuery(['key'], fetcher)"),
        "第 3 行应命中（data + useQuery），实际: {out}"
    );
    // 第 4 行含 data 但无 useQuery 调用 → 不命中
    assert!(
        !out.contains("return <div>{data}</div>;\n"),
        "第 4 行 data 但无 useQuery 不应命中，实际: {out}"
    );
}

// ===== L3 .jsx 扩展名回归测试 =====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_tsx_matches_jsx_file() {
    // .jsx 文件应被 ast(tsx, ...) 解析（扩展名过滤曾遗漏 .jsx → 回归）
    let src = "const x = <div>hello</div>;\n";
    // write_temp 生成 .tmpXXXXXX 无扩展名，手动写到 .jsx 文件
    let jsx_path = std::env::temp_dir().join("gxp_test_jsx.jsx");
    std::fs::write(&jsx_path, src).unwrap();
    let out = run(&args(
        "ast(tsx, \"const $NAME = $$$EXPR\")",
        jsx_path.to_str().unwrap(),
    ))
    .expect("ast(tsx, ...) 应执行成功");
    assert!(
        out.contains("const x = <div>hello</div>;"),
        "ast(tsx) 应命中 .jsx 文件，实际: {out}"
    );
    let _ = std::fs::remove_file(&jsx_path);
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_js_matches_jsx_file() {
    // .jsx 文件应被 ast(js, ...) 解析（JavaScript parser 能解析 JSX）
    let src = "const x = <div>hello</div>;\n";
    let jsx_path = std::env::temp_dir().join("gxp_test_jsx2.jsx");
    std::fs::write(&jsx_path, src).unwrap();
    let out = run(&args(
        "ast(js, \"const $NAME = $$$EXPR\")",
        jsx_path.to_str().unwrap(),
    ))
    .expect("ast(js, ...) 应执行成功");
    assert!(
        out.contains("const x = <div>hello</div>;"),
        "ast(js) 应命中 .jsx 文件，实际: {out}"
    );
    let _ = std::fs::remove_file(&jsx_path);
}

// ===== L3 Java 通路 e2e（扩展第七语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_java_println_matches() {
    // gxp 'ast(java, "System.out.println($$$A)")' file.java
    let src = "public class Main {\n\
               public static void main(String[] args) {\n\
               System.out.println(\"hello\");\n\
               }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(java, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(java, ...) 应执行成功");
    assert!(
        out.contains("System.out.println"),
        "应含 println 调用行，实际: {out}"
    );
    assert!(
        !out.contains("public class Main"),
        "class 声明行不应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_java_no_match_returns_empty() {
    let src = "public class Main {\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(java, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(java, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_java_combined_with_literal_filter() {
    // gxp '"hello" && ast(java, "System.out.println($$$A)")' file.java
    let src = "public class Main {\n\
               public static void main(String[] args) {\n\
               System.out.println(\"hello world\");\n\
               System.out.println(\"other\");\n\
               }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"hello\" && ast(java, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 3 行含 hello + println → 命中
    assert!(
        out.contains("System.out.println(\"hello world\")"),
        "第 3 行应命中（hello + println），实际: {out}"
    );
    // 第 4 行有 println 但无 "hello" → 不命中
    assert!(
        !out.contains("\"other\""),
        "第 4 行无 hello 不应命中，实际: {out}"
    );
}

// ===== L3 C 通路 e2e（扩展第八语言）=====
// V5 验证结论（2026-10-23）：ast-grep 对 C/C++ 不做预处理
//（不展开 #include/#define/#ifdef），AST 行号准确对应源码行。

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_c_return_matches() {
    // gxp 'ast(c, "return $X;")' file.c
    let src = "#include <stdio.h>\n\
               int main() {\n\
               printf(\"hello\");\n\
               return 0;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args("ast(c, \"return $X;\")", f.path().to_str().unwrap()))
        .expect("ast(c, ...) 应执行成功");
    assert!(out.contains("return 0;"), "应含 return 行，实际: {out}");
    assert!(
        !out.contains("#include"),
        "#include 行不应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_c_no_match_returns_empty() {
    let src = "int main() {\n    int x = 1;\n}\n";
    let f = write_temp(src);
    let out = run(&args("ast(c, \"return $X;\")", f.path().to_str().unwrap()))
        .expect("ast(c, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_c_combined_with_literal_filter() {
    // gxp '"hello" && ast(c, "return $X;")' file.c
    // 第 3 行 printf 含 hello，第 4 行 return 不含 hello → 不命中
    // 构造：return 行含 hello 才命中
    let src = "#include <stdio.h>\n\
               int main() {\n\
               printf(\"hello\");\n\
               return hello_code;\n\
               return 0;\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"hello\" && ast(c, \"return $X;\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 4 行含 hello + return → 命中
    assert!(
        out.contains("return hello_code;"),
        "第 4 行应命中（hello + return），实际: {out}"
    );
    // 第 5 行 return 但无 "hello" → 不命中
    assert!(
        !out.contains("return 0;"),
        "第 5 行无 hello 不应命中，实际: {out}"
    );
}

// ===== L3 C++ 通路 e2e（扩展第九语言）=====

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_namespace_matches() {
    // gxp 'ast(cpp, "namespace $NS { $$$B }")' file.cpp
    let src = "#include <iostream>\n\
               namespace mylib {\n\
               int add(int a, int b) { return a + b; }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(cpp, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(cpp, ...) 应执行成功");
    assert!(
        out.contains("namespace mylib"),
        "应含 namespace 声明行，实际: {out}"
    );
    assert!(
        !out.contains("#include"),
        "#include 行不应命中，实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_no_match_returns_empty() {
    let src = "int main() {\n    int x = 1;\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(cpp, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(cpp, ...) 应执行成功（空结果不报错）");
    assert!(out.trim().is_empty(), "无匹配应空输出，实际: {out:?}");
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_combined_with_literal_filter() {
    // gxp '"mylib" && ast(cpp, "namespace $NS { $$$B }")' file.cpp
    let src = "#include <iostream>\n\
               namespace mylib {\n\
               int add(int a, int b) { return a + b; }\n\
               }\n\
               namespace other {\n\
               int sub(int a, int b) { return a - b; }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "\"mylib\" && ast(cpp, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast + 字面量组合应执行成功");
    // 第 2 行含 mylib + namespace → 命中
    assert!(
        out.contains("namespace mylib"),
        "第 2 行应命中（mylib + namespace），实际: {out}"
    );
    // 第 5 行 namespace other 无 "mylib" → 不命中
    assert!(
        !out.contains("namespace other"),
        "第 5 行无 mylib 不应命中，实际: {out}"
    );
}

// ===== L3 目录模式 e2e（bug 修复：目录模式 L3 路径缺失）=====
// PoC 留下的 bug：run_gxp_mode_directory 缺失 L3 预计算路径，
// 纯 ast() 查询 needs_l2=false → 直接收集所有行，L3 过滤被跳过。
// 所有 e2e 测试用单文件 write_temp 漏掉了这个 bug。
// 以下测试用多文件目录场景覆盖 L3 目录模式路径。

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_directory_unwrap_matches() {
    // 目录模式：ast(rs, "$OBJ.unwrap()") 应只输出含 .unwrap() 的行
    let dir = tempdir().expect("创建临时目录失败");
    fs::write(
        dir.path().join("a.rs"),
        "fn main() {\n    let x = Some(1).unwrap();\n}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("b.rs"),
        "fn other() {\n    let y = 1 + 2;\n}\n",
    )
    .unwrap();

    let out = run(&args_dir(
        "ast(rs, \"$OBJ.unwrap()\")",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录模式 ast(rs, ...) 应执行成功");
    // 只应命中 a.rs 第 2 行，不应命中 b.rs 任何行
    assert!(out.contains("unwrap"), "应命中含 unwrap 的行，实际: {out}");
    assert!(
        !out.contains("let y = 1 + 2"),
        "不应命中无 unwrap 的行（b.rs），实际: {out}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_directory_no_match_returns_empty() {
    // 目录模式：无匹配文件 → 空输出
    let dir = tempdir().expect("创建临时目录失败");
    fs::write(dir.path().join("a.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
    fs::write(dir.path().join("b.rs"), "fn other() {\n    let y = 2;\n}\n").unwrap();

    let out = run(&args_dir(
        "ast(rs, \"$OBJ.unwrap()\")",
        dir.path().to_str().unwrap(),
    ))
    .expect("目录模式 ast(rs, ...) 应执行成功");
    assert!(
        out.trim().is_empty(),
        "无 unwrap 调用应空输出，实际: {out:?}"
    );
}

#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_rs_directory_verify_passes() {
    // 目录模式 --verify：暴力路径与优化路径结果应一致（PASS）
    let dir = tempdir().expect("创建临时目录失败");
    fs::write(
        dir.path().join("a.rs"),
        "fn main() {\n    let x = Some(1).unwrap();\n}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("b.rs"),
        "fn other() {\n    let y = Ok(2).unwrap();\n}\n",
    )
    .unwrap();

    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args_dir("ast(rs, \"$OBJ.unwrap()\")", dir.path().to_str().unwrap());
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(out.contains("PASS"), "verify 应输出 PASS，实际: {out}");
    assert!(!out.contains("FAIL"), "verify 不应有 FAIL，实际: {out}");
}

/// 修复：`ast(...) && "literal"` 暴力路径假阳性 bug。
///
/// 修复前：`run_verify` 暴力路径在 `needs_l3=true` 时直接调 `eval_prepared_with_ctx`，
/// 该函数把 `PreparedPrim::Literal` 当永真（不检查字面量是否在行中出现），
/// 导致 `ast(ts, "ref($A)") && "dict"` 对 `ref(initialData)` 行（无 "dict"）假阳性。
/// 修复后：暴力路径先过 `l1_match_no_prefix` 正则粗筛（字面量匹配），再走 L2/L3 求值。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_literal_no_false_positive() {
    let src = "const x = ref('hello');\n\
               const y = ref('world');\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args(
        "ast(ts, \"ref($A)\") && \"nonexistent\"",
        f.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    // 暴力路径不应有假阳性：ref() 命中但 "nonexistent" 不在行中 → 0 行命中 → PASS
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（ast 命中但 literal 不匹配 → 0 行），实际: {out}"
    );
    assert!(
        !out.contains("FAIL"),
        "verify 不应 FAIL（暴力路径不应有假阳性），实际: {out}"
    );
}

// ===== §8.5.1 --verify 覆盖矩阵待补场景 =====
//
// 场景 A-G 按 Rust 方案 §8.5.1 盲区矩阵优先级排序。
// 修复了 `ast(...) && "literal"` 暴力路径假阳性，但只测了负向路径
//（ast 命中但 literal 不匹配）。以下场景覆盖正向路径、目录/stdin 变体、
// needs_l2+needs_l3 同时为真、OR 语义、Not 三态取反、多 ast 节点。

/// 场景 A：`ast(...) && "literal"` 正向命中路径。
///
/// 验证 brute vs opt 在**有命中行**时一致（不只测"不命中"）。
/// 构造 source：一行同时含 ast 命中和 literal 文本 → 应命中 1 行。
/// 如果暴力路径有 bug（Literal 永真导致多找），或优化路径有 bug（漏找），
/// brute != opt → FAIL。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_literal_positive_path() {
    // 第 1 行：ref('dict') —— ast 命中 ref($A) + literal "dict" 命中 → 应命中
    // 第 2 行：ref('other') —— ast 命中 ref($A) + literal "dict" 不匹配 → 不应命中
    let src = "const x = ref('dict');\n\
               const y = ref('other');\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args(
        "ast(ts, \"ref($A)\") && \"dict\"",
        f.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（brute == opt，第 1 行命中），实际: {out}"
    );
    assert!(
        !out.contains("FAIL"),
        "verify 不应 FAIL（暴力路径不应有假阳性），实际: {out}"
    );
    // 确认命中 1 行（PASS [1 行命中]）
    assert!(out.contains("1 行命中"), "应命中 1 行，实际: {out}");
}

/// 场景 B：`ast(...) && "literal"` × 目录模式。
///
/// 修复在 `run_verify` 层，目录模式逐文件调 `run_verify`。
/// 此测试验证目录级 ast+literal --verify 回归：多文件中部分命中、部分不命中。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_literal_directory() {
    let dir = tempdir().expect("创建临时目录失败");
    // a.ts：ref('dict') → ast 命中 + literal "dict" 命中 → 应命中
    fs::write(dir.path().join("a.ts"), "const x = ref('dict');\n").unwrap();
    // b.ts：ref('other') → ast 命中 + literal "dict" 不匹配 → 不应命中
    fs::write(dir.path().join("b.ts"), "const y = ref('other');\n").unwrap();
    // c.rs：不含 ref() → ast 不命中 → 不应命中
    fs::write(dir.path().join("c.rs"), "fn main() { let x = 1; }\n").unwrap();

    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args_dir(
        "ast(ts, \"ref($A)\") && \"dict\"",
        dir.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(out.contains("PASS"), "所有文件 verify 应 PASS，实际: {out}");
    assert!(
        !out.contains("FAIL"),
        "不应有 FAIL（暴力路径不应有假阳性），实际: {out}"
    );
}

/// 场景 C：`ast(...) && "literal"` × stdin。
///
/// stdin 走 `dispatch_with_content` → `run_gxp_mode_with_content` → `run_verify`，
/// 与文件路径相同。此测试覆盖 stdin 输入源的 ast+literal --verify。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_literal_stdin() {
    // 第 1 行：ref('target') —— ast 命中 + literal "target" 命中 → 应命中
    // 第 2 行：ref('other') —— ast 命中 + literal "target" 不匹配 → 不应命中
    let content = "const x = ref('target');\n\
                   const y = ref('other');\n";
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let fc = FileContent::Vec(content.as_bytes().to_vec());
    dispatch_with_content(
        "ast(ts, \"ref($A)\") && \"target\"",
        "<stdin>",
        opts,
        fc,
        &mut buf,
    )
    .expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(out.contains("PASS"), "stdin verify 应 PASS，实际: {out}");
    assert!(!out.contains("FAIL"), "stdin verify 不应 FAIL，实际: {out}");
    assert!(out.contains("1 行命中"), "应命中 1 行，实际: {out}");
}

/// 场景 D：`ast(...) && cost_ms>800`。
///
/// `needs_l2=true && needs_l3=true` 同时为真——暴力路径 L2 字段抽取 + L3 集合判定
/// 同时发生，从未测过。
/// 构造 source：logfmt 行 `cost_ms=900 ... ref('data')` 同时满足数值谓词和 ast。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_field_predicate() {
    // 第 1 行：cost_ms=900 + ref('data') → L2 cost_ms>800 命中 + L3 ast 命中 → 应命中
    // 第 2 行：cost_ms=100 + ref('data') → L2 不命中（100<800）+ L3 ast 命中 → 不应命中
    // 第 3 行：cost_ms=900 + ref('other') → L2 命中 + L3 ast 命中（ref($A) 匹配任意 ref 调用）→ 应命中
    // 注：ast(ts, "ref($A)") 匹配 ref(X) 调用——ref 是变量名不是函数调用
    // 需要构造真正的函数调用形式：ref(...) 作为 ast-grep pattern
    let src = "cost_ms=900 msg=ref('data')\n\
               cost_ms=100 msg=ref('data')\n\
               cost_ms=900 msg=ref('other')\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args(
        "ast(ts, \"ref($A)\") && cost_ms>800",
        f.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（needs_l2+needs_l3 同时为真，brute == opt），实际: {out}"
    );
    assert!(!out.contains("FAIL"), "verify 不应 FAIL，实际: {out}");
}

/// 场景 E：`ast(...) || "error"`。
///
/// OR 改变 eval 语义：`Literal` 在 OR 侧返回 `True`（设计正确——L1 已做正则粗筛），
/// 但 L1 对 OR 的字面量收集是合并两侧到同一 required group。
/// 如果一侧是 ast()（无字面量），另一侧的 literal 形成 required group，
/// 只含 literal 的行过 L1，ast-only 行被 L1 过滤（L1 ⊇ L2 对 ast+OR 的边界）。
/// 此测试验证 brute == opt（两条路径对 OR 的处理一致）。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_or_literal() {
    // 第 1 行：ref('hello') → ast 命中，literal "error" 不在行中 → L1 过滤（无 "error"），两条路径都不命中
    // 第 2 行：error: timeout → literal "error" 命中 → L1 通过，eval OR(Ast, Literal) → True → 命中
    // 第 3 行：ref('world') + error → ast 命中 + literal 命中 → L1 通过，OR → True → 命中
    let src = "const x = ref('hello');\n\
               error: timeout retrying\n\
               const z = ref('world') + error\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args(
        "ast(ts, \"ref($A)\") || \"error\"",
        f.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（brute == opt 对 OR 语义一致），实际: {out}"
    );
    assert!(!out.contains("FAIL"), "verify 不应 FAIL，实际: {out}");
}

/// 场景 F：`!(ast(...))`。
///
/// Not 三态取反 × ast：`Not` 在 eval_prepared_with_ctx 中对 `EvalResult::True → False`、
/// `False | NotNumeric → True`。ast 命中 → True → Not → False（不命中）；
/// ast 不命中 → False → Not → True（命中）。
/// L1 对 Not 子树不下推字面量（保守不漏），Not 子树的 Pred 仍收集字段名前缀。
/// 此测试验证 brute == opt 对 Not+ast 的处理一致。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_not_ast() {
    // 第 1 行：ref('hello') → ast 命中 → Not → False → 不命中
    // 第 2 行：const x = 1 → ast 不命中 → Not → True → 命中
    // 第 3 行：ref('world') → ast 命中 → Not → False → 不命中
    let src = "const x = ref('hello');\n\
               const y = 1;\n\
               const z = ref('world');\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args("!(ast(ts, \"ref($A)\"))", f.path().to_str().unwrap());
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（Not 三态取反 × ast，brute == opt），实际: {out}"
    );
    assert!(!out.contains("FAIL"), "verify 不应 FAIL，实际: {out}");
}

/// 场景 G：`ast(...) && ast(...)`。
///
/// 多 ast 节点同语言 `run_all_ast` root 复用——暴力路径与优化路径
/// 对多 ast 节点的集合判定一致性。
/// 构造 source：同一行同时匹配两个 ast pattern。
#[cfg(feature = "l3-structural")]
#[test]
fn test_verify_ast_and_ast() {
    // 第 1 行：ref('data') + useRef(null) → 两个 ast pattern 都命中 → AND → True → 命中
    // 第 2 行：ref('data') → 只命中第一个 pattern → AND → False → 不命中
    // 第 3 行：useRef(null) → 只命中第二个 pattern → AND → False → 不命中
    let src = "const x = ref('data') && useRef(null);\n\
               const y = ref('data');\n\
               const z = useRef(null);\n";
    let f = write_temp(src);
    let opts = RunOpts {
        verify: true,
        ..Default::default()
    };
    let mut buf: Vec<u8> = Vec::new();
    let arg_list = args(
        "ast(ts, \"ref($A)\") && ast(ts, \"useRef($A)\")",
        f.path().to_str().unwrap(),
    );
    run_with_writer(&arg_list, opts, &mut buf).expect("verify 应执行成功");
    let out = String::from_utf8(buf).expect("输出应为 UTF-8");
    assert!(
        out.contains("PASS"),
        "verify 应 PASS（多 ast 节点 brute == opt），实际: {out}"
    );
    assert!(!out.contains("FAIL"), "verify 不应 FAIL，实际: {out}");
}

// ===== LLM 友好性 e2e：大小写不敏感 + c++/C++ 别名 + fail-loud =====
//
// P0：lang_from_str 大小写不敏感（Java/JAVA/Python/TypeScript 等）
// P1：parser NAME 接受 `+`（ast(c++, ...) / ast(C++, ...)）
// P2：ast() 单引号/无引号 pattern → fail-loud 报错（不静默落 BARE）

/// P0：ast(Java, ...) 大写 J 应与 ast(java, ...) 行为一致。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_java_uppercase_matches() {
    let src = "public class Main {\n\
               public static void main(String[] args) {\n\
               System.out.println(\"hello\");\n\
               }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(Java, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(Java, ...) 应执行成功（大小写不敏感）");
    assert!(
        out.contains("System.out.println"),
        "大写 Java 应与小写 java 行为一致，实际: {out}"
    );
}

/// P0：ast(JAVA, ...) 全大写应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_java_all_caps_matches() {
    let src = "public class Main {\n\
               public static void main(String[] args) {\n\
               System.out.println(\"hello\");\n\
               }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(JAVA, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(JAVA, ...) 应执行成功（大小写不敏感）");
    assert!(
        out.contains("System.out.println"),
        "全大写 JAVA 应工作，实际: {out}"
    );
}

/// P0：ast(Python, ...) 全拼应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_python_full_name_matches() {
    let src = "def main():\n    print(\"hello\")\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(Python, \"print($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(Python, ...) 应执行成功（全拼）");
    assert!(out.contains("print"), "全拼 Python 应工作，实际: {out}");
}

/// P0：ast(TypeScript, ...) 全拼应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_typescript_full_name_matches() {
    let src = "const x = ref('hello');\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(TypeScript, \"ref($A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(TypeScript, ...) 应执行成功（全拼）");
    assert!(out.contains("ref"), "全拼 TypeScript 应工作，实际: {out}");
}

/// P1：ast(c++, ...) NAME 含 `+` 应解析成功并执行。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_plus_alias_matches() {
    let src = "namespace mylib {\nint add(int a, int b) { return a + b; }\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(c++, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(c++, ...) 应执行成功（NAME 含 +）");
    assert!(
        out.contains("namespace mylib"),
        "c++ 别名应与 cpp 行为一致，实际: {out}"
    );
}

/// P1：ast(C++, ...) 大写 + 加号应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_uppercase_plus_matches() {
    let src = "namespace mylib {\nint add(int a, int b) { return a + b; }\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(C++, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(C++, ...) 应执行成功（大写 + 加号）");
    assert!(
        out.contains("namespace mylib"),
        "C++ 大写+加号应工作，实际: {out}"
    );
}

/// P1：ast(CPP, ...) 全大写应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_all_caps_matches() {
    let src = "namespace mylib {\nint add(int a, int b) { return a + b; }\n}\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(CPP, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(CPP, ...) 应执行成功（全大写）");
    assert!(
        out.contains("namespace mylib"),
        "CPP 全大写应工作，实际: {out}"
    );
}

/// P1：ast(cxx, ...) / ast(cc, ...) 别名应工作。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_cpp_cxx_cc_aliases_match() {
    let src = "namespace mylib {\nint add(int a, int b) { return a + b; }\n}\n";
    let f = write_temp(src);
    // cxx
    let out_cxx = run(&args(
        "ast(cxx, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(cxx, ...) 应执行成功");
    assert!(
        out_cxx.contains("namespace mylib"),
        "cxx 别名应工作，实际: {out_cxx}"
    );
    // cc
    let out_cc = run(&args(
        "ast(cc, \"namespace $NS { $$$B }\")",
        f.path().to_str().unwrap(),
    ))
    .expect("ast(cc, ...) 应执行成功");
    assert!(
        out_cc.contains("namespace mylib"),
        "cc 别名应工作，实际: {out_cc}"
    );
}

/// P2：ast(java, 'pattern') 单引号 → fail-loud 报错。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_single_quote_fails_loud() {
    let src = "public class Main {\n}\n";
    let f = write_temp(src);
    let result = run(&args(
        "ast(java, 'System.out.println($$$A)')",
        f.path().to_str().unwrap(),
    ));
    let Err(e) = result else {
        panic!("单引号 pattern 应 fail-loud 报错，实际成功返回");
    };
    let msg = e.to_string();
    assert!(
        msg.contains("ast() 语法错误"),
        "错误消息应含 ast() 语法错误提示，实际: {msg}"
    );
    assert!(
        msg.contains("双引号"),
        "错误消息应提示使用双引号，实际: {msg}"
    );
}

/// P2：ast(java, pattern) 无引号 → fail-loud 报错。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_no_quote_fails_loud() {
    let src = "public class Main {\n}\n";
    let f = write_temp(src);
    let result = run(&args(
        "ast(java, System.out.println)",
        f.path().to_str().unwrap(),
    ));
    let Err(e) = result else {
        panic!("无引号 pattern 应 fail-loud 报错，实际成功返回");
    };
    let msg = e.to_string();
    assert!(
        msg.contains("ast() 语法错误"),
        "错误消息应含 ast() 语法错误提示，实际: {msg}"
    );
}

/// P2：capture(cost_ms, 'regex') 单引号 → fail-loud 报错。
#[cfg(feature = "l3-structural")]
#[test]
fn test_capture_single_quote_fails_loud() {
    let src = "cost_ms=900\n";
    let f = write_temp(src);
    let result = run(&args(
        "capture(cost_ms, '(\\d+)')",
        f.path().to_str().unwrap(),
    ));
    let Err(e) = result else {
        panic!("capture 单引号应 fail-loud 报错，实际成功返回");
    };
    let msg = e.to_string();
    assert!(
        msg.contains("capture() 语法错误"),
        "错误消息应含 capture() 语法错误提示，实际: {msg}"
    );
}

/// P2 回归：正常双引号 ast() 不受 fail-loud 改动影响。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_normal_double_quote_regression() {
    let src = "public class Main {\n\
               public static void main(String[] args) {\n\
               System.out.println(\"hello\");\n\
               }\n\
               }\n";
    let f = write_temp(src);
    let out = run(&args(
        "ast(java, \"System.out.println($$$A)\")",
        f.path().to_str().unwrap(),
    ))
    .expect("正常双引号 ast() 不受 fail-loud 改动影响");
    assert!(
        out.contains("System.out.println"),
        "正常双引号 ast 应正常工作，实际: {out}"
    );
}

/// P2 回归：BARE 模式不以 ast( 开头时不受影响。
#[cfg(feature = "l3-structural")]
#[test]
fn test_bare_mode_unaffected_by_fail_loud() {
    let src = "some error line\n";
    let f = write_temp(src);
    let out = run(&args("error", f.path().to_str().unwrap())).expect("BARE 模式应正常工作");
    assert!(out.contains("error"), "BARE 模式不受影响，实际: {out}");
}

/// P2 回归：ast 大写开头但不是 ast()（如 "asthma"）不误报。
/// "asthma" 不以 "ast(" 开头，应走 BARE 模式正常匹配。
#[cfg(feature = "l3-structural")]
#[test]
fn test_ast_prefix_not_false_positive() {
    let src = "asthma is a condition\n";
    let f = write_temp(src);
    let out = run(&args("asthma", f.path().to_str().unwrap())).expect("BARE 'asthma' 应正常匹配");
    assert!(
        out.contains("asthma"),
        "'asthma' 应走 BARE 模式正常匹配，实际: {out}"
    );
}
