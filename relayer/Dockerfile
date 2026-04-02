# ── Stage 1: Builder ─────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev protobuf-compiler libudev-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY relayer/Cargo.toml relayer/Cargo.lock* ./
COPY relayer/core/ core/
COPY relayer/listener/ listener/
COPY relayer/submitter/ submitter/

RUN cargo build --release --bin listener --bin submitter \
    && strip target/release/listener target/release/submitter

# ── Stage 2: Runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 relayer \
    && useradd -u 1000 -g relayer -m -s /bin/bash relayer

COPY --from=builder /build/target/release/listener  /usr/local/bin/listener
COPY --from=builder /build/target/release/submitter  /usr/local/bin/submitter
COPY relayer/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:${HEALTH_PORT:-8080}/health || exit 1

USER relayer

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
