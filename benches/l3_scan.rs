//! L3 ast() 查询基准（度量 ast-grep find_all 在六语言源码上的延迟）。
//!
//! 用 gxp 自身源码（src/）作为测试语料，对六语言（ts/rs/py/js/go/tsx）
//! 分别跑典型 ast() pattern，度量 `run_single` 延迟。
//!
//! 运行：`cargo bench --bench l3_scan --features l3-structural`

use std::fs;
use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use gxp::l3::structural::{run_single, L3Query};

use ast_grep_language::SupportLang;

/// 获取 gxp 源码文件作为基准语料。
fn src_file(name: &str) -> (PathBuf, Vec<u8>) {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("src");
    p.push(name);
    let content = fs::read(&p).unwrap_or_else(|e| panic!("读取 {name} 失败: {e}"));
    (p, content)
}

/// 六语言基准用例：每语言一个 (lang, pattern, source_file) 三元组。
///
/// source_file 用 gxp 自身源码或内联合成代码（语言不匹配时用合成片段）。
struct L3BenchCase {
    /// 语言标签（用于 BenchmarkId）
    label: &'static str,
    /// ast-grep SupportLang
    lang: SupportLang,
    /// ast-grep pattern
    pattern: &'static str,
    /// 源文件内容
    content: Vec<u8>,
}

/// 构建六语言基准用例。
///
/// - rs: 用 gxp 自身 `src/lib.rs`（最大的 Rust 文件）
/// - ts/js/tsx: 合成片段（gxp 无 TS/JS 源码）
/// - py: 合成片段
/// - go: 合成片段
fn build_cases() -> Vec<L3BenchCase> {
    let (_, rs_content) = src_file("lib.rs");

    // 合成 TS 代码（~1000 行级，有 useQuery 调用）
    let ts_src = (0..500)
        .map(|i| {
            format!(
                "import {{ useQuery }} from 'react-query';\n\
                 function Comp{i}() {{\n\
                 const data = useQuery(['key{i}'], fetcher);\n\
                 return data;\n\
                 }}\n"
            )
        })
        .collect::<Vec<String>>()
        .join("");

    // 合成 JS 代码（~1000 行级，有 fetch 调用）
    let js_src = (0..500)
        .map(|i| {
            format!(
                "async function load{i}() {{\n\
                 const res = await fetch('/api/{i}');\n\
                 return res.json();\n\
                 }}\n"
            )
        })
        .collect::<Vec<String>>()
        .join("");

    // 合成 TSX 代码（~1000 行级，有 JSX + useQuery）
    let tsx_src = (0..500)
        .map(|i| {
            format!(
                "import {{ useQuery }} from 'react-query';\n\
                 function Widget{i}() {{\n\
                 const data = useQuery(['key{i}'], fetcher);\n\
                 return <div>{{data}}</div>;\n\
                 }}\n"
            )
        })
        .collect::<Vec<String>>()
        .join("");

    // 合成 Python 代码（~1000 行级，有 requests.get 调用）
    let py_src = (0..500)
        .map(|i| {
            format!(
                "import requests\n\
                 def fetch{i}():\n\
                 resp = requests.get('https://api.example.com/{i}')\n\
                 return resp.status_code\n"
            )
        })
        .collect::<Vec<String>>()
        .join("");

    // 合成 Go 代码（~1000 行级，有 fmt.Println 调用）
    let go_src = (0..500)
        .map(|i| {
            format!(
                "package main\n\
                 import \"fmt\"\n\
                 func handler{i}() {{\n\
                 fmt.Println(\"handler {i}\")\n\
                 }}\n"
            )
        })
        .collect::<Vec<String>>()
        .join("");

    vec![
        L3BenchCase {
            label: "rs_unwrap",
            lang: SupportLang::Rust,
            pattern: "$OBJ.unwrap()",
            content: rs_content.clone(),
        },
        L3BenchCase {
            label: "rs_if_let",
            lang: SupportLang::Rust,
            pattern: "if let $P = $E { $$$B }",
            content: rs_content,
        },
        L3BenchCase {
            label: "ts_usequery",
            lang: SupportLang::TypeScript,
            pattern: "useQuery($$$A)",
            content: ts_src.as_bytes().to_vec(),
        },
        L3BenchCase {
            label: "js_fetch",
            lang: SupportLang::JavaScript,
            pattern: "fetch($$$A)",
            content: js_src.as_bytes().to_vec(),
        },
        L3BenchCase {
            label: "tsx_jsx",
            lang: SupportLang::Tsx,
            pattern: "useQuery($$$A)",
            content: tsx_src.as_bytes().to_vec(),
        },
        L3BenchCase {
            label: "py_requests",
            lang: SupportLang::Python,
            pattern: "requests.get($$$A)",
            content: py_src.as_bytes().to_vec(),
        },
        L3BenchCase {
            label: "go_println",
            lang: SupportLang::Go,
            pattern: "fmt.Println($$$A)",
            content: go_src.as_bytes().to_vec(),
        },
    ]
}

/// 度量 `run_single` 在六语言上的延迟。
fn bench_l3_run_single(c: &mut Criterion) {
    let cases = build_cases();

    let mut g = c.benchmark_group("l3_scan");
    g.sample_size(20);
    g.measurement_time(std::time::Duration::from_secs(5));

    for case in &cases {
        g.throughput(Throughput::Bytes(case.content.len() as u64));
        g.bench_with_input(
            BenchmarkId::new("run_single", case.label),
            &case,
            |b, case| {
                let q = L3Query {
                    language: case.lang,
                    pattern: case.pattern.to_string(),
                };
                b.iter(|| {
                    let lines = run_single(black_box(&q), black_box(&case.content))
                        .expect("ast-grep run_single 应成功");
                    black_box(lines);
                });
            },
        );
    }

    g.finish();
}

/// 度量 pattern 编译开销（Pattern::try_new + AstGrep::new 解析），
/// 与 `run_single` 的 find_all 阶段分离。
fn bench_l3_parse_only(c: &mut Criterion) {
    let cases = build_cases();

    let mut g = c.benchmark_group("l3_parse");
    g.sample_size(20);
    g.measurement_time(std::time::Duration::from_secs(5));

    for case in &cases {
        g.throughput(Throughput::Bytes(case.content.len() as u64));
        g.bench_with_input(
            BenchmarkId::new("parse_only", case.label),
            &case,
            |b, case| {
                b.iter(|| {
                    use ast_grep_core::{AstGrep, Pattern};
                    let src = std::str::from_utf8(&case.content).unwrap();
                    let pattern = Pattern::try_new(case.pattern, case.lang).unwrap();
                    let grep = AstGrep::new(src, case.lang);
                    black_box(pattern);
                    black_box(grep.root());
                });
            },
        );
    }

    g.finish();
}

criterion_group!(benches, bench_l3_run_single, bench_l3_parse_only);
criterion_main!(benches);
