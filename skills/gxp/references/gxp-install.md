# gxp 安装引导（供 agent 在 `gxp --version` 不可用时读取）

> 核心原则：**不自动安装**——安装必须经用户明确批准；装不了就降级，不硬试不空转。
> exe 装到**全局用户级 PATH**（无 agent 专属路径）：任何能跑 shell 的 agent 直接 `gxp ...` 即可。

## 分级流程

1. **检测**：`gxp --version`
2. **不可用时**：向用户说明需要安装 gxp（独立的 Rust CLI，预编译包 4 平台），给出选项并等待批准：

   **a. 预编译二进制（推荐，零工具链依赖）**

   ```bash
   curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh
   ```

   自动探测 OS/ARCH → 下载 GitHub Release 预编译包（Linux / Windows / macOS arm64 / macOS x64）→ sha256 校验 → 装入 PATH。前置仅 curl + tar/unzip；Windows 需在 Git Bash 运行。

   **b. Windows 原生（无 Git Bash）**：从 [Releases](https://github.com/cianlabs/gxp/releases) 手动下载 `gxp-*-x86_64-pc-windows-msvc.zip`，解压到 `%LOCALAPPDATA%\gxp\` 并加入用户 PATH。

   **c. 已有 Rust 工具链时的备选**：`cargo install --locked gxp`

3. **装完验证 + PATH 坑**：**当前会话不会继承新 PATH**——装完 `command -v gxp` 仍空属正常，不要误判安装失败。用 install.sh 结尾打印的绝对路径调用（默认：`~/.local/bin/gxp`、Windows `~/AppData/Local/gxp/gxp.exe`、cargo bin `~/.cargo/bin/gxp`），新开的会话/终端起 PATH 自然生效。

4. **降级出口**：环境装不了（无网 / 沙箱只读）或用户拒绝 → 本 skill 不适用，明确改用环境内置搜索工具（grep 等）完成当前任务，不要反复重试 gxp。
