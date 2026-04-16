#!/usr/bin/env bash
# evm/fund-vault.sh — Transfer USDC to Bridge1024 contract vault
# Sourced by bridge.sh; do not execute directly.

op_evm_fund_vault() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  local bridge_addr
  bridge_addr=$(read_address ".evm.${chain}.bridge")
  if [[ -z "$bridge_addr" ]]; then error "Bridge not deployed on $chain_name. Deploy first."; return; fi

  local usdc_addr
  usdc_addr=$(read_address ".evm.${chain}.usdc")
  if [[ -z "$usdc_addr" ]]; then
    usdc_addr=$(get_usdc_address "$chain")
  fi
  if [[ -z "$usdc_addr" ]]; then error "USDC address unknown for $chain_name. Configure bridge first."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Fund Vault on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"
  info "USDC:     $usdc_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  # Show current vault balance
  local current_balance
  current_balance=$(evm_read "$rpc" "$usdc_addr" "balanceOf(address)(uint256)" "$bridge_addr" 2>/dev/null | xargs) || true
  info "Current vault USDC balance: ${current_balance:-0} ($(echo "scale=2; ${current_balance:-0} / 1000000" | bc) USDC)"

  # Show signer's USDC balance
  local signer
  signer=$(evm_signer_address)
  if [[ -n "$signer" ]]; then
    local signer_balance
    signer_balance=$(evm_read "$rpc" "$usdc_addr" "balanceOf(address)(uint256)" "$signer" 2>/dev/null | xargs) || true
    info "Your USDC balance: ${signer_balance:-0} ($(echo "scale=2; ${signer_balance:-0} / 1000000" | bc) USDC)"
  fi

  local amount
  amount=$(prompt_input "Amount to transfer (raw USDC, 6 decimals)" "" uint) || return 0

  print_summary "Fund Vault" \
    "Bridge"     "$bridge_addr" \
    "USDC"       "$usdc_addr" \
    "Amount"     "${amount} ($(echo "scale=2; $amount / 1000000" | bc) USDC)" \
    "Chain"      "$chain_name"

  prompt_confirm "Proceed with USDC transfer?" || return

  # ERC20 transfer
  evm_simulate "$rpc" "$usdc_addr" "transfer(address,uint256)(bool)" "$bridge_addr" "$amount" || return

  local output
  output=$(evm_send "$rpc" "$usdc_addr" "transfer(address,uint256)(bool)" "$bridge_addr" "$amount" 2>&1)

  local tx_hash
  tx_hash=$(echo "$output" | grep -i "transactionHash" | awk '{print $NF}') || \
    tx_hash=$(echo "$output" | head -1)

  # Verify
  local new_balance
  new_balance=$(evm_read "$rpc" "$usdc_addr" "balanceOf(address)(uint256)" "$bridge_addr" 2>/dev/null | xargs) || true
  info "New vault USDC balance: ${new_balance:-?} ($(echo "scale=2; ${new_balance:-0} / 1000000" | bc) USDC)"

  append_log "[evm/fundVault] chain=${chain} bridge=${bridge_addr} usdc=${usdc_addr} amount=${amount} tx=${tx_hash:-unknown}"
  print_tx_result "$chain" "${tx_hash:-unknown}"
}
