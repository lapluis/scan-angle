$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
    & cargo clippy --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    & cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
    & cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
    New-Item -ItemType Directory -Path dist -Force | Out-Null
    Copy-Item -LiteralPath target\release\scan-angle.exe -Destination dist\ScanAngle-Rust.exe -Force
    & .\dist\ScanAngle-Rust.exe --help
    if ($LASTEXITCODE -ne 0) { throw 'Packaged CLI smoke test failed.' }
    Write-Output (Join-Path $projectRoot 'dist\ScanAngle-Rust.exe')
} finally {
    Pop-Location
}

