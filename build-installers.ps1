$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoRoot

# Packaging stays with this checkout: `binaries-dir` and `out-dir` in Cargo.toml are
# relative to it, and the CI workflow uploads from those same places. The machine-local
# shared target dir in `.cargo/target-dir.local.toml` is opted out of here rather than
# followed. CI is unaffected: there the default target dir is this same directory.
$env:CARGO_TARGET_DIR = Join-Path $repoRoot "target"

if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) {
    Write-Host "cargo-packager not found. Installing with cargo..."
    cargo install cargo-packager --locked --version 0.11.8
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

# cargo-packager writes its own downloaded NSIS into Local AppData, and a sandboxed
# shell is refused that write before installer generation starts.
$localAppData = [System.Environment]::GetFolderPath("LocalApplicationData")
$packagerToolsRoot = Join-Path $localAppData ".cargo-packager"
try {
    New-Item -ItemType Directory -Force -Path $packagerToolsRoot | Out-Null
}
catch {
    Write-Error @"
cargo-packager needs write access to its Windows tool cache:
$packagerToolsRoot

Windows denied access before installer generation could start. Run this script from
a normal, non-sandboxed PowerShell session for your user, or grant your current
user write access to Local AppData.

Original error:
$($_.Exception.Message)
"@
    exit 1
}

# The binary the installer carries is the one this build links, so it is built here and
# from this checkout's own target dir (see the note above): an exe another checkout left
# at a shared target dir is not one this installer should carry.
Write-Host "Building release binary..."
cargo build --release
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Building NSIS installer..."
cargo packager --formats nsis
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$outputDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "target\packager"))

Write-Host ""
Write-Host "Installers generated in:"
Write-Host $outputDir

if (-not (Test-Path -LiteralPath $outputDir)) {
    Write-Warning "Output directory was not created. No installers were found."
    exit 0
}

$installers = Get-ChildItem -LiteralPath $outputDir -File |
    Where-Object { $_.Name -like "*-setup.exe" }

if (-not $installers) {
    Write-Warning "No .exe installers were found in the output directory."
    exit 0
}

$installers | ForEach-Object { Write-Host (" - " + $_.FullName) }
