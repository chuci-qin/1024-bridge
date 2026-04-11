# ============================================
# Bridge1024 Relayer - Multi-stage Docker Build
# ============================================
# Cargo workspace: builds all 4 binaries in one pass
# Persistent data stored under /data (mount a volume)

# --- Stage 1: Build ---
FROM rust:1.88-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY relayer/ .

RUN cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    jq curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN mkdir -p /app/config /app/artifacts /data

COPY --from=builder /build/target/release/s2e-listener /app/s2e-listener
COPY --from=builder /build/target/release/s2e-submitter /app/s2e-submitter
COPY --from=builder /build/target/release/e2s-listener /app/e2s-listener
COPY --from=builder /build/target/release/e2s-submitter /app/e2s-submitter

# 复制桥接对配置
COPY deploy/config/bridges.json /app/config/bridges.json

# 复制部署产物（构建时从 Release 下载的合约地址 JSON）
# 目录可能为空（首次构建尚无 release 时），entrypoint 会 fallback 到 API
COPY deploy/artifacts/ /app/artifacts/

# 复制入口脚本
COPY relayer/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

VOLUME /data

EXPOSE 8081 8082 8083 8084

ENTRYPOINT ["/app/entrypoint.sh"]
