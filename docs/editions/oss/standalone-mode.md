# Standalone Mode

Standalone mode boots Kryneth Gateway without external database dependencies (PostgreSQL, Redis, ClickHouse). It is intended for local developer loops and minor edge proxy topologies.

---

## 1. Launch Requirements

To execute Kryneth Gateway in standalone mode:

1.  Specify the environment variables directly (in your shell or via `.env` file).
2.  Provide a local `routing.yaml` file in the same directory as the binary.

```bash
# Standalone run using cargo
cargo run --release
```

All configurations default to local standard out (`stdout`) for telemetry logging. The routing engine checks keys in memory against the `KRYNETH_VALID_KEYS` variable.

---

## 2. Advantages & Constraints

### Advantages
-   **No Infrastructure Requirements**: Runs instantly on raw virtual machines or small Raspberry Pi instances.
-   **Low Overhead**: Employs no network calls for config lookups, keeping processing overhead under 1.5ms.

### Constraints
-   **Horizontal Scalability Limits**: If running multiple standalone instances, configuration updates must be synchronized to the file system of all instances manually.
-   **No Aggregated Metrics**: Standard logs are emitted in JSON format via stdout, requiring external aggregators (like Vector or FluentBit) to collect telemetry metrics.
