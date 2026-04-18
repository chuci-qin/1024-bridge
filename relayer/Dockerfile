# ── Stage 1: Builder ─────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev protobuf-compiler libudev-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependency build by copying manifests first.
#
# 思路：用一个 `fn main(){}` 桩跑一次 release build，把全部依赖 .rlib 编进
# target/release/deps，依赖编译结果落在本层缓存里；只要 Cargo.toml/Cargo.lock
# 不变，下次 build 这一层就直接命中、跳过几百个 crate 的编译。
#
# 易踩坑：桩 build 也会产出一个 hello-world `relayer` 二进制以及对应的
# fingerprint。GitHub Actions checkout 出来的源文件 mtime 通常早于这个产物，
# 下一步 `cargo build` 会被 cargo 的 mtime 启发式判定为"产物已新于源码、无需
# 重编"，导致最终镜像里塞的是 hello-world 桩 → 容器一启动立刻 exit 0、零日志。
#
# 因此这里在 stub build 之后必须显式删掉 relayer 自己的产物 + fingerprint，
# 强制下一步 `cargo build` 重编 relayer crate；依赖 .rlib 全部保留，缓存效益不变。
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release --bin relayer 2>/dev/null || true \
    && rm -rf src \
        target/release/relayer \
        target/release/deps/relayer-* \
        target/release/deps/bridge1024_relayer-* \
        target/release/.fingerprint/bridge1024-relayer-*

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
