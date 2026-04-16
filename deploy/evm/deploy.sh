#!/usr/bin/env bash
# evm/deploy.sh — Deploy Bridge1024 EVM contract
# Sourced by bridge.sh; do not execute directly.

op_evm_deploy() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Deploy Bridge1024 on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"

  # Pre-flight checks
  evm_check_chain_id "$rpc" "$chain_id" || return
  evm_check_balance "$rpc"

  # Admin = deployer (msg.sender), prompt for other 3 role addresses
  local admin guardian operator recovery
  admin=$(evm_signer_address)

  guardian=$(prompt_address_or_gen "Guardian address" "evm" "guardian" \
    "$(read_address '.roles.guardian_evm')")

  operator=$(prompt_address_or_gen "Operator address" "evm" "operator" \
    "$(read_address '.roles.operator_evm')")

  recovery=$(prompt_address_or_gen "Recovery address" "evm" "recovery" \
    "$(read_address '.roles.recovery_evm')")

  # Summary
  print_summary "Deploy Bridge1024" \
    "Chain"    "$chain_name (${chain_id})" \
    "RPC"      "$rpc" \
    "Admin"    "$admin" \
    "Guardian" "$guardian" \
    "Operator" "$operator" \
    "Recovery" "$recovery"

  prompt_confirm "Proceed with deployment?" || return

  # Deploy
  info "Deploying Bridge1024..."
  local sign_flags
  sign_flags=$(evm_sign_flags)
  local output
  # shellcheck disable=SC2086
  output=$(forge create \
    --root "$PROJECT_ROOT/contracts/evm" \
    --rpc-url "$rpc" \
    --broadcast \
    --json \
    $sign_flags \
    "src/Bridge1024.sol:Bridge1024" \
    --constructor-args "$guardian" "$operator" "$recovery" 2>&1)

  # Parse JSON output
  local contract_addr tx_hash
  contract_addr=$(echo "$output" | jq -r '.deployedTo // empty' 2>/dev/null)
  tx_hash=$(echo "$output" | jq -r '.transactionHash // empty' 2>/dev/null)

  if [[ -z "$contract_addr" ]]; then
    error "Deployment failed. Output:"
    echo "$output" >&2
    return 1
  fi

  success "Bridge1024 deployed to: ${contract_addr}"
  info "Tx: ${tx_hash}"

  # Post-deploy verification via getBridgeInfo()
  echo "" >&2
  info "Verifying on-chain roles..."
  local -a bi
  mapfile -t bi < <(evm_read "$rpc" "$contract_addr" \
    "getBridgeInfo()(address,address,address,address,address,address,bytes32,uint64,uint64,bool,bool,uint256)")

  print_verification "Admin"    "$admin"    "$(echo "${bi[0]}" | xargs)"
  print_verification "Guardian" "$guardian" "$(echo "${bi[1]}" | xargs)"
  print_verification "Operator" "$operator" "$(echo "${bi[2]}" | xargs)"
  print_verification "Recovery" "$recovery" "$(echo "${bi[3]}" | xargs)"

  # Save addresses
  write_address ".evm.${chain}.bridge" "$contract_addr"
  write_address ".roles.admin_evm" "$admin"
  write_address ".roles.guardian_evm" "$guardian"
  write_address ".roles.operator_evm" "$operator"
  write_address ".roles.recovery_evm" "$recovery"

  append_log "[evm/deploy] chain=${chain} contract=${contract_addr} admin=${admin} guardian=${guardian} operator=${operator} recovery=${recovery} tx=${tx_hash:-unknown}"
  print_tx_result "$chain" "${tx_hash:-unknown}"

  # Optional: verify contract on block explorer
  if prompt_confirm "Verify contract source on block explorer?"; then
    local api_var="${CHAIN_EXPLORER_API_VAR[$chain]:-}"
    local api_key="${!api_var:-}"
    local verify_url="${CHAIN_VERIFY_URL[$chain]:-}"

    if [[ -z "$api_key" ]]; then
      warn "No API key configured (set ${api_var} in config/.env). Skipping verification."
    else
      info "Verifying contract on ${chain_name}..."
      forge verify-contract \
        --root "$PROJECT_ROOT/contracts/evm" \
        --rpc-url "$rpc" \
        --verifier-url "$verify_url" \
        --etherscan-api-key "$api_key" \
        --constructor-args "$(cast abi-encode 'constructor(address,address,address)' "$guardian" "$operator" "$recovery")" \
        "$contract_addr" \
        "src/Bridge1024.sol:Bridge1024" || warn "Verification failed (can be retried later)"
    fi
  fi
}
