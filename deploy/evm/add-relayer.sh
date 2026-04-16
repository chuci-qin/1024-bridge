#!/usr/bin/env bash
# evm/add-relayer.sh — Add relayer to Bridge1024 EVM contract
# Sourced by bridge.sh; do not execute directly.

op_evm_add_relayer() {
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
  echo -e "  ${BOLD}── Add Relayer on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  # Check timelock
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow to add relayers."; return
  fi

  # Show current relayer count
  local count
  count=$(evm_read "$rpc" "$bridge_addr" "getRelayerCount()(uint256)" 2>/dev/null | xargs) || true
  info "Current relayer count: ${count:-unknown}"

  # Select from relayers.json or manual input
  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  local relayer_addr=""

  if [[ -f "$relayer_file" ]] && [[ "$(jq length "$relayer_file")" -gt 0 ]]; then
    local names
    mapfile -t names < <(jq -r '.[].name' "$relayer_file")
    names+=("Manual input")

    local idx
    idx=$(prompt_select "Select relayer:" "${names[@]}")

    if [[ "$idx" -lt $((${#names[@]} - 1)) ]]; then
      local selected_name="${names[$idx]}"
      relayer_addr=$(get_relayer_field "$selected_name" "evm_address")
      info "Selected: ${selected_name} (${relayer_addr})"
    fi
  fi

  if [[ -z "$relayer_addr" ]]; then
    relayer_addr=$(prompt_input "Relayer EVM address" "" evm_address) || return 0
  fi

  # Check if already added
  local is_relayer
  is_relayer=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$relayer_addr" 2>/dev/null | xargs) || true
  if [[ "$is_relayer" == "true" ]]; then
    warn "This address is already a registered relayer."
    return
  fi

  print_summary "Add Relayer" \
    "Bridge"  "$bridge_addr" \
    "Chain"   "$chain_name" \
    "Relayer" "$relayer_addr"

  prompt_confirm "Proceed?" || return

  evm_simulate "$rpc" "$bridge_addr" "addRelayer(address)" "$relayer_addr" || return

  local output
  output=$(evm_send "$rpc" "$bridge_addr" "addRelayer(address)" "$relayer_addr" 2>&1)

  local tx_hash
  tx_hash=$(echo "$output" | grep -i "transactionHash" | awk '{print $NF}') || \
    tx_hash=$(echo "$output" | head -1)

  # Verify
  local verified
  verified=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$relayer_addr" 2>/dev/null | xargs) || true
  print_verification "isRelayer" "true" "$verified"

  local new_count
  new_count=$(evm_read "$rpc" "$bridge_addr" "getRelayerCount()(uint256)" 2>/dev/null | xargs) || true
  info "Relayer count: ${new_count}"

  append_log "[evm/addRelayer] chain=${chain} bridge=${bridge_addr} relayer=${relayer_addr} tx=${tx_hash:-unknown}"
  print_tx_result "$chain" "${tx_hash:-unknown}"
}
