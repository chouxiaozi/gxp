//! DSL：gxp 查询表达式的 AST、nom 解析、管道 tokenizer。
//!
//! 设计文档 §2.1-2.2（方案 D' 语法）：
//! - 邻接 = 字面空格（非隐式 AND）
//! - `&&` / `||` / `!` 为 shell 风格操作符
//! - STRING 强制双引号
//! - 管道动词：`context N` / `files` / `count` / `head N` / `fields a,b` / `rank top K by fld [desc]`
//!
//! 子模块：
//! - `ast`：EBNF AST
//! - `parser`：nom 解析（含 `!` 词法规则，Rust 方案 §7.5）
//! - `pipeline`：手写管道 tokenizer（`context`/`files`，§7.6）

pub mod ast;
pub mod parser;
pub mod pipeline;
