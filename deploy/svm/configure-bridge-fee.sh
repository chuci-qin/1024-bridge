#!/usr/bin/env bash
# svm/configure-bridge-fee.sh — Configure leaf bridge fee on bridge1024
# Sourced by bridge.sh; do not execute directly.
#
# Leaf-only: hub uses per-peer fees (see configure-peer-fee.sh).

op_svm_configure_bridge_fee() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "leaf" ]]; then
    error "configure-bridge-fee is leaf-only. Use 'Configure peer fee' on the hub."
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
  echo -e "  ${BOLD}── Configure Bridge Fee on ${target_name} (leaf) ──${NC}" >&2
  echo "" >&2

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "Program: $program_id"
  info "Target:  $target_name"

  # Show current fee via read-state.ts (leaf returns .bridgeFee at top level)
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind leaf 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
  local current_fee=0
  if [[ -n "$on_chain_json" ]]; then
    current_fee=$(echo "$on_chain_json" | jq -r '.bridgeFee // "0"')
  fi
  info "Current bridge fee: ${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "MAX_FEE:           1000000000 (1000 USDC)"

  echo "" >&2

  local fee
  fee=$(prompt_input "New bridge fee (raw USDC, 6 decimals; 0 = no fee)" "$current_fee" uint) || return 0
  if (( fee > 1000000000 )); then
    error "Fee ${fee} exceeds MAX_FEE (1000000000 = 1000 USDC)"; return
  fi

  print_summary "Configure Bridge Fee (leaf)" \
    "Target"       "$target_name" \
    "Program"      "$program_id" \
    "Current fee"  "${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "New fee"      "${fee} ($(echo "scale=6; ${fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  prompt_confirm "Proceed?" || return

  info "Running configure_bridge_fee instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure-bridge-fee.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind leaf \
    --fee "$fee"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/configureBridgeFee] target=${target} kind=leaf program=${program_id} fee=${fee}"
    success "Bridge fee configured"
  fi
}
