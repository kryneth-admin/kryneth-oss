# Docker Hub Deployment Guide

Kryneth Gateway is available on Docker Hub for easy production and development deployments.

---

## 📦 Docker Hub Images

| Image | Use Case | Size |
|-------|----------|------|
| `krynethgw/kryneth-gateway:latest` | Production-ready, multi-arch | ~45MB |
| `krynethgw/kryneth-gateway:latest-slim` | Minimal footprint | ~35MB |
| `krynethgw/kryneth-gateway:v0.1.0` | Pinned version (immutable) | ~45MB |

**Supported Platforms:**
- `linux/amd64` (x86_64 - Intel/AMD)
- `linux/arm64` (ARM64 - Apple Silicon, AWS Graviton)
- `linux/arm/v7` (ARM 32-bit - Raspberry Pi)

---

## 🚀 Quick Start

### 1. Pull the Latest Image
```bash
docker pull krynethgw/kryneth-gateway:latest
```

### 2. Create Configuration Files

### 2. Configure Environment Variables

Create a strict `.env` file to control the gateway's behavior and inject LLM credentials securely. 

| Variable | Requirement | Default | Description |
| :--- | :---: | :---: | :--- |
| `GATEWAY_PORT` | **Required** | `8080` | The port the control plane binds to. |
| `KRYNETH_VALID_KEYS` | **Required** | None | Comma-separated list of Bearer tokens allowed to hit the ingress. |
| `[PROVIDER]_API_KEY` | **Required** | None | Your upstream API keys (e.g., `OPENAI_API_KEY`, `GROQ_API_KEY`). |
| `MAX_SESSION_TOOL_CALLS` | Optional | `20` | Tool storm breaker: max concurrent tool calls per session window. |
| `MAX_IDENTICAL_TOOL_CALLS` | Optional | `5` | Infinite loop breaker: max identical tool payload signatures. |
| `RATE_LIMIT_MAX_REQUESTS` | Optional | `60` | RPM limit per tenant. |
| `RUST_LOG` | Optional | `info` | Logging verbosity (`info`, `debug`, `trace`). |

**Example `.env`:**
```bash
cat > .env << 'EOF'
# Gateway Settings
GATEWAY_PORT=8080
RUST_LOG=info
KRYNETH_VALID_KEYS=dev_secret_123

# Upstream Credentials
GROQ_API_KEY=gsk_...
OPENAI_API_KEY=sk_...
ANTHROPIC_API_KEY=sk-ant-...

# Guardrails
MAX_SESSION_TOOL_CALLS=20
MAX_IDENTICAL_TOOL_CALLS=5
EOF
```

**Create `routing.yaml` file:**
```bash
cat > routing.yaml << 'EOF'
# Production Tenant
"prod-tenant-uuid":
  "gpt-4-turbo":
    targets:
      - priority: 1
        provider_name: "openai"
        api_key_alias: "OPENAI_API_KEY"
        base_url: "https://api.openai.com/v1"
        target_model: "gpt-4-turbo"
        schema_format: "openai"
  
  "claude-3-opus":
    targets:
      - priority: 1
        provider_name: "anthropic"
        api_key_alias: "ANTHROPIC_API_KEY"
        base_url: "https://api.anthropic.com"
        target_model: "claude-3-opus-20240229"
        schema_format: "anthropic"

# Development Tenant
"dev-tenant-uuid":
  "llama-3.3-70b-versatile":
    targets:
      - priority: 1
        provider_name: "groq"
        api_key_alias: "GROQ_API_KEY"
        base_url: "https://api.groq.com/openai/v1"
        target_model: "llama-3.3-70b-versatile"
        schema_format: "openai"
      
      - priority: 2
        provider_name: "cohere"
        api_key_alias: "COHERE_API_KEY"
        base_url: "https://api.cohere.com/v1"
        target_model: "command-r-plus-08-2024"
        schema_format: "openai"
EOF
```

### 3. Run the Container & Mount Configs

Kryneth uses a rigid file-based routing architecture. You **must** mount your `routing.yaml` into the container at `/app/routing.yaml`.

**Basic Run:**
```bash
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml:ro \
  krynethgw/kryneth-gateway:latest
```
> [!IMPORTANT]
> The `:ro` flag ensures the container only has read-only access to your local routing configuration.

**With Custom Port:**
```bash
docker run -d \
  --name kryneth-gateway \
  -p 9000:8080 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  krynethgw/kryneth-gateway:latest
```

**With Health Check:**
```bash
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  --health-cmd="curl -f http://localhost:8080/health || exit 1" \
  --health-interval=30s \
  --health-timeout=10s \
  --health-retries=3 \
  krynethgw/kryneth-gateway:latest
```

### 4. Verify the Container

```bash
# Check status
docker ps | grep kryneth-gateway

# View logs
docker logs -f kryneth-gateway

# Test the gateway
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama-3.3-70b-versatile",
    "messages": [
      {
        "role": "user",
        "content": "What is 2+2?"
      }
    ]
  }'
```

---

