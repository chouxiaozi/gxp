#!/bin/sh
# gxp governance 体检脚本
#
# 跑 docs/document-governance.md §4 体检 checklist（不依赖 PowerShell，纯 bash + grep）。
# pre-commit hook 调用：fmt+clippy 之前先跑 governance 体检。
# CI job 调用：直接 bash 此脚本。
#
# 退出码：0 = 全绿，1 = 有阻断
#
# 用法：bash scripts/check_governance.sh

# 不用 set -e：grep 无匹配返回 1 是正常行为，不应触发退出
REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

# 阻断计数
VIOLATIONS=0
warn() { echo "[governance] $*" >&2; VIOLATIONS=$((VIOLATIONS+1)); }

# 4.1.5 代码本体不复述（除 design §2.2 EBNF 例外）
TOML_RUST=$(grep -rn '```toml\|```rust$' docs/ AGENTS.md CONTRIBUTING.md 2>/dev/null | grep -v 'document-governance')
if [ -n "$TOML_RUST" ]; then
    warn "4.1.5 代码本体不复述：发现 toml/rust 代码块（应改链接到代码文件）"
    echo "$TOML_RUST" >&2
fi

# 4.1.6 跨文档引用死链（粗扫描，需人工评估）
# 此项不阻断（仅提示），因为死链判定需要读者确认目标章节存在
DEAD_LINKS=$(grep -rn '详见.*§\|见.*§\|§[0-9]' docs/ AGENTS.md CONTRIBUTING.md 2>/dev/null | grep -v "^docs/changelog" | wc -l)
DEAD_LINKS=${DEAD_LINKS:-0}
echo "[governance] 4.1.6 跨文档引用扫描：$DEAD_LINKS 处（需人工评估目标章节存在）"

# 4.2.1 过程性噪声词扫描（除 changelog）
NOISE=$(grep -rn "PoC\|时刻\|早期\|曾经\|原本\|最初\|实验后\|选型实验" \
    docs/gxp-design-full.md docs/rust-implementation-plan.md \
    docs/performance-baseline.md AGENTS.md CONTRIBUTING.md 2>/dev/null \
    | grep -v "^docs/changelog")
if [ -n "$NOISE" ]; then
    warn "4.2.1 过程性噪声词："
    echo "$NOISE" >&2
fi

# 4.3 性能数据单点
PERF=$(grep -rn "66.*76ms\|18\.66ms\|327ms\|835ms" \
    docs/gxp-design-full.md docs/rust-implementation-plan.md \
    AGENTS.md CONTRIBUTING.md 2>/dev/null)
if [ -n "$PERF" ]; then
    warn "4.3 性能数字散落（应只在 perf-baseline §1.1）："
    echo "$PERF" >&2
fi

# 4.1.3 Cargo.toml 版本号 vs changelog 最新版本
# changelog 用表格行 "| v0.X | ... |"，按时间排序，最新是最后一行
# Cargo.toml 用 0.X.Y，changelog 用 v0.X — 比较去掉 v 前缀和 .0 后缀的主次版本号
CARGO_VER=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
LATEST_CL=$(grep -E '^\| v0\.[0-9]+' docs/changelog.md | tail -1 | sed 's/^| v\([0-9.]*\) .*/\1/')
# 归一化：去 trailing .0（0.33.0 → 0.33）
CARGO_VER_NORM=$(echo "$CARGO_VER" | sed 's/\.0$//')
echo "[governance] 4.1.3 Cargo.toml=$CARGO_VER  changelog 最新=$LATEST_CL"
if [ -z "$LATEST_CL" ]; then
    warn "4.1.3 无法从 changelog 提取最新版本号（grep 模式失效？）"
elif [ "$CARGO_VER_NORM" != "$LATEST_CL" ]; then
    warn "4.1.3 Cargo.toml version=$CARGO_VER 与 changelog 最新版本=v$LATEST_CL 不一致"
fi

