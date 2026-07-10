# Kubernetes Deployments

Deploy Kryneth Gateway to Kubernetes (EKS, GKE, AKS) to handle high-availability LLM routing across auto-scaling clusters.

---

## 1. Kubernetes Resource Configurations

Create `kryneth-deployment.yaml` containing the deployment spec, load balancer service, config map for `routing.yaml`, and secrets template:

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
        ports:
        - name: http
          containerPort: 8080
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
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: kryneth-routing-config
data:
  routing.yaml: |
    "00000000-0000-0000-0000-000000000000":
      "llama-3.3-70b-versatile":
        targets:
          - priority: 1
            provider_name: "groq"
            api_key_alias: "GROQ_API_KEY"
            base_url: "https://api.groq.com/openai/v1"
            target_model: "llama-3.3-70b-versatile"
            schema_format: "openai"
---
apiVersion: v1
kind: Secret
metadata:
  name: kryneth-secrets
type: Opaque
stringData:
  groq-api-key: "gsk_your_key_here"
```

Apply the files to the active namespace:
```bash
kubectl apply -f kryneth-deployment.yaml
```

---

## 2. Resource Bounding & Limits

To ensure optimal operations and prevent resource exhaustion under heavy agent loop storms:

```yaml
resources:
  requests:
    cpu: 200m
    memory: 256Mi
  limits:
    cpu: 500m
    memory: 512Mi
```
