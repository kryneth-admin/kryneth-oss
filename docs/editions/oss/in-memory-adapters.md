---
icon: "memory-stick"
---
# OSS In-Memory Adapters

To guarantee frictionless developer onboarding and local stack testing, Kryneth OSS ships with memory-bound, transient state adapters. This avoids dependencies on external distributed databases or configuration caches.

---

## 1. Transient Data Structures

### `DashMap` (Thread-Safe Hash Tables)
-   **Usage**: Used for tracking active API keys (`KRYNETH_VALID_KEYS`) and thread-local configuration lookups.
-   **Characteristics**: Provides lock-free concurrent read capability and low-latency atomic writes, making it suitable for multi-core processors without thread contention.

### `Moka` Cache
-   **Usage**: Manages the L1 semantic cache index and slides window frequencies for agentic rate limits.
-   **Characteristics**: Provides high-concurrency eviction structures and time-to-live (TTL) bounding in memory.

---

## 2. Operational Considerations

> [!WARNING]
> **State Transience:**
> Because all session metrics and cache vector allocations are held strictly in memory, restarting the Kryneth OSS container clears all active limits, cost tracking indicators, and cache lists. Do not use standalone mode if transaction persistence is required across restarts.
