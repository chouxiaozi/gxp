#!/usr/bin/env bash
# gxp 远程一键安装（curl | sh 模式，业界 rustup/starship 同构）
#
# 用法（仓库转公开后生效）：
#   curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh
#   curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh -s -- --with-skill
#   sh install.sh --version v0.49.1 --with-skill --bin-dir ~/.local/bin
#
# 功能：
#   1. 探测 OS/ARCH → 映射 GitHub Release 预编译包（4 平台）
#   2. 下载对应包 + .sha256 → 校验 → 解压 gxp 到 PATH 目录
#   3. PATH 不在则永久加入用户级（Windows PowerShell / bashrc）
#   4. --with-skill：下载 gxp-skill-<ver>.zip 解压到 ~/.claude/skills/gxp/（Claude Code；--skill-dir 可覆盖）
#
# 前置：curl + tar（Windows Git Bash 自带；zip 解压走 PowerShell Expand-Archive）

set -euo pipefail

REPO="cianlabs/gxp"
VERSION="${GXP_VERSION:-latest}"
WITH_SKILL=0
BIN_DIR=""
SKILL_DIR=""

die() { echo "ERROR: $*" >&2; exit 1; }

usage() {
  cat <<EOF
Usage: install.sh [OPTIONS]

Options:
  --version <TAG>   安装指定版本（默认 latest）
  --with-skill      同时安装 Claude Code skill 到 ~/.claude/skills/gxp/
  --skill-dir <DIR> skill 安装目标目录（覆盖默认 ~/.claude/skills/gxp/）
  --bin-dir <DIR>   gxp 安装目录（默认：cargo bin > ~/.local/bin > ~/AppData/Local/gxp）
  -h, --help        显示帮助
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)    VERSION="$2"; shift 2 ;;
    --with-skill) WITH_SKILL=1; shift ;;
    --skill-dir)  SKILL_DIR="$2"; shift 2 ;;
    --bin-dir)    BIN_DIR="$2"; shift 2 ;;
    -h|--help)    usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

# --- OS / ARCH 探测 ---
case "$(uname -s)" in
  Linux*)  OS=linux ;;
  Darwin*) OS=macos ;;
  MINGW*|MSYS*|CYGWIN*) OS=windows ;;
  *) die "Unsupported OS: $(uname -s)" ;;
esac
ARCH="$(uname -m)"

case "$OS/$ARCH" in
  linux/x86_64)   TARGET="x86_64-unknown-linux-gnu"; PKG_EXT="tar.gz" ;;
  macos/arm64)    TARGET="aarch64-apple-darwin";     PKG_EXT="tar.gz" ;;
  macos/x86_64)   TARGET="x86_64-apple-darwin";      PKG_EXT="tar.gz" ;;
  windows/x86_64) TARGET="x86_64-pc-windows-msvc";   PKG_EXT="zip" ;;
  linux/aarch64)  die "aarch64-linux 预编译包暂未发布：cargo install --locked --git https://github.com/$REPO --features l3-structural" ;;
  *) die "Unsupported platform: $OS/$ARCH" ;;
esac

# --- 版本解析 ---
if [ "$VERSION" = "latest" ]; then
  echo "[1/5] 解析 latest 版本..."
  TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"tag_name": *"//;s/"//')"
  [ -n "$TAG" ] || die "无法解析 latest 版本（仓库是否公开？是否已有 Release？）"
else
  TAG="$VERSION"
fi
BASE_URL="https://github.com/$REPO/releases/download/$TAG"
PKG="gxp-$TAG-$TARGET"
echo "  版本: $TAG    平台: $TARGET"

# --- 安装目录 ---
if [ -n "$BIN_DIR" ]; then
  TARGET_DIR="$BIN_DIR"
elif command -v cargo >/dev/null 2>&1; then
  TARGET_DIR="$(dirname "$(command -v cargo)")"
elif [ "$OS" = "windows" ]; then
  TARGET_DIR="$HOME/AppData/Local/gxp"
else
  TARGET_DIR="$HOME/.local/bin"
fi

# --- 下载 + 校验 + 解压 ---
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "[2/5] 下载 $PKG.$PKG_EXT..."
curl -fL --retry 3 -o "$TMP/$PKG.$PKG_EXT" "$BASE_URL/$PKG.$PKG_EXT" \
  || die "下载失败（版本 $TAG 是否含 $TARGET 工件？见 Release 页面）"

echo "[3/5] 校验 sha256..."
curl -fL --retry 3 -o "$TMP/$PKG.$PKG_EXT.sha256" "$BASE_URL/$PKG.$PKG_EXT.sha256" \
  || die "校验文件下载失败"
EXPECTED="$(awk '{print $1}' "$TMP/$PKG.$PKG_EXT.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$TMP/$PKG.$PKG_EXT" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL="$(shasum -a 256 "$TMP/$PKG.$PKG_EXT" | awk '{print $1}')"
else
  die "无 sha256sum/shasum，无法校验（Git Bash / Linux / macOS 均自带）"
fi
[ "$ACTUAL" = "$EXPECTED" ] || die "sha256 不匹配：期望 $EXPECTED 实际 $ACTUAL"

echo "[4/5] 安装到 $TARGET_DIR/..."
mkdir -p "$TARGET_DIR"
if [ "$PKG_EXT" = "tar.gz" ]; then
  tar -xzf "$TMP/$PKG.$PKG_EXT" -C "$TMP"
  cp "$TMP/$PKG/gxp" "$TARGET_DIR/gxp"
  chmod +x "$TARGET_DIR/gxp"
