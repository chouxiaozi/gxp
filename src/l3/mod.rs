//! L3 结构层：ast-grep + tree-sitter（v1 默认不启用，feature `l3-structural`）。
//!
//! v1 边界（AGENTS.md §9）：L3 扩展语言（Swift/Haskell/Erlang）v1 不做，v2 评估。

#[cfg(feature = "l3-structural")]
pub mod structural;
