#!/usr/bin/env bash
# svm/deploy.sh — Deploy bridge1024 program to SVM chain
# Sourced by bridge.sh; do not execute directly.

op_svm_deploy() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Deploy bridge1024 to ${target_name} ──${NC}" >&2
  echo "" >&2

  local svm_dir="$PROJECT_ROOT/contracts/svm"
  local program_so="$svm_dir/target/deploy/bridge1024.so"
  local program_keypair="$svm_dir/target/deploy/bridge1024-keypair.json"

  if [[ ! -f "$program_so" ]]; then
    error "Program binary not found. Run build first."; return
  fi
  if [[ ! -f "$program_keypair" ]]; then
    error "Program keypair not found at target/deploy/bridge1024-keypair.json. Run build first."; return
  fi

  # Signer keypair (payer)
  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM signer keypair path") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then error "Signer keypair not found: $keypair_path"; return; fi

  local signer_pubkey
  signer_pubkey=$(solana-keygen pubkey "$keypair_path" 2>/dev/null)

  local program_id
  program_id=$(solana-keygen pubkey "$program_keypair" 2>/dev/null)

  local balance
  balance=$(solana balance --url "$rpc" "$signer_pubkey" 2>/dev/null || echo "unknown")

  local addr_key
  if [[ "$target" == 1024_* ]]; then
    addr_key=".\"1024\".program_id"
  else
    addr_key=".solana.program_id"
  fi
  local existing_id
  existing_id=$(read_address "$addr_key")

  info "Target:   $target_name"
  info "RPC:      $rpc"
  info "Signer:   $signer_pubkey"
  info "Balance:  $balance"
  info "Program:  $program_id"
  if [[ -n "$existing_id" ]]; then
    warn "Existing: $existing_id (will be replaced)"
  fi

  prompt_confirm "Deploy program to ${target_name}?" || return

  info "Deploying..."
  local output
  output=$(solana program deploy \
    --url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_keypair" \
    "$program_so" 2>&1)

  local deployed_id
  deployed_id=$(echo "$output" | grep -i "program id" | awk '{print $NF}') || true

  if [[ -n "$deployed_id" ]]; then
    success "Program deployed: $deployed_id"
    write_address "$addr_key" "$deployed_id"
    append_log "[svm/deploy] target=${target} program_id=${deployed_id} signer=${signer_pubkey}"
  else
    error "Deployment may have failed. Output:"
    echo "$output" >&2
  fi
}
