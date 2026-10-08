# Non-destructive local release gate for Thorn Intelligence.
# GitHub-hosted runners may be blocked before job steps by account billing.
# Run from PowerShell 7 on Windows with Node 22 and Rust stable installed.
[CmdletBinding()]
param(
    [switch]$SkipInstall
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
function Invoke-Checked {
    param([string]$Command, [string[]]$Arguments)
    Write-Host ("> {0} {1}" -f $Command, ($Arguments -join ' ')) -ForegroundColor Cyan
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw ("Validation failed: {0} exited with code {1}" -f $Command, $LASTEXITCODE)
    }
}
foreach ($binary in @('node', 'npm.cmd', 'cargo', 'rustc')) {
    if (-not (Get-Command $binary -ErrorAction SilentlyContinue)) {
        throw "Missing prerequisite: $binary"
    }
}
Push-Location $root
try {
    if (-not $SkipInstall) {
        Invoke-Checked 'npm.cmd' @('install', '--no-audit', '--no-fund')
    }
    Invoke-Checked 'npm.cmd' @('run', 'build')
    Invoke-Checked 'cargo' @('fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--all', '--', '--check')
    Invoke-Checked 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml')
    Invoke-Checked 'cargo' @('check', '--manifest-path', 'src-tauri/Cargo.toml')
    Write-Host 'Frontend, Rust formatting, unit tests and compile check all passed.' -ForegroundColor Green
}
finally {
    Pop-Location
}
