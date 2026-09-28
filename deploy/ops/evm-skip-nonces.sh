#!/usr/bin/env bash
# evm-skip-nonces.sh — docs1024#1125 refund path, STEP 1 of 3 (target chain).
#
# A 1024→EVM withdrawal stuck in `staked` is refunded in three steps and the ORDER is
# the only thing that makes it double-spend-safe:
#   1. operator skipNonce(nonce) on the TARGET bridge   ← this script
#        sets isProcessed=1/isUnlocked=0 ⇒ every later confirmEvent reverts AlreadyProcessed
#   2. operator initiate_refund(nonce) on the 1024 HUB   (deploy/svm/src/instructions/initiate-refund.ts,
#        which refuses any nonce this step has not skipped)
#   3. REFUND_DELAY (6h) later, 1024-core relayer1 StakedResolver (stake owner) runs
#        execute_refund, returns the USDC to the user's vault and books `refunded`.
#
# Input: a file with one `<target_chain_id>\t<nonce>` per line, e.g. the
# `target_skip_nonces.txt` written by 1024-core scripts/bridge_1125_staked_disposition.py.
#
# Dry run by default (reads only). `--apply` sends skipNonce with $EVM_OPERATOR_PRIVATE_KEY.
# Per nonce it re-reads nonceConfirmations first and only skips if isProcessed == false:
# a nonce the relayers unlocked in the meantime is left alone (the user was paid).
#
# Usage:
#   RPC=https://ethereum-sepolia-rpc.publicnode.com BRIDGE=0x3dB1A5a9430E87DC198BcE21FF7A7190E9C492eb \
#     deploy/ops/evm-skip-nonces.sh nonces.txt [--apply]
set -euo pipefail
FILE="${1:?nonce file}"; APPLY="${2:-}"
: "${RPC:?RPC}" "${BRIDGE:?BRIDGE}"
command -v cast >/dev/null || { echo "needs foundry 'cast'" >&2; exit 2; }
if [[ "$APPLY" == "--apply" ]]; then
  : "${EVM_OPERATOR_PRIVATE_KEY:?EVM_OPERATOR_PRIVATE_KEY}"
  op=$(cast wallet address --private-key "$EVM_OPERATOR_PRIVATE_KEY")
  onchain=$(cast call "$BRIDGE" "operator()(address)" --rpc-url "$RPC")
  [[ "${op,,}" == "${onchain,,}" ]] || { echo "key $op is not the bridge operator $onchain" >&2; exit 2; }
fi
skipped=0; already=0; unlocked=0
while IFS=$'\t' read -r _chain nonce; do
  [[ -z "${nonce:-}" ]] && continue
  read -r processed isUnlocked _thr < <(cast call "$BRIDGE" "nonceConfirmations(uint64)(bool,bool,uint8)" "$nonce" --rpc-url "$RPC" | tr '\n' ' ')
  if [[ "$isUnlocked" == "true" ]]; then echo "$nonce UNLOCKED (paid on target) — leave; core reconciles to completed"; unlocked=$((unlocked+1)); continue; fi
  if [[ "$processed" == "true" ]]; then echo "$nonce already skipped"; already=$((already+1)); continue; fi
  if [[ "$APPLY" == "--apply" ]]; then
    cast send "$BRIDGE" "skipNonce(uint64)" "$nonce" --private-key "$EVM_OPERATOR_PRIVATE_KEY" --rpc-url "$RPC" >/dev/null
    echo "$nonce skipNonce sent"
  else
    echo "$nonce would skipNonce (dry run)"
  fi
  skipped=$((skipped+1))
done < "$FILE"
echo "summary: to_skip/skipped=$skipped already_skipped=$already unlocked=$unlocked apply=${APPLY:-no}"
