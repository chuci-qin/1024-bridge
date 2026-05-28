#!/usr/bin/env bash
# svm/configure.sh — Configure bridge1024 / bridge1024_hub program
# Sourced by bridge.sh; do not execute directly.
#
# Hub  (1024_*):    sets usdc_mint + local_chain_id only. Per-peer details
#                   (peer_contract, fees, max stake) go through register_peer.
# Leaf (solana*):   single-shot configure mirroring EVM Bridge1024.configure(...)
#                   — usdc, peer_contract (the 1024 hub program ID), peer_chain_id,
#                   bridge_fee. Without all five the leaf can neither stake nor unlock.

op_svm_configure() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")

  local addr_key
  if [[ "$target" == 1024_* ]]; then
    addr_key=".\"1024\".program_id"
  else
    addr_key=".solana.program_id"
  fi
  local program_id
  program_id=$(read_address "$addr_key")
  if [[ -z "$program_id" ]]; then error "Program not deployed on $target_name. Deploy first."; return; fi

  echo ""
  echo -e "  ${BOLD}── Configure bridge1024 (${kind}) on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  # USDC mint: prefer the historical value in addresses.json, fall back to the
  # bundled default in config/.env.
  local usdc_key
  if [[ "$target" == 1024_* ]]; then
    usdc_key=".\"1024\".usdc_mint"
  else
    usdc_key=".solana.usdc_mint"
  fi
  local default_usdc
  default_usdc=$(read_address "$usdc_key")
  [[ -z "$default_usdc" ]] && default_usdc=$(get_usdc_address "$target" 2>/dev/null || echo "")
  local usdc_mint
  usdc_mint=$(prompt_input "USDC mint address" "$default_usdc" svm_pubkey) || return 0

  # Local chain ID (always set, both kinds)
  local local_chain_id="${CHAIN_ID[$target]}"
  local_chain_id=$(prompt_input "Local chain ID" "$local_chain_id" uint)

  # Leaf-only extra fields: peer = the 1024 hub program ID (32B), peer_chain = 1024
  local peer_contract_hex="" peer_chain_id="" bridge_fee=""
  if [[ "$kind" == "leaf" ]]; then
    # Default peer = the 1024 hub program id from addresses.json
    local default_peer_hex=""
    local hub_prog
    hub_prog=$(read_address ".\"1024\".program_id")
    if [[ -n "$hub_prog" ]]; then
      default_peer_hex=$(python3 -c "import base58; print(base58.b58decode('$hub_prog').hex())" 2>/dev/null) || true
    fi
    peer_contract_hex=$(prompt_input "Peer contract (64-char hex; default = 1024 hub program id)" "$default_peer_hex" hex64) || return 0

    local c1024_key default_peer_chain
    c1024_key=$(get_1024_chain_key "$CURRENT_ENV")
    default_peer_chain="${CHAIN_ID[$c1024_key]}"
    peer_chain_id=$(prompt_input "Peer chain ID" "$default_peer_chain" uint)

    bridge_fee=$(prompt_input "Bridge fee (raw USDC, 6 decimals; 0 = no fee)" "0" uint)
  fi

  if [[ "$kind" == "hub" ]]; then
    print_summary "Configure bridge1024_hub" \
      "Target"         "$target_name" \
      "Program"        "$program_id" \
      "Kind"           "hub" \
      "USDC mint"      "$usdc_mint" \
      "Local chain ID" "$local_chain_id"
  else
    print_summary "Configure bridge1024 (leaf)" \
      "Target"         "$target_name" \
      "Program"        "$program_id" \
      "Kind"           "leaf" \
      "USDC mint"      "$usdc_mint" \
      "Peer contract"  "0x${peer_contract_hex}" \
      "Local chain ID" "$local_chain_id" \
      "Peer chain ID"  "$peer_chain_id" \
      "Bridge fee"     "${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running configure instruction..."

  if [[ "$kind" == "hub" ]]; then
    npx ts-node "$svm_deploy_dir/src/instructions/configure.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --program-kind hub \
      --usdc-mint "$usdc_mint" \
      --local-chain-id "$local_chain_id"
    local rc=$?
  else
    npx ts-node "$svm_deploy_dir/src/instructions/configure.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --program-kind leaf \
      --usdc-mint "$usdc_mint" \
      --peer-contract "$peer_contract_hex" \
      --local-chain-id "$local_chain_id" \
      --peer-chain-id "$peer_chain_id" \
      --bridge-fee "$bridge_fee"
    local rc=$?
  fi

  if [[ $rc -eq 0 ]]; then
    write_address "$usdc_key" "$usdc_mint"
    if [[ "$kind" == "leaf" ]]; then
      append_log "[svm/configure] target=${target} kind=leaf program=${program_id} usdcMint=${usdc_mint} localChainId=${local_chain_id} peerChainId=${peer_chain_id} peerContract=0x${peer_contract_hex} bridgeFee=${bridge_fee}"
    else
      append_log "[svm/configure] target=${target} kind=hub  program=${program_id} usdcMint=${usdc_mint} localChainId=${local_chain_id}"
    fi
    success "Configuration complete"
  fi
}
