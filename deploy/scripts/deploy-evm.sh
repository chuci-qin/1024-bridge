#!/bin/bash
set -euo pipefail

RPC_URL="${1:?Usage: ./deploy-evm.sh <rpc-url> <admin-address>}"
ADMIN="${2:?Usage: ./deploy-evm.sh <rpc-url> <admin-address>}"

: "${PRIVATE_KEY:?PRIVATE_KEY env var required}"

forge create --rpc-url "$RPC_URL" \
  --private-key "$PRIVATE_KEY" \
  --constructor-args "$ADMIN" \
  contracts/evm/src/Bridge1024.sol:Bridge1024
