# gxp skill 安装指南

> 用途：把 gxp 二进制 + skill 文档装到本机，供 CodeAgent 在所有项目使用
> 前置依赖：CodeAgent CLI（`~/.cac/` 存在）

## 分发内容

```
dist/skill/
├── SKILL.md           ← skill 主入口（YAML frontmatter + 指令正文）
├── INSTALL.md         ← 本文档
├── FEEDBACK.md        ← 整改意见收集
├── bin/
│   └── gxp.exe        ← Windows 二进制（随包分发，无需 cargo install）
├── references/
│   ├── gxp-syntax.md     ← BARE + DSL + 13 管道动词 + AST + CLI flag 速查
│   ├── gxp-examples.md   ← A-H 8 场景查询模板
│   └── gxp-bootstrap.md  ← 自举约束 + Edit 后引用检查清单
└── scripts/
    └── install.sh     ← 一键安装脚本
```

## 平台支持

| 平台 | 二进制 | 默认安装路径 | PATH 配置 |
|---|---|---|---|
| Windows | `bin/gxp.exe`（随包） | `$HOME/AppData/Local/gxp/` | 自动 PowerShell `SetEnvironmentVariable` 用户级 |
| Linux | 需源码编译 | `$HOME/.local/bin/` | 自动写入 `~/.bashrc` 或 `~/.zshrc` |
| macOS | 需源码编译 | `$HOME/.local/bin/` | 自动写入 `~/.bashrc` 或 `~/.zshrc` |

> 有 Rust 工具链（cargo 在 PATH 中）时，install.sh 优先装到 cargo bin（已在 PATH，零摩擦）。

## Windows 一键安装（推荐）

```bash
# 在 Git Bash 或 MSYS2 终端
cd <gxp_repo>/dist/skill/scripts
./install.sh
```

脚本自动完成：
1. 拷贝 `dist/skill/bin/gxp.exe` 到 `~/AppData/Local/gxp/gxp.exe`（用户级，无需 admin）
2. 若该目录不在 PATH 中，**自动**用 PowerShell `[Environment]::SetEnvironmentVariable` 永久加入用户级 PATH
3. 拷贝 SKILL.md + references/ + FEEDBACK.md 到 `~/.cac/skills/gxp/`
4. 验证 `gxp --version` + skill 文件结构

**安装完成后请新开 Git Bash 窗口让 PATH 生效。**

## Windows 手动安装

### 步骤 1：拷贝 gxp.exe 到目标目录

推荐用户级目录 `~/AppData/Local/gxp/`（无需 admin）：

```bash
mkdir -p ~/AppData/Local/gxp
cp dist/skill/bin/gxp.exe ~/AppData/Local/gxp/gxp.exe
```

也可装到 PATH 中已有的目录（如 cargo bin 或 scoop shims）：

```bash
# 探测可用目录（任选其一）
command -v cargo && dirname "$(command -v cargo)"   # cargo bin（若有 Rust 工具链）
ls /d/soft/scoop/shims/ 2>&1 | head -5               # scoop shims（若有 scoop）
```

### 步骤 2：把目标目录加入 PATH（永久用户级）

```powershell
# PowerShell（永久用户级，无需管理员）
$targetDir = "$env:LOCALAPPDATA\gxp"
$currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($currentPath -notlike "*$targetDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$currentPath;$targetDir", 'User')
}
```

加完后**新开 Git Bash 窗口**让 PATH 生效。

### 步骤 3：验证 gxp 在 PATH 中

```bash
gxp --version      # 应输出 gxp 0.42.0
```

### 步骤 4：拷贝 skill 到 CodeAgent skill 目录

```bash
SKILL_TARGET="$HOME/.cac/skills/gxp"
mkdir -p "$SKILL_TARGET"
cp dist/skill/SKILL.md "$SKILL_TARGET/SKILL.md"
cp -r dist/skill/references "$SKILL_TARGET/references"
cp dist/skill/FEEDBACK.md "$SKILL_TARGET/FEEDBACK.md"
```

### 步骤 5：验证 skill 加载

新开 CodeAgent 会话，输入 `/skills`，应看到 `gxp` skill。

## Linux/macOS 编译（源码安装）

Linux/macOS 二进制不随包分发（需对应 glibc / musl / arm64 变体，且本项目目前只测 Windows）。从源码编译：

### 前置依赖

- Rust 工具链（rustup + cargo）：https://www.rust-lang.org/tools/install

### 编译 + 安装

```bash
cd <gxp_repo>
cargo install --path . --force
# 装到 $CARGO_HOME/bin（默认 ~/.cargo/bin）
```

### 确保 $CARGO_HOME/bin 在 PATH 中

```bash
echo 'export PATH="$PATH:$CARGO_HOME/bin"' >> ~/.bashrc
source ~/.bashrc
gxp --version    # 验证
```

### 拷贝 skill 文件

同 Windows 步骤 4。

## 升级

```bash
# 拉取最新 dist/skill/（含新 gxp.exe + skill 文档）
cd <gxp_repo>
git pull

# 重跑 install.sh（覆盖安装）
cd dist/skill/scripts
./install.sh
```

Linux/macOS：

```bash
cd <gxp_repo>
git pull
cargo install --path . --force   # 重新编译
# skill 文件如未变，无需重新拷贝
```

## 卸载

```bash
# 1. 删 gxp 二进制
rm ~/AppData/Local/gxp/gxp.exe          # Windows（默认目标）
rm $CARGO_HOME/bin/gxp                 # Linux/macOS（或 cargo uninstall gxp）

# 2. 删 skill
rm -rf ~/.cac/skills/gxp

# 3. （可选）从 PATH 移除目标目录
# Windows PowerShell：
#   $targetDir = "$env:LOCALAPPDATA\gxp"
#   $currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')
#   $newPath = ($currentPath -split ';' | Where-Object { $_ -ne $targetDir }) -join ';'
#   [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
# Linux/macOS：编辑 ~/.bashrc 或 ~/.zshrc 删除对应 export 行
```

## 故障排查

### Q: `gxp: command not found`

A: 目标目录不在 PATH。检查：

```bash
command -v gxp          # 应输出完整路径；空则未在 PATH
echo $PATH | tr ':' '\n' | grep -i <目标目录>
```

Windows：新开 Git Bash 窗口（PowerShell 改 PATH 后当前 session 不生效）。
Linux/macOS：`source ~/.bashrc` 或新开终端。

### Q: install.sh 报 "不存在此平台的 gxp 二进制"

A: 你在 Linux/macOS 上跑 install.sh，但 `dist/skill/bin/` 下只有 `gxp.exe`。Linux/macOS 需源码编译，见"Linux/macOS 编译"一节。

### Q: skill 不被 CodeAgent 识别

A: 检查 SKILL.md frontmatter：

```bash
head -5 ~/.cac/skills/gxp/SKILL.md
# 应以 --- 开头，含 name 和 description 字段
# 文件无 BOM（od -c -N 6 应输出 ---\nna，不是 \xef\xbb\xbf---）
```

### Q: PowerShell 改了 PATH 但 Git Bash 还找不到 gxp

A: Git Bash 启动时缓存 PATH，PowerShell 改用户级 PATH 后**必须新开 Git Bash 窗口**才生效。当前 session 即使重启 bash 也不行——只能新开窗口。

### Q: 评审发现 skill 内容有误

A: 写入 `~/.cac/skills/gxp/FEEDBACK.md`，gxp 维护者定期同步处理。
