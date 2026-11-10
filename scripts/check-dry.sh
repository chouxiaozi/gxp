#!/bin/sh
# gxp DRY 门禁（jscpd）— 本地与 CI 同源调用（ci.yml dry job 执行 `sh scripts/check-dry.sh`）
#
# 为什么是脚本而不是 yml 内联命令：jscpd 5.x 参数语义有陷阱且跨版本漂移
# （--reporter 移除 / --exitCode→--exit-code 改名 / --exit-code 语义变化），
# 内联在 yml 里本地无法复现，只能推 CI 试错（2026-11-09 三轮才修对）。
# 抽成脚本后本地一条命令复现 CI 判定，与 governance 体检、check_no_e2e_inline.sh
# 同属"scripts/ + CI 调用"模式。
#
# jscpd 5.4.3 参数语义（2026-11-09 本地阶梯实测）：
#   --threshold N   重复率超 N% 才 exit 1；判定口径是 **duplicated lines 百分比**
#                   （tokens 百分比只在汇总表格展示，不参与判定）
#   --exit-code     "发现任何 duplicates 即退出"的独立开关，**会覆盖 --threshold
#                   的超率判定**——两者同传 = 仓库存在重复就必红（42639769811 根因），
#                   故本脚本只传 --threshold
#
# 基线（2026-11-09，threshold=7）：lines 6.09% / 182 clones / 28663 行
#   src 5.34%/93 + tests 6.39%/76 + 跨目录 13（min-tokens 50）。
#   整改方向见 rust-implementation-plan.md §11.3.2 暂不评估清单（同构镜像受控重复）。
set -e

THRESHOLD="${1:-7}"

command -v jscpd >/dev/null 2>&1 || {
  echo "error: jscpd 未安装。本地安装：npm install -g jscpd（CI 由 dry job 预装）" >&2
  exit 127
}

echo "[dry] jscpd threshold = ${THRESHOLD}%（lines 口径）"
exec jscpd --format rust --threshold "$THRESHOLD" src/ tests/
