# RootCX Windows build script
# Usage: .\build-win.ps1
# Run from "Developer PowerShell for VS 2022" for MSVC linker access.

$ErrorActionPreference = "Stop"
$TARGET = "x86_64-pc-windows-msvc"

# Ensure cargo/rustc are in PATH
if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    $cargobin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path $cargobin) {
        $env:PATH += ";$cargobin"
    } else {
        Write-Error "rustc not found. Install Rust: https://rustup.rs"
        exit 1
    }
}

# Ensure link.exe is available (MSVC)
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    Write-Error "link.exe not found. Run this script from 'Developer PowerShell for VS 2022'."
    exit 1
}

Write-Host "[build] Target: $TARGET" -ForegroundColor Cyan

# 1. Fetch dependencies (PostgreSQL + Bun)
Write-Host "[build] Fetching dependencies..." -ForegroundColor Cyan
powershell -ExecutionPolicy Bypass -File scripts\fetch-deps.ps1 $TARGET
if ($LASTEXITCODE -ne 0) { Write-Error "fetch-deps failed"; exit 1 }

# Build the server and command-line client.
Write-Host "[build] Building Core and CLI..." -ForegroundColor Cyan
cargo build --locked --release --target $TARGET -p rootcx-core -p rootcx-cli
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed"; exit 1 }
Write-Host "[build] Binaries: target\$TARGET\release\rootcx-core.exe and rootcx.exe" -ForegroundColor Green
