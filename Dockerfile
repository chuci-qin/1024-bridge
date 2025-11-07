FROM ubuntu:24.04

# 设置环境变量
ENV DEBIAN_FRONTEND=noninteractive
ENV DOCKER_VERSION=24.0.7

# 安装基础工具
RUN apt-get update && apt-get install -y \
    curl \
    wget \
    git \
    vim \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    gnupg \
    lsb-release \
    sudo \
    && rm -rf /var/lib/apt/lists/*

# 安装 Docker (Docker-in-Docker)
RUN mkdir -p /etc/apt/keyrings && \
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg && \
    echo \
    "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
    $(lsb_release -cs) stable" | tee /etc/apt/sources.list.d/docker.list > /dev/null && \
    apt-get update && \
    apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin && \
    rm -rf /var/lib/apt/lists/*

# 安装 Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# 安装 Solana CLI - 使用更稳定的方式
RUN mkdir -p /root/.local/share/solana && \
    cd /root/.local/share/solana && \
    curl -sSfL https://release.solana.com/v1.18.22/install -o install.sh && \
    sh install.sh v1.18.22 && \
    rm install.sh
ENV PATH="/root/.local/share/solana/install/active_release/bin:${PATH}"

# 验证 Solana 安装
RUN solana --version && solana-test-validator --version

# 安装 Anchor (暂时跳过，后续手动安装以避免编译问题)
# 可在容器内运行: cargo install --git https://github.com/coral-xyz/anchor avm --locked
# RUN cargo install --git https://github.com/coral-xyz/anchor avm --locked && \
#     avm install 0.29.0 && \
#     avm use 0.29.0

# 安装 Node.js 和 pnpm (用于测试脚本)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y nodejs && \
    npm install -g pnpm yarn

# 安装 Foundry (EVM 开发工具链)
RUN curl -L https://foundry.paradigm.xyz | bash
ENV PATH="/root/.foundry/bin:${PATH}"
RUN foundryup

# 创建工作目录
WORKDIR /workspace

# 暴露常用端口
EXPOSE 8545 8546 8899 8900 9900

# 启动 Docker daemon 的脚本
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["/bin/bash"]
