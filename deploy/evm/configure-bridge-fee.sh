#!/usr/bin/env bash
# evm/configure-bridge-fee.sh — Configure bridge fee on Bridge1024 EVM
# Sourced by bridge.sh; do not execute directly.

op_evm_configure_bridge_fee() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  local bridge_addr
  bridge_addr=$(read_address ".evm.${chain}.bridge")
  if [[ -z "$bridge_addr" ]]; then error "Bridge not deployed on $chain_name. Deploy first."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Configure Bridge Fee on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  local on_admin
  on_admin=$(evm_read "$rpc" "$bridge_addr" "admin()(address)" 2>/dev/null | xargs) || true
  if [[ -z "$on_admin" || "$on_admin" == "0x0000000000000000000000000000000000000000" ]]; then
    error "Cannot read admin from $bridge_addr"; return
  fi

  # Check timelock
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow for configureBridgeFee."; return
  fi

  # Show current fee
  local current_fee
  current_fee=$(evm_read "$rpc" "$bridge_addr" "bridgeFee()(uint64)" 2>/dev/null | xargs) || current_fee="0"
  info "Current bridge fee: ${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "MAX_FEE:           1000000000 (1000 USDC)"

  echo "" >&2

  local fee
  fee=$(prompt_input "New bridge fee (raw USDC, 6 decimals; 0 = no fee)" "$current_fee" uint) || return 0

  if (( fee > 1000000000 )); then
    error "Fee ${fee} exceeds MAX_FEE (1000000000 = 1000 USDC)"; return
  fi

  print_summary "Configure Bridge Fee" \
    "Bridge"       "$bridge_addr" \
    "Chain"        "$chain_name" \
    "Current fee"  "${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "New fee"      "${fee} ($(echo "scale=6; ${fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  prompt_confirm "Proceed?" || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
    "configureBridgeFee(uint64)" "$fee") || return

  if [[ -n "$tx_hash" ]]; then
    info "Verifying bridge fee..."
    local verified_fee
    verified_fee=$(evm_read "$rpc" "$bridge_addr" "bridgeFee()(uint64)" 2>/dev/null | xargs) || true
    print_verification "bridgeFee" "$fee" "$verified_fee"

    append_log "[evm/configureBridgeFee] chain=${chain} bridge=${bridge_addr} fee=${fee} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/configureBridgeFee] chain=${chain} bridge=${bridge_addr} fee=${fee} status=safe-queued"
  fi
}
