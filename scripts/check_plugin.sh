#!/bin/sh
# gxp 分发资产门禁 — Claude Code plugin + skills.sh 双渠道兼容性
# 用法（仓库根）: sh scripts/check_plugin.sh
# CI: ci.yml plugin-lint job 调用；本地可复现同口径判定
# ⚠️ 遵循 CI 脚本红线：判定不用管道接 if（用重定向 + grep 文件）
set -eu
cd "$(dirname "$0")/.."

echo "== [1/4] claude plugin validate =="
if ! command -v claude >/dev/null 2>&1; then
  echo "FAIL: claude CLI 不在 PATH（本地需装 Claude Code；CI 由 workflow 安装）" >&2
  exit 1
fi
claude plugin validate .

echo "== [2/4] SKILL.md 结构 =="
SKILL=skills/gxp/SKILL.md
if [ ! -f "$SKILL" ]; then echo "FAIL: $SKILL 不存在" >&2; exit 1; fi
# 无 BOM
if head -c 3 "$SKILL" | od -An -tx1 | grep -qi 'ef bb bf'; then
  echo "FAIL: SKILL.md 含 UTF-8 BOM（Claude Code 不识别 frontmatter）" >&2; exit 1
fi
# 首行 ---，frontmatter 含 name 与 description
if ! head -n 1 "$SKILL" | grep -q '^---$'; then
  echo "FAIL: SKILL.md 首行非 ---" >&2; exit 1
fi
if ! awk '/^---$/{c++;next} c==1{print}' "$SKILL" | grep -q '^name:'; then
  echo "FAIL: frontmatter 缺 name" >&2; exit 1
fi
if ! awk '/^---$/{c++;next} c==1{print}' "$SKILL" | grep -q '^description:'; then
  echo "FAIL: frontmatter 缺 description" >&2; exit 1
fi
# description 长度 ≤1536 字符（Claude Code 截断阈值）
DESC_LEN=$(awk '/^---$/{c++;next} c==1 && /^description:/{sub(/^description: */,""); print length($0)}' "$SKILL")
if [ "${DESC_LEN:-0}" -gt 1536 ]; then
  echo "FAIL: description ${DESC_LEN} 字符 > 1536（会被截断）" >&2; exit 1
fi
echo "  description: ${DESC_LEN} chars"
# LF 行尾（无 CRLF）
CR=$(printf '\r')
if grep -q "$CR" "$SKILL"; then echo "FAIL: SKILL.md 含 CRLF 行尾" >&2; exit 1; fi

echo "== [3/4] marketplace/plugin 清单一致性 =="
if ! command -v jq >/dev/null 2>&1; then echo "FAIL: jq 不在 PATH" >&2; exit 1; fi
MKT_NAME=$(jq -r '.name' .claude-plugin/marketplace.json)
PLG_NAME=$(jq -r '.name' .claude-plugin/plugin.json)
ENTRY_NAME=$(jq -r '.plugins[0].name' .claude-plugin/marketplace.json)
if [ "$MKT_NAME" != "cianlabs" ]; then
  echo "FAIL: marketplace 名预期 cianlabs，实际 $MKT_NAME" >&2; exit 1
fi
if [ "$PLG_NAME" != "$ENTRY_NAME" ]; then
  echo "FAIL: plugin.json name($PLG_NAME) != marketplace 条目 name($ENTRY_NAME)——安装将报 not found" >&2; exit 1
fi
# version 策略：双处均不写（跟 commit 更新；官方目录提交时才补）
if jq -e 'has("version")' .claude-plugin/plugin.json > /dev/null; then
  echo "FAIL: plugin.json 不应写 version（无版本=跟 commit 策略，见 AGENTS 任务决策）" >&2; exit 1
fi
if jq -e '.plugins[0] | has("version")' .claude-plugin/marketplace.json > /dev/null; then
  echo "FAIL: marketplace 条目不应写 version" >&2; exit 1
fi

echo "== [4/4] skills.sh 发现（npx skills add ./ -l）=="
if ! command -v npx >/dev/null 2>&1; then echo "FAIL: npx 不在 PATH（需 node）" >&2; exit 1; fi
TMPF=$(mktemp)
if ! npx -y skills add ./ -l -y > "$TMPF" 2>&1; then
  cat "$TMPF" >&2; rm -f "$TMPF"
  echo "FAIL: skills.sh CLI 枚举失败" >&2; exit 1
fi
if ! grep -q gxp "$TMPF"; then
  cat "$TMPF" >&2; rm -f "$TMPF"
  echo "FAIL: skills.sh CLI 未发现 gxp（布局兼容性回归）" >&2; exit 1
fi
rm -f "$TMPF"

echo "PASS: plugin + skills.sh 分发资产门禁全绿"
