# gxp 开发环境引导脚本（Windows + GNU toolchain）
#
# 用法（PowerShell）：
#   . .\scripts\env.ps1
#
# 作用：把 Rust GNU toolchain 与 scoop gcc 加入当前会话 PATH，
#       并设置 CARGO_HOME / RUSTUP_HOME（已持久化到用户级，
#       这里再次显式设置用于多机器/CI/新 shell 兜底）。
#
# 不修改系统注册表、不写 .cargo/config.toml — 那些由 scoop 持久化。
# 本脚本只保证"在任意新 PowerShell 会话里 source 一次即可用 cargo/rustc/gcc"。

# Scoop 根（默认 ~/scoop；装在自定义路径时由环境变量 SCOOP 提供）
$ScoopRoot = if ($env:SCOOP) { $env:SCOOP } else { "$env:USERPROFILE\scoop" }

# Rust GNU toolchain
$env:CARGO_HOME  = "$ScoopRoot\persist\rustup\.cargo"
$env:RUSTUP_HOME = "$ScoopRoot\persist\rustup\.rustup"

# PATH 前置：rustup 的 cargo/bin + scoop 的 gcc/bin
$env:PATH = "$ScoopRoot\apps\rustup\current\.cargo\bin;$ScoopRoot\apps\gcc\current\bin;$env:PATH"

# 校验
function Test-Toolchain {
    $ok = $true
    foreach ($t in @("cargo", "rustc", "gcc")) {
        $cmd = Get-Command $t -ErrorAction SilentlyContinue
        if ($cmd) {
            $ver = & $t --version 2>&1 | Select-Object -First 1
            Write-Host "  [OK]  $t => $ver" -ForegroundColor Green
        } else {
            Write-Host "  [MISS] $t not found in PATH" -ForegroundColor Red
            $ok = $false
        }
    }
    return $ok
}

Write-Host "[env] CARGO_HOME  = $env:CARGO_HOME" -ForegroundColor Cyan
Write-Host "[env] RUSTUP_HOME = $env:RUSTUP_HOME" -ForegroundColor Cyan
Write-Host "[env] PATH prefix = $ScoopRoot\apps\rustup\current\.cargo\bin;$ScoopRoot\apps\gcc\current\bin" -ForegroundColor Cyan
Write-Host "[env] verifying toolchain..." -ForegroundColor Cyan
$null = Test-Toolchain
