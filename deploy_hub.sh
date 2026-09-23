#!/bin/bash

# Strict mode: Fail fast on errors and pipeline failures
set -euo pipefail

echo "========================================"
echo "   Kryneth Gateway Docker Publisher     "
echo "========================================"

# 1. Docker Login Check
echo "[1/4] Checking Docker Hub authentication..."
# We check if the user is logged in as kryenthhq
if ! docker info | grep -i "Username: kryenthhq" > /dev/null 2>&1; then
    echo "You are not logged in as 'kryenthhq'. Prompting for login..."
    docker login -u kryenthhq
else
    echo "Already authenticated as 'kryenthhq'."
fi

# 2. Buildx Initialization
echo "[2/4] Setting up multi-architecture builder..."
BUILDER_NAME="kryneth-multiarch-builder"

# Check if builder already exists
if ! docker buildx ls | grep -q "$BUILDER_NAME"; then
    echo "Creating new builder instance: $BUILDER_NAME"
    docker buildx create --name "$BUILDER_NAME" --use --bootstrap
else
    echo "Builder '$BUILDER_NAME' already exists. Using it."
    docker buildx use "$BUILDER_NAME"
fi

# 3. Build & Push
echo "[3/4] Building and pushing linux/amd64 and linux/arm64..."
IMAGE_TAG="kryenthhq/krynethgw:latest"
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    -t "$IMAGE_TAG" \
    --push \
    .

echo "Successfully built and pushed $IMAGE_TAG!"

# 4. Output Compose Snippet
echo "[4/4] Docker Compose Configuration"
echo "Use the following docker-compose.yml snippet on your Oracle Cloud ARM64 VM:"
echo "----------------------------------------------------------------------"
cat << 'EOF'
version: '3.8'

services:
  kryneth-gateway:
    image: kryenthhq/krynethgw:latest
    container_name: kryneth-gateway
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      # Add necessary environment variables here
      - RUST_LOG=info
EOF
echo "----------------------------------------------------------------------"
echo "Deployment pipeline complete!"
