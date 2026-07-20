# What is Zerotrace Agent

**Zerotrace Agent** is a non-intrusive data collection agent that runs on Kubernetes nodes, cloud VMs, and bare-metal hosts. Leveraging eBPF-based packet capture and protocol-aware parsing, it reconstructs application requests, traces, and network flows without requiring code instrumentation or SDK integration. The Agent streams enriched telemetry to the Zerotrace Server via a high-performance gRPC-based protocol, enabling end-to-end observability with minimal overhead.

# Key Features

### 1. eBPF-Based Zero-Instrumentation Capture
Uses eBPF probes attached to kernel network paths to capture every packet entering and leaving application processes. Supports both eBPF (modern kernels) and cBPF (legacy kernels) backends, with automatic fallback. No application restarts, no code changes, no sidecar injection required.

### 2. Protocol-Aware Request Parsing
Parses over 20 application-layer protocols (HTTP/1.x, HTTP/2, gRPC, Redis, MySQL, PostgreSQL, Kafka, MongoDB, DNS, Dubbo, MQTT, and more) directly from captured packets. Extracts request boundaries via length-field inspection or full protocol parsing, producing structured spans with method, endpoint, status code, and latency metadata.

### 3. Efficient Data Plane
Communicates with the Zerotrace Server using a custom gRPC-based protocol optimized for high-throughput telemetry. Implements on-host compression, batching, and flow control to minimize bandwidth consumption. The Agent offloads tag resolution and resource enrichment to the Server side, keeping its own CPU and memory footprint low.

### 4. Multi-Architecture Support
Provides native binaries for Linux (amd64, arm64) with experimental Darwin builds for local development. Built in Rust for memory safety and predictable performance. Supports both glibc and musl-based distributions.

### 5. Pluggable Processing Pipeline
Internal architecture uses a modular pipeline of collectors, processors, and reporters. New protocol parsers, exporters, and processing stages can be added through the plugin system without modifying core Agent code. Configuration is managed through a YAML file with hot-reload support.

# Documentation

| Document | Location |
|----------|----------|
| Build Guide | [docs/user/build.md](docs/user/build.md) |
| Developer Guide | [docs/dev/DEVELOPER_GUIDE.md](docs/dev/DEVELOPER_GUIDE.md) |
| Contribution Guide | [docs/dev/CONTRIBUTING.md](docs/dev/CONTRIBUTING.md) |
| Architecture | [docs/design/](docs/design/) |
| Configuration | [docs/dev/how-to-add-config-for-agent.md](docs/dev/how-to-add-config-for-agent.md) |

# Getting Started

### Installation

```bash
curl -fsSL http://<web-ip>:5173/agent/install.sh | bash
```

This installs the Agent to `/opt/zerotrace-agent/` and generates a default configuration pointing to your Zerotrace Server.

### Build from Source

```bash
# Docker-based build (recommended — no local Rust toolchain needed)
# See docs/user/build.md for full instructions

# Or with local Rust toolchain
cargo build --release -p zerotrace-agent
```

### Run

```bash
ZT_API_KEY=<your-api-key> /opt/zerotrace-agent/bin/zerotrace-agent \
    -c /opt/zerotrace-agent/etc/zerotrace-agent.yaml
```

# Related Repositories

- [Zerotrace Server](https://github.com/DeepShield-AI/zerotrace-server) — Data ingestion, storage, and query engine
- [Zerotrace Web](https://github.com/DeepShield-AI/zerotrace-web) — Web UI and API gateway
