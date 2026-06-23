# Kryneth Gateway OSS - One-Command Setup Script (PowerShell)
# Automates the entire setup process for local development and Docker deployment

$ErrorActionPreference = "Stop"

# Colors
function Write-Success { Write-Host $args -ForegroundColor Green }
function Write-Error-Custom { Write-Host $args -ForegroundColor Red }
function Write-Warning-Custom { Write-Host $args -ForegroundColor Yellow }
function Write-Info { Write-Host $args -ForegroundColor Cyan }

# Banner
Write-Info "`n╭─────────────────────────────────────────────────────────────╮"
Write-Info "│                 Kryneth Gateway OSS Setup                   │"
Write-Info "│       The Production Reliability Layer for AI Agents        │"
Write-Info "╰─────────────────────────────────────────────────────────────╯`n"

# Check if running from correct directory
if (-not (Test-Path "Cargo.toml")) {
    Write-Error-Custom "❌ Error: Cargo.toml not found. Run this script from the project root."
    exit 1
}

Write-Warning-Custom "📋 Checking prerequisites..."

# Check for required tools
$prereqs = @{
    "git" = $false
    "docker" = $false
    "cargo" = $false
}

if (Get-Command git -ErrorAction SilentlyContinue) {
    Write-Success "✓ Git"
    $prereqs["git"] = $true
} else {
    Write-Error-Custom "❌ Git is not installed"
    exit 1
}

if (Get-Command docker -ErrorAction SilentlyContinue) {
    Write-Success "✓ Docker"
    $prereqs["docker"] = $true
} else {
    Write-Warning-Custom "⚠ Docker not found (optional for local development)"
}

if (Get-Command cargo -ErrorAction SilentlyContinue) {
    Write-Success "✓ Rust"
    $prereqs["cargo"] = $true
} else {
    Write-Warning-Custom "⚠ Rust/Cargo not found (required for local development)"
    Write-Info "   Install from: https://rustup.rs/"
}

Write-Warning-Custom "`n📦 Setting up configuration files..."

# Step 1: Copy .env file
if (-not (Test-Path ".env")) {
    Copy-Item "env.example" ".env"
    Write-Success "✓ Created .env (copy of env.example)"
    Write-Info "   → Edit .env to add your API keys"
} else {
    Write-Warning-Custom "⚠ .env already exists"
}

# Step 2: Copy routing.yaml file
if (-not (Test-Path "routing.yaml")) {
    Copy-Item "routing.yaml.example" "routing.yaml"
    Write-Success "✓ Created routing.yaml (copy of routing.yaml.example)"
    Write-Info "   → Customize routing.yaml for your providers"
} else {
    Write-Warning-Custom "⚠ routing.yaml already exists"
}

Write-Warning-Custom "`n📋 Configuration checklist:"

# Parse API keys from .env
$envContent = Get-Content ".env" -Raw
$groqKey = if ($envContent -match 'GROQ_API_KEY=(.+)') { $Matches[1].Trim() } else { "" }
$cohereKey = if ($envContent -match 'COHERE_API_KEY=(.+)') { $Matches[1].Trim() } else { "" }
$openaiKey = if ($envContent -match 'OPENAI_API_KEY=(.+)') { $Matches[1].Trim() } else { "" }
$anthropicKey = if ($envContent -match 'ANTHROPIC_API_KEY=(.+)') { $Matches[1].Trim() } else { "" }

if ([string]::IsNullOrEmpty($groqKey) -or $groqKey -eq "gsk_your_groq_key_here") {
    Write-Error-Custom "✗ GROQ_API_KEY - Add your key from https://console.groq.com"
} else {
    Write-Success "✓ GROQ_API_KEY - Configured"
}

if ([string]::IsNullOrEmpty($cohereKey)) {
    Write-Warning-Custom "⚠ COHERE_API_KEY - Optional (failover provider)"
} else {
    Write-Success "✓ COHERE_API_KEY - Configured"
}

if ([string]::IsNullOrEmpty($openaiKey)) {
    Write-Warning-Custom "⚠ OPENAI_API_KEY - Optional (if using OpenAI)"
} else {
    Write-Success "✓ OPENAI_API_KEY - Configured"
}

if ([string]::IsNullOrEmpty($anthropicKey)) {
    Write-Warning-Custom "⚠ ANTHROPIC_API_KEY - Optional (if using Anthropic)"
} else {
    Write-Success "✓ ANTHROPIC_API_KEY - Configured"
}

Write-Warning-Custom "`n🚀 Deployment options:`n"

if ($prereqs["docker"]) {
    Write-Info "Option 1: Docker Compose (Recommended)"
    Write-Info "   docker-compose up -d --build"
    Write-Info "   → Kryneth will be available at http://localhost:8080`n"
}

if ($prereqs["cargo"]) {
    Write-Info "Option 2: Local Development"
    Write-Info "   cargo run --release"
    Write-Info "   → Kryneth will be available at http://localhost:8080`n"
}

if ($prereqs["docker"]) {
    Write-Info "Option 3: Raw Docker"
    Write-Info "   docker build -t kryneth-gateway:latest ."
    Write-Info "   docker run -p 8080:8080 --env-file .env -v `$(pwd)/routing.yaml:/app/routing.yaml kryneth-gateway:latest`n"
}

Write-Warning-Custom "✅ Test your setup:`n"
Write-Info "   `$headers = @{'Content-Type'='application/json'}"
Write-Info "   `$body = @{"
Write-Info "       model = 'llama-3.3-70b-versatile'"
Write-Info "       messages = @(@{role = 'user'; content = 'Hello!'})"
Write-Info "   } | ConvertTo-Json"
Write-Info "   Invoke-WebRequest -Uri 'http://localhost:8080/v1/chat/completions' -Method Post -Headers `$headers -Body `$body`n"

Write-Warning-Custom "📚 Documentation:"
Write-Info "   Getting Started: ./docs/GETTING_STARTED.mdx"
Write-Info "   Configuration:   ./docs/CONFIGURATION.md"
Write-Info "   API Reference:   ./docs/API.md`n"

Write-Success "🎉 Setup complete!`n"

Write-Warning-Custom "Next steps:"
if ([string]::IsNullOrEmpty($groqKey) -or $groqKey -eq "gsk_your_groq_key_here") {
    Write-Info "1. Add your API keys to .env"
    Write-Info "2. Customize routing.yaml for your providers"
    Write-Info "3. Run: docker-compose up -d --build (or: cargo run --release)"
} else {
    Write-Info "1. API keys are configured in .env"
    Write-Info "2. Run: docker-compose up -d --build (or: cargo run --release)"
    Write-Info "3. Test with: Invoke-WebRequest http://localhost:8080/health"
}
Write-Info ""
