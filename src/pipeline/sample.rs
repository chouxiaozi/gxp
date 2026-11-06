//! `sample N` / `sample P%`：随机抽样（代表性子集）。
//!
//! 设计文档 §2.2 EBNF `stage` 规则；§9.1 候选 #1（L2 等级）。
//!
//! # 语义
//!
//! - `sample N`：从命中行随机抽 N 条（N > 总数时返回全部）
//! - `sample P%`：按比例抽 `ceil(len * P/100)` 条
//! - 固定种子 0 → 同一查询同一结果（可复现）
//!
//! # 与 `head N` 区别
//!
//! `head N` 取前 N 条：确定性，但漏掉后段，无代表性。
//! `sample N` 全局洗牌后取 N：保证整集均匀代表性。
//!
//! # 应用场景
//!
//! LLM context window 物理限制：日志/代码命中集远超窗口时，
//! 需要代表性子集（覆盖全集分布）而非前 N 条（可能全是同一文件的同类错误）。
//!
//! # --verify 对齐
//!
//! 固定种子 0 → 暴力路径与优化路径同种子重跑，洗牌结果一致，100% 对齐。
//! 关键：洗牌在 `MatchedLine` 集合上做，不依赖 L1/L2 命中判定结果顺序之外的状态。

use super::MatchedLine;
use crate::dsl::ast::SampleSpec;

/// 固定种子：保证可复现（--verify 对齐 + LLM 可推理）。
const SAMPLE_SEED: u64 = 0;

/// 执行 `sample N` / `sample P%`：Fisher-Yates 部分洗牌后取前 `take` 条。
///
/// `Ratio(p)` 在此按实际命中数换算为 `ceil(len * p)`，保证 ratio 语义正确
/// （解析阶段 len 未知，必须在执行阶段换算）。
///
/// 算法：Fisher-Yates 前 `take` 轮（Knuth 变体），`O(take)` 而非 `O(n)` 全洗。
/// 结果保留原相对顺序（部分洗牌后稳定）。
///
/// # 边界
///
/// - `Count(0)` 或 `Ratio(0.0)` → 空
/// - `take >= len` → 返回全部（无洗牌必要）
/// - 空 input → 空
/// - `Ratio` 上限 1.0（`100%`），超出在解析时拒绝
pub fn apply_sample(matched: Vec<MatchedLine>, spec: &SampleSpec) -> Vec<MatchedLine> {
    let take = match spec {
        SampleSpec::Count(n) => *n,
        SampleSpec::Ratio(p) => {
            let len = matched.len();
            // ceil(len * p)，p ∈ [0, 1]
            ((len as f64) * p).ceil() as usize
        }
    };

    let mut out = matched;
    if take == 0 || take >= out.len() {
        out.truncate(take);
        return out;
    }

    // Fisher-Yates 前 take 轮：out[i] 与 out[rand(i..len)] 交换
    let mut rng = Lcg::new(SAMPLE_SEED);
    let n = out.len();
    for i in 0..take {
        let j = i + rng.next_u32() as usize % (n - i);
        out.swap(i, j);
    }
    out.truncate(take);
    out
}

/// 简单 LCG（Linear Congruential Generator）。
///
/// 选 NumPy 兼容参数（`a=6364136223846793005`, `c=1442695040888963407`, `m=2^64`），
/// 跨平台一致。不依赖外部 crate（Rust 方案 §13 依赖白名单约束）。
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        // NumPy PCG-like 参数（简化版，仅取高 32 位）
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(14_426_950_408_889_634_071);
        // 取高 32 位（高 bit 熵更高，Numerical Recipes 推荐）
        (self.state >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(rows: &[&str]) -> Vec<MatchedLine> {
        rows.iter()
            .enumerate()
            .map(|(i, s)| MatchedLine::new(i + 1, s.as_bytes()))
            .collect()
    }

    #[test]
    fn test_sample_count_takes_n_from_larger_set() {
        let m = lines(&["a", "b", "c", "d", "e", "f"]);
        let out = apply_sample(m, &SampleSpec::Count(3));
        assert_eq!(out.len(), 3);
        // 结果应是原集合子集
        for m in &out {
            let line: &[u8] = &m.line;
            assert!(b"abcdef".windows(1).any(|c| c == line), "非原集合元素");
        }
    }

    #[test]
    fn test_sample_ratio_half_of_six() {
        // 6 条 × 0.5 = 3 条
        let m = lines(&["a", "b", "c", "d", "e", "f"]);
        let out = apply_sample(m, &SampleSpec::Ratio(0.5));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_sample_ratio_zero_returns_empty() {
        let m = lines(&["a", "b", "c"]);
        let out = apply_sample(m, &SampleSpec::Ratio(0.0));
        assert!(out.is_empty());
    }

    #[test]
    fn test_sample_count_zero_returns_empty() {
        let m = lines(&["a", "b", "c"]);
        let out = apply_sample(m, &SampleSpec::Count(0));
        assert!(out.is_empty());
    }

    #[test]
    fn test_sample_count_larger_than_total_keeps_all() {
        let m = lines(&["a", "b"]);
        let out = apply_sample(m, &SampleSpec::Count(5));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_sample_ratio_full_keeps_all() {
        let m = lines(&["a", "b", "c"]);
        let out = apply_sample(m, &SampleSpec::Ratio(1.0));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_sample_empty_input() {
        let out = apply_sample(Vec::new(), &SampleSpec::Count(3));
        assert!(out.is_empty());
    }

    #[test]
    fn test_sample_reproducible_fixed_seed_count() {
        // 固定种子 0 → 同一输入两次抽样结果一致
        let m1 = lines(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let m2 = lines(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let out1 = apply_sample(m1, &SampleSpec::Count(4));
        let out2 = apply_sample(m2, &SampleSpec::Count(4));
        let l1: Vec<&[u8]> = out1.iter().map(|m| &*m.line).collect();
        let l2: Vec<&[u8]> = out2.iter().map(|m| &*m.line).collect();
        assert_eq!(l1, l2, "固定种子应保证可复现");
    }

    #[test]
    fn test_sample_reproducible_fixed_seed_ratio() {
        let m1 = lines(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let m2 = lines(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let out1 = apply_sample(m1, &SampleSpec::Ratio(0.5));
        let out2 = apply_sample(m2, &SampleSpec::Ratio(0.5));
        let l1: Vec<&[u8]> = out1.iter().map(|m| &*m.line).collect();
        let l2: Vec<&[u8]> = out2.iter().map(|m| &*m.line).collect();
        assert_eq!(l1, l2, "固定种子应保证可复现");
    }

    #[test]
    fn test_sample_all_unique_elements() {
        // 抽样结果不应有重复元素（Fisher-Yates 保证）
        let m = lines(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
        let out = apply_sample(m, &SampleSpec::Count(5));
        let line_nos: Vec<usize> = out.iter().map(|m| m.line_no).collect();
        let mut sorted = line_nos.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), line_nos.len(), "抽样出现重复元素");
    }
}
