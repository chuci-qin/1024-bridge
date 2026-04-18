#!/usr/bin/env bash
# evm/fund-relayers.sh — Top up relayer wallets with native gas (ETH).
# Sourced by bridge.sh; do not execute directly.
#
# Iterates relayers.json, shows each one's current native balance on the
# selected EVM chain, and lets you enter a per-relayer amount in ETH. Empty
# input or "0" skips that relayer, so a single pass can top up only the
# ones that are running low.

op_evm_fund_relayers() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  if [[ ! -f "$relayer_file" ]]; then
    error "relayers.json not found: $relayer_file"; return
  fi
  local rcount
  rcount=$(jq length "$relayer_file" 2>/dev/null || echo 0)
  if [[ "$rcount" -eq 0 ]]; then
    error "No relayers configured in $relayer_file"; return
  fi

  echo "" >&2
  echo -e "  ${BOLD}── Fund Relayers on ${chain_name} (native gas) ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"

  evm_check_chain_id "$rpc" "$chain_id" || return

  local signer
  signer=$(evm_signer_address) || signer=""
  if [[ -z "$signer" ]]; then error "Cannot determine signer address"; return; fi
  local signer_bal
  signer_bal=$(cast balance --ether --rpc-url "$rpc" "$signer" 2>/dev/null || echo "?")
  info "Signer:   $signer"
  info "Balance:  ${signer_bal} ETH"

  echo "" >&2
  info "Showing all relayers from $relayer_file with current balance on ${chain_name}."
  info "Enter amount in ETH per relayer (e.g. 0.05). Empty / 0 = skip."

  # Collect names + addresses, plus pretty current balance per relayer
  local names addrs balances
  mapfile -t names < <(jq -r '.[].name' "$relayer_file")

  local n addr bal
  declare -a addr_arr=() bal_arr=()
  for n in "${names[@]}"; do
    addr=$(get_relayer_field "$n" "evm_address")
    if [[ -z "$addr" ]]; then
      addr_arr+=("")
      bal_arr+=("(no evm_address)")
    else
      bal=$(cast balance --ether --rpc-url "$rpc" "$addr" 2>/dev/null || echo "?")
      addr_arr+=("$addr")
      bal_arr+=("${bal} ETH")
    fi
  done

  echo "" >&2
  printf "  %-12s %-44s %s\n" "Name" "Address" "Current balance" >&2
  local i
  for i in "${!names[@]}"; do
    printf "  %-12s %-44s %s\n" "${names[$i]}" "${addr_arr[$i]:-—}" "${bal_arr[$i]}" >&2
  done
  echo "" >&2

  # Optional default amount applied to every prompt
  local default_amt
  default_amt=$(prompt_input "Default amount per relayer (ETH; blank = no default)" "0")
  [[ -z "$default_amt" ]] && default_amt="0"

  # Per-relayer amount with the default pre-filled
  local -a amounts=()
  for i in "${!names[@]}"; do
    if [[ -z "${addr_arr[$i]}" ]]; then
      amounts+=("0")
      continue
    fi
    local raw
    raw=$(prompt_input "Amount for ${names[$i]} (${addr_arr[$i]}, current ${bal_arr[$i]})" "$default_amt")
    [[ -z "$raw" || "$raw" == "0" || "$raw" == "0.0" ]] && raw="0"
    amounts+=("$raw")
  done

  # Build a confirmation summary listing only the non-skipped recipients
  local total_eth="0"
  local -a plan_args=()
  for i in "${!names[@]}"; do
    local amt="${amounts[$i]}"
    [[ "$amt" == "0" ]] && continue
    [[ -z "${addr_arr[$i]}" ]] && continue
    plan_args+=("${names[$i]} → ${addr_arr[$i]}" "${amt} ETH")
    total_eth=$(echo "${total_eth} + ${amt}" | bc -l 2>/dev/null || echo "$total_eth")
  done

  if (( ${#plan_args[@]} == 0 )); then
    info "Nothing to send (all amounts are 0)."
    return
  fi

  print_summary "Fund Relayers" \
    "Chain"  "$chain_name" \
    "Signer" "$signer" \
    "Total"  "${total_eth} ETH" \
    "${plan_args[@]}"

  prompt_confirm "Proceed with sending these transfers?" || return

  # Send them sequentially so any failure stops the loop with context
  local sign_flags
  sign_flags=$(evm_sign_flags)
  local sent=0 failed=0
  for i in "${!names[@]}"; do
    local amt="${amounts[$i]}" addr="${addr_arr[$i]}"
    [[ "$amt" == "0" || -z "$addr" ]] && continue
    info "Sending ${amt} ETH → ${names[$i]} (${addr})..."
    local out tx_hash
    if ! out=$(cast send --rpc-url "$rpc" $sign_flags --value "${amt}ether" "$addr" 2>&1); then
      error "Transfer failed for ${names[$i]}: $out"
      ((failed++))
      continue
    fi
    tx_hash=$(evm_extract_tx_hash "$out")
    success "  tx: ${tx_hash}"
    append_log "[evm/fundRelayer] chain=${chain} relayer=${names[$i]} addr=${addr} amount=${amt}ETH tx=${tx_hash:-unknown}"
    ((sent++))
  done

  echo "" >&2
  if (( failed > 0 )); then
    warn "${sent} transfer(s) sent, ${failed} failed."
  else
    success "All ${sent} transfer(s) sent."
  fi

  # Print the post-funding balances so the user can sanity-check at a glance
  echo "" >&2
  info "Post-funding balances:"
  for i in "${!names[@]}"; do
    [[ -z "${addr_arr[$i]}" ]] && continue
    local newbal
    newbal=$(cast balance --ether --rpc-url "$rpc" "${addr_arr[$i]}" 2>/dev/null || echo "?")
    printf "  %-12s %-44s %s ETH\n" "${names[$i]}" "${addr_arr[$i]}" "$newbal" >&2
  done
}