# 4.1.7 src/ 注释 §X.Y 引用 vs docs/ 章节锚点交叉校验
# 机器能查：§X.Y.Z 子节号 → docs/ 中是否有对应 #/##/### X.Y.Z 锚点
# 机器查不出：§X.Y 锚点存在但语义不对应（需 agent 自觉 + code review）
TMP_DIR=${TMPDIR:-/tmp}
SRC_REFS_FILE="$TMP_DIR/gxp_src_refs.$$"
DOC_ANCHORS_FILE="$TMP_DIR/gxp_doc_anchors.$$"
grep -rhoE '§[0-9]+\.[0-9]+(\.[0-9]+)?' src/ 2>/dev/null | sed 's/§//' | sort -u > "$SRC_REFS_FILE"
grep -rhoE '^#+ [0-9]+\.[0-9]+(\.[0-9]+)?' docs/*.md AGENTS.md CONTRIBUTING.md 2>/dev/null | sed -E 's/^#+ //' | sort -u > "$DOC_ANCHORS_FILE"
DEAD_REFS=$(comm -23 "$SRC_REFS_FILE" "$DOC_ANCHORS_FILE")
rm -f "$SRC_REFS_FILE" "$DOC_ANCHORS_FILE"
if [ -n "$DEAD_REFS" ]; then
    warn "4.1.7 src/ 注释引用了 docs/ 不存在的 §章节号（死链）："
    echo "$DEAD_REFS" | sed 's/^/  §/' >&2
fi

# 4.1.8 测试基线数字对账（capability-matrix §4 vs cargo test --list）
# 防漂移：文档写死的测试数必须与 cargo test --list 实际计数一致
# 跳过条件：CARGO_SKIP_TEST_COUNT=1（用于离线/无 cargo 工具链环境）
# 性能：<3s（target/ 已编译时 cargo test --list 仅遍历二进制，不重编译）
if [ "$CARGO_SKIP_TEST_COUNT" = "1" ]; then
    echo "[governance] 4.1.8 测试数对账：跳过（CARGO_SKIP_TEST_COUNT=1）"
else
    # 提取文档声明的测试数（capability-matrix §4 表格里两行）
    DOC_DEFAULT=$(grep -E '^\| 默认 feature 测试' docs/capability-matrix.md | sed -E 's/.*\| ([0-9]+) 全绿.*/\1/')
    DOC_L3=$(grep -E '^\| l3-structural feature 测试' docs/capability-matrix.md | sed -E 's/.*\| ([0-9]+) 全绿.*/\1/')

    if [ -z "$DOC_DEFAULT" ] || [ -z "$DOC_L3" ]; then
        warn "4.1.8 无法从 capability-matrix §4 提取测试基线数字（grep 模式失效？）"
    else
        # cargo test -- --list 输出格式："test_name: test"，最后一行汇总 "N tests, ..."
        # 不依赖汇总行（不同 cargo 版本格式不同），直接 grep -c ": test$"
        # 编译 profile：GXP_TEST_PROFILE=release（默认，本地 pre-commit 复用 target/release
        # 既有产物）/ dev（CI check job 传 dev——紧随 cargo test 之后复用刚编好的产物；
        # 注 default 自 l3 默认开启后与 --features l3-structural 指纹相同，第二次 --list 恒秒回）
        TEST_PROFILE=${GXP_TEST_PROFILE:-release}
        PROFILE_FLAG=""
        [ "$TEST_PROFILE" = "release" ] && PROFILE_FLAG="--release"
        # 故意不加引号：PROFILE_FLAG 需按词展开（release 为 --release，dev 为空）
        ACTUAL_DEFAULT=$(cargo test $PROFILE_FLAG -- --list 2>/dev/null | grep -c ": test$")
        ACTUAL_L3=$(cargo test --features l3-structural $PROFILE_FLAG -- --list 2>/dev/null | grep -c ": test$")

        if [ -z "$ACTUAL_DEFAULT" ] || [ -z "$ACTUAL_L3" ]; then
            echo "[governance] 4.1.8 测试数对账：cargo test --list 失败（跳过，可能未编译）"
        else
            echo "[governance] 4.1.8 测试数对账：文档 default=$DOC_DEFAULT / l3=$DOC_L3，实际 default=$ACTUAL_DEFAULT / l3=$ACTUAL_L3"
            if [ "$DOC_DEFAULT" != "$ACTUAL_DEFAULT" ]; then
                warn "4.1.8 默认 feature 测试数漂移：文档 $DOC_DEFAULT ≠ 实际 $ACTUAL_DEFAULT（更新 capability-matrix §4）"
            fi
            if [ "$DOC_L3" != "$ACTUAL_L3" ]; then
                warn "4.1.8 l3-structural 测试数漂移：文档 $DOC_L3 ≠ 实际 $ACTUAL_L3（更新 capability-matrix §4）"
            fi
        fi
    fi
fi

# 4.1.9 cargo doc broken intra-doc links（防函数名漂移）
# src/**/*.rs 的 /// 注释中 [fn_name] 被 rustdoc 解析为 intra-doc link，
# 指向不存在的函数/类型 → error。覆盖 B9 清理 10 处函数名漂移的场景。
# 跳过条件：CARGO_SKIP_DOC_CHECK=1
if [ "$CARGO_SKIP_DOC_CHECK" = "1" ]; then
    echo "[governance] 4.1.9 cargo doc 检查：跳过（CARGO_SKIP_DOC_CHECK=1）"
else
    DOC_ERRORS=$(RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::invalid_html_tags" \
        cargo doc --no-deps --document-private-items 2>&1)
    DOC_FAIL=$(echo "$DOC_ERRORS" | grep -c "^error")
    if [ "$DOC_FAIL" -gt 0 ]; then
        echo "[governance] 4.1.9 cargo doc 检查：$DOC_FAIL 个 error"
        warn "4.1.9 cargo doc broken intra-doc links / invalid HTML tags："
        echo "$DOC_ERRORS" | grep "^error" >&2
    else
        echo "[governance] 4.1.9 cargo doc 检查：0 error"
    fi
fi

# 输出结果
if [ "$VIOLATIONS" -gt 0 ]; then
    echo "[governance] ✗ $VIOLATIONS 项阻断，请修复后再 commit" >&2
    echo "[governance] 逃生通道：git commit --no-verify（慎用）" >&2
    exit 1
fi

echo "[governance] ✓ 体检全绿"
exit 0
