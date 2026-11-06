//! 统一错误类型 + 四段式报错模板（设计文档 §1.5）。
//!
//! 四段式结构：
//! 1. **错误标识**：`gxp: error: <类别>` — LLM 可解析的错误分类
//! 2. **人类描述**：发生了什么、为什么
//! 3. **等价语法提示**：如果用户写了 grep 短选项，给出 gxp 原生语法替代
//! 4. **示例可执行**：可直接复制到 shell 的修正示例

use thiserror::Error;

/// gxp 错误类别（四段式的第一段）。
#[derive(Debug, Error)]
pub enum GxpError {
    /// grep 短选项拒绝（设计文档 D1）。
    #[error("gxp: error: invalid grep short option: {raw}\n\n{suggestion}")]
    InvalidGrepOption {
        /// 用户输入的原始选项（如 `-A`）。
        raw: String,
        /// gxp 原生语法等价提示。
        suggestion: String,
    },

    /// DSL 解析错误（设计文档 §2）。
    #[error("gxp: error: parse: {detail}")]
    Parse {
        /// 解析失败原因。
        detail: String,
    },

    /// IO 错误（文件读、readdir 等）。
    ///
    /// 模板含 `{0}` 透传内层 `std::io::Error` Display（如 "No such file or directory"
    /// / "系统找不到指定的文件"），避免用户只看到 "gxp: error: io" 而不知具体原因。
    #[error("gxp: error: io: {0}")]
    Io(#[from] std::io::Error),

    /// 运行时错误（不属于上述类别）。
    #[error("gxp: error: runtime: {detail}")]
    Runtime {
        /// 运行时错误详情。
        detail: String,
    },
}

/// gxp 统一 Result 类型别名。
pub type Result<T> = std::result::Result<T, GxpError>;
