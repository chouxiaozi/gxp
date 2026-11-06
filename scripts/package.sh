#!/usr/bin/env bash
# gxp 分发包打包脚本
#
# 用法：
#   ./package.sh                  # 默认 cargo build --release + cp exe 到 dist/skill/bin/
#   ./package.sh --skip-build     # 跳过 cargo build（已 build 过的情况）
#   ./package.sh --zip            # 额外打 zip 包（dist/skill -> dist/gxp-skill-<version>.zip）
#   ./package.sh --help
#
# 产物：
#   dist/skill/bin/gxp.exe       ← Windows 二进制（被 .gitignore 忽略，不入库）
#   dist/gxp-skill-<version>.zip ← 可选 zip 包（含完整分发物 + bin/）
#
# 仓库内 dist/skill/ 跟踪文本分发物（SKILL.md / INSTALL.md / references/ / scripts/）；
# bin/ 由本脚本生成，git 不跟踪，避免二进制污染历史。

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# gxp 仓库根（scripts/.. ）
GXP_REPO="$(dirname "$SCRIPT_DIR")"
# skill 分发目录（dist/skill/）
SKILL_DIST_DIR="$GXP_REPO/dist/skill"

SKIP_BUILD=0
DO_ZIP=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --zip)        DO_ZIP=1; shift ;;
    -h|--help)
      cat <<EOF
Usage: package.sh [OPTIONS]

Options:
  --skip-build   跳过 cargo build --release（用已有 target/release/gxp.exe）
  --zip          额外打 zip 包到 dist/gxp-skill-<version>.zip
  -h, --help     显示帮助
EOF
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 2
      ;;
  esac
done

# 探测 OS + 选二进制名
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) OS=windows; BIN_NAME=gxp.exe ;;
  Linux*)  OS=linux;  BIN_NAME=gxp ;;
  Darwin*) OS=macos;  BIN_NAME=gxp ;;
  *) echo "ERROR: Unknown OS: $(uname -s)"; exit 1 ;;
esac

echo "=== gxp 分发包打包 ==="
echo "OS:           $OS"
echo "gxp 仓库:     $GXP_REPO"
echo "skill dist:  $SKILL_DIST_DIR"
echo ""

# 1. cargo build --release
if [ "$SKIP_BUILD" = "0" ]; then
  echo "[1/3] cargo build --release..."
  (cd "$GXP_REPO" && cargo build --release)
else
  echo "[1/3] 跳过 cargo build（--skip-build）"
fi

# 2. 拷贝 exe 到 dist/skill/bin/
RELEASE_BIN="$GXP_REPO/target/release/$BIN_NAME"
DIST_BIN="$SKILL_DIST_DIR/bin/$BIN_NAME"

if [ ! -f "$RELEASE_BIN" ]; then
  echo "ERROR: $RELEASE_BIN 不存在"
  echo "  请先 cargo build --release，或去掉 --skip-build 重新跑本脚本"
  exit 1
fi

mkdir -p "$SKILL_DIST_DIR/bin"
echo ""
echo "[2/3] 拷贝 $RELEASE_BIN -> $DIST_BIN..."
cp "$RELEASE_BIN" "$DIST_BIN"
[ "$OS" != "windows" ] && chmod +x "$DIST_BIN"

# 验证 exe 可用
"$DIST_BIN" --version

# 3. 输出分发物清单
echo ""
echo "[3/3] 分发物清单："
echo ""
echo "  dist/skill/"
( cd "$SKILL_DIST_DIR" && find . -type f -not -path "./bin/*" | sort | sed 's/^/    /' )
echo "  dist/skill/bin/"
echo "    $BIN_NAME ($(du -h "$DIST_BIN" | cut -f1))"

# 4. 可选 zip 包
if [ "$DO_ZIP" = "1" ]; then
  VERSION=$("$DIST_BIN" --version | awk '{print $2}')
  ZIP_NAME="gxp-skill-${VERSION}.zip"
  ZIP_PATH="$GXP_REPO/dist/$ZIP_NAME"
  echo ""
  echo "[zip] 打包 $ZIP_PATH..."

  # 优先用 zip（Linux/macOS 自带）；回退 PowerShell Compress-Archive（Windows）
  if command -v zip >/dev/null 2>&1; then
    ( cd "$GXP_REPO/dist" && rm -f "$ZIP_NAME" && zip -r "$ZIP_NAME" "skill/" -x 'skill/scripts/package.sh' )
  else
    # Windows Git Bash 无 zip，用 PowerShell Compress-Archive
    WIN_DIST=$(cygpath -w "$GXP_REPO/dist" 2>/dev/null || echo "$GXP_REPO/dist")
    WIN_ZIP=$(cygpath -w "$ZIP_PATH" 2>/dev/null || echo "$ZIP_PATH")
    powershell.exe -NoProfile -Command "
      \$skillSrc = Join-Path '$WIN_DIST' 'skill'
      if (Test-Path '$WIN_ZIP') { Remove-Item '$WIN_ZIP' -Force }
      Compress-Archive -Path \$skillSrc -DestinationPath '$WIN_ZIP' -Force
    " 2>&1 | sed 's/^/    /'
  fi
  echo "  zip: $ZIP_PATH ($(du -h "$ZIP_PATH" | cut -f1))"
fi

echo ""
echo "=== 打包完成 ==="
echo ""
echo "分发流程："
echo "  1. （本机已 build）跑 ./package.sh 重新生成 bin/"
echo "  2. 拷贝 dist/skill/ 整个目录给对方（或 --zip 打 zip 包）"
echo "  3. 对方在 Git Bash 跑 scripts/install.sh"
echo ""
echo "注意："
echo "  - dist/skill/bin/ 在 .gitignore 中，git 不跟踪"
echo "  - 升级 gxp.exe 后需重跑 package.sh 刷新 bin/"
