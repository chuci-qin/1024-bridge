#!/usr/bin/env bash
# svm/register-peer.sh — Register a peer chain on bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_register_peer() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

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
  echo -e "  ${BOLD}── Register Peer on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  # Determine available peer chains
  local peer_options=()
  local peer_keys=()

  if [[ "$target" == 1024_* ]]; then
    # 1024 is the hub — can peer with EVM chains + Solana
    local evm_chains
    read -ra evm_chains <<< "$(get_evm_chains "$CURRENT_ENV")"
    for c in "${evm_chains[@]}"; do
      peer_options+=("${CHAIN_DISPLAY[$c]} (chain_id: ${CHAIN_ID[$c]})")
      peer_keys+=("$c")
    done
    # Also Solana
    local sol_targets
    read -ra sol_targets <<< "$(get_svm_targets "$CURRENT_ENV")"
    for t in "${sol_targets[@]}"; do
      if [[ "$t" != "$target" ]]; then
        peer_options+=("${CHAIN_DISPLAY[$t]} (chain_id: ${CHAIN_ID[$t]})")
        peer_keys+=("$t")
      fi
    done
  else
    # Solana — only peer with 1024Chain
    local c1024_key
    c1024_key=$(get_1024_chain_key "$CURRENT_ENV")
    peer_options+=("${CHAIN_DISPLAY[$c1024_key]} (chain_id: ${CHAIN_ID[$c1024_key]})")
    peer_keys+=("$c1024_key")
  fi
  peer_options+=("Manual input")
  peer_keys+=("manual")

  local idx
  idx=$(prompt_select "Select peer chain:" "${peer_options[@]}")

  local peer_chain_id peer_contract_hex

  if [[ "${peer_keys[$idx]}" == "manual" ]]; then
    peer_chain_id=$(prompt_input "Peer chain ID" "" uint) || return 0
    peer_contract_hex=$(prompt_input "Peer contract (64-char hex, no 0x)" "" hex64) || return 0
  else
    local peer_key="${peer_keys[$idx]}"
    peer_chain_id="${CHAIN_ID[$peer_key]}"

    # Auto-lookup peer contract address
    if [[ "$peer_key" == 1024_* ]]; then
      local prog
      prog=$(read_address ".\"1024\".program_id")
      if [[ -n "$prog" ]]; then
        # SVM pubkey -> 32 bytes hex
        peer_contract_hex=$(python3 -c "import base58; print(base58.b58decode('$prog').hex())" 2>/dev/null) || true
      fi
    elif [[ "$peer_key" == solana* ]]; then
      local prog
      prog=$(read_address ".solana.program_id")
      if [[ -n "$prog" ]]; then
        peer_contract_hex=$(python3 -c "import base58; print(base58.b58decode('$prog').hex())" 2>/dev/null) || true
      fi
    else
      # EVM chain — bridge contract address -> bytes32 (left-padded)
      local bridge
      bridge=$(read_address ".evm.${peer_key}.bridge")
      if [[ -n "$bridge" ]]; then
        local addr_no_prefix="${bridge#0x}"
        peer_contract_hex=$(printf '%064s' "$addr_no_prefix" | tr ' ' '0')
      fi
    fi

    peer_contract_hex=$(prompt_input "Peer contract (64-char hex)" "$peer_contract_hex" hex64)
  fi

  info "All amounts in USDC raw units (6 decimals)"
  echo ""

  # Bridge fee 仅在 1024 链上收取（hub 端统一记账）；
  # Solana 等卫星链强制 fee=0，不再提示输入
  local bridge_fee
  if [[ "$target" == 1024_* ]]; then
    bridge_fee=$(prompt_input "Bridge fee (raw, 0 to disable)" "0" uint)
  else
    bridge_fee="0"
    info "Bridge fee 强制为 0（仅 1024 链可设置 fee）"
  fi
  local max_stake_amount
  max_stake_amount=$(prompt_input "Max stake amount (raw)" "5000000000" uint)

  local fee_disp stake_disp
  fee_disp="${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  stake_disp="${max_stake_amount} ($(echo "scale=0; ${max_stake_amount} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  print_summary "Register Peer" \
    "Target"          "$target_name" \
    "Program"         "$program_id" \
    "Peer chain ID"   "$peer_chain_id" \
    "Peer contract"   "$peer_contract_hex" \
    "Bridge fee"      "$fee_disp" \
    "Max stake amount" "$stake_disp"

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running register_peer instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/register-peer.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --chain-id "$peer_chain_id" \
    --peer-contract "$peer_contract_hex" \
    --bridge-fee "$bridge_fee" \
    --max-stake-amount "$max_stake_amount"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/registerPeer] target=${target} program=${program_id} peerChainId=${peer_chain_id} peerContract=${peer_contract_hex} fee=${bridge_fee} maxStake=${max_stake_amount}"
    success "Peer registered"
  fi
}
