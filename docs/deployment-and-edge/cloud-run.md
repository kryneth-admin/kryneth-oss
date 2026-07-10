# Cloud Run & AWS ECS Container Deployments

Deploy Kryneth Gateway to AWS ECS (Elastic Container Service) or Google Cloud Run for elastic, serverless scaling.

---

## 1. Google Cloud Run Deployment

Google Cloud Run allows serverless execution of containerized applications with scales down to zero.

### Deploy Command
Inject credentials and launch the service:

```bash
gcloud run deploy kryneth-gateway \
  --image gcr.io/PROJECT_ID/kryneth-gateway:latest \
  --platform managed \
  --region us-central1 \
  --port 8080 \
  --set-env-vars "GROQ_API_KEY=gsk_...,GATEWAY_PORT=8080" \
  --memory 512Mi \
  --cpu 1 \
  --allow-unauthenticated
```

---

## 2. AWS ECS Task Definition

AWS ECS runs the container within Fargate serverless infrastructure.

### ECS JSON Task Definition
Specify the task container configuration, including port mappings and env injections:

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
      "image": "krynethgw/kryneth-gateway:latest",
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
      "essential": true
    }
  ]
}
```
