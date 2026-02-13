#!/bin/bash

NUM="$1"


if [ -z "${NUM}" ]; then
  echo "请输入实例编号"
  exit 1
fi
if ! [[ "$NUM" =~ ^[0-9]+$ ]]; then
  echo "NUM必须是数字"
  exit 1
fi

INSTANCE_NAME="relayer${NUM}"
CONTAINER_NAME="relayer-container-${INSTANCE_NAME}"

# 确保在 relayer 目录下执行
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# 先停止并删除已存在的容器（如果存在）
docker stop ${CONTAINER_NAME} 2>/dev/null || true
docker rm ${CONTAINER_NAME} 2>/dev/null || true

# 运行容器
docker run -d \
  --name ${CONTAINER_NAME} \
  -p $((8081 + (NUM - 1) * 100)):8081 \
  -p $((8082 + (NUM - 1) * 100)):8082 \
  -p $((8083 + (NUM - 1) * 100)):8083 \
  -v "$(pwd)/.${INSTANCE_NAME}/logs:/app/logs" \
  relayer:latest

# 使用 docker cp 复制配置文件（避免某些环境下 bind mount 文件的问题）
docker cp "$(pwd)/.${INSTANCE_NAME}/envs/.env.s2e" ${CONTAINER_NAME}:/app/s2e/.env
docker cp "$(pwd)/.${INSTANCE_NAME}/envs/.env.e2s-listener" ${CONTAINER_NAME}:/app/e2s-listener/.env
docker cp "$(pwd)/.${INSTANCE_NAME}/envs/.env.e2s-submitter" ${CONTAINER_NAME}:/app/e2s-submitter/.env

# 启动 relayer 服务
docker exec ${CONTAINER_NAME} bash -c "cd /app && chmod +x start-relayer-indocker.sh && ./start-relayer-indocker.sh"
