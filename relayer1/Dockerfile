# ============================================
# Bridge1024 Relayer - Multi-stage Docker Build
# ============================================
# Sol-1024 桥专用：Solana <-> 1024chain 双向 relayer
# 每个容器仅需 3 个环境变量即可运行

# --- Stage 1: Build ---
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY relayer1/shared/ shared/
COPY relayer1/sol2svm-listener/ sol2svm-listener/
COPY relayer1/sol2svm-submitter/ sol2svm-submitter/
COPY relayer1/svm2sol-listener/ svm2sol-listener/
COPY relayer1/svm2sol-submitter/ svm2sol-submitter/

RUN cd sol2svm-listener && cargo build --release
RUN cd sol2svm-submitter && cargo build --release
RUN cd svm2sol-listener && cargo build --release
RUN cd svm2sol-submitter && cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    jq curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN mkdir -p /app/sol2svm-listener /app/sol2svm-submitter \
    /app/svm2sol-listener /app/svm2sol-submitter \
    /app/config /app/artifacts

COPY --from=builder /build/sol2svm-listener/target/release/sol2svm-listener /app/sol2svm-listener/sol2svm-listener
COPY --from=builder /build/sol2svm-submitter/target/release/sol2svm-submitter /app/sol2svm-submitter/sol2svm-submitter
COPY --from=builder /build/svm2sol-listener/target/release/svm2sol-listener /app/svm2sol-listener/svm2sol-listener
COPY --from=builder /build/svm2sol-submitter/target/release/svm2sol-submitter /app/svm2sol-submitter/svm2sol-submitter

COPY deploy1/config/bridges.json /app/config/bridges.json
COPY deploy1/artifacts/ /app/artifacts/

COPY relayer1/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

EXPOSE 8084 8085 8086 8087

ENTRYPOINT ["/app/entrypoint.sh"]
