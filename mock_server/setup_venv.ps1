# PowerShell setup & launch script for Kryneth Upstream Mock Server venv
$ErrorActionPreference = "Stop"

Write-Host "===========================================================" -ForegroundColor Cyan
Write-Host "🐍 Setting up Python Virtual Environment (venv)..." -ForegroundColor Cyan
Write-Host "===========================================================" -ForegroundColor Cyan

# Find python command (prefer 'py' on Windows, fallback to 'python')
$pythonCmd = if (Get-Command py -ErrorAction SilentlyContinue) { "py" } else { "python" }

$VENV_DIR = "mock_server\.venv"

if (-not (Test-Path $VENV_DIR)) {
    Write-Host "Creating Python venv at '$VENV_DIR'..." -ForegroundColor Yellow
    & $pythonCmd -m venv $VENV_DIR
    Write-Host "✓ Virtual environment created." -ForegroundColor Green
} else {
    Write-Host "✓ Virtual environment '$VENV_DIR' already exists." -ForegroundColor Green
}

$VENV_PYTHON = "$VENV_DIR\Scripts\python.exe"

Write-Host "Installing/Upgrading pip & requirements in venv..." -ForegroundColor Yellow
& $VENV_PYTHON -m pip install --upgrade pip --quiet
& $VENV_PYTHON -m pip install -r mock_server\requirements.txt

Write-Host "===========================================================" -ForegroundColor Green
Write-Host "🚀 Launching Upstream Mock Server on http://localhost:9090..." -ForegroundColor Green
Write-Host "===========================================================" -ForegroundColor Green

& $VENV_PYTHON -m uvicorn mock_server.main:app --host 0.0.0.0 --port 9090 --reload
