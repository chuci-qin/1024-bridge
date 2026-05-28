#!/usr/bin/env bash
# svm/stake-gasless.sh — Leaf-only: stake USDC via `stake_gasless` (paymaster pays gas)
# Sourced by bridge.sh; do not execute directly.
#
# Mirrors svm/stake.sh but calls `stake_gasless`, which deducts both
# `bridge_fee` and `gasless_fee` from the user's USDC and leaves them in the
# vault. The Solana fee_payer is a separate paymaster keypair (or the user
# themself for a self-test).

op_svm_stake_gasless() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local target_id="${CHAIN_ID[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  if [[ "$kind" != "leaf" ]]; then
    error "stake-gasless is leaf-only. Hub program has no gasless path."
    return
  fi

  local addr_key
  if [[ "$target" == 1024_* ]]; then
    addr_key=".\"1024\".program_id"
  else
    addr_key=".solana.program_id"
  fi
  local program_id
  program_id=$(read_address "$addr_key")
  if [[ -z "$program_id" ]]; then error "Program not deployed on $target_name."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Bridge Transfer (gasless) from ${target_name} ──${NC}" >&2
  echo "" >&2

  info "Program: $program_id"
  info "Kind:    leaf"
  info "Source:  $target_name (chain ID: $target_id)"
  info "RPC:     $rpc"

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM user keypair path (USDC authority)") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then error "Keypair file not found: $keypair_path"; return; fi

  # Optional paymaster (fee-payer) keypair — defaults to the user keypair for
  # self-testing on devnet. In production this is a paymaster service.
  local paymaster_path=""
  if prompt_confirm "Use a separate paymaster keypair for the fee-payer?"; then
    paymaster_path=$(prompt_input "Paymaster (fee-payer) SVM keypair path") || return 0
    if [[ ! -f "$paymaster_path" ]]; then error "Keypair file not found: $paymaster_path"; return; fi
  fi

  local svm_deploy_dir="$DEPLOY_DIR/svm"

  # Pull state — leaf returns its single peer inline under .peers[0] + bridgeFee / gaslessFee
  local on_chain_json
  on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind leaf 2>/dev/null) || on_chain_json=""
  on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
  if [[ -z "$on_chain_json" ]]; then error "Failed to read program state"; return; fi

  local peer_chain_id usdc_mint bridge_fee gasless_fee peer_max_stake
  peer_chain_id=$(echo "$on_chain_json" | jq -r '.peers[0].chainId // ""')
  usdc_mint=$(echo "$on_chain_json"     | jq -r '.usdcMint')
  bridge_fee=$(echo "$on_chain_json"    | jq -r '.bridgeFee // "0"')
  gasless_fee=$(echo "$on_chain_json"   | jq -r '.gaslessFee // "0"')
  peer_max_stake=$(echo "$on_chain_json" | jq -r '.peers[0].maxStakeAmount // "0"')

  if [[ -z "$peer_chain_id" ]]; then
    error "Leaf has no peer configured. Run 'Configure' first."; return
  fi
  if [[ "$gasless_fee" == "0" ]]; then
    error "gasless_fee == 0 → gasless path is disabled (GaslessDisabled). Run 'Configure gasless fee' first."
    return
  fi

  local peer_kind peer_name pk
  pk=""
  local c
  for c in "${!CHAIN_ID[@]}"; do
    if [[ "${CHAIN_ID[$c]}" == "$peer_chain_id" ]]; then pk="$c"; break; fi
  done
  peer_name="${CHAIN_DISPLAY[$pk]:-ID:$peer_chain_id}"
  case "$peer_chain_id" in
    91024|91025|91026|101|103) peer_kind="svm" ;;
    *) peer_kind="evm" ;;
  esac

  info "Target:     $peer_name (chain ID: $peer_chain_id, kind: $peer_kind)"
  info "USDC mint:  $usdc_mint"
  info "Bridge fee: ${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC) — deducted at stake"
  info "Gasless fee:${gasless_fee} ($(echo "scale=6; ${gasless_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC) — extra deducted on gasless path"
  local total_fee=$(( bridge_fee + gasless_fee ))
  info "Total fees: ${total_fee} ($(echo "scale=6; ${total_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  if [[ "$peer_max_stake" != "0" ]]; then
    info "Max stake:  ${peer_max_stake} ($(echo "scale=6; ${peer_max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  local user_pk user_bal
  user_pk=$(solana-keygen pubkey "$keypair_path" 2>/dev/null || echo "")
  if [[ -n "$user_pk" && -n "$usdc_mint" && "$usdc_mint" != "11111111111111111111111111111111" ]]; then
    info "User:       $user_pk"
    user_bal=$(npx ts-node "$svm_deploy_dir/src/instructions/get-token-balance.ts" \
      --rpc-url "$rpc" \
      --mint "$usdc_mint" \
      --owner "$user_pk" 2>/dev/null) || user_bal="0"
    [[ -z "$user_bal" ]] && user_bal="0"
    info "Your USDC:  ${user_bal} ($(echo "scale=6; ${user_bal} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  echo "" >&2

  local amount
  amount=$(prompt_input "Stake amount (raw USDC, 6 decimals)" "" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi
  if [[ "$user_bal" =~ ^[0-9]+$ ]] && (( user_bal > 0 )) && (( amount > user_bal )); then
    error "Amount ${amount} exceeds your USDC balance ${user_bal}."; return
  fi
  if (( total_fee > 0 )) && (( amount <= total_fee )); then
    error "Amount ${amount} <= total fee ${total_fee}; this tx would revert FeeExceedsAmount."
    return
  fi
  if [[ "$peer_max_stake" != "0" ]] && (( amount > peer_max_stake )); then
    error "Amount ${amount} exceeds max stake ${peer_max_stake} (StakeAmountExceeded)."
    return
  fi
  local net_amount=$(( amount - total_fee ))

  # Receiver: format depends on target kind
  local receiver_input receiver_hex=""
  if [[ "$peer_kind" == "svm" ]]; then
    receiver_input=$(prompt_input "Receiver on ${peer_name} (SVM base58 pubkey)" "${user_pk:-}" svm_pubkey) || return 0
  else
    local default_evm_recv=""
    default_evm_recv=$(evm_signer_address 2>/dev/null || echo "")
    local receiver_evm addr_no_prefix
    receiver_evm=$(prompt_input "Receiver on ${peer_name} (EVM address)" "$default_evm_recv" evm_address) || return 0
    addr_no_prefix="${receiver_evm#0x}"
    receiver_hex=$(printf '%064s' "$addr_no_prefix" | tr ' ' '0')
    receiver_input="0x${receiver_hex}"
    info "Receiver bytes32: ${receiver_input}"
  fi

  print_summary "Bridge Transfer (Gasless Stake)" \
    "Program"          "$program_id" \
    "Kind"             "leaf" \
    "Source"           "$target_name (chain ID: $target_id)" \
    "Target"           "$peer_name (chain ID: $peer_chain_id)" \
    "USDC mint"        "$usdc_mint" \
    "Amount (debit)"   "${amount} ($(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Bridge fee"       "${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Gasless fee"      "${gasless_fee} ($(echo "scale=6; ${gasless_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Net to target"    "${net_amount} ($(echo "scale=6; ${net_amount} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Receiver"         "$receiver_input" \
    "Fee payer"        "${paymaster_path:-<same as user>}"

  prompt_confirm "Proceed with gasless stake?" || return

  # Snapshot target-side baseline so we can poll for delivery after stake.
  # Mirrors svm/stake.sh — _svm_stake_chain_key_by_id is defined there and
  # available because stake.sh is sourced before stake-gasless.sh in bridge.sh.
  local target_chain_key="" target_usdc="" baseline_bal="" receiver_for_target=""
  target_chain_key=$(_svm_stake_chain_key_by_id "$peer_chain_id")
  if [[ -n "$target_chain_key" ]]; then
    target_usdc=$(resolve_usdc_address "$target_chain_key")
    if [[ "$peer_kind" == "svm" ]]; then
      receiver_for_target="$receiver_input"
    else
      # EVM target: read_usdc_balance expects a 0x... address (20 bytes), not
      # the 32-byte bytes32 form we pass to the contract.
      receiver_for_target="0x${receiver_hex:24:40}"
    fi
    baseline_bal=$(read_usdc_balance "$target_chain_key" "$target_usdc" "$receiver_for_target")
    info "Target-side baseline (${peer_name}, USDC ${target_usdc:-?}): ${baseline_bal:-?}"
  else
    warn "Cannot resolve target chain key for ID ${peer_chain_id}; skipping post-stake polling."
  fi

  info "Running stake_gasless instruction..."

  local stake_rc
  if [[ -n "$paymaster_path" ]]; then
    npx ts-node "$svm_deploy_dir/src/instructions/stake-gasless.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --program-kind leaf \
      --amount "$amount" \
      --receiver "$receiver_input" \
      --fee-payer-keypair "$paymaster_path"
    stake_rc=$?
  else
    npx ts-node "$svm_deploy_dir/src/instructions/stake-gasless.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --program-kind leaf \
      --amount "$amount" \
      --receiver "$receiver_input"
    stake_rc=$?
  fi

  if (( stake_rc == 0 )); then
    append_log "[svm/stakeGasless] target=${target} program=${program_id} peerChainId=${peer_chain_id} amount=${amount} bridgeFee=${bridge_fee} gaslessFee=${gasless_fee} netAmount=${net_amount} receiver=${receiver_input} paymaster=${paymaster_path:-self}"
    success "Gasless stake submitted. Receiver should see ~${net_amount} (raw USDC) on ${peer_name} once the relayer finishes."

    if [[ -n "$target_chain_key" && -n "$target_usdc" && -n "$baseline_bal" ]]; then
      if prompt_confirm "Wait for the relayer to deliver ~${net_amount} (raw USDC) to ${receiver_for_target} on ${peer_name}?"; then
        poll_target_balance "$target_chain_key" "$target_usdc" "$receiver_for_target" \
          "$baseline_bal" "$net_amount" 300 10 || true
      fi
    fi
  else
    error "Gasless stake failed (see logs above)."
  fi
}
