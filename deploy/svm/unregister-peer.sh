#!/usr/bin/env bash
# svm/unregister-peer.sh — Unregister a peer chain from bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_unregister_peer() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "hub" ]]; then
    error "unregister-peer is a hub-only operation (target ${target_name} runs the leaf program)."
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
  echo -e "  ${BOLD}── Unregister Peer on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  # Determine all candidate peer chain IDs for this target
  local peer_keys=()
  local peer_chain_ids=()

  if [[ "$target" == 1024_* ]]; then
    local evm_chains
    read -ra evm_chains <<< "$(get_evm_chains "$CURRENT_ENV")"
    for c in "${evm_chains[@]}"; do
      peer_keys+=("$c")
      peer_chain_ids+=("${CHAIN_ID[$c]}")
    done
    local sol_targets
    read -ra sol_targets <<< "$(get_svm_targets "$CURRENT_ENV")"
    for t in "${sol_targets[@]}"; do
      if [[ "$t" != "$target" ]]; then
        peer_keys+=("$t")
        peer_chain_ids+=("${CHAIN_ID[$t]}")
      fi
    done
  else
    local c1024_key
    c1024_key=$(get_1024_chain_key "$CURRENT_ENV")
    peer_keys+=("$c1024_key")
    peer_chain_ids+=("${CHAIN_ID[$c1024_key]}")
  fi

  local svm_deploy_dir="$DEPLOY_DIR/svm"

  # Fetch currently registered peer chain IDs from on-chain state
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

  # Build menu showing only registered peers
  local peer_options=()
  local menu_keys=()
  local menu_chain_ids=()

  local i
  for i in "${!peer_keys[@]}"; do
    local k="${peer_keys[$i]}"
    local cid="${peer_chain_ids[$i]}"
    if _is_registered_peer "$cid"; then
      peer_options+=("${CHAIN_DISPLAY[$k]} (chain_id: ${cid})")
      menu_keys+=("$k")
      menu_chain_ids+=("$cid")
    fi
  done
  peer_options+=("Manual input")
  menu_keys+=("manual")
  menu_chain_ids+=("")

  if [[ "${#peer_options[@]}" -eq 1 ]]; then
    warn "No registered peers found on $target_name."
    return
  fi

  local idx
  idx=$(prompt_select "Select peer to unregister:" "${peer_options[@]}")

  local peer_chain_id

  if [[ "${menu_keys[$idx]}" == "manual" ]]; then
    peer_chain_id=$(prompt_input "Peer chain ID" "" uint) || return 0
  else
    peer_chain_id="${menu_chain_ids[$idx]}"
  fi

  print_summary "Unregister Peer" \
    "Target"        "$target_name" \
    "Program"       "$program_id" \
    "Peer chain ID" "$peer_chain_id"

  prompt_confirm "Proceed? (This will close the peer config account and is irreversible without re-registering)" || return

  info "Running unregister_peer instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/unregister-peer.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind hub \
    --chain-id "$peer_chain_id"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/unregisterPeer] target=${target} program=${program_id} peerChainId=${peer_chain_id}"
    success "Peer unregistered"
  fi
}
