//! gxp 库根：grep-aligned query language for LLM agents.
//!
//! gxp **不是** grep 的严格超集（见 AGENTS.md §0、设计文档 §0/§1.4）。
//! 不接受 grep 短选项；等价功能用原生语法（`context N` / `files` / `count` / `fields`）。
//! BARE 模式与 `grep -E` 行为一致（`gxp 'pattern' file` ≡ `grep -E 'pattern' file`）。
//!
//! 模块组成（见 Rust 方案 §14）：
//! - `cli` / `mode`：CLI 入口与 BARE/gxp 模式判定
//! - `dsl`：EBNF AST + nom 解析 + 管道 tokenizer
//! - `l1` / `l2` / `l3`：三层查询执行
//! - `dir` / `io`：目录索引与文件读取
//! - `pipeline`：查询后动词（rank / fields / head / count / context / files）
//! - `plan`：ExecutionPlan（`--explain`）
//! - `error`：四段式错误模板（设计文档 §1.5）

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
// 废弃代码 + 不可达路径（机器确定性 lint，对所有提交者一视同仁）：
// - dead_code：同 crate 内私有未使用项（pub API 不受影响，rustc 视为对外可能被用）
// - unreachable_pub：标 `pub` 但无对应路径可达的项（catch 模块重构后的孤儿导出）
// - unreachable_patterns：match 中永不会命中的模式分支
// 跨 crate 未使用 pub API 无成熟社区工具，单 crate 下不是问题；语义层伪断言靠
// cargo-mutants + AGENTS.md §6.5 agent 规范兜底（见 feedback_gate_config_adapts_to_context.md）
#![deny(dead_code)]
#![deny(unreachable_pub)]
#![deny(unreachable_patterns)]

pub mod cli;
pub mod dir;
pub mod dsl;
pub mod error;
pub mod io;
pub mod l1;
pub mod l2;
pub mod l3;
pub mod mode;
pub mod pipeline;
pub mod plan;

/// 错误类型与四段式报错模板（设计文档 §1.5）。
pub use error::{GxpError, Result};