else
  # Windows zip：PowerShell Expand-Archive（Git Bash 无 unzip 时兜底）
  WIN_TMP=$(cygpath -w "$TMP" 2>/dev/null || echo "$TMP")
  WIN_PKG=$(cygpath -w "$PKG" 2>/dev/null || echo "$PKG")
  WIN_TARGET=$(cygpath -w "$TARGET_DIR" 2>/dev/null || echo "$TARGET_DIR")
  powershell.exe -NoProfile -Command "
    Expand-Archive -Path (Join-Path '$WIN_TMP' '$WIN_PKG.zip') -DestinationPath '$WIN_TMP' -Force;
    Copy-Item (Join-Path (Join-Path '$WIN_TMP' '$WIN_PKG') 'gxp.exe') '$WIN_TARGET\\gxp.exe' -Force
  " || die "解压/拷贝失败"
fi

# --- PATH 处理 ---
path_contains() {
  case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac
}
PATH_NOTE=""
if ! path_contains "$TARGET_DIR"; then
  if [ "$OS" = "windows" ]; then
    powershell.exe -NoProfile -Command "
      \$cur = [Environment]::GetEnvironmentVariable('Path', 'User')
      if (\$cur -notlike \"*$WIN_TARGET*\") {
        [Environment]::SetEnvironmentVariable('Path', \"\$cur;$WIN_TARGET\", 'User')
      }
    " >/dev/null || true
    PATH_NOTE="已永久加入用户级 PATH——请新开终端窗口生效"
  else
    SHELL_RC="$HOME/.bashrc"
    [ -f "$HOME/.zshrc" ] && [ "$SHELL" = */zsh ] && SHELL_RC="$HOME/.zshrc"
    grep -q "export PATH=\"\$PATH:$TARGET_DIR\"" "$SHELL_RC" 2>/dev/null || {
      printf '\n# Added by gxp install.sh\nexport PATH="$PATH:%s"\n' "$TARGET_DIR" >> "$SHELL_RC"
    }
    PATH_NOTE="已写入 $SHELL_RC——请新开终端或 source 生效"
  fi
fi
export PATH="$PATH:$TARGET_DIR"

# --- skill 安装 ---
if [ "$WITH_SKILL" = "1" ]; then
  # skill 目标：--skill-dir 覆盖，默认 ~/.claude/skills/gxp（Claude Code）
  if [ -n "$SKILL_DIR" ]; then
    SKILL_TARGET="$SKILL_DIR"
  elif [ -d "$HOME/.claude" ]; then
    SKILL_TARGET="$HOME/.claude/skills/gxp"
  else
    die "未检测到 ~/.claude（Claude Code 未安装？）：安装 Claude Code 后重试，或 --skill-dir <DIR> 指定目录，或去掉 --with-skill 只装二进制"
  fi
  # 双装提示（不阻断）：plugin 版与个人 skill 版并存会双份 description 占 context
  if [ -f "$HOME/.claude/plugins/installed_plugins.json" ] \
     && grep -q '"gxp"' "$HOME/.claude/plugins/installed_plugins.json" 2>/dev/null; then
    echo "  提示: 检测到 gxp plugin（marketplace 版）已安装。skill 与 plugin 二选一即可——"
    echo "        继续装个人 skill 版功能不受影响，但两份 skill 描述会同时占用 agent context。"
  fi
  echo "[5/5] 安装 skill 到 $SKILL_TARGET..."
  SKILL_ZIP="gxp-skill-$TAG.zip"
  curl -fL --retry 3 -o "$TMP/$SKILL_ZIP" "$BASE_URL/$SKILL_ZIP" || die "skill 包下载失败"
  mkdir -p "$SKILL_TARGET"
  if [ "$OS" = "windows" ]; then
    WIN_SKILL=$(cygpath -w "$SKILL_TARGET" 2>/dev/null || echo "$SKILL_TARGET")
    powershell.exe -NoProfile -Command "
      Expand-Archive -Path '$WIN_TMP\\$SKILL_ZIP' -DestinationPath '$WIN_SKILL' -Force
    " || die "skill 解压失败"
  else
    unzip -qo "$TMP/$SKILL_ZIP" -d "$SKILL_TARGET" || die "skill 解压失败（unzip 缺失？）"
  fi
  echo "  skill: $SKILL_TARGET/SKILL.md ($(wc -c < "$SKILL_TARGET/SKILL.md" 2>/dev/null || echo '?') bytes)"
else
  echo "[5/5] 跳过 skill（--with-skill 可一并安装）"
fi

# --- 验证 ---
if ! command -v gxp >/dev/null 2>&1; then
  if [ -n "$PATH_NOTE" ]; then
    echo "=== 安装完成（PATH 待生效）==="
    echo "  gxp: $TARGET_DIR/gxp"
    echo "  注意: $PATH_NOTE"
    exit 0
  fi
  die "gxp 安装异常：$TARGET_DIR 不在 PATH 且无法验证"
fi
echo ""
echo "=== 安装完成 ==="
echo "  gxp: $(command -v gxp || echo "$TARGET_DIR/gxp")"
gxp --version || true
[ -n "$PATH_NOTE" ] && echo "  注意: $PATH_NOTE"
echo ""
echo "试运行: gxp 'ERROR' <file>   （≡ grep -E）"
