#!/bin/bash

NUM="$1"

if [ -z "${NUM}" ]; then
  echo "请输入实例编号"
  exit 1
fi
if ! [[ "$NUM" =~ ^[0-9]+$ ]]; then
  echo "参数必须是数字"
  exit 1
fi

# 与 start-container.sh 保持一致的目录命名
NEW_RELAYER_NAME="relayer${NUM}"

mkdir -p .${NEW_RELAYER_NAME}
mkdir -p .${NEW_RELAYER_NAME}/envs
mkdir -p .${NEW_RELAYER_NAME}/logs

cp e2s-listener/.env .${NEW_RELAYER_NAME}/envs/.env.e2s-listener
cp e2s-submitter/.env .${NEW_RELAYER_NAME}/envs/.env.e2s-submitter
cp s2e/.env .${NEW_RELAYER_NAME}/envs/.env.s2e

# 修改 submitter 的 QUEUE__PATH 为 /app/relayer/e2s-listener/.relayer/queue
SUBMITTER_ENV_FILE=".${NEW_RELAYER_NAME}/envs/.env.e2s-submitter"
if grep -q "^QUEUE__PATH=" "$SUBMITTER_ENV_FILE"; then
  awk -v path="/app/e2s-listener/.relayer/queue" '/^QUEUE__PATH=/ { print "QUEUE__PATH=" path; next }1' "$SUBMITTER_ENV_FILE" > "${SUBMITTER_ENV_FILE}.tmp" && mv "${SUBMITTER_ENV_FILE}.tmp" "$SUBMITTER_ENV_FILE"
else
  echo "QUEUE__PATH=/app/e2s-listener/.relayer/queue" >> "$SUBMITTER_ENV_FILE"
fi

