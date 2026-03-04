# ============================================
# Bridge1024 Relayer - Multi-stage Docker Build
# ============================================
# 通用镜像：支持任意桥接对（通过 BRIDGE_ID 环境变量选择）
# 每个容器仅需 3 个环境变量即可运行

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
COPY relayer/sol2svm-listener/ sol2svm-listener/
COPY relayer/sol2svm-submitter/ sol2svm-submitter/
COPY relayer/svm2sol-listener/ svm2sol-listener/
COPY relayer/svm2sol-submitter/ svm2sol-submitter/

# 构建 EVM <-> SVM 组件
RUN cd s2e && cargo build --release
RUN cd e2s-listener && cargo build --release
RUN cd e2s-submitter && cargo build --release

# 构建 Solana -> SVM 组件
RUN cd sol2svm-listener && cargo build --release
RUN cd sol2svm-submitter && cargo build --release

# 构建 SVM -> Solana 组件
RUN cd svm2sol-listener && cargo build --release
RUN cd svm2sol-submitter && cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    jq curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 创建组件目录
RUN mkdir -p /app/s2e /app/e2s-listener /app/e2s-submitter \
    /app/sol2svm-listener /app/sol2svm-submitter \
    /app/svm2sol-listener /app/svm2sol-submitter \
    /app/config /app/artifacts

# 复制 EVM <-> SVM 构建产物
COPY --from=builder /build/s2e/target/release/s2e-relayer /app/s2e/s2e-relayer
COPY --from=builder /build/e2s-listener/target/release/e2s-listener /app/e2s-listener/e2s-listener
COPY --from=builder /build/e2s-submitter/target/release/e2s-submitter /app/e2s-submitter/e2s-submitter

# 复制 Solana -> SVM 构建产物
COPY --from=builder /build/sol2svm-listener/target/release/sol2svm-listener /app/sol2svm-listener/sol2svm-listener
COPY --from=builder /build/sol2svm-submitter/target/release/sol2svm-submitter /app/sol2svm-submitter/sol2svm-submitter

# 复制 SVM -> Solana 构建产物
COPY --from=builder /build/svm2sol-listener/target/release/svm2sol-listener /app/svm2sol-listener/svm2sol-listener
COPY --from=builder /build/svm2sol-submitter/target/release/svm2sol-submitter /app/svm2sol-submitter/svm2sol-submitter

# 复制桥接对配置
COPY deploy/config/bridges.json /app/config/bridges.json

# 复制部署产物
COPY deploy/artifacts/ /app/artifacts/

# 复制入口脚本
COPY relayer/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

EXPOSE 8081 8082 8083 8084 8085 8086 8087

ENTRYPOINT ["/app/entrypoint.sh"]
