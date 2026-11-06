#!/usr/bin/env bash
# gxp skill + exe 一键安装（exe 随包分发，无需 cargo install）
#
# 用法：
#   ./install.sh                     # 默认目标：Linux/macOS ~/.local/bin/，Windows ~/AppData/Local/gxp/
#   ./install.sh --target <DIR>     # 装到指定目录
#   ./install.sh --help
#
# 功能：
#   1. 拷贝 dist/skill/bin/gxp(.exe) 到目标 PATH 目录
#   2. 若目标目录不在 PATH 中，自动永久加入用户级 PATH（无需 admin/sudo）
#   3. 拷贝 SKILL.md + references/ + FEEDBACK.md 到 ~/.cac/skills/gxp/
#   4. 验证 gxp --version + skill 文件结构
#
# 前置依赖：
#   - CodeAgent CLI（~/.cac/ 目录存在）
#   - 平台匹配的 gxp 二进制（dist/skill/bin/ 下）
#     Windows:  gxp.exe
#     Linux:   gxp   （需源码编译，见 INSTALL.md "Linux/macOS 编译"）
#     macOS:   gxp   （需源码编译，见 INSTALL.md "Linux/macOS 编译"）

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKILL_DIST_DIR="$(dirname "$SCRIPT_DIR")"  # dist/skill/

# 探测 OS + 选二进制名
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) OS=windows; BIN_NAME=gxp.exe ;;
  Linux*)  OS=linux;  BIN_NAME=gxp ;;
  Darwin*) OS=macos;  BIN_NAME=gxp ;;
  *) echo "ERROR: Unknown OS: $(uname -s)"; exit 1 ;;
esac

# 默认目标目录：
#   1. 优先复用 cargo bin（若 cargo 在 PATH 中，零摩擦）
#   2. Linux/macOS: ~/.local/bin/（XDG 开源标准）
#   3. Windows: ~/AppData/Local/gxp/（Windows 原生用户级，不污染系统目录）
if command -v cargo >/dev/null 2>&1; then
  DEFAULT_TARGET="$(dirname "$(command -v cargo)")"
elif [ "$OS" = "windows" ]; then
  DEFAULT_TARGET="$HOME/AppData/Local/gxp"
else
  DEFAULT_TARGET="$HOME/.local/bin"
fi

TARGET_DIR="$DEFAULT_TARGET"

# 参数解析
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET_DIR="$2"
      shift 2
      ;;
    -h|--help)
      cat <<EOF
Usage: install.sh [OPTIONS]

Options:
  --target <DIR>   gxp 二进制装到 <DIR>（默认：$DEFAULT_TARGET）
  -h, --help        显示帮助
EOF
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 2
      ;;
  esac
done

GXP_SRC="$SKILL_DIST_DIR/bin/$BIN_NAME"

# 1. 检查分发 exe 是否存在
if [ ! -f "$GXP_SRC" ]; then
  echo "ERROR: $GXP_SRC 不存在"
  echo ""
  echo "当前分发包未含此平台 ($OS) 的 gxp 二进制。"
  echo "Linux/macOS 用户需从源码编译，见 INSTALL.md \"Linux/macOS 编译\" 一节。"
  exit 1
fi

echo "=== gxp skill + exe 一键安装 ==="
echo "OS:           $OS"
echo "分发 exe:     $GXP_SRC"
echo "目标目录:     $TARGET_DIR"
echo "skill 目标:   $HOME/.cac/skills/gxp/"
echo ""

# 2. 拷贝 gxp 二进制到目标目录
mkdir -p "$TARGET_DIR"
echo "[1/4] 拷贝 gxp 到 $TARGET_DIR/..."
cp "$GXP_SRC" "$TARGET_DIR/$BIN_NAME"
[ "$OS" != "windows" ] && chmod +x "$TARGET_DIR/$BIN_NAME"

# 3. 检查目标目录在 PATH 中，不在则自动永久加入用户级 PATH
echo ""
echo "[2/4] 检查 PATH..."

