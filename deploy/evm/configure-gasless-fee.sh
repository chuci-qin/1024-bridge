#!/usr/bin/env bash
# evm/configure-gasless-fee.sh — Configure gasless service fee on Bridge1024 EVM
# Sourced by bridge.sh; do not execute directly.
#
# gaslessFee 与 bridgeFee 形态对称：admin 通过 timelock 调整，
# 设为 0 即熔断 gasless 路径（stakeWithAuthorization 会 revert GaslessDisabled）

op_evm_configure_gasless_fee() {
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
  echo -e "  ${BOLD}── Configure Gasless Fee on ${chain_name} ──${NC}" >&2
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
    error "Timelock is active. Use schedule/execute flow for configureGaslessFee."; return
  fi

  # Show current fee
  local current_fee
  current_fee=$(evm_read "$rpc" "$bridge_addr" "gaslessFee()(uint64)" 2>/dev/null | xargs) || current_fee="0"
  info "Current gasless fee: ${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "MAX_FEE:             1000000000 (1000 USDC)"
  if [[ "$current_fee" == "0" ]]; then
    warn "gaslessFee == 0: gasless deposit path is currently DISABLED"
    warn "  stakeWithAuthorization will revert GaslessDisabled"
  fi

  echo "" >&2

  local fee
  fee=$(prompt_input "New gasless fee (raw USDC, 6 decimals; 0 = disable gasless path)" "$current_fee" uint) || return 0

  if (( fee > 1000000000 )); then
    error "Fee ${fee} exceeds MAX_FEE (1000000000 = 1000 USDC)"; return
  fi

  print_summary "Configure Gasless Fee" \
    "Bridge"       "$bridge_addr" \
    "Chain"        "$chain_name" \
    "Current fee"  "${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "New fee"      "${fee} ($(echo "scale=6; ${fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  if [[ "$fee" == "0" ]]; then
    warn "Setting gaslessFee to 0 will DISABLE the gasless deposit path."
    warn "Users will not be able to use stakeWithAuthorization (existing stake() unaffected)."
  fi

  prompt_confirm "Proceed?" || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
    "configureGaslessFee(uint64)" "$fee") || return

  if [[ -n "$tx_hash" ]]; then
    info "Verifying gasless fee..."
    local verified_fee
    verified_fee=$(evm_read "$rpc" "$bridge_addr" "gaslessFee()(uint64)" 2>/dev/null | xargs) || true
    print_verification "gaslessFee" "$fee" "$verified_fee"

    append_log "[evm/configureGaslessFee] chain=${chain} bridge=${bridge_addr} fee=${fee} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/configureGaslessFee] chain=${chain} bridge=${bridge_addr} fee=${fee} status=safe-queued"
  fi
}
