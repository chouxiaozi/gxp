//! 模式判定（BARE / gxp）。
//!
//! 方案 E（设计文档 §1.5）：单一 gxp 模式，取消模式判定 Halting Problem。
//! 入口扫描 argv 决定 BARE（等价 `grep -E`）或 gxp DSL。
//!
//! 子模块：
//! - `detect`：入口解析 + `PreparedScan` + `detect_and_dispatch`
//! - `prepared`：L2 谓词预编译产物（`PreparedExpr`/`PreparedPred`/`PreparedPrim`）
//! - `eval`：L2 求值（`eval_prepared_with_captures`/`EvalCtx`）
//! - `strict`：`--strict` 字段探活
//! - `bare`：BARE 模式执行（ERE 兼容）
//! - `gxp`：gxp 模式执行（DSL parse → compile → scan → L2 精判）
//! - `verify`：`--verify` 暴力对账
//! - `reject_grep_opts`：四段式报错（覆盖 11 个 grep 短选项）

pub mod bare;
pub mod detect;
pub mod eval;
pub mod gxp;
pub mod prepared;
pub mod reject_grep_opts;
pub mod strict;
pub mod verify;

pub use detect::detect_and_dispatch;
