#!/usr/bin/env bash
# svm/activate-timelock.sh — Activate timelock on bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_activate_timelock() {
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
  echo -e "  ${BOLD}── Activate Timelock on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  echo -e "  ${RED}${BOLD}⚠  WARNING: This operation is IRREVERSIBLE.${NC}"
  echo "  After activation, all admin operations require a 24h delay."
  echo ""

  prompt_confirm "Activate timelock? This CANNOT be undone." || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running activate_timelock instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/activate-timelock.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/activateTimelock] target=${target} program=${program_id}"
    success "Timelock activated"
  fi
}
