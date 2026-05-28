#!/usr/bin/env bash
# svm/configure-peer-rate-limits.sh — Hub-only: per-peer rate limits
# Sourced by bridge.sh; do not execute directly.

op_svm_configure_peer_rate_limits() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "hub" ]]; then
    error "configure-peer-rate-limits is hub-only (leaf has a single global rate-limit set)."
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
  echo -e "  ${BOLD}── Configure Peer Rate Limits on ${target_name} (hub) ──${NC}" >&2
  echo "" >&2

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "Program: $program_id"
  info "Target:  $target_name"

  # Pull all candidate peer chain IDs so read-state.ts can return their PeerConfigs
  local peer_ids_csv=""
  local c
  for c in $(get_evm_chains "$CURRENT_ENV"); do
    peer_ids_csv="${peer_ids_csv:+${peer_ids_csv},}${CHAIN_ID[$c]}"
  done
  for c in $(get_svm_targets "$CURRENT_ENV"); do
    [[ "$c" == "$target" ]] && continue
    peer_ids_csv="${peer_ids_csv:+${peer_ids_csv},}${CHAIN_ID[$c]}"
  done

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind hub \
    --peer-chain-ids "$peer_ids_csv" 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
  if [[ -z "$on_chain_json" ]]; then error "Failed to read program state"; return; fi

  local peer_count
  peer_count=$(echo "$on_chain_json" | jq '.peers | length' 2>/dev/null)
  if [[ "${peer_count:-0}" -eq 0 ]]; then
    error "No peers registered. Register a peer first."; return
  fi

  local peer_labels=() peer_ids=()
  local i=0
  while [[ $i -lt $peer_count ]]; do
    local cid m_w w_d m_s m_stake
    cid=$(echo "$on_chain_json" | jq -r ".peers[$i].chainId")
    m_w=$(echo "$on_chain_json" | jq -r ".peers[$i].maxUnlockPerWindow")
    w_d=$(echo "$on_chain_json" | jq -r ".peers[$i].windowDuration")
    m_s=$(echo "$on_chain_json" | jq -r ".peers[$i].maxSingleUnlock")
    m_stake=$(echo "$on_chain_json" | jq -r ".peers[$i].maxStakeAmount")
    peer_ids+=("$cid")
    peer_labels+=("Chain $cid — maxPerWindow=${m_w} windowDuration=${w_d}s maxSingle=${m_s} maxStake=${m_stake}")
    ((i++))
  done

  local idx
  idx=$(prompt_select "Select peer to configure rate limits:" "${peer_labels[@]}")
  local chain_id="${peer_ids[$idx]}"

  info "Selected peer: chain ID $chain_id"

  # Pull current values for sensible defaults
  local cur_w cur_d cur_s cur_st
  cur_w=$(echo "$on_chain_json" | jq -r --arg cid "$chain_id" '.peers[] | select(.chainId == $cid) | .maxUnlockPerWindow')
  cur_d=$(echo "$on_chain_json" | jq -r --arg cid "$chain_id" '.peers[] | select(.chainId == $cid) | .windowDuration')
  cur_s=$(echo "$on_chain_json" | jq -r --arg cid "$chain_id" '.peers[] | select(.chainId == $cid) | .maxSingleUnlock')
  cur_st=$(echo "$on_chain_json" | jq -r --arg cid "$chain_id" '.peers[] | select(.chainId == $cid) | .maxStakeAmount')

  info "All amounts in USDC raw units (6 decimals)"
  echo ""

  local max_per_window window_duration max_single max_stake
  max_per_window=$(prompt_input "Max unlock per window (0 = unlimited)" "${cur_w:-10000000000}" uint)
  window_duration=$(prompt_input "Window duration (seconds)" "${cur_d:-3600}" uint)
  max_single=$(prompt_input "Max single unlock (0 = unlimited)" "${cur_s:-5000000000}" uint)
  max_stake=$(prompt_input "Max stake amount (0 = unlimited)" "${cur_st:-5000000000}" uint)

  print_summary "Configure Peer Rate Limits" \
    "Target"          "$target_name" \
    "Program"         "$program_id" \
    "Peer chain"      "$chain_id" \
    "Max per window"  "${max_per_window} ($(echo "scale=0; ${max_per_window} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Window duration" "${window_duration}s" \
    "Max single"      "${max_single} ($(echo "scale=0; ${max_single} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Max stake"       "${max_stake} ($(echo "scale=0; ${max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  prompt_confirm "Proceed?" || return

  info "Running configure_peer_rate_limits instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure-peer-rate-limits.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind hub \
    --chain-id "$chain_id" \
    --max-per-window "$max_per_window" \
    --window-duration "$window_duration" \
    --max-single "$max_single" \
    --max-stake "$max_stake"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/configurePeerRateLimits] target=${target} program=${program_id} chainId=${chain_id} maxPerWindow=${max_per_window} windowDuration=${window_duration} maxSingle=${max_single} maxStake=${max_stake}"
    success "Peer rate limits configured"
  fi
}
