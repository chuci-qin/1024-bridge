#!/usr/bin/env bash
# evm/withdraw.sh — Withdraw tokens or ETH from Bridge1024 EVM contract
# Sourced by bridge.sh; do not execute directly.

op_evm_withdraw() {
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
  echo -e "  ${BOLD}── Withdraw from ${chain_name} ──${NC}" >&2
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

  # Check timelock — both withdrawToken and withdrawETH are timelock-protected
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow for withdrawals."; return
  fi

  # Select withdrawal type
  local withdraw_opts=("Withdraw Token (ERC-20)" "Withdraw ETH" "← Back")
  local idx
  idx=$(prompt_select "Select withdrawal type:" "${withdraw_opts[@]}")

  case "$idx" in
    0) _evm_withdraw_token "$chain" "$chain_name" "$rpc" "$bridge_addr" "$on_admin" ;;
    1) _evm_withdraw_eth "$chain" "$chain_name" "$rpc" "$bridge_addr" "$on_admin" ;;
    *) return 0 ;;
  esac
}

_evm_withdraw_token() {
  local chain="$1" chain_name="$2" rpc="$3" bridge_addr="$4" on_admin="$5"

  echo "" >&2

  # Default token is the bridge's configured USDC
  local usdc_addr
  usdc_addr=$(evm_read "$rpc" "$bridge_addr" "usdcContract()(address)" 2>/dev/null | xargs) || true

  local token_addr
  token_addr=$(prompt_input "Token address to withdraw" "${usdc_addr:-}" evm_address) || return 0

  # Show current bridge balance of this token
  local vault_balance
  vault_balance=$(evm_read "$rpc" "$token_addr" "balanceOf(address)(uint256)" "$bridge_addr" 2>/dev/null | xargs) || vault_balance="0"

  local decimals
  decimals=$(evm_read "$rpc" "$token_addr" "decimals()(uint8)" 2>/dev/null | xargs) || decimals="6"

  info "Token:         $token_addr"
  info "Vault balance: ${vault_balance} ($(echo "scale=${decimals}; ${vault_balance} / 10^${decimals}" | bc 2>/dev/null || echo "?"))"

  local amount
  amount=$(prompt_input "Amount to withdraw (raw units)" "$vault_balance" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi

  local signer
  signer=$(evm_signer_address) || signer=""
  local to_addr
  to_addr=$(prompt_input "Recipient address" "${signer:-}" evm_address) || return 0

  print_summary "Withdraw Token" \
    "Bridge"    "$bridge_addr" \
    "Chain"     "$chain_name" \
    "Token"     "$token_addr" \
    "Amount"    "$amount" \
    "Recipient" "$to_addr"

  prompt_confirm "Proceed?" || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
    "withdrawToken(address,uint256,address)" "$token_addr" "$amount" "$to_addr") || return

  if [[ -n "$tx_hash" ]]; then
    local new_balance
    new_balance=$(evm_read "$rpc" "$token_addr" "balanceOf(address)(uint256)" "$bridge_addr" 2>/dev/null | xargs) || true
    info "New vault balance: ${new_balance:-?}"
    append_log "[evm/withdrawToken] chain=${chain} bridge=${bridge_addr} token=${token_addr} amount=${amount} to=${to_addr} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/withdrawToken] chain=${chain} bridge=${bridge_addr} token=${token_addr} amount=${amount} to=${to_addr} status=safe-queued"
  fi
}

_evm_withdraw_eth() {
  local chain="$1" chain_name="$2" rpc="$3" bridge_addr="$4" on_admin="$5"

  echo "" >&2

  # Show current ETH balance of the contract
  local eth_balance
  eth_balance=$(cast balance --rpc-url "$rpc" "$bridge_addr" 2>/dev/null) || eth_balance="0"
  info "Contract ETH balance: ${eth_balance} wei ($(cast from-wei "$eth_balance" 2>/dev/null || echo "?") ETH)"

  if [[ "$eth_balance" == "0" ]]; then
    warn "No ETH in the contract. Nothing to withdraw."
    return
  fi

  local signer
  signer=$(evm_signer_address) || signer=""
  local to_addr
  to_addr=$(prompt_input "Recipient address" "${signer:-}" evm_address) || return 0

  print_summary "Withdraw ETH" \
    "Bridge"    "$bridge_addr" \
    "Chain"     "$chain_name" \
    "Amount"    "${eth_balance} wei ($(cast from-wei "$eth_balance" 2>/dev/null || echo "?") ETH)" \
    "Recipient" "$to_addr"

  prompt_confirm "Proceed?" || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
    "withdrawETH(address)" "$to_addr") || return

  if [[ -n "$tx_hash" ]]; then
    local new_balance
    new_balance=$(cast balance --rpc-url "$rpc" "$bridge_addr" 2>/dev/null) || true
    info "New contract ETH balance: ${new_balance:-?} wei"
    append_log "[evm/withdrawETH] chain=${chain} bridge=${bridge_addr} to=${to_addr} amount=${eth_balance} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/withdrawETH] chain=${chain} bridge=${bridge_addr} to=${to_addr} amount=${eth_balance} status=safe-queued"
  fi
}
