#!/usr/bin/env bash
# svm/configure-peer-fee.sh — Configure per-peer bridge fee on bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_configure_peer_fee() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "hub" ]]; then
    error "configure-peer-fee is a hub-only operation (per-peer fee on PeerConfig)."
    info "On leaf targets the bridge fee is set via 'Configure bridge fee'."
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

  echo "" >&2
  echo -e "  ${BOLD}── Configure Peer Fee on ${target_name} ──${NC}" >&2
  echo "" >&2

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "Program: $program_id"
  info "Target:  $target_name"
  info "RPC:     $rpc"

  # Collect all possible peer chain IDs so read-state.ts returns their PeerConfig
  local peer_ids_csv=""
  local c
  for c in $(get_evm_chains "$CURRENT_ENV"); do
    peer_ids_csv="${peer_ids_csv:+${peer_ids_csv},}${CHAIN_ID[$c]}"
  done
  for c in $(get_svm_targets "$CURRENT_ENV"); do
    [[ "$c" == "$target" ]] && continue
    peer_ids_csv="${peer_ids_csv:+${peer_ids_csv},}${CHAIN_ID[$c]}"
  done

  # Read on-chain peers to show current fees and let user pick
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind hub \
    --peer-chain-ids "$peer_ids_csv" 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)

  if [[ -z "$on_chain_json" ]]; then
    error "Failed to read program state"; return
  fi

  # List registered peers
  local peer_count
  peer_count=$(echo "$on_chain_json" | jq '.peers | length' 2>/dev/null)
  if [[ "${peer_count:-0}" -eq 0 ]]; then
    error "No peers registered. Register a peer first."; return
  fi

  local peer_labels=() peer_ids=() peer_fees=()
  local i=0
  while [[ $i -lt $peer_count ]]; do
    local cid fee_raw
    cid=$(echo "$on_chain_json" | jq -r ".peers[$i].chainId" 2>/dev/null)
    fee_raw=$(echo "$on_chain_json" | jq -r ".peers[$i].bridgeFee" 2>/dev/null)
    peer_ids+=("$cid")
    peer_fees+=("$fee_raw")
    local fee_human
    fee_human=$(echo "scale=6; ${fee_raw} / 1000000" | bc 2>/dev/null || echo "?")
    peer_labels+=("Chain $cid — current fee: ${fee_raw} (${fee_human} USDC)")
    ((i++))
  done

  local idx
  idx=$(prompt_select "Select peer to configure fee:" "${peer_labels[@]}")

  local chain_id="${peer_ids[$idx]}"
  local current_fee="${peer_fees[$idx]}"

  echo "" >&2
  info "Selected peer: chain ID $chain_id"
  info "Current fee: ${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  local new_fee
  new_fee=$(prompt_input "New bridge fee (raw USDC, 6 decimals; 0 = no fee)" "$current_fee" uint) || return 0

  print_summary "Configure Peer Fee" \
    "Target"       "$target_name" \
    "Program"      "$program_id" \
    "Peer chain"   "$chain_id" \
    "Current fee"  "${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "New fee"      "${new_fee} ($(echo "scale=6; ${new_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  prompt_confirm "Proceed?" || return

  info "Running configure_peer_fee instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure-peer-fee.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind hub \
    --chain-id "$chain_id" \
    --fee "$new_fee"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/configurePeerFee] target=${target} program=${program_id} chainId=${chain_id} fee=${new_fee}"
    success "Peer fee configured"
  fi
}
