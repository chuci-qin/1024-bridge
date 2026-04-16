#!/usr/bin/env bash
# svm/add-relayer.sh — Add relayer to bridge1024 program
# Sourced by bridge.sh; do not execute directly.

op_svm_add_relayer() {
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
  echo -e "  ${BOLD}── Add Relayer on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  # Select from relayers.json or manual input
  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  local relayer_pubkey=""

  if [[ -f "$relayer_file" ]] && [[ "$(jq length "$relayer_file")" -gt 0 ]]; then
    local names
    mapfile -t names < <(jq -r '.[].name' "$relayer_file")
    names+=("Manual input")

    local idx
    idx=$(prompt_select "Select relayer:" "${names[@]}")

    if [[ "$idx" -lt $((${#names[@]} - 1)) ]]; then
      local selected_name="${names[$idx]}"
      relayer_pubkey=$(get_relayer_field "$selected_name" "svm_pubkey")
      info "Selected: ${selected_name} (${relayer_pubkey})"
    fi
  fi

  if [[ -z "$relayer_pubkey" ]]; then
    relayer_pubkey=$(prompt_input "Relayer SVM public key" "" svm_pubkey) || return 0
  fi

  print_summary "Add Relayer" \
    "Target"  "$target_name" \
    "Program" "$program_id" \
    "Relayer" "$relayer_pubkey"

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running add_relayer instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/add-relayer.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --relayer "$relayer_pubkey"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/addRelayer] target=${target} program=${program_id} relayer=${relayer_pubkey}"
    success "Relayer added"
  fi
}
