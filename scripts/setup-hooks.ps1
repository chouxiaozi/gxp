# gxp git hook installer
#
# Copies scripts/hooks/pre-commit to .git/hooks/pre-commit and sets executable.
# Use -Remove to uninstall.
#
# Usage:
#   .\scripts\setup-hooks.ps1           # install
#   .\scripts\setup-hooks.ps1 -Remove   # uninstall
#
# Why not core.hooksPath: that path is shared across all clones and not portable.
# .git/hooks/ is per-clone, needs setup after each fresh clone. This installer
# is the one-command bridge.
#
# Scenario adaptation (feedback_gate_config_adapts_to_context.md):
# - v1 solo: pre-commit only (fmt + clippy)
# - next: add pre-push hook (cargo test --no-fail-fast) when team grows
# - retire when: v2 migrates to xtask + cargo-husky or core.hooksPath in repo

[CmdletBinding()]
param(
    [switch]$Remove
)

$repoRoot = (git rev-parse --show-toplevel 2>&1)
if ($LASTEXITCODE -ne 0) {
    Write-Host '[setup-hooks] not in a git repo' -ForegroundColor Red
    exit 1
}

$gitDir = (git rev-parse --git-dir 2>&1) | Out-String
$gitDir = $gitDir.Trim()
$hooksDir = Join-Path $repoRoot (Join-Path $gitDir 'hooks')

if (-not (Test-Path $hooksDir)) {
    New-Item -ItemType Directory -Path $hooksDir -Force | Out-Null
}

$hookName = 'pre-commit'
$srcHook = Join-Path $PSScriptRoot (Join-Path 'hooks' $hookName)
$dstHook = Join-Path $hooksDir $hookName

if ($Remove) {
    if (Test-Path $dstHook) {
        Remove-Item $dstHook -Force
        Write-Host "[setup-hooks] removed $dstHook" -ForegroundColor Green
    } else {
        Write-Host "[setup-hooks] $dstHook does not exist (nothing to remove)" -ForegroundColor Yellow
    }
    exit 0
}

# Validate source exists
if (-not (Test-Path $srcHook)) {
    Write-Host "[setup-hooks] source hook not found: $srcHook" -ForegroundColor Red
    exit 1
}

# Copy (overwrite if exists)
Copy-Item -Path $srcHook -Destination $dstHook -Force

# Set executable bit (mingw bash needs +x; on Windows, ACL allows by default but be explicit)
# Use icacls to ensure execute permission (Windows equivalent of chmod +x)
# Note: git for Windows mingw bash checks +x via ACL; icacls is the safe cross-version way.
# Fallback: if icacls fails, the file is still copy-executable in git bash by default.
try {
    & icacls $dstHook /grant "*S-1-5-32-545:(RX)" 2>&1 | Out-Null
} catch {
    # non-fatal: git for Windows often accepts hooks without explicit +x
    Write-Host "[setup-hooks] icacls failed (non-fatal): $_" -ForegroundColor Yellow
}

Write-Host "[setup-hooks] installed $hookName -> $dstHook" -ForegroundColor Green
Write-Host "[setup-hooks] pre-commit runs: fmt --check + clippy -D warnings (no tests)" -ForegroundColor Cyan
Write-Host "[setup-hooks] bypass with: git commit --no-verify" -ForegroundColor Cyan
exit 0
