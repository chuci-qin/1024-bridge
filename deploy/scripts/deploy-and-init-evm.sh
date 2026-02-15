#!/usr/bin/env bash
# deploy-and-init-evm.sh
#
# Initialize and configure a deployed EVM Bridge1024 contract.
# Called after forge create has already deployed the contract.
#
# Required environment variables:
#   PRIVATE_KEY        - Admin private key (hex, with 0x prefix)
#   RPC_URL            - EVM RPC endpoint
#   CONTRACT_ADDRESS   - Deployed Bridge1024 contract address
#   TOKEN_ADDRESS      - ERC20 token (USDC/USDT) contract address
#   PEER_CONTRACT      - SVM Program ID (will be converted to bytes32)
#   SOURCE_CHAIN_ID    - EVM chain ID
#   TARGET_CHAIN_ID    - SVM chain ID
#   DECIMAL_RATIO      - Decimal conversion ratio (default: 1)
#   LIQUIDITY_AMOUNT   - Amount of tokens to add as liquidity (smallest unit)
#   SKIP_LIQUIDITY     - "true" to skip liquidity step
#   RELAYERS_FILE      - Path to relayers.json

set -euo pipefail

echo "============================================"
echo "  EVM Bridge1024 - Initialize & Configure"
echo "============================================"
echo "Contract:       $CONTRACT_ADDRESS"
echo "Token:          $TOKEN_ADDRESS"
echo "Peer (SVM):     $PEER_CONTRACT"
echo "Source Chain:    $SOURCE_CHAIN_ID"
echo "Target Chain:   $TARGET_CHAIN_ID"
echo "Decimal Ratio:  ${DECIMAL_RATIO:-1}"
echo "Liquidity:      $LIQUIDITY_AMOUNT"
echo ""

CAST_COMMON="--rpc-url $RPC_URL --private-key $PRIVATE_KEY"

# ---- Step 1: Configure USDC/USDT token address ----
echo ">>> Step 1: Configure token address..."
cast send $CONTRACT_ADDRESS \
  "configureUsdc(address)" \
  "$TOKEN_ADDRESS" \
  $CAST_COMMON
echo "    Token configured: $TOKEN_ADDRESS"

# ---- Step 2: Configure peer contract and chain IDs ----
echo ">>> Step 2: Configure peer contract..."

# Convert base58 SVM Program ID to bytes32 hex
PEER_BYTES32=$(python3 -c "
ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + ALPHABET.index(c)
    return n.to_bytes(32, 'big')
raw = b58decode('$PEER_CONTRACT')
print('0x' + raw.hex())
")

echo "    Peer bytes32: $PEER_BYTES32"

cast send $CONTRACT_ADDRESS \
  "configurePeer(bytes32,uint64,uint64)" \
  "$PEER_BYTES32" \
  "$SOURCE_CHAIN_ID" \
  "$TARGET_CHAIN_ID" \
  $CAST_COMMON
echo "    Peer configured: chains $SOURCE_CHAIN_ID <-> $TARGET_CHAIN_ID"

# ---- Step 3: Configure decimal ratio (if not default) ----
RATIO="${DECIMAL_RATIO:-1}"
if [ "$RATIO" != "1" ]; then
  echo ">>> Step 3: Configure decimal ratio..."
  cast send $CONTRACT_ADDRESS \
    "configureDecimalRatio(uint64)" \
    "$RATIO" \
    $CAST_COMMON
  echo "    Decimal ratio configured: $RATIO"
else
  echo ">>> Step 3: Decimal ratio is 1 (default), skipping."
fi

# ---- Step 4: Register relayers ----
echo ">>> Step 4: Register relayers..."
RELAYER_COUNT=$(jq '.relayers | length' "$RELAYERS_FILE")
echo "    Found $RELAYER_COUNT relayers in $RELAYERS_FILE"

for i in $(seq 0 $((RELAYER_COUNT - 1))); do
  RELAYER_NAME=$(jq -r ".relayers[$i].name" "$RELAYERS_FILE")
  RELAYER_ADDR=$(jq -r ".relayers[$i].evm_address" "$RELAYERS_FILE")
  
  echo "    Adding relayer $RELAYER_NAME ($RELAYER_ADDR)..."
  cast send $CONTRACT_ADDRESS \
    "addRelayer(address)" \
    "$RELAYER_ADDR" \
    $CAST_COMMON
  echo "    Relayer $RELAYER_NAME registered."
done

# ---- Step 5: Add liquidity ----
if [ "${SKIP_LIQUIDITY:-false}" = "true" ]; then
  echo ">>> Step 5: Skipping liquidity (SKIP_LIQUIDITY=true)"
else
  echo ">>> Step 5: Add liquidity ($LIQUIDITY_AMOUNT tokens)..."
  
  # Transfer tokens directly to the contract (contract is its own vault)
  cast send "$TOKEN_ADDRESS" \
    "transfer(address,uint256)" \
    "$CONTRACT_ADDRESS" \
    "$LIQUIDITY_AMOUNT" \
    $CAST_COMMON
  echo "    Transferred $LIQUIDITY_AMOUNT tokens to contract"
fi

# ---- Verification ----
echo ""
echo "=== EVM Deployment Verification ==="
echo "Contract address: $CONTRACT_ADDRESS"

RELAYER_COUNT_ONCHAIN=$(cast call $CONTRACT_ADDRESS "getRelayerCount()(uint256)" --rpc-url "$RPC_URL")
echo "Relayer count: $RELAYER_COUNT_ONCHAIN"

SENDER_NONCE=$(cast call $CONTRACT_ADDRESS "getSenderNonce()(uint64)" --rpc-url "$RPC_URL")
echo "Sender nonce: $SENDER_NONCE"

if [ "${SKIP_LIQUIDITY:-false}" != "true" ]; then
  CONTRACT_BALANCE=$(cast call "$TOKEN_ADDRESS" "balanceOf(address)(uint256)" "$CONTRACT_ADDRESS" --rpc-url "$RPC_URL")
  echo "Contract token balance: $CONTRACT_BALANCE"
fi

echo ""
echo "EVM initialization complete!"
