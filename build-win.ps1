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

# Ensure node/pnpm are in PATH
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    Write-Error "pnpm not found. Install: npm install -g pnpm"
    exit 1
}

# Ensure tauri-cli is available
if (-not (Get-Command "cargo-tauri" -ErrorAction SilentlyContinue)) {
    Write-Host "[build] Installing tauri-cli..." -ForegroundColor Cyan
    cargo install tauri-cli
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

# 2. Build core daemon
Write-Host "[build] Building rootcx-core..." -ForegroundColor Cyan
cargo build --release --target $TARGET -p rootcx-core
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed"; exit 1 }

# 3. Copy sidecar binary for Tauri
$src = "target\$TARGET\release\rootcx-core.exe"
$dst = "studio\src-tauri\rootcx-core-$TARGET.exe"
Copy-Item $src $dst -Force
Write-Host "[build] Sidecar: $dst" -ForegroundColor Cyan

# 4. Build frontend (Tauri's beforeBuildCommand has CWD issues in workspace setups)
Write-Host "[build] Building frontend..." -ForegroundColor Cyan
pnpm --dir studio/ui install
if ($LASTEXITCODE -ne 0) { Write-Error "pnpm install failed"; exit 1 }
pnpm --dir studio/ui build
if ($LASTEXITCODE -ne 0) { Write-Error "pnpm build failed"; exit 1 }

# 5. Build Tauri NSIS installer
Write-Host "[build] Building NSIS installer..." -ForegroundColor Cyan
cargo tauri build --target $TARGET --bundles nsis --config studio/src-tauri/tauri.build.json
if ($LASTEXITCODE -ne 0) { Write-Error "tauri build failed"; exit 1 }

# 6. Show output
$nsis = Get-ChildItem "target\$TARGET\release\bundle\nsis\*.exe" -ErrorAction SilentlyContinue
if ($nsis) {
    Write-Host "`n[build] Done! Installer:" -ForegroundColor Green
    $nsis | ForEach-Object { Write-Host "  $_" -ForegroundColor Green }
} else {
    Write-Error "NSIS output not found"
}
