# Contributing to Kryneth Gateway OSS

Thank you for contributing to Kryneth Gateway. We maintain strict standards for performance, memory-safety, and latency.

To maintain the high performance and enterprise-ready standards of our zero-copy control plane, please adhere to the following guidelines before submitting a Pull Request (PR).

---

## 🚀 Local Development Setup

To get started, clone the repository and set up your local development environment:

```bash
# Clone the repository
git clone https://github.com/kryneth/Kryneth-Gateway-OSS.git
cd Kryneth-Gateway-OSS

# Set up environment variables
export JWT_SECRET="dummy-secret-for-local-dev"
```

This open-source repository uses strictly in-memory adapters (`OssAuth`, `OssRateLimit`) to guarantee frictionless local development without external database dependencies.

---

## 🛡️ Pre-Submission Verification (Mandatory)

Because this is the core open-source distribution of Kryneth Gateway, **any submission that fails compilation will be blocked.**

Before pushing your changes, run the following validation suite locally:

### 1. Verification of the OSS Build
Verify that the package builds successfully:
```bash
cargo check
```

### 2. Run the OSS Test Suite
Run local tests to ensure no regressions are introduced:
```bash
cargo test
```

### 3. Run Clippy (Linter)
All code must pass clippy checks with no warnings:
```bash
cargo clippy -- -D warnings
```

### 4. Zero-Copy Constraints
Ensure new features do not introduce unnecessary heap allocations. The core routing engine and JSON mutation paths (like Lazy Schema injection in `tool_router.rs`) rely on arena allocation via `bumpalo` and `simd-json`. Avoid using `serde_json::Value` on the hot path where possible.

---

## 📬 Pull Request Guidelines

1. **Keep PRs Focused:** Avoid bundling unrelated features, fixes, or documentation modifications into a single pull request. Keep branches small and review cycles short.
2. **Follow Rust Idioms:** Ensure your code is clean, memory-safe, and follows typical Rust coding conventions (run `cargo fmt` before submitting).
3. **Document Your Changes:** If you introduce new features or change configurations, update the documentation files inside the `docs/` directory accordingly.
4. **Link Issues:** Reference any relevant open issues in your pull request description using standard GitHub syntax (e.g., `Closes #12`).
