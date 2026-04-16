#!/usr/bin/env bash
# svm/initialize.sh — Initialize bridge1024 program
# Sourced by bridge.sh; do not execute directly.

op_svm_initialize() {
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
  echo -e "  ${BOLD}── Initialize bridge1024 on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then error "Keypair file not found: $keypair_path"; return; fi

  local guardian operator recovery

  guardian=$(prompt_address_or_gen "Guardian SVM pubkey" "svm" "guardian" \
    "$(read_address '.roles.guardian_svm')")

  operator=$(prompt_address_or_gen "Operator SVM pubkey" "svm" "operator" \
    "$(read_address '.roles.operator_svm')")

  recovery=$(prompt_address_or_gen "Recovery SVM pubkey" "svm" "recovery" \
    "$(read_address '.roles.recovery_svm')")

  print_summary "Initialize bridge1024" \
    "Target"     "$target_name" \
    "Program"    "$program_id" \
    "Guardian"   "$guardian" \
    "Operator"   "$operator" \
    "Recovery"   "$recovery"

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running initialize instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/initialize.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --guardian "$guardian" \
    --operator "$operator" \
    --recovery "$recovery"

  if [[ $? -eq 0 ]]; then
    write_address ".roles.guardian_svm" "$guardian"
    write_address ".roles.operator_svm" "$operator"
    write_address ".roles.recovery_svm" "$recovery"

    local admin_pubkey
    admin_pubkey=$(solana-keygen pubkey "$keypair_path" 2>/dev/null)
    write_address ".roles.admin_svm" "$admin_pubkey"

    append_log "[svm/initialize] target=${target} program=${program_id} guardian=${guardian} operator=${operator} recovery=${recovery}"
    success "Initialization complete"
  fi
}
