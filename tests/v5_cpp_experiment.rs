//! V5 验证实验：ast-grep 对 C/C++ 的预处理行为。
//!
//! 关键问题：C/C++ 的 `#include` 宏展开后 AST 行号是否与源码对应？
//! ast-grep 解析预处理前还是后？gxp 不做编译无法预处理。
//!
//! 实验方法：用 ast-grep 对含 #include/#define/#ifdef 的真实 C/C++ 代码跑 find_all，
//! 观察返回行号是否对应源码行。
//!
//! V5 结论（2026-10-23）：
//! - ast-grep 对 C/C++ **不做预处理**（不展开 #include/#define/#ifdef）
//! - AST 行号**准确对应源码行**
//! - C/C++ function call pattern 需带分号（`printf($$$A);`），与 Go tree-sitter 类似
//! - C function definition pattern 需带函数体大括号（`int $F() { $$$B }`）
//! - V5 通过 → C/C++ L3 可做
//!
//! 只在 `l3-structural` feature 开启时编译（依赖 ast-grep crate）。

#![cfg(feature = "l3-structural")]

#[cfg(test)]
mod tests {
    use ast_grep_core::{AstGrep, Pattern};
    use ast_grep_language::SupportLang;

    fn find_lines(lang: SupportLang, src: &[u8], pattern_str: &str) -> Vec<usize> {
        let src_str = std::str::from_utf8(src).expect("src 应为 UTF-8");
        let pattern = Pattern::try_new(pattern_str, lang).expect("pattern 应编译成功");
        let grep = AstGrep::new(src_str, lang);
        let root = grep.root();
        let mut lines: Vec<usize> = Vec::new();
        for nm in root.find_all(pattern) {
            let range = nm.range();
            let line_no = count_lines(&src[..range.start]) + 1;
            lines.push(line_no);
        }
        lines.sort();
        lines.dedup();
        lines
    }

    fn count_lines(bytes: &[u8]) -> usize {
        memchr::memchr_iter(b'\n', bytes).count()
    }

    // ===== 实验 1：简单 C 代码（无预处理） =====

    #[test]
    fn test_v5_simple_c_return_matches_line() {
        let c_src = b"int main() {\n    int x = 1;\n    return x;\n}\n";
        let lines = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(lines.contains(&3), "return 应命中第 3 行，实际: {lines:?}");
    }

    // ===== 实验 2：含 #include + #define =====
    // 关键验证：预处理器指令是否导致行号偏移

    #[test]
    fn test_v5_c_with_includes_return_matches_line() {
        let c_src = b"#include <stdio.h>\n\
                       #define MAX 100\n\
                       int main() {\n\
                       printf(\"hello\");\n\
                       return 0;\n\
                       }\n";
        let lines = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(
            lines.contains(&5),
            "return 应命中第 5 行（#include/#define 不偏移行号），实际: {lines:?}"
        );
    }

    #[test]
    fn test_v5_c_with_includes_printf_matches_line() {
        let c_src = b"#include <stdio.h>\n\
                       #define MAX 100\n\
                       int main() {\n\
                       printf(\"hello\");\n\
                       return 0;\n\
                       }\n";
        // C tree-sitter：function call 是 statement-level node，需带分号
        let lines = find_lines(SupportLang::C, c_src, "printf($$$A);");
        assert!(lines.contains(&4), "printf 应命中第 4 行，实际: {lines:?}");
    }

    // ===== 实验 3：含 #ifdef / #endif 条件编译 =====

    #[test]
    fn test_v5_c_ifdef_does_not_shift_lines() {
        let c_src = b"#ifdef DEBUG\n\
                       int debug_var = 1;\n\
                       #endif\n\
                       int main() {\n\
                       return 0;\n\
                       }\n";
        let lines = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(
            lines.contains(&5),
            "#ifdef 块不偏移行号，return 应在第 5 行，实际: {lines:?}"
        );
    }

    // ===== 实验 4：C++ 含模板 + namespace =====

