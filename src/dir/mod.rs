//! 目录层：路径谓词下推剪枝 + 目录指纹校验 + append-only 增量 + commit_key。
//!
//! 移植自 `prototype/gxp_dir.py`（27 项 TDD 测试将移植到 `tests/dir_*.rs`）。
//! Python 原型删除后，本模块承担跨平台职责（Windows + Linux + macOS）。
//!
//! 子模块：
//! - `plan`：PathPlan（路径谓词 + 剪枝）
//! - `index`：DirIndex（lax/strict/off 三模式）
//! - `tail`：TailIndex（单次查询内 append-only 增量）
//! - `git`：commit_key（HEAD + 工作区脏标记）

pub mod git;
pub mod index;
pub mod plan;
pub mod tail;
