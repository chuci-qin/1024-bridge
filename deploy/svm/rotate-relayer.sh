#!/usr/bin/env bash
# svm/rotate-relayer.sh — Atomically replace a relayer on bridge1024 / bridge1024_hub
# Sourced by bridge.sh; do not execute directly.
#
# Calls `rotateRelayer(old_relayer, new_relayer)`. Available on both hub and
# leaf programs. Removes `old` and inserts `new` in one timelock-gated tx,
# avoiding the brief window between remove+add where the relayer count is
# below the BFT threshold.

op_svm_rotate_relayer() {
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

  echo "" >&2
  echo -e "  ${BOLD}── Rotate Relayer on ${target_name} (${kind}) ──${NC}" >&2
  echo "" >&2

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "Program: $program_id"
  info "Target:  $target_name"

  # Pull current relayer list so the menu only offers slots that exist
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind "$kind" 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
  if [[ -z "$on_chain_json" ]]; then
    error "Failed to read program state"; return
  fi

  local relayer_count
  relayer_count=$(echo "$on_chain_json" | jq -r '.relayers | length' 2>/dev/null)
  if [[ "${relayer_count:-0}" -eq 0 ]]; then
    error "No relayers registered. Use 'Add relayer' first."
    return
  fi

  local old_options=()
  local i=0
  while [[ $i -lt $relayer_count ]]; do
    local r
    r=$(echo "$on_chain_json" | jq -r ".relayers[$i]")
    old_options+=("[$i] $r")
    ((i++))
  done
  old_options+=("Manual input")

  local idx
  idx=$(prompt_select "Select OLD relayer to replace:" "${old_options[@]}")

  local old_relayer
  if [[ "$idx" -lt "$relayer_count" ]]; then
    old_relayer=$(echo "$on_chain_json" | jq -r ".relayers[$idx]")
  else
    old_relayer=$(prompt_input "Old relayer SVM pubkey" "" svm_pubkey) || return 0
  fi

  # Sanity: confirm it's actually on-chain (avoid the inevitable revert)
  local found_old=0
  i=0
  while [[ $i -lt $relayer_count ]]; do
    local r
    r=$(echo "$on_chain_json" | jq -r ".relayers[$i]")
    if [[ "$r" == "$old_relayer" ]]; then found_old=1; break; fi
    ((i++))
  done
  if [[ "$found_old" != "1" ]]; then
    error "$old_relayer is not a registered relayer; nothing to rotate."
    return
  fi

  # Pick the new relayer — prefer relayers.json entries that aren't already on-chain
  local on_chain_relayers
  on_chain_relayers=$(echo "$on_chain_json" | jq -r '.relayers[]?' | tr '\n' ' ')

  local new_relayer=""
  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  if [[ -f "$relayer_file" ]] && [[ "$(jq length "$relayer_file")" -gt 0 ]]; then
    local names display_names=()
    mapfile -t names < <(jq -r '.[].name' "$relayer_file")
    local n pk
    for n in "${names[@]}"; do
      pk=$(get_relayer_field "$n" "svm_pubkey")
      if [[ -z "$pk" ]]; then
        display_names+=("${n}  (missing svm_pubkey)")
      elif [[ " $on_chain_relayers " == *" $pk "* ]]; then
        display_names+=("${n}  (already added)")
      else
        display_names+=("${n}  -> $pk")
      fi
    done
    display_names+=("Manual input")

    local nidx
    nidx=$(prompt_select "Select NEW relayer:" "${display_names[@]}")
    if [[ "$nidx" -lt "${#names[@]}" ]]; then
      local selected_name="${names[$nidx]}"
      new_relayer=$(get_relayer_field "$selected_name" "svm_pubkey")
    fi
  fi
  if [[ -z "$new_relayer" ]]; then
    new_relayer=$(prompt_input "New relayer SVM pubkey" "" svm_pubkey) || return 0
  fi

  if [[ " $on_chain_relayers " == *" $new_relayer "* ]]; then
    error "$new_relayer is already a registered relayer; rotation would revert."
    return
  fi
  if [[ "$new_relayer" == "$old_relayer" ]]; then
    error "Old and new relayer are the same."
    return
  fi

  print_summary "Rotate Relayer" \
    "Target"  "$target_name" \
    "Program" "$program_id" \
    "Kind"    "$kind" \
    "Old"     "$old_relayer" \
    "New"     "$new_relayer"

  prompt_confirm "Proceed?" || return

  info "Running rotate_relayer instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/rotate-relayer.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind "$kind" \
    --old "$old_relayer" \
    --new "$new_relayer"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/rotateRelayer] target=${target} kind=${kind} program=${program_id} old=${old_relayer} new=${new_relayer}"
    success "Relayer rotated"
  fi
}
