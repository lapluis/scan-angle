$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    & uv sync --locked
    if ($LASTEXITCODE -ne 0) { throw 'Dependency sync failed.' }
    & uv run --locked python -m unittest discover -s tests -v
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
    & uv run --locked python -m PyInstaller --noconfirm --clean --noupx --onefile --windowed --paths src --name ScanAngle main.py
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
    Write-Output (Join-Path $projectRoot 'dist\ScanAngle.exe')
} finally {
    Pop-Location
}
