# ============================================
# Bridge1024 Relayer - Multi-stage Docker Build
# ============================================
# 通用镜像：支持任意桥接对（通过 BRIDGE_ID 环境变量选择）
# 每个容器仅需 5 个环境变量即可运行

# --- Stage 1: Build ---
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 复制所有 relayer 源码
COPY relayer/shared/ shared/
COPY relayer/s2e/ s2e/
COPY relayer/e2s-listener/ e2s-listener/
COPY relayer/e2s-submitter/ e2s-submitter/

# 构建三个组件（release 模式）
RUN cd s2e && cargo build --release
RUN cd e2s-listener && cargo build --release
RUN cd e2s-submitter && cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    jq curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 创建组件目录
RUN mkdir -p /app/s2e /app/e2s-listener /app/e2s-submitter /app/config

# 复制构建产物
COPY --from=builder /build/s2e/target/release/s2e-relayer /app/s2e/s2e-relayer
COPY --from=builder /build/e2s-listener/target/release/e2s-listener /app/e2s-listener/e2s-listener
COPY --from=builder /build/e2s-submitter/target/release/e2s-submitter /app/e2s-submitter/e2s-submitter

# 复制桥接对配置和入口脚本
COPY deploy/config/bridges.json /app/config/bridges.json
COPY relayer/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

# 暴露三个组件的 API 端口
EXPOSE 8081 8082 8083

ENTRYPOINT ["/app/entrypoint.sh"]
