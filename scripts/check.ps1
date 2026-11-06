# gxp local one-shot check script
#
# Runs fmt --check / clippy -D warnings / test --no-fail-fast.
# Exits non-zero if any step fails.
# Three gate layers share this script (pre-commit hook uses -SkipTest; CI runs full).
#
# Usage:
#   .\scripts\check.ps1                 # full (default)
#   .\scripts\check.ps1 -Only fmt       # fmt only (fast feedback)
#   .\scripts\check.ps1 -Only clippy    # clippy only
#   .\scripts\check.ps1 -Only test      # test only
#   .\scripts\check.ps1 -SkipTest       # skip tests (pre-commit hook, fast)
#   .\scripts\check.ps1 -Quiet          # quiet mode (only print on failure)
#
# Exit code: 0=all green, 1=has failure
#
# Scenario adaptation (feedback_gate_config_adapts_to_context.md):
# - v1 solo: fmt + clippy + test trio
# - next: add cargo doc --no-deps / cargo build --all-features
# - retire when: v2 migrates to xtask, replaced by `cargo xtask check`

[CmdletBinding()]
param(
    [ValidateSet('fmt', 'clippy', 'test', $null)]
    [string]$Only = $null,

    [switch]$SkipTest,

    [switch]$Quiet
)

$ErrorActionPreference = 'Continue'  # don't let cargo stderr abort the script
$script:hadFailure = $false

function Invoke-Check {
    param(
        [string]$Name,
        [scriptblock]$Block
    )
    if ($Quiet) {
        $output = & $Block 2>&1
        $code = $LASTEXITCODE
        if ($code -ne 0) {
            Write-Host "[FAIL] $Name" -ForegroundColor Red
            $output | ForEach-Object { Write-Host "  $_" }
            $script:hadFailure = $true
        }
    } else {
        Write-Host "[..] $Name" -ForegroundColor Cyan
        & $Block
        $code = $LASTEXITCODE
        if ($code -ne 0) {
            Write-Host "[FAIL] $Name (exit=$code)" -ForegroundColor Red
            $script:hadFailure = $true
        } else {
            Write-Host "[PASS] $Name" -ForegroundColor Green
        }
    }
}

# 1. Source env.ps1 (idempotent)
$envScript = Join-Path $PSScriptRoot 'env.ps1'
if (Test-Path $envScript) {
    . $envScript | Out-Null
} else {
    Write-Host "[FAIL] env.ps1 not found at $envScript" -ForegroundColor Red
    exit 1
}

# 2. Decide what to run
$runFmt    = (-not $Only) -or ($Only -eq 'fmt')
$runClippy = (-not $Only) -or ($Only -eq 'clippy')
$runTest   = ((-not $Only) -or ($Only -eq 'test')) -and (-not $SkipTest)

# 3. fmt
if ($runFmt) {
    Invoke-Check 'fmt' {
        cargo fmt --check
        if ($LASTEXITCODE -ne 0) {
            Write-Host '  fix: cargo fmt' -ForegroundColor Yellow
        }
    }
}

# 4. clippy
if ($runClippy) {
    Invoke-Check 'clippy' {
        cargo clippy --all-features --all-targets -- -D warnings
    }
}

# 5. test
if ($runTest) {
    Invoke-Check 'test' {
        cargo test --all-features --no-fail-fast
    }
}

# 6. Summary
if ($script:hadFailure) {
    Write-Host "`n[check.ps1] FAILED: see [FAIL] lines above" -ForegroundColor Red
    exit 1
} else {
    $ran = @()
    if ($runFmt) { $ran += 'fmt' }
    if ($runClippy) { $ran += 'clippy' }
    if ($runTest) { $ran += 'test' }
    Write-Host "`n[check.ps1] all green: $($ran -join ', ')" -ForegroundColor Green
    exit 0
}