## 🐳 Docker Compose Deployment

**Create `docker-compose.yml`:**
```yaml
version: '3.8'

services:
  kryneth-gateway:
    image: krynethgw/kryneth-gateway:latest
    container_name: kryneth-gateway
    ports:
      - "8080:8080"
    environment:
      - GATEWAY_PORT=8080
      - RUST_LOG=info
      - KRYNETH_VALID_KEYS=${KRYNETH_VALID_KEYS}
      - GROQ_API_KEY=${GROQ_API_KEY}
      - COHERE_API_KEY=${COHERE_API_KEY}
      - OPENAI_API_KEY=${OPENAI_API_KEY}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
      - MAX_SESSION_TOOL_CALLS=20
      - MAX_IDENTICAL_TOOL_CALLS=5
    volumes:
      - ./routing.yaml:/app/routing.yaml:ro
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 5s
    restart: unless-stopped
```

**Deploy:**
```bash
docker-compose up -d
```

---

## ☁️ Cloud Deployments

### AWS ECS (Elastic Container Service)

**1. Create ECR Repository:**
```bash
aws ecr create-repository --repository-name kryneth-gateway --region us-east-1
```

**2. Push Image:**
```bash
# Get login token
aws ecr get-login-password --region us-east-1 | \
  docker login --username AWS --password-stdin <account-id>.dkr.ecr.us-east-1.amazonaws.com

# Tag and push
docker tag krynethgw/kryneth-gateway:latest \
  <account-id>.dkr.ecr.us-east-1.amazonaws.com/kryneth-gateway:latest
docker push <account-id>.dkr.ecr.us-east-1.amazonaws.com/kryneth-gateway:latest
```

**3. Create ECS Task Definition:**
```json
{
  "family": "kryneth-gateway",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "512",
  "memory": "1024",
  "containerDefinitions": [
    {
      "name": "kryneth-gateway",
      "image": "<account-id>.dkr.ecr.us-east-1.amazonaws.com/kryneth-gateway:latest",
      "portMappings": [
        {
          "containerPort": 8080,
          "protocol": "tcp"
        }
      ],
      "environment": [
        { "name": "GATEWAY_PORT", "value": "8080" },
        { "name": "RUST_LOG", "value": "info" }
      ],
      "secrets": [
        { "name": "GROQ_API_KEY", "valueFrom": "arn:aws:secretsmanager:us-east-1:<account-id>:secret:groq-key" },
        { "name": "OPENAI_API_KEY", "valueFrom": "arn:aws:secretsmanager:us-east-1:<account-id>:secret:openai-key" }
      ],
      "mountPoints": [
        {
          "sourceVolume": "routing-config",
          "containerPath": "/app/routing.yaml"
        }
      ],
      "healthCheck": {
        "command": ["CMD-SHELL", "curl -f http://localhost:8080/health || exit 1"],
        "interval": 30,
        "timeout": 5,
        "retries": 3
      },
      "logConfiguration": {
        "logDriver": "awslogs",
        "options": {
          "awslogs-group": "/ecs/kryneth-gateway",
          "awslogs-region": "us-east-1",
          "awslogs-stream-prefix": "ecs"
        }
      }
    }
  ],
  "volumes": [
    {
      "name": "routing-config",
      "efsVolumeConfiguration": {
        "fileSystemId": "fs-xxxxx",
        "transitEncryption": "ENABLED"
      }
    }
  ]
}
```

### Google Cloud Run

**1. Build and Push to Google Container Registry:**
```bash
docker tag krynethgw/kryneth-gateway:latest gcr.io/PROJECT_ID/kryneth-gateway:latest
docker push gcr.io/PROJECT_ID/kryneth-gateway:latest
```

**2. Deploy to Cloud Run:**
```bash
gcloud run deploy kryneth-gateway \
  --image gcr.io/PROJECT_ID/kryneth-gateway:latest \
  --platform managed \
  --region us-central1 \
  --port 8080 \
  --set-env-vars "GROQ_API_KEY=gsk_...,GATEWAY_PORT=8080" \
  --set-secrets "OPENAI_API_KEY=openai-key:latest" \
  --memory 512Mi \
  --cpu 1 \
  --allow-unauthenticated
```

### Kubernetes (K8s/AKS/EKS)

**Create `kryneth-deployment.yaml`:**
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: kryneth-gateway
  labels:
    app: kryneth-gateway
