#!/usr/bin/env bash
# svm/fund-relayers.sh — Top up relayer wallets with native gas (SOL).
# Sourced by bridge.sh; do not execute directly.
#
# Iterates relayers.json, shows each one's current SOL balance on the
# selected SVM chain, and lets you enter a per-relayer amount in SOL.
# Empty input or "0" skips that relayer, so a single pass can top up only
# the ones that are running low. Works for both 1024_* and solana* chains.

op_svm_fund_relayers() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local relayer_file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  if [[ ! -f "$relayer_file" ]]; then
    error "relayers.json not found: $relayer_file"; return
  fi
  local rcount
  rcount=$(jq length "$relayer_file" 2>/dev/null || echo 0)
  if [[ "$rcount" -eq 0 ]]; then
    error "No relayers configured in $relayer_file"; return
  fi

  echo ""
  echo -e "  ${BOLD}── Fund Relayers on ${target_name} (native gas) ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM signer keypair path") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then
    error "Keypair file not found: $keypair_path"; return
  fi

  local signer_pk
  signer_pk=$(solana-keygen pubkey "$keypair_path" 2>/dev/null || echo "")
  if [[ -z "$signer_pk" ]]; then error "Cannot derive signer pubkey from $keypair_path"; return; fi
  local signer_bal
  signer_bal=$(solana balance --url "$rpc" "$signer_pk" 2>/dev/null || echo "?")

  info "Chain:    $target_name"
  info "RPC:      $rpc"
  info "Signer:   $signer_pk"
  info "Balance:  ${signer_bal}"

  echo ""
  info "Showing all relayers from $relayer_file with current balance on ${target_name}."
  info "Enter amount in SOL per relayer (e.g. 0.1). Empty / 0 = skip."

  local names
  mapfile -t names < <(jq -r '.[].name' "$relayer_file")

  declare -a pk_arr=() bal_arr=()
  local n pk bal
  for n in "${names[@]}"; do
    pk=$(get_relayer_field "$n" "svm_pubkey")
    if [[ -z "$pk" ]]; then
      pk_arr+=("")
      bal_arr+=("(no svm_pubkey)")
    else
      bal=$(solana balance --url "$rpc" "$pk" 2>/dev/null || echo "?")
      pk_arr+=("$pk")
      bal_arr+=("$bal")
    fi
  done

  echo ""
  printf "  %-12s %-46s %s\n" "Name" "Pubkey" "Current balance"
  local i
  for i in "${!names[@]}"; do
    printf "  %-12s %-46s %s\n" "${names[$i]}" "${pk_arr[$i]:-—}" "${bal_arr[$i]}"
  done
  echo ""

  # Optional default amount applied to every prompt
  local default_amt
  default_amt=$(prompt_input "Default amount per relayer (SOL; blank = no default)" "0")
  [[ -z "$default_amt" ]] && default_amt="0"

  local -a amounts=()
  for i in "${!names[@]}"; do
    if [[ -z "${pk_arr[$i]}" ]]; then
      amounts+=("0")
      continue
    fi
    local raw
    raw=$(prompt_input "Amount for ${names[$i]} (${pk_arr[$i]}, current ${bal_arr[$i]})" "$default_amt")
    [[ -z "$raw" || "$raw" == "0" || "$raw" == "0.0" ]] && raw="0"
    amounts+=("$raw")
  done

  # Build a confirmation summary listing only the non-skipped recipients
  local total="0"
  local -a plan_args=()
  for i in "${!names[@]}"; do
    local amt="${amounts[$i]}"
    [[ "$amt" == "0" ]] && continue
    [[ -z "${pk_arr[$i]}" ]] && continue
    plan_args+=("${names[$i]} → ${pk_arr[$i]}" "${amt} SOL")
    total=$(echo "${total} + ${amt}" | bc -l 2>/dev/null || echo "$total")
  done

  if (( ${#plan_args[@]} == 0 )); then
    info "Nothing to send (all amounts are 0)."
    return
  fi

  print_summary "Fund Relayers" \
    "Chain"  "$target_name" \
    "Signer" "$signer_pk" \
    "Total"  "${total} SOL" \
    "${plan_args[@]}"

  prompt_confirm "Proceed with sending these transfers?" || return

  local sent=0 failed=0
  for i in "${!names[@]}"; do
    local amt="${amounts[$i]}" pk="${pk_arr[$i]}"
    [[ "$amt" == "0" || -z "$pk" ]] && continue
    info "Sending ${amt} SOL → ${names[$i]} (${pk})..."
    local out sig
    if ! out=$(solana transfer \
        --url "$rpc" \
        --keypair "$keypair_path" \
        --allow-unfunded-recipient \
        --output json \
        "$pk" "$amt" 2>&1); then
      error "Transfer failed for ${names[$i]}: $out"
      ((failed++))
      continue
    fi
    sig=$(echo "$out" | jq -r '.signature // empty' 2>/dev/null)
    [[ -z "$sig" ]] && sig=$(echo "$out" | grep -iE 'signature' | head -1)
    success "  sig: ${sig:-(see above)}"
    append_log "[svm/fundRelayer] target=${target} relayer=${names[$i]} pk=${pk} amount=${amt}SOL sig=${sig:-unknown}"
    ((sent++))
  done

  echo ""
  if (( failed > 0 )); then
    warn "${sent} transfer(s) sent, ${failed} failed."
  else
    success "All ${sent} transfer(s) sent."
  fi

  echo ""
  info "Post-funding balances:"
  for i in "${!names[@]}"; do
    [[ -z "${pk_arr[$i]}" ]] && continue
    local newbal
    newbal=$(solana balance --url "$rpc" "${pk_arr[$i]}" 2>/dev/null || echo "?")
    printf "  %-12s %-46s %s\n" "${names[$i]}" "${pk_arr[$i]}" "$newbal"
  done
}