    #[test]
    fn test_v5_cpp_namespace_matches_line() {
        let cpp_src = b"#include <iostream>\n\
                         namespace mylib {\n\
                         template <typename T>\n\
                         T add(T a, T b) { return a + b; }\n\
                         }\n";
        let lines = find_lines(SupportLang::Cpp, cpp_src, "namespace $NS { $$$B }");
        assert!(
            lines.contains(&2),
            "namespace 应命中第 2 行，实际: {lines:?}"
        );
    }

    #[test]
    fn test_v5_cpp_return_in_template_matches_line() {
        let cpp_src = b"#include <iostream>\n\
                         namespace mylib {\n\
                         template <typename T>\n\
                         T add(T a, T b) { return a + b; }\n\
                         }\n";
        let lines = find_lines(SupportLang::Cpp, cpp_src, "return $A + $B;");
        assert!(lines.contains(&4), "return 应命中第 4 行，实际: {lines:?}");
    }

    // ===== 实验 5：宏调用 vs 函数调用 =====

    #[test]
    fn test_v5_c_macro_call_line_mapping() {
        let c_src = b"#define MY_MACRO(x) (x * 2)\n\
                        int main() {\n\
                        int y = MY_MACRO(5);\n\
                        printf(\"%d\", y);\n\
                        return 0;\n\
                        }\n";
        let lines = find_lines(SupportLang::C, c_src, "printf($$$A);");
        assert!(lines.contains(&4), "printf 应命中第 4 行，实际: {lines:?}");
        let lines2 = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(
            lines2.contains(&5),
            "return 应命中第 5 行，实际: {lines2:?}"
        );
    }

    // ===== 实验 6：多行 #define 宏（反斜杠续行） =====

    #[test]
    fn test_v5_c_multiline_define_no_line_shift() {
        let c_src = b"#define LONG_MACRO(x) \\\n\
                        do { \\\n\
                            x + 1; \\\n\
                        } while(0)\n\
                        \n\
                        int main() {\n\
                        return 0;\n\
                        }\n";
        let lines = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(
            lines.contains(&7),
            "多行 #define 不展开，return 应在第 7 行，实际: {lines:?}"
        );
    }

    // ===== 实验 7：include 未展开——所有行号对应源码 =====

    #[test]
    fn test_v5_c_include_not_expanded() {
        let c_src = b"#include <stdio.h>\n\
                        #include <stdlib.h>\n\
                        int main() {\n\
                        printf(\"hello\");\n\
                        return 0;\n\
                        }\n";
        // function definition 需带函数体（C tree-sitter 特性）
        let lines_main = find_lines(SupportLang::C, c_src, "int main() { $$$B }");
        assert!(
            lines_main.contains(&3),
            "main 应在第 3 行（include 未展开），实际: {lines_main:?}"
        );

        let lines_printf = find_lines(SupportLang::C, c_src, "printf($$$A);");
        assert!(
            lines_printf.contains(&4),
            "printf 应在第 4 行（include 未展开），实际: {lines_printf:?}"
        );

        let lines_ret = find_lines(SupportLang::C, c_src, "return $X;");
        assert!(
            lines_ret.contains(&5),
            "return 应在第 5 行（include 未展开），实际: {lines_ret:?}"
        );
    }

    // ===== 实验 8：C++ 类定义 =====

    #[test]
    fn test_v5_cpp_class_method_matches_line() {
        let cpp_src = b"#include <string>\n\
                         class Foo {\n\
                         public:\n\
                         int bar() { return 0; }\n\
                         };\n";
        let lines = find_lines(SupportLang::Cpp, cpp_src, "return $X;");
        assert!(lines.contains(&4), "return 应在第 4 行，实际: {lines:?}");
    }

    // ===== 实验 9：C 头文件 .h =====

    #[test]
    fn test_v5_c_header_file_matches() {
        let h_src = b"#pragma once\n\
                       #define VERSION 1\n\
                       \n\
                       int get_version();\n";
        // C tree-sitter：function declaration 需带分号
        let lines = find_lines(SupportLang::C, h_src, "int $F();");
        assert!(
            lines.contains(&4),
            "int get_version() 应在第 4 行，实际: {lines:?}"
        );
    }
}