spec:
  replicas: 3
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxSurge: 1
      maxUnavailable: 0
  selector:
    matchLabels:
      app: kryneth-gateway
  template:
    metadata:
      labels:
        app: kryneth-gateway
    spec:
      containers:
      - name: kryneth-gateway
        image: krynethgw/kryneth-gateway:latest
        imagePullPolicy: Always
        ports:
        - name: http
          containerPort: 8080
          protocol: TCP
        env:
        - name: GATEWAY_PORT
          value: "8080"
        - name: RUST_LOG
          value: "info"
        - name: GROQ_API_KEY
          valueFrom:
            secretKeyRef:
              name: kryneth-secrets
              key: groq-api-key
        - name: OPENAI_API_KEY
          valueFrom:
            secretKeyRef:
              name: kryneth-secrets
              key: openai-api-key
        volumeMounts:
        - name: routing-config
          mountPath: /app/routing.yaml
          subPath: routing.yaml
          readOnly: true
        livenessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 10
          periodSeconds: 30
          timeoutSeconds: 5
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /health
            port: http
          initialDelaySeconds: 5
          periodSeconds: 10
          timeoutSeconds: 3
          failureThreshold: 2
        resources:
          requests:
            cpu: 200m
            memory: 256Mi
          limits:
            cpu: 500m
            memory: 512Mi
      volumes:
      - name: routing-config
        configMap:
          name: kryneth-routing-config
---
apiVersion: v1
kind: Service
metadata:
  name: kryneth-gateway
spec:
  selector:
    app: kryneth-gateway
  type: LoadBalancer
  ports:
  - name: http
    port: 80
    targetPort: 8080
    protocol: TCP
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: kryneth-routing-config
data:
  routing.yaml: |
    # Production Tenant
    "prod-tenant-uuid":
      "gpt-4-turbo":
        targets:
          - priority: 1
            provider_name: "openai"
            api_key_alias: "OPENAI_API_KEY"
            base_url: "https://api.openai.com/v1"
            target_model: "gpt-4-turbo"
            schema_format: "openai"
---
apiVersion: v1
kind: Secret
metadata:
  name: kryneth-secrets
type: Opaque
stringData:
  groq-api-key: "gsk_your_key_here"
  openai-api-key: "sk_your_key_here"
```

**Deploy to Kubernetes:**
```bash
kubectl apply -f kryneth-deployment.yaml
kubectl get service kryneth-gateway
```

---

## 🔒 Security Best Practices

### 1. Use Secret Management
**Never hardcode API keys in docker-compose or manifests.**

```bash
# AWS Secrets Manager
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  -e GROQ_API_KEY=$(aws secretsmanager get-secret-value --secret-id groq-key --query SecretString --output text) \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  krynethgw/kryneth-gateway:latest
```

### 2. Run as Non-Root
The Docker image runs as the `kryneth` user (UID 1000) by default.

```bash
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  --user kryneth \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml:ro \
  krynethgw/kryneth-gateway:latest
```

### 3. Network Policies (Kubernetes)
```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: kryneth-ingress
spec:
  podSelector:
    matchLabels:
      app: kryneth-gateway
  policyTypes:
  - Ingress
  - Egress
  ingress:
  - from:
    - podSelector:
        matchLabels:
          app: agent-frontend
    ports:
    - protocol: TCP
      port: 8080
  egress:
  - to:
    - namespaceSelector: {}
    ports:
    - protocol: TCP
      port: 443
```

### 4. Resource Limits
```bash
docker run -d \
  --name kryneth-gateway \
  -p 8080:8080 \
  --memory 512m \
  --cpus 0.5 \
  --env-file .env \
  -v $(pwd)/routing.yaml:/app/routing.yaml \
  krynethgw/kryneth-gateway:latest
```

---

## 🔄 Updating Images

### Automatic Updates
```bash
# Pull latest image
docker pull krynethgw/kryneth-gateway:latest

# Stop and remove old container
docker stop kryneth-gateway
docker rm kryneth-gateway

# Run new container
docker run -d --name kryneth-gateway ... krynethgw/kryneth-gateway:latest
```

### Version Pinning (Recommended for Production)
```bash
# Use specific version tag
docker pull krynethgw/kryneth-gateway:v0.1.0
docker run -d ... krynethgw/kryneth-gateway:v0.1.0
```

### Multi-Arch Builds
The Docker Hub image supports multiple architectures. Pull works automatically:
```bash
# Automatically detects your platform
docker pull krynethgw/kryneth-gateway:latest
```

---

## 🐛 Troubleshooting

### Container Won't Start
```bash
# Check logs
docker logs kryneth-gateway

# Check environment variables
docker inspect kryneth-gateway | grep -A 50 "Env"

# Test image locally
docker run -it krynethgw/kryneth-gateway:latest /bin/bash
```

### Port Already in Use
```bash
# Find process using port 8080
lsof -i :8080

# Use different port
docker run -d -p 9000:8080 krynethgw/kryneth-gateway:latest
```

### API Key Issues
```bash
# Verify env variables are set
docker exec kryneth-gateway env | grep GROQ_API_KEY

# Check routing.yaml is mounted
docker exec kryneth-gateway cat /app/routing.yaml
```

---

## 📊 Performance Monitoring

### Docker Stats
```bash
docker stats kryneth-gateway
```

### Prometheus Metrics (if enabled)
```bash
curl http://localhost:8080/metrics
```

---

## 🆘 Support

- **Issues:** https://github.com/kryneth-admin/kryneth-oss/issues
- **Discord:** https://discord.gg/uurgj9fMy8
- **Documentation:** https://github.com/kryneth-admin/kryneth-oss/docs
