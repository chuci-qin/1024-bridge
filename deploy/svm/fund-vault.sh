#!/usr/bin/env bash
# svm/fund-vault.sh — Transfer USDC into the bridge1024 program vault.
# Sourced by bridge.sh; do not execute directly.
#
# Mirrors deploy/evm/fund-vault.sh: fetches current vault balance, asks
# for a raw USDC amount (6 decimals), then SPL-transfers from the signer's
# ATA into the vault PDA's ATA. If the vault ATA doesn't exist yet (very
# first top-up after configure), the underlying TS helper creates it
# atomically in the same transaction.

op_svm_fund_vault() {
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
  echo -e "  ${BOLD}── Fund Vault on ${target_name} ──${NC}" >&2
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

  # Read current state (usdcMint, vault PDA + ATA + balance) via read-state.ts.
  # Same trick as info.sh: don't merge stderr into stdout (npm warns would
  # corrupt the JSON), and only keep the last line that starts with '{'.
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

  local usdc_mint vault_pda vault_ata vault_bal
  usdc_mint=$(echo "$out" | jq -r '.usdcMint')
  vault_pda=$(echo "$out" | jq -r '.vaultPda')
  vault_ata=$(echo "$out" | jq -r '.vaultAta')
  vault_bal=$(echo "$out" | jq -r '.vaultBalance')

  if [[ -z "$usdc_mint" || "$usdc_mint" == "11111111111111111111111111111111" ]]; then
    error "USDC mint not configured on this program — run 'Configure' first."
    return
  fi

  info "USDC mint:  $usdc_mint"
  info "Vault PDA:  $vault_pda"
  if [[ -n "$vault_ata" && "$vault_ata" != "null" ]]; then
    info "Vault ATA:  $vault_ata"
  else
    info "Vault ATA:  (not created yet; will be created on first top-up)"
  fi
  local vault_bal_human
  vault_bal_human=$(echo "scale=6; ${vault_bal:-0} / 1000000" | bc 2>/dev/null || echo "?")
  info "Vault balance: ${vault_bal:-0} (${vault_bal_human} USDC)"

  # Show signer USDC balance — uses the same get-token-balance helper that
  # stake.sh's polling uses, so behavior is consistent (prints "0" if ATA missing)
  local signer_bal signer_bal_human
  signer_bal=$(npx ts-node "$svm_deploy_dir/src/instructions/get-token-balance.ts" \
    --rpc-url "$rpc" \
    --mint "$usdc_mint" \
    --owner "$signer_pk" 2>/dev/null) || signer_bal="0"
  [[ -z "$signer_bal" ]] && signer_bal="0"
  signer_bal_human=$(echo "scale=6; $signer_bal / 1000000" | bc 2>/dev/null || echo "?")
  info "Signer USDC balance: ${signer_bal} (${signer_bal_human} USDC)"

  local amount
  amount=$(prompt_input "Amount to transfer (raw USDC, 6 decimals)" "" uint) || return 0
  if [[ -z "$amount" || "$amount" == "0" ]]; then
    info "Cancelled (amount is 0)."
    return
  fi

  # Pre-flight: signer must hold enough — better to fail here than burn a tx
  if [[ "$signer_bal" =~ ^[0-9]+$ ]] && (( amount > signer_bal )); then
    error "Insufficient USDC: need ${amount}, have ${signer_bal}."
    return
  fi

  local amount_human
  amount_human=$(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?")

  print_summary "Fund Vault" \
    "Target"     "$target_name" \
    "Program"    "$program_id" \
    "USDC mint"  "$usdc_mint" \
    "Vault PDA"  "$vault_pda" \
    "Amount"     "${amount} (${amount_human} USDC)"

  prompt_confirm "Proceed with USDC transfer to vault?" || return

  local result
  if ! result=$(npx ts-node "$svm_deploy_dir/src/instructions/fund-vault.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --amount "$amount"); then
    error "Vault funding failed (see stderr above)"
    return
  fi

  # The TS helper prints status text on stdout (e.g. "Vault USDC ATA ... creating...")
  # plus a trailing JSON line — keep only the JSON for parsing
  local result_json
  result_json=$(echo "$result" | grep -E '^\{' | tail -n 1)
  local sig new_bal new_bal_human
  if [[ -n "$result_json" ]]; then
    sig=$(echo "$result_json" | jq -r '.signature // empty')
    new_bal=$(echo "$result_json" | jq -r '.vaultBalance // empty')
  fi
  [[ -z "$new_bal" ]] && new_bal="?"
  new_bal_human=$(echo "scale=6; ${new_bal:-0} / 1000000" | bc 2>/dev/null || echo "?")

  success "Vault funded."
  info "  signature:    ${sig:-?}"
  info "  vault balance: ${new_bal} (${new_bal_human} USDC)"

  append_log "[svm/fundVault] target=${target} program=${program_id} mint=${usdc_mint} amount=${amount} sig=${sig:-unknown}"
}
