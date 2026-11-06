//! L2 精判：字段抽取 + 数值比较（设计文档 §3.3）。
//!
//! 11 维数值字段格式契约（D5）：支持整数/浮点/负数/前导零/正号/科学计数法；
//! 拒绝千分位/单位/空白 trim/空值/布尔/NaN。
//!
//! ## 4 种格式支持（设计文档 §3.3）
//!
//! - logfmt：`k=v` / `k:v`（手写状态机 + memchr）
//! - JSON line：`{"k": v}` / `[{...}]`（memchr 直扫，零拷贝）
//! - `[tag]`：方括号标签，伪字段 `_tag`
//! - RFC 3339 时间戳：伪字段 `_ts`
//!
//! ## 子模块
//!
//! - `extract`：字段抽取（4 种格式 + 零拷贝视图）
//! - `format`：格式探测（LineFormat + detect_format）
//! - `numeric`：11 维数值解析与比较
//! - `time`：ISO 8601 时间戳解析与比较（speedate）

pub mod extract;
pub mod format;
pub mod numeric;
pub mod time;

pub use extract::{ExtractedFields, PSEUDO_TAG, PSEUDO_TIMESTAMP};
pub use format::LineFormat;
pub use numeric::{
    build_between_bounds, eval_between, eval_exists, eval_in, eval_numeric, parse_numeric_value,
    BetweenBoundError, EvalResult,
};
pub use time::{eval_time_between, parse_timestamp};
