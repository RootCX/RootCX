# Download platform-specific runtime dependencies (Bun) into core/resources/.
# Usage: powershell -ExecutionPolicy Bypass -File scripts/fetch-deps.ps1 [TARGET]

param(
    [string]$Target = (rustc -vV | Select-String 'host:').ToString().Split(' ')[-1]
)

$ErrorActionPreference = "Stop"

$BunVersion = "1.3.10"
if ($env:ROOTCX_BUN_VERSION) { $BunVersion = $env:ROOTCX_BUN_VERSION }

$Resources = Join-Path $PSScriptRoot "..\core\resources"
New-Item -ItemType Directory -Force -Path $Resources | Out-Null

# --- Bun ----------------------------------------------------------------------

$bunBin = Join-Path $Resources "bun.exe"

if (Test-Path $bunBin) {
    Write-Host "[fetch-deps] Bun already present, skipping."
} else {
    $bunTarget = $null
    switch ($Target) {
        "x86_64-pc-windows-msvc"  { $bunTarget = "bun-windows-x64" }
        "aarch64-pc-windows-msvc" { $bunTarget = "bun-windows-x64" }
        default { throw "No Bun binary for target: $Target" }
    }
    $bunUrl = "https://github.com/oven-sh/bun/releases/download/bun-v$BunVersion/$bunTarget.zip"
    $bunTmp = Join-Path $env:TEMP "rootcx-bun.zip"
    $bunExtract = Join-Path $env:TEMP "rootcx-bun-extract"
    Write-Host "[fetch-deps] Downloading Bun $BunVersion for $Target..."
    curl.exe -fsSL --retry 3 -o $bunTmp $bunUrl
    if ($LASTEXITCODE -ne 0) { throw "Bun download failed" }
    Expand-Archive -Path $bunTmp -DestinationPath $bunExtract -Force
    Copy-Item (Join-Path $bunExtract "$bunTarget\bun.exe") $bunBin -Force
    Remove-Item $bunTmp, $bunExtract -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "[fetch-deps] Bun ready at $bunBin"
}

Write-Host "[fetch-deps] All dependencies ready in $Resources"
