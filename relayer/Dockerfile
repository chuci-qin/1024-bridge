# ── Stage 1: Builder ─────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev protobuf-compiler libudev-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependency build by copying manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release --bin relayer 2>/dev/null || true \
    && rm -rf src

COPY src/ src/
RUN cargo build --release --bin relayer

# ── Stage 2: Runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 relayer \
    && useradd -u 1000 -g relayer -m -s /bin/bash relayer

COPY --from=builder /build/target/release/relayer /usr/local/bin/relayer

RUN mkdir -p /data && chown relayer:relayer /data

USER relayer

VOLUME ["/data"]

ENV DATA_DIR=/data

ENTRYPOINT ["relayer"]
