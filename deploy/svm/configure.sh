#!/usr/bin/env bash
# svm/configure.sh — Configure bridge1024 program
# Sourced by bridge.sh; do not execute directly.

op_svm_configure() {
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
  echo -e "  ${BOLD}── Configure bridge1024 on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  # USDC mint：优先取 addresses.json 的历史值，否则回退到 config/.env 里的内建默认
  local usdc_key
  if [[ "$target" == 1024_* ]]; then
    usdc_key=".\"1024\".usdc_mint"
  else
    usdc_key=".solana.usdc_mint"
  fi
  local default_usdc
  default_usdc=$(read_address "$usdc_key")
  [[ -z "$default_usdc" ]] && default_usdc=$(get_usdc_address "$target" 2>/dev/null || echo "")
  local usdc_mint
  usdc_mint=$(prompt_input "USDC mint address" "$default_usdc" svm_pubkey) || return 0

  # Local chain ID
  local local_chain_id="${CHAIN_ID[$target]}"
  local_chain_id=$(prompt_input "Local chain ID" "$local_chain_id" uint)

  print_summary "Configure bridge1024" \
    "Target"        "$target_name" \
    "Program"       "$program_id" \
    "USDC mint"     "$usdc_mint" \
    "Local chain ID" "$local_chain_id"

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running configure instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --usdc-mint "$usdc_mint" \
    --local-chain-id "$local_chain_id"

  if [[ $? -eq 0 ]]; then
    write_address "$usdc_key" "$usdc_mint"
    append_log "[svm/configure] target=${target} program=${program_id} usdcMint=${usdc_mint} localChainId=${local_chain_id}"
    success "Configuration complete"
  fi
}
