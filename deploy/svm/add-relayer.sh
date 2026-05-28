#!/usr/bin/env bash
# svm/add-relayer.sh — Add relayer to bridge1024 program
# Sourced by bridge.sh; do not execute directly.

op_svm_add_relayer() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")

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

  # Fetch the on-chain relayer list once so the menu can mark duplicates upfront
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind "$kind" 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
  local on_chain_relayers=""
  if [[ -n "$on_chain_json" ]]; then
    on_chain_relayers=$(echo "$on_chain_json" | jq -r '.relayers[]?' 2>/dev/null | tr '\n' ' ')
  fi

  _is_on_chain_relayer() {
    local pk="$1"
    [[ -z "$pk" ]] && return 1
    [[ " $on_chain_relayers " == *" $pk "* ]]
  }

  # Select from relayers.json or manual input
  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  local relayer_pubkey=""

  if [[ -f "$relayer_file" ]] && [[ "$(jq length "$relayer_file")" -gt 0 ]]; then
    local names display_names=()
    mapfile -t names < <(jq -r '.[].name' "$relayer_file")
    local n pk
    for n in "${names[@]}"; do
      pk=$(get_relayer_field "$n" "svm_pubkey")
      if [[ -z "$pk" ]]; then
        display_names+=("${n}  (missing svm_pubkey)")
      elif _is_on_chain_relayer "$pk"; then
        display_names+=("${n}  (already added)")
      else
        display_names+=("$n")
      fi
    done
    display_names+=("Manual input")

    local idx
    idx=$(prompt_select "Select relayer:" "${display_names[@]}")

    if [[ "$idx" -lt "${#names[@]}" ]]; then
      local selected_name="${names[$idx]}"
      relayer_pubkey=$(get_relayer_field "$selected_name" "svm_pubkey")
      info "Selected: ${selected_name} (${relayer_pubkey})"
    fi
  fi

  if [[ -z "$relayer_pubkey" ]]; then
    relayer_pubkey=$(prompt_input "Relayer SVM public key" "" svm_pubkey) || return 0
  fi

  # Bail out early if the relayer is already registered, to avoid a guaranteed-revert tx
  if _is_on_chain_relayer "$relayer_pubkey"; then
    warn "$relayer_pubkey is already a registered relayer; nothing to do."
    return
  fi

  print_summary "Add Relayer" \
    "Target"  "$target_name" \
    "Program" "$program_id" \
    "Relayer" "$relayer_pubkey"

  prompt_confirm "Proceed?" || return

  info "Running add_relayer instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/add-relayer.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind "$kind" \
    --relayer "$relayer_pubkey"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/addRelayer] target=${target} program=${program_id} relayer=${relayer_pubkey}"
    success "Relayer added"
  fi
}
