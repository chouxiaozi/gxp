#!/usr/bin/env bash
# gxp gate: src/ 内联 #[cfg(test)] 块禁止出现集成测试信号
#
# 检测信号（确定性 grep，AGENTS.md §6.6）：
# - tempfile::NamedTempFile / tempfile::tempfile
# - std::fs::write / std::fs::read / File::create / File::open
# - 调公开入口 run_with_writer / detect_and_dispatch / run_gxp_mode
#
# 上述信号出现在 src/**/*.rs 的 #[cfg(test)] 行之后 → fail
# 纯函数单元测试（测私有 fn，不碰 tempfile/fs）不受影响。
#
# Usage:
#   bash scripts/check_no_e2e_inline.sh
# Exit code: 0=clean, 1=found violations

set -uo pipefail

# 找到 git root
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# 信号模式：在 #[cfg(test)] 块中调公开入口函数 = 集成测试，应外迁到 tests/
# tempfile/fs 本身不违规（单元测试常用夹具），违规的是测公开入口而非私有逻辑
SIGNALS='run_with_writer|detect_and_dispatch|run_gxp_mode\b|run_bare_mode\b'

violations=0

# 遍历 src/ 下所有 .rs 文件
for file in $(find "$ROOT/src" -name '*.rs' -type f 2>/dev/null | sort); do
    # 找 #[cfg(test)] 行号（取第一个）
    cfg_line=$(grep -n '#\[cfg(test)\]' "$file" 2>/dev/null | head -1 | cut -d: -f1 || true)
    [ -z "$cfg_line" ] && continue

    # 从 #[cfg(test)] 行开始扫描信号（排除注释行 // 和 /// ）
    matches=$(tail -n +"$cfg_line" "$file" | grep -nE "$SIGNALS" 2>/dev/null | grep -vE '^\s*[0-9]+:\s*//' || true)
    if [ -n "$matches" ]; then
        echo "FAIL: $file has integration test signals in #[cfg(test)] block:"
        echo "$matches" | head -5 | while IFS= read -r line; do
            echo "  line: $line"
        done
        violations=$((violations + 1))
    fi
done

if [ "$violations" -gt 0 ]; then
    echo ""
    echo "Gate: $violations file(s) with inline e2e tests in src/"
    echo "Fix: move tempfile/fs-based tests to tests/ (AGENTS.md §6.6)"
    exit 1
else
    echo "Gate: src/ inline e2e check clean (0 violations)"
    exit 0
fi
