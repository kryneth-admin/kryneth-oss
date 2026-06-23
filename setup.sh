#!/bin/bash
# Kryneth Gateway OSS - One-Command Setup Script
# Automates the entire setup process for local development and Docker deployment

set -e  # Exit on any error

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Banner
echo -e "${BLUE}"
cat << "EOF"
╭─────────────────────────────────────────────────────────────╮
│                 Kryneth Gateway OSS Setup                   │
│       The Production Reliability Layer for AI Agents        │
╰─────────────────────────────────────────────────────────────╯
EOF
echo -e "${NC}"

# Check if running from correct directory
if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}❌ Error: Cargo.toml not found. Run this script from the project root.${NC}"
    exit 1
fi

echo -e "${YELLOW}📋 Checking prerequisites...${NC}"

# Check for required tools
if ! command -v git &> /dev/null; then
    echo -e "${RED}❌ Git is not installed${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Git${NC}"

# Check for Docker (optional for local dev)
if command -v docker &> /dev/null; then
    echo -e "${GREEN}✓ Docker${NC}"
    DOCKER_AVAILABLE=true
else
    echo -e "${YELLOW}⚠ Docker not found (optional for local development)${NC}"
    DOCKER_AVAILABLE=false
fi

# Check for Rust (required for local dev)
if ! command -v cargo &> /dev/null; then
    echo -e "${YELLOW}⚠ Rust/Cargo not found (required for local development)${NC}"
    echo -e "${BLUE}   Install from: https://rustup.rs/${NC}"
    RUST_AVAILABLE=false
else
    echo -e "${GREEN}✓ Rust ($(cargo --version))${NC}"
    RUST_AVAILABLE=true
fi

echo ""
echo -e "${YELLOW}📦 Setting up configuration files...${NC}"

# Step 1: Copy .env file
if [ ! -f ".env" ]; then
    cp env.example .env
    echo -e "${GREEN}✓ Created .env (copy of env.example)${NC}"
    echo -e "${BLUE}   → Edit .env to add your API keys${NC}"
else
    echo -e "${YELLOW}⚠ .env already exists${NC}"
fi

# Step 2: Copy routing.yaml file
if [ ! -f "routing.yaml" ]; then
    cp routing.yaml.example routing.yaml
    echo -e "${GREEN}✓ Created routing.yaml (copy of routing.yaml.example)${NC}"
    echo -e "${BLUE}   → Customize routing.yaml for your providers${NC}"
else
    echo -e "${YELLOW}⚠ routing.yaml already exists${NC}"
fi

echo ""
echo -e "${YELLOW}📋 Configuration checklist:${NC}"

# Parse API keys from .env
GROQ_KEY=$(grep "GROQ_API_KEY" .env | cut -d'=' -f2 | xargs)
COHERE_KEY=$(grep "COHERE_API_KEY" .env | cut -d'=' -f2 | xargs)
OPENAI_KEY=$(grep "OPENAI_API_KEY" .env | cut -d'=' -f2 | xargs)
ANTHROPIC_KEY=$(grep "ANTHROPIC_API_KEY" .env | cut -d'=' -f2 | xargs)

if [ -z "$GROQ_KEY" ] || [ "$GROQ_KEY" = "gsk_your_groq_key_here" ]; then
    echo -e "${RED}✗ GROQ_API_KEY${NC} - Add your key from https://console.groq.com"
else
    echo -e "${GREEN}✓ GROQ_API_KEY${NC} - Configured"
fi

if [ -z "$COHERE_KEY" ] || [ "$COHERE_KEY" = "h28_your_cohere_key_here" ]; then
    echo -e "${YELLOW}⚠ COHERE_API_KEY${NC} - Optional (failover provider)"
else
    echo -e "${GREEN}✓ COHERE_API_KEY${NC} - Configured"
fi

if [ -z "$OPENAI_KEY" ]; then
    echo -e "${YELLOW}⚠ OPENAI_API_KEY${NC} - Optional (if using OpenAI)"
else
    echo -e "${GREEN}✓ OPENAI_API_KEY${NC} - Configured"
fi

if [ -z "$ANTHROPIC_KEY" ]; then
    echo -e "${YELLOW}⚠ ANTHROPIC_API_KEY${NC} - Optional (if using Anthropic)"
else
    echo -e "${GREEN}✓ ANTHROPIC_API_KEY${NC} - Configured"
fi

echo ""
echo -e "${YELLOW}🚀 Deployment options:${NC}"
echo ""

# Option 1: Docker Compose (if Docker available)
if [ "$DOCKER_AVAILABLE" = true ]; then
    echo -e "${BLUE}Option 1: Docker Compose (Recommended)${NC}"
    echo "   docker-compose up -d --build"
    echo "   → Kryneth will be available at http://localhost:8080"
    echo ""
fi

# Option 2: Local Cargo
if [ "$RUST_AVAILABLE" = true ]; then
    echo -e "${BLUE}Option 2: Local Development${NC}"
    echo "   cargo run --release"
    echo "   → Kryneth will be available at http://localhost:8080"
    echo ""
fi

# Option 3: Docker build (if Docker available)
if [ "$DOCKER_AVAILABLE" = true ]; then
    echo -e "${BLUE}Option 3: Raw Docker${NC}"
    echo "   docker build -t kryneth-gateway:latest ."
    echo "   docker run -p 8080:8080 --env-file .env -v \$(pwd)/routing.yaml:/app/routing.yaml kryneth-gateway:latest"
    echo ""
fi

echo -e "${YELLOW}✅ Test your setup:${NC}"
echo ""
echo -e "${BLUE}   curl -X POST http://localhost:8080/v1/chat/completions \\${NC}"
echo -e "${BLUE}     -H 'Content-Type: application/json' \\${NC}"
echo -e "${BLUE}     -d '{${NC}"
echo -e "${BLUE}       \"model\": \"llama-3.3-70b-versatile\",${NC}"
echo -e "${BLUE}       \"messages\": [{\"role\": \"user\", \"content\": \"Hello!\"}]${NC}"
echo -e "${BLUE}     }'${NC}"
echo ""

echo -e "${YELLOW}📚 Documentation:${NC}"
echo "   Getting Started: ./docs/GETTING_STARTED.mdx"
echo "   Configuration:   ./docs/CONFIGURATION.md"
echo "   API Reference:   ./docs/API.md"
echo ""

echo -e "${GREEN}🎉 Setup complete!${NC}"
echo ""
echo -e "${YELLOW}Next steps:${NC}"
if [ -z "$GROQ_KEY" ] || [ "$GROQ_KEY" = "gsk_your_groq_key_here" ]; then
    echo "1. Add your API keys to .env"
    echo "2. Customize routing.yaml for your providers"
    echo "3. Run: docker-compose up -d --build (or: cargo run --release)"
else
    echo "1. API keys are configured in .env"
    echo "2. Run: docker-compose up -d --build (or: cargo run --release)"
    echo "3. Test with: curl http://localhost:8080/health"
fi
echo ""
