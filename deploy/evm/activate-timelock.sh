#!/usr/bin/env bash
# evm/activate-timelock.sh — Activate timelock on Bridge1024 EVM
# Sourced by bridge.sh; do not execute directly.

op_evm_activate_timelock() {
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
  echo -e "  ${BOLD}── Activate Timelock on ${chain_name} ──${NC}" >&2
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

  # Check if already active
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    warn "Timelock is already active on this contract."
    return
  fi

  # Pre-activation checklist
  echo -e "  ${RED}${BOLD}⚠  WARNING: This operation is IRREVERSIBLE.${NC}"
  echo "  After activation, all admin operations require a 24h delay."
  echo ""
  echo "  Pre-activation checklist:"

  local usdc_configured
  usdc_configured=$(evm_read "$rpc" "$bridge_addr" "usdcContract()(address)" 2>/dev/null | xargs) || true
  if [[ -n "$usdc_configured" && "$usdc_configured" != "0x0000000000000000000000000000000000000000" ]]; then
    echo -e "  ${GREEN}✓${NC} Bridge configured"
  else
    echo -e "  ${RED}✗${NC} Bridge NOT configured — configure first!"
  fi

  local relayer_count
  relayer_count=$(evm_read "$rpc" "$bridge_addr" "getRelayerCount()(uint256)" 2>/dev/null | xargs) || true
  if [[ "${relayer_count:-0}" -ge 3 ]]; then
    echo -e "  ${GREEN}✓${NC} Relayers: ${relayer_count} (>= 3)"
  else
    echo -e "  ${YELLOW}⚠${NC} Relayers: ${relayer_count:-0} (recommended >= 3)"
  fi

  echo ""
  prompt_confirm "Activate timelock? This CANNOT be undone." || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" "activateTimelock()") || return

  if [[ -n "$tx_hash" ]]; then
    local verified
    verified=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
    print_verification "timelockActive" "true" "$verified"
    append_log "[evm/activateTimelock] chain=${chain} bridge=${bridge_addr} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/activateTimelock] chain=${chain} bridge=${bridge_addr} status=safe-queued"
  fi
}
