#!/usr/bin/env bash
# evm/configure.sh — Configure Bridge1024 EVM contract
# Sourced by bridge.sh; do not execute directly.

op_evm_configure() {
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
  echo -e "  ${BOLD}── Configure Bridge on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  # Pre-flight
  evm_check_chain_id "$rpc" "$chain_id" || return

  local on_admin
  on_admin=$(evm_read "$rpc" "$bridge_addr" "admin()(address)" 2>/dev/null | xargs) || true
  if [[ -z "$on_admin" || "$on_admin" == "0x0000000000000000000000000000000000000000" ]]; then
    error "Cannot read admin from $bridge_addr"; return
  fi

  # Check if already configured
  local current_usdc
  current_usdc=$(evm_read "$rpc" "$bridge_addr" "usdcContract()(address)" 2>/dev/null | xargs) || true
  if [[ -n "$current_usdc" && "$current_usdc" != "0x0000000000000000000000000000000000000000" ]]; then
    warn "Bridge already configured. USDC: ${current_usdc}"
    warn "Re-configuring will change peer settings. In-flight nonces may be affected."
    prompt_confirm "Continue anyway?" || return
  fi

  # Check timelock status
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow for configuration changes."; return
  fi

  # Parameters
  local usdc_addr
  local default_usdc
  default_usdc=$(get_usdc_address "$chain")
  usdc_addr=$(prompt_input "USDC contract address" "$default_usdc" evm_address)

  # Peer contract: the 1024Chain program ID as bytes32
  local peer_program_id
  peer_program_id=$(read_address ".\"1024\".program_id")
  if [[ -z "$peer_program_id" ]]; then
    peer_program_id=$(prompt_input "Peer contract (1024Chain program ID, base58)" "" svm_pubkey) || return 0
  else
    peer_program_id=$(prompt_input "Peer contract (1024Chain program ID)" "$peer_program_id" svm_pubkey)
  fi
  # SVM pubkey is 32 bytes; pad to bytes32. For EVM<->SVM, the raw 32-byte pubkey is used directly.
  local peer_bytes32
  peer_bytes32="0x$(echo -n "$peer_program_id" | python3 -c "import sys, base58; print(base58.b58decode(sys.stdin.read().strip()).hex())" 2>/dev/null)" || \
    peer_bytes32=$(prompt_input "Peer contract as bytes32 (0x...)" "" bytes32) || return 0

  local local_chain_id="$chain_id"
  local peer_chain_id_key
  peer_chain_id_key=$(get_1024_chain_key "$CURRENT_ENV")
  local peer_chain_id="${CHAIN_ID[$peer_chain_id_key]}"

  print_summary "Configure Bridge" \
    "Bridge"         "$bridge_addr" \
    "Chain"          "$chain_name ($local_chain_id)" \
    "USDC"           "$usdc_addr" \
    "Peer contract"  "$peer_bytes32" \
    "Local chain ID" "$local_chain_id" \
    "Peer chain ID"  "$peer_chain_id"

  prompt_confirm "Proceed?" || return

  local tx_hash
  tx_hash=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
    "configure(address,bytes32,uint64,uint64)" \
    "$usdc_addr" "$peer_bytes32" "$local_chain_id" "$peer_chain_id") || return

  if [[ -n "$tx_hash" ]]; then
    success "Configuration applied"
    write_address ".evm.${chain}.usdc" "$usdc_addr"
    append_log "[evm/configure] chain=${chain} bridge=${bridge_addr} usdc=${usdc_addr} peer=${peer_bytes32} localChainId=${local_chain_id} peerChainId=${peer_chain_id} tx=${tx_hash}"
    print_tx_result "$chain" "$tx_hash"
  else
    append_log "[evm/configure] chain=${chain} bridge=${bridge_addr} usdc=${usdc_addr} peer=${peer_bytes32} localChainId=${local_chain_id} peerChainId=${peer_chain_id} status=safe-queued"
  fi
}
