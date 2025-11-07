FROM rust:1.75-slim

WORKDIR /app

# 安装依赖
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 复制源代码
COPY Cargo.toml ./
COPY src ./src

# 构建
RUN cargo build --release --bin guardian

# 创建数据目录
RUN mkdir -p /data/keys

# 运行Guardian
CMD ["./target/release/guardian", "--config", "/config/guardian.toml"]