path_contains() {
  local dir="$1"
  case ":$PATH:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

if path_contains "$TARGET_DIR"; then
  echo "  $TARGET_DIR 已在 PATH 中"
else
  echo "  $TARGET_DIR 不在 PATH 中，自动永久加入用户级 PATH..."

  if [ "$OS" = "windows" ]; then
    # Windows: PowerShell [Environment]::SetEnvironmentVariable 用户级永久
    WIN_TARGET=$(cygpath -w "$TARGET_DIR" 2>/dev/null || echo "$TARGET_DIR")
    powershell.exe -NoProfile -Command "
      \$current = [Environment]::GetEnvironmentVariable('Path', 'User')
      if (\$current -notlike \"*$WIN_TARGET*\") {
        [Environment]::SetEnvironmentVariable('Path', \"\$current;$WIN_TARGET\", 'User')
        Write-Host \"  已永久加入用户级 PATH: $WIN_TARGET\"
      } else {
        Write-Host \"  用户级 PATH 已含此目录\"
      }
    " 2>&1 | sed 's/^/  /'
    echo ""
    echo "  注意：当前 Git Bash session 不会立即生效，请新开 Git Bash 窗口让 PATH 生效"
  else
    # Linux/macOS: 写入 ~/.bashrc（若文件存在），并提示重开终端或 source
    SHELL_RC="$HOME/.bashrc"
    [ -f "$HOME/.zshrc" ] && [ "$SHELL" = */zsh ] && SHELL_RC="$HOME/.zshrc"
    if ! grep -q "export PATH=\"\$PATH:$TARGET_DIR\"" "$SHELL_RC" 2>/dev/null; then
      echo "" >> "$SHELL_RC"
      echo "# Added by gxp install.sh" >> "$SHELL_RC"
      echo "export PATH=\"\$PATH:$TARGET_DIR\"" >> "$SHELL_RC"
      echo "  已写入 $SHELL_RC"
    else
      echo "  $SHELL_RC 已含此 PATH 配置"
    fi
    echo "  注意：当前 session 不会立即生效，请新开终端或执行 source $SHELL_RC"
  fi
fi

# 临时加进 PATH 让本次验证可用
export PATH="$PATH:$TARGET_DIR"

# 4. 拷贝 skill 文件
echo ""
echo "[3/4] 拷贝 skill 到 ~/.cac/skills/gxp/..."

if [ ! -d "$HOME/.cac" ]; then
  echo "ERROR: ~/.cac/ 不存在。CodeAgent CLI 未安装？"
  exit 1
fi

SKILL_TARGET="$HOME/.cac/skills/gxp"
mkdir -p "$SKILL_TARGET"
cp "$SKILL_DIST_DIR/SKILL.md" "$SKILL_TARGET/SKILL.md"
cp -r "$SKILL_DIST_DIR/references" "$SKILL_TARGET/references"
[ -f "$SKILL_DIST_DIR/FEEDBACK.md" ] && cp "$SKILL_DIST_DIR/FEEDBACK.md" "$SKILL_TARGET/FEEDBACK.md"

echo "  SKILL.md + references/ 已拷贝"

# 5. 验证
echo ""
echo "[4/4] 验证..."

if ! command -v gxp >/dev/null 2>&1; then
  echo "ERROR: gxp 仍不在 PATH 中"
  echo "  本次临时加入 PATH 也不行——可能是 Git Bash 缓存或 Windows PATH 重启后生效"
  exit 1
fi

echo "  gxp:    $(command -v gxp)"
echo "  version: $(gxp --version)"

if [ ! -f "$SKILL_TARGET/SKILL.md" ]; then
  echo "ERROR: SKILL.md 拷贝失败"
  exit 1
fi

echo "  skill:  $SKILL_TARGET/SKILL.md ($(wc -c < "$SKILL_TARGET/SKILL.md") bytes)"

echo ""
echo "=== 安装完成 ==="
echo ""
echo "下一步："
echo "  1. 新开 CodeAgent 会话，输入 /skills，应见 gxp"
echo "  2. 试触发：'帮我搜索日志中的 ERROR 行 + 上下文 3 行'"
echo "  3. 反馈：~/.cac/skills/gxp/FEEDBACK.md"
