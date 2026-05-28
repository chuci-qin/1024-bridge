#!/usr/bin/env bash
# svm/register-peer.sh — Register a peer chain on bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_register_peer() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "hub" ]]; then
    error "register-peer is a hub-only operation (target ${target_name} runs the leaf program)."
    info "On leaf targets the single peer is set via 'Configure'."
    return
  fi

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
  local peer_keys=()
  local peer_chain_ids=()

  if [[ "$target" == 1024_* ]]; then
    # 1024 is the hub — can peer with EVM chains + Solana
    local evm_chains
    read -ra evm_chains <<< "$(get_evm_chains "$CURRENT_ENV")"
    for c in "${evm_chains[@]}"; do
      peer_keys+=("$c")
      peer_chain_ids+=("${CHAIN_ID[$c]}")
    done
    # Also Solana
    local sol_targets
    read -ra sol_targets <<< "$(get_svm_targets "$CURRENT_ENV")"
    for t in "${sol_targets[@]}"; do
      if [[ "$t" != "$target" ]]; then
        peer_keys+=("$t")
        peer_chain_ids+=("${CHAIN_ID[$t]}")
      fi
    done
  else
    # Solana — only peer with 1024Chain
    local c1024_key
    c1024_key=$(get_1024_chain_key "$CURRENT_ENV")
    peer_keys+=("$c1024_key")
    peer_chain_ids+=("${CHAIN_ID[$c1024_key]}")
  fi

  # Fetch already-registered peer chain ids from on-chain state, so the menu
  # can mark them as "(already registered)" upfront.
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local registered_ids=""
  if ((${#peer_chain_ids[@]} > 0)); then
    local cid_csv
    cid_csv=$(IFS=,; echo "${peer_chain_ids[*]}")
    local on_chain_json
    on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --program-kind hub \
      --peer-chain-ids "$cid_csv" 2>/dev/null) || on_chain_json=""
    on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
    if [[ -n "$on_chain_json" ]]; then
      registered_ids=$(echo "$on_chain_json" | jq -r '.peers[]?.chainId' 2>/dev/null | tr '\n' ' ')
    fi
  fi

  _is_registered_peer() {
    local cid="$1"
    [[ -z "$cid" ]] && return 1
    [[ " $registered_ids " == *" $cid "* ]]
  }

  local peer_options=()
  local i
  for i in "${!peer_keys[@]}"; do
    local k="${peer_keys[$i]}"
    local cid="${peer_chain_ids[$i]}"
    local label="${CHAIN_DISPLAY[$k]} (chain_id: ${cid})"
    if _is_registered_peer "$cid"; then
      label="${label}  (already registered)"
    fi
    peer_options+=("$label")
  done
  peer_options+=("Manual input")
  peer_keys+=("manual")
  peer_chain_ids+=("")

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

  # Bail out early if the peer is already registered to avoid a guaranteed-revert tx
  if _is_registered_peer "$peer_chain_id"; then
    warn "Peer chain $peer_chain_id is already registered; use a separate update flow to change it."
    return
  fi

  info "All amounts in USDC raw units (6 decimals)"
  echo ""

  local bridge_fee
  bridge_fee=$(prompt_input "Bridge fee (raw USDC, 6 decimals; 0 = no fee)" "0" uint)
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
    --program-kind hub \
    --chain-id "$peer_chain_id" \
    --peer-contract "$peer_contract_hex" \
    --bridge-fee "$bridge_fee" \
    --max-stake-amount "$max_stake_amount"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/registerPeer] target=${target} program=${program_id} peerChainId=${peer_chain_id} peerContract=${peer_contract_hex} fee=${bridge_fee} maxStake=${max_stake_amount}"
    success "Peer registered"
  fi
}
