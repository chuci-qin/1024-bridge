#!/usr/bin/env bash
# svm/configure-gasless-fee.sh — Configure leaf gasless fee on bridge1024
# Sourced by bridge.sh; do not execute directly.
#
# Leaf-only: hub has no gasless path. Setting fee = 0 disables stake_gasless
# without touching the plain stake path.

op_svm_configure_gasless_fee() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "leaf" ]]; then
    error "configure-gasless-fee is leaf-only (hub program has no gasless path)."
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
  echo -e "  ${BOLD}── Configure Gasless Fee on ${target_name} (leaf) ──${NC}" >&2
  echo "" >&2

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "Program: $program_id"
  info "Target:  $target_name"

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
    current_fee=$(echo "$on_chain_json" | jq -r '.gaslessFee // "0"')
  fi
  info "Current gasless fee: ${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "MAX_FEE:             1000000000 (1000 USDC)"
  if [[ "$current_fee" == "0" ]]; then
    warn "gaslessFee == 0: gasless deposit path is currently DISABLED"
    warn "  stake_gasless will revert GaslessDisabled"
  fi

  echo "" >&2

  local fee
  fee=$(prompt_input "New gasless fee (raw USDC, 6 decimals; 0 = disable gasless path)" "$current_fee" uint) || return 0
  if (( fee > 1000000000 )); then
    error "Fee ${fee} exceeds MAX_FEE (1000000000 = 1000 USDC)"; return
  fi

  print_summary "Configure Gasless Fee (leaf)" \
    "Target"       "$target_name" \
    "Program"      "$program_id" \
    "Current fee"  "${current_fee} ($(echo "scale=6; ${current_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "New fee"      "${fee} ($(echo "scale=6; ${fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  if [[ "$fee" == "0" ]]; then
    warn "Setting gaslessFee to 0 will DISABLE the gasless deposit path."
    warn "Users will not be able to use stake_gasless (existing stake() unaffected)."
  fi

  prompt_confirm "Proceed?" || return

  info "Running configure_gasless_fee instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure-gasless-fee.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind leaf \
    --fee "$fee"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/configureGaslessFee] target=${target} kind=leaf program=${program_id} fee=${fee}"
    success "Gasless fee configured"
  fi
}
