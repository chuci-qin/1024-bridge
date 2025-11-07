#!/bin/bash
set -e

# 启动 Docker daemon
dockerd-entrypoint.sh &

# 等待 Docker daemon 启动
echo "Waiting for Docker daemon to start..."
while ! docker info > /dev/null 2>&1; do
    sleep 1
done
echo "Docker daemon started successfully"

# 执行传入的命令
exec "$@"
