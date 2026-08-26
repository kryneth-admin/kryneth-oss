---
icon: "globe"
---
# Edge Deployments (Fly.io & Cloudflare)

Deploying Kryneth Gateway at the edge close to your agent clients reduces initial connection latencies and secures egress traffic efficiently.

---

## 1. Fly.io Deployment

Kryneth compiles to a lightweight, statically-linked binary, making it extremely suitable for running inside Fly.io's MicroVM architecture.

### Example `fly.toml`
Create a `fly.toml` configuration in the project root:

```toml
app = "kryneth-gateway"
primary_region = "cdg"

[build]
  dockerfile = "Dockerfile"

[[services]]
  internal_port = 8080
  protocol = "tcp"
  processes = ["app"]

  [[services.ports]]
    force_https = true
    handlers = ["http"]
    port = 80

  [[services.ports]]
    handlers = ["tls", "http"]
    port = 443

  [services.concurrency]
    hard_limit = 2000
    soft_limit = 1500
    type = "connections"
```

Deploy the instance:
```bash
fly deploy --env GATEWAY_PORT=8080
```

---

## 2. Cloudflare Tunnel integration

To securely bridge Kryneth Gateway to the public web without opening ports in your local firewall, route your edge traffic through Cloudflare Tunnels:

```bash
# Authenticate Cloudflare daemon
cloudflared tunnel login

# Create a tunnel
cloudflared tunnel create kryneth-edge

# Map DNS route to local proxy
cloudflared tunnel route dns kryneth-edge gateway.kryneth.tech
```
