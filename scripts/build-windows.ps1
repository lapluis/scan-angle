$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$pythonExe = Join-Path $projectRoot '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $pythonExe)) {
    throw 'Create .venv and install the project with the build extra first. See README.md.'
}
Push-Location $projectRoot
try {
    & $pythonExe -m unittest discover -s tests -v
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
    & $pythonExe -m PyInstaller --noconfirm --clean --noupx --onefile --windowed --name ScanAngle scan_angles.py
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
    Write-Output (Join-Path $projectRoot 'dist\ScanAngle.exe')
} finally {
    Pop-Location
}
