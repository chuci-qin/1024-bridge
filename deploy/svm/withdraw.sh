#!/usr/bin/env bash
# svm/withdraw.sh — Withdraw tokens from bridge1024 program vault
# Sourced by bridge.sh; do not execute directly.

op_svm_withdraw() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local target_id="${CHAIN_ID[$target]}"
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
  if [[ -z "$program_id" ]]; then
    error "Program not deployed on $target_name. Deploy first."
    return
  fi

  echo "" >&2
  echo -e "  ${BOLD}── Withdraw from ${target_name} ──${NC}" >&2
  echo "" >&2

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

  info "Program: $program_id"
  info "Target:  $target_name (ID: $target_id)"
  info "RPC:     $rpc"
  info "Signer:  $signer_pk"

  # Read current state to get USDC mint + vault balance
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local out
  out=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id") || {
    error "Failed to read program state (see stderr above)"
    return
  }
  out=$(echo "$out" | grep -E '^\{' | tail -n 1)
  if [[ -z "$out" ]]; then error "read-state.ts returned empty/non-JSON output"; return; fi

  local usdc_mint vault_bal timelock_active
  usdc_mint=$(echo "$out" | jq -r '.usdcMint')
  vault_bal=$(echo "$out" | jq -r '.vaultBalance')
  timelock_active=$(echo "$out" | jq -r '.timelockActive // false')

  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow for withdrawals."; return
  fi

  if [[ -z "$usdc_mint" || "$usdc_mint" == "11111111111111111111111111111111" ]]; then
    error "USDC mint not configured. Run 'Configure' first."; return
  fi

  local vault_bal_human
  vault_bal_human=$(echo "scale=6; ${vault_bal:-0} / 1000000" | bc 2>/dev/null || echo "?")
  info "USDC mint:      $usdc_mint"
  info "Vault balance:  ${vault_bal:-0} (${vault_bal_human} USDC)"

  echo "" >&2

  # Token mint — default to the bridge's USDC
  local mint
  mint=$(prompt_input "Token mint to withdraw" "$usdc_mint" svm_pubkey) || return 0

  local amount
  amount=$(prompt_input "Amount to withdraw (raw units, 6 decimals for USDC)" "${vault_bal:-}" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi

  local to_addr
  to_addr=$(prompt_input "Recipient wallet pubkey" "$signer_pk" svm_pubkey) || return 0

  local amount_human
  amount_human=$(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?")

  print_summary "Withdraw Token" \
    "Program"   "$program_id" \
    "Target"    "$target_name" \
    "Mint"      "$mint" \
    "Amount"    "${amount} (${amount_human} USDC)" \
    "Recipient" "$to_addr"

  prompt_confirm "Proceed with withdrawal?" || return

  local result
  if ! result=$(npx ts-node "$svm_deploy_dir/src/instructions/withdraw-token.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --mint "$mint" \
      --amount "$amount" \
      --to "$to_addr"); then
    error "Withdrawal failed (see stderr above)"
    return
  fi

  local result_json
  result_json=$(echo "$result" | grep -E '^\{' | tail -n 1)
  local sig new_bal new_bal_human
  if [[ -n "$result_json" ]]; then
    sig=$(echo "$result_json" | jq -r '.signature // empty')
    new_bal=$(echo "$result_json" | jq -r '.newVaultBalance // empty')
  fi
  [[ -z "$new_bal" ]] && new_bal="?"
  new_bal_human=$(echo "scale=6; ${new_bal:-0} / 1000000" | bc 2>/dev/null || echo "?")

  success "Withdrawal complete."
  info "  signature:    ${sig:-?}"
  info "  vault balance: ${new_bal} (${new_bal_human} USDC)"

  append_log "[svm/withdrawToken] target=${target} program=${program_id} mint=${mint} amount=${amount} to=${to_addr} sig=${sig:-unknown}"
}
