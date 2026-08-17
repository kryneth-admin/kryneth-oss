#!/usr/bin/env bash
set -e

echo "==========================================================="
echo "🐍 Setting up Python Virtual Environment (venv)..."
echo "==========================================================="

VENV_DIR="mock_server/.venv"

if [ ! -d "$VENV_DIR" ]; then
    echo "Creating Python venv at '$VENV_DIR'..."
    python3 -m venv "$VENV_DIR"
    echo "✓ Virtual environment created."
else
    echo "✓ Virtual environment '$VENV_DIR' already exists."
fi

VENV_PYTHON="$VENV_DIR/bin/python"

echo "Installing/Upgrading pip & requirements in venv..."
"$VENV_PYTHON" -m pip install --upgrade pip --quiet
"$VENV_PYTHON" -m pip install -r mock_server/requirements.txt

echo "==========================================================="
echo "🚀 Launching Upstream Mock Server on http://localhost:9090..."
echo "==========================================================="

"$VENV_PYTHON" -m uvicorn mock_server.main:app --host 0.0.0.0 --port 9090 --reload
