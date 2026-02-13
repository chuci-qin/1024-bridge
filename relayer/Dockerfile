FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    curl \
    wget \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    gnupg \
    lsb-release \
    sudo \
    jq tree net-tools 

# 安装最新版本的Rust工具链
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

RUN mkdir -p /app/
RUN mkdir -p /app/logs

COPY e2s-listener /app/e2s-listener
COPY s2e /app/s2e
COPY e2s-submitter /app/e2s-submitter
COPY shared /app/shared
COPY start-relayer-indocker.sh /app/start-relayer-indocker.sh
COPY entrypoint.sh /app/entrypoint.sh

RUN chmod +x /app/start-relayer-indocker.sh /app/entrypoint.sh

# 构建三个 relayer 组件
RUN cd /app/s2e && cargo build --release
RUN cd /app/e2s-listener && cargo build --release
RUN cd /app/e2s-submitter && cargo build --release

# 暴露 s2e(8081)、e2s-submitter(8082)、e2s-listener(8083) 端口
EXPOSE 8081 8082 8083

CMD ["/app/entrypoint.sh"]
