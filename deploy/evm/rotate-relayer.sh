#!/usr/bin/env bash
# evm/rotate-relayer.sh — Atomically rotate a relayer on Bridge1024 EVM
# Sourced by bridge.sh; do not execute directly.
#
# rotateRelayer(address oldRelayer, address newRelayer) replaces one relayer in
# one tx — preferred over remove+add because it never drops the relayer count
# below the BFT threshold during the swap.
#
# Timelock: when timelockActive, op_hash =
#   keccak256(abi.encode("rotateRelayer", oldRelayer, newRelayer))
# matching Bridge1024.sol's compute_op_hashv at line 657-659.

# Compute the op hash that matches Bridge1024.sol's timelock keying for
# rotateRelayer (lines 658-659 in src/Bridge1024.sol).
_evm_compute_rotate_op_hash() {
  local old_addr="$1" new_addr="$2"
  local data
  data=$(cast abi-encode "f(string,address,address)" "rotateRelayer" "$old_addr" "$new_addr")
  cast keccak "$data"
}

op_evm_rotate_relayer() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  local bridge_addr
  bridge_addr=$(read_address ".evm.${chain}.bridge")
  if [[ -z "$bridge_addr" ]]; then error "Bridge not deployed on $chain_name. Deploy first."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Rotate Relayer on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  local on_admin
  on_admin=$(evm_read "$rpc" "$bridge_addr" "admin()(address)" 2>/dev/null | xargs) || true
  if [[ -z "$on_admin" || "$on_admin" == "0x0000000000000000000000000000000000000000" ]]; then
    error "Cannot read admin from $bridge_addr"; return
  fi

  local count
  count=$(evm_read "$rpc" "$bridge_addr" "getRelayerCount()(uint256)" 2>/dev/null | xargs) || count="0"
  if [[ "${count:-0}" == "0" ]]; then
    error "No relayers registered. Use 'Add relayer' first."
    return
  fi

  # Build the current on-chain relayer list for the picker
  local current_relayers=()
  local i=0
  while [[ $i -lt $count ]]; do
    local r
    r=$(evm_read "$rpc" "$bridge_addr" "relayers(uint256)(address)" "$i" 2>/dev/null | xargs) || break
    [[ -n "$r" ]] && current_relayers+=("$r")
    ((i++))
  done

  # Pick OLD relayer from the on-chain list
  local old_options=()
  for r in "${current_relayers[@]}"; do
    old_options+=("$r")
  done
  old_options+=("Manual input")

  local oidx
  oidx=$(prompt_select "Select OLD relayer to replace:" "${old_options[@]}")

  local old_addr
  if [[ "$oidx" -lt "${#current_relayers[@]}" ]]; then
    old_addr="${current_relayers[$oidx]}"
  else
    old_addr=$(prompt_input "Old relayer EVM address" "" evm_address) || return 0
  fi

  # Verify the chosen address is on-chain — otherwise the contract would
  # revert RelayerNotFound after we just paid for simulation.
  local found_old=0
  for r in "${current_relayers[@]}"; do
    if [[ "${r,,}" == "${old_addr,,}" ]]; then found_old=1; break; fi
  done
  if [[ "$found_old" != "1" ]]; then
    error "${old_addr} is not a registered relayer."
    return
  fi

  # Pick NEW relayer — prefer relayers.json entries that aren't already on-chain
  local new_addr=""
  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  if [[ -f "$relayer_file" ]] && [[ "$(jq length "$relayer_file")" -gt 0 ]]; then
    local names display_names=()
    mapfile -t names < <(jq -r '.[].name' "$relayer_file")
    local n addr is_r
    for n in "${names[@]}"; do
      addr=$(get_relayer_field "$n" "evm_address")
      if [[ -z "$addr" ]]; then
        display_names+=("${n}  (missing evm_address)")
      else
        is_r=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$addr" 2>/dev/null | xargs) || is_r=""
        if [[ "$is_r" == "true" ]]; then
          display_names+=("${n}  (already added)")
        else
          display_names+=("${n}  -> $addr")
        fi
      fi
    done
    display_names+=("Manual input")

    local nidx
    nidx=$(prompt_select "Select NEW relayer:" "${display_names[@]}")

    if [[ "$nidx" -lt "${#names[@]}" ]]; then
      local selected_name="${names[$nidx]}"
      new_addr=$(get_relayer_field "$selected_name" "evm_address")
    fi
  fi
  if [[ -z "$new_addr" ]]; then
    new_addr=$(prompt_input "New relayer EVM address" "" evm_address) || return 0
  fi

  # Pre-flight: new must not be already a relayer; new != old
  local new_is_r
  new_is_r=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$new_addr" 2>/dev/null | xargs) || new_is_r=""
  if [[ "$new_is_r" == "true" ]]; then
    error "${new_addr} is already a relayer; rotation would revert RelayerAlreadyExists."
    return
  fi
  if [[ "${new_addr,,}" == "${old_addr,,}" ]]; then
    error "Old and new addresses are the same."
    return
  fi

  print_summary "Rotate Relayer" \
    "Bridge"  "$bridge_addr" \
    "Chain"   "$chain_name" \
    "Old"     "$old_addr" \
    "New"     "$new_addr"

  prompt_confirm "Proceed?" || return

  # Timelock-aware dispatch. We can't reuse _evm_send_role_op because that
  # builds opHash from (string,address) — rotateRelayer is (string,address,address).
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true

  local tx
  if [[ "$timelock_active" != "true" ]]; then
    info "Timelock not active — executing rotateRelayer directly..."
    tx=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" \
      "rotateRelayer(address,address)" "$old_addr" "$new_addr") || return 1
  else
    # Timelock active: schedule (24h) or execute (after eta) per existing role-op semantics
    local op_hash data eta now grace
    op_hash=$(_evm_compute_rotate_op_hash "$old_addr" "$new_addr")
    data=$(cast abi-encode "f(string,address,address)" "rotateRelayer" "$old_addr" "$new_addr")
    eta=$(evm_read "$rpc" "$bridge_addr" "timelockEta(bytes32)(uint64)" "$op_hash" 2>/dev/null | xargs) || eta=0
    now=$(date +%s)
    grace=$((48 * 3600))

    info "Timelock active"
    info "  opHash: $op_hash"

    if [[ -z "$eta" || "$eta" == "0" ]]; then
      info "Not scheduled yet."
      local idx
      idx=$(prompt_select "Choose:" "Schedule (wait 24h before execute)" "← Back")
      [[ "$idx" == "0" ]] || return 0
      tx=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" "scheduleOperation(bytes)" "$data") || return 1
      [[ -n "$tx" ]] && success "Scheduled; executable after ~$(date -u -d "@$(($(date +%s) + 86400))" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo 'now+24h')"
    else
      local eta_str expire_str
      eta_str=$(date -u -d "@$eta" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "@$eta")
      expire_str=$(date -u -d "@$((eta + grace))" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "@$((eta + grace))")
      info "Scheduled ETA:     $eta_str"
      info "          Expires: $expire_str"

      if (( now < eta )); then
        local remain=$((eta - now))
        warn "Not executable yet — $((remain / 3600))h $((remain % 3600 / 60))m remaining."
        return 0
      fi
      if (( now > eta + grace )); then
        error "Operation expired (> 48h grace). Re-schedule."
        return 1
      fi

      local idx
      idx=$(prompt_select "Status: executable. Choose:" "Execute rotateRelayer" "← Back")
      [[ "$idx" == "0" ]] || return 0

      tx=$(evm_send_as "$on_admin" "$rpc" "$bridge_addr" "rotateRelayer(address,address)" "$old_addr" "$new_addr") || return 1
    fi
  fi

  if [[ -n "$tx" ]]; then
    local old_is new_is
    old_is=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$old_addr" 2>/dev/null | xargs) || true
    new_is=$(evm_read "$rpc" "$bridge_addr" "isRelayer(address)(bool)" "$new_addr" 2>/dev/null | xargs) || true
    print_verification "isRelayer(old)" "false" "$old_is"
    print_verification "isRelayer(new)" "true"  "$new_is"

    append_log "[evm/rotateRelayer] chain=${chain} bridge=${bridge_addr} old=${old_addr} new=${new_addr} tx=${tx}"
    print_tx_result "$chain" "$tx"
  else
    append_log "[evm/rotateRelayer] chain=${chain} bridge=${bridge_addr} old=${old_addr} new=${new_addr} status=safe-queued"
  fi
}
