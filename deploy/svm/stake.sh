#!/usr/bin/env bash
# svm/stake.sh — Stake USDC on the SVM bridge to trigger a cross-chain transfer
# Sourced by bridge.sh; do not execute directly.

# Look up a chain key by numeric chain ID. Returns the key (e.g. "1024_testnet")
# or empty if unknown — caller can fall back to "ID:<n>" for display.
_svm_stake_chain_key_by_id() {
  local target_id="$1"
  local c
  for c in "${!CHAIN_ID[@]}"; do
    if [[ "${CHAIN_ID[$c]}" == "$target_id" ]]; then
      echo "$c"
      return 0
    fi
  done
  echo ""
}

# Classify a chain ID as evm or svm — matters for receiver encoding.
_svm_stake_kind_for_chain_id() {
  local cid="$1"
  case "$cid" in
    91024|91025|91026|101|103) echo "svm" ;;
    *) echo "evm" ;;
  esac
}

op_svm_stake() {
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
  if [[ -z "$program_id" ]]; then error "Program not deployed on $target_name."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Bridge Transfer from ${target_name} ──${NC}" >&2
  echo "" >&2

  info "Program: $program_id"
  info "Source:  $target_name (chain ID: $target_id)"
  info "RPC:     $rpc"

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM signer keypair path") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then error "Keypair file not found: $keypair_path"; return; fi

  # Build the candidate peer chain ID list (everything we might have registered)
  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local peer_keys=() peer_chain_ids=()
  if [[ "$target" == 1024_* ]]; then
    # 1024 hub can talk to every EVM chain in the env + the satellite Solana chain
    local c
    for c in $(get_evm_chains "$CURRENT_ENV"); do
      peer_keys+=("$c"); peer_chain_ids+=("${CHAIN_ID[$c]}")
    done
    for c in $(get_svm_targets "$CURRENT_ENV"); do
      [[ "$c" == "$target" ]] && continue
      peer_keys+=("$c"); peer_chain_ids+=("${CHAIN_ID[$c]}")
    done
  else
    # Solana satellites only peer with the 1024 hub
    local c1024_key
    c1024_key=$(get_1024_chain_key "$CURRENT_ENV")
    peer_keys+=("$c1024_key"); peer_chain_ids+=("${CHAIN_ID[$c1024_key]}")
  fi

  # Ask read-state for the on-chain peers among that candidate set, so we only
  # show registered peers in the menu (and pull each peer's chain_id as auth).
  local cid_csv on_chain_json registered_csv=""
  if ((${#peer_chain_ids[@]} > 0)); then
    cid_csv=$(IFS=,; echo "${peer_chain_ids[*]}")
    on_chain_json=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
      --rpc-url "$rpc" \
      --keypair "$keypair_path" \
      --program-id "$program_id" \
      --peer-chain-ids "$cid_csv" 2>/dev/null) || on_chain_json=""
    on_chain_json=$(echo "$on_chain_json" | grep -E '^\{' | tail -n 1)
    if [[ -n "$on_chain_json" ]]; then
      registered_csv=$(echo "$on_chain_json" | jq -r '.peers[]?.chainId' 2>/dev/null | tr '\n' ' ')
    fi
  fi

  if [[ -z "$registered_csv" ]]; then
    error "No peer chains registered on this bridge yet. Run 'Register peer' first."
    return
  fi

  # Build the menu from the intersection of (candidate peers) ∩ (on-chain peers)
  local opt_keys=() opt_ids=() opt_labels=()
  local i k cid
  for i in "${!peer_keys[@]}"; do
    k="${peer_keys[$i]}"
    cid="${peer_chain_ids[$i]}"
    if [[ " $registered_csv " == *" $cid "* ]]; then
      opt_keys+=("$k")
      opt_ids+=("$cid")
      opt_labels+=("${CHAIN_DISPLAY[$k]} (chain ID: ${cid})")
    fi
  done

  if ((${#opt_labels[@]} == 0)); then
    error "Registered peers don't match any known chain in this env. Cannot bridge."
    return
  fi

  local idx
  idx=$(prompt_select "Select target chain:" "${opt_labels[@]}" "Manual chain ID")

  local peer_chain_id peer_kind peer_name
  if [[ "$idx" -lt "${#opt_keys[@]}" ]]; then
    peer_chain_id="${opt_ids[$idx]}"
    peer_name="${CHAIN_DISPLAY[${opt_keys[$idx]}]}"
    peer_kind=$(_svm_stake_kind_for_chain_id "$peer_chain_id")
  else
    peer_chain_id=$(prompt_input "Target chain ID" "" uint) || return 0
    if [[ " $registered_csv " != *" $peer_chain_id "* ]]; then
      error "Chain ID $peer_chain_id is not a registered peer on this bridge."
      return
    fi
    local pk
    pk=$(_svm_stake_chain_key_by_id "$peer_chain_id")
    peer_name="${CHAIN_DISPLAY[$pk]:-ID:$peer_chain_id}"
    peer_kind=$(_svm_stake_kind_for_chain_id "$peer_chain_id")
  fi

  info "Target:  $peer_name (chain ID: $peer_chain_id, kind: $peer_kind)"

  # Pull the chosen peer's source-side bridge_fee + max_stake from the JSON we
  # already fetched above. The contract deducts pc.bridge_fee at *stake* time
  # on whichever side stakes (hub: real fee; satellite: forced 0 by policy).
  local source_fee peer_max_stake
  source_fee=$(echo "$on_chain_json" \
    | jq -r --arg cid "$peer_chain_id" '.peers[] | select(.chainId == $cid) | .bridgeFee' 2>/dev/null)
  peer_max_stake=$(echo "$on_chain_json" \
    | jq -r --arg cid "$peer_chain_id" '.peers[] | select(.chainId == $cid) | .maxStakeAmount' 2>/dev/null)
  source_fee="${source_fee:-0}"
  peer_max_stake="${peer_max_stake:-0}"

  # Target-side fee: also deducted at *unlock*. EVM has no fee field, Solana
  # is policy-locked to 0, so a non-zero target_fee only happens when the
  # target is the 1024 hub (i.e. we're staking from a satellite into the hub).
  local target_fee="0"
  if [[ "$target" != 1024_* ]]; then
    target_fee=$(hub_fee_for_peer_chain_id "$target_id")
    target_fee="${target_fee:-0}"
  fi
  local total_fee=$(( source_fee + target_fee ))

  info "Source-side fee: ${source_fee} ($(echo "scale=6; ${source_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "Target-side fee: ${target_fee} ($(echo "scale=6; ${target_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC) (deducted at unlock on ${peer_name})"
  if [[ "$peer_max_stake" != "0" ]]; then
    info "Per-peer max stake: ${peer_max_stake} ($(echo "scale=6; ${peer_max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  # Show signer's USDC balance — use the same get-token-balance.ts helper that
  # stake polling uses, so the number we show here matches what gets debited.
  # It returns the raw u64 amount on stdout, or "0" if the ATA doesn't exist.
  local user_pk usdc_mint user_bal=""
  user_pk=$(solana-keygen pubkey "$keypair_path" 2>/dev/null || echo "")
  usdc_mint=$(echo "$on_chain_json" | jq -r '.usdcMint')
  if [[ -n "$user_pk" && -n "$usdc_mint" && "$usdc_mint" != "11111111111111111111111111111111" ]]; then
    info "Signer:  $user_pk"
    info "USDC:    $usdc_mint"
    user_bal=$(npx ts-node "$svm_deploy_dir/src/instructions/get-token-balance.ts" \
      --rpc-url "$rpc" \
      --mint "$usdc_mint" \
      --owner "$user_pk" 2>/dev/null) || user_bal=""
    [[ -z "$user_bal" ]] && user_bal="0"
    info "Your USDC balance: ${user_bal} ($(echo "scale=6; ${user_bal} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  echo "" >&2

  local amount
  amount=$(prompt_input "Stake amount (raw USDC, 6 decimals)" "" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi
  # Local pre-flight: don't waste a tx if the signer can't even cover `amount`
  if [[ "$user_bal" =~ ^[0-9]+$ ]] && (( user_bal > 0 )) && (( amount > user_bal )); then
    error "Amount ${amount} exceeds your USDC balance ${user_bal}."
    return
  fi

  # Contract requires `amount > bridge_fee` on every leg that charges fee:
  # source-side at stake (FeeExceedsAmount on this tx) and target-side at
  # unlock (FeeExceedsAmount on relayer tx). Bail out fast for both.
  if (( amount <= source_fee )); then
    error "Amount ${amount} <= source fee ${source_fee}; this tx would revert with FeeExceedsAmount."
    return
  fi
  if (( target_fee > 0 )) && (( (amount - source_fee) <= target_fee )); then
    error "Amount after source fee (${amount}-${source_fee}) <= target fee ${target_fee}; the unlock would revert."
    return
  fi
  if [[ "$peer_max_stake" != "0" ]] && (( amount > peer_max_stake )); then
    error "Amount ${amount} exceeds per-peer max stake ${peer_max_stake} (StakeAmountExceeded)."
    return
  fi
  local net_amount=$(( amount - source_fee - target_fee ))

  # Receiver: format depends on target kind. For SVM targets the sensible
  # default is the signer itself (admin keypair), so a self-test "just works".
  local receiver_input
  if [[ "$peer_kind" == "svm" ]]; then
    receiver_input=$(prompt_input "Receiver on ${peer_name} (SVM base58 pubkey)" "${user_pk:-}" svm_pubkey) || return 0
  else
    # Default to the EVM signer from .env (mirrors EVM→SVM behavior, where the
    # SVM signer is the default receiver). Falls back to no default if no EVM
    # signer is configured in this env.
    local default_evm_recv=""
    default_evm_recv=$(evm_signer_address 2>/dev/null || echo "")
    local receiver_evm addr_no_prefix receiver_hex
    receiver_evm=$(prompt_input "Receiver on ${peer_name} (EVM address)" "$default_evm_recv" evm_address) || return 0
    addr_no_prefix="${receiver_evm#0x}"
    receiver_hex=$(printf '%064s' "$addr_no_prefix" | tr ' ' '0')
    receiver_input="0x${receiver_hex}"
    info "Receiver bytes32: ${receiver_input}"
  fi

  print_summary "Bridge Transfer (Stake)" \
    "Program"          "$program_id" \
    "Source"           "$target_name (chain ID: $target_id)" \
    "Target"           "$peer_name (chain ID: $peer_chain_id)" \
    "USDC mint"        "${usdc_mint:-?}" \
    "Amount (debit)"   "${amount} ($(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Source-side fee"  "${source_fee} ($(echo "scale=6; ${source_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Target-side fee"  "${target_fee} ($(echo "scale=6; ${target_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Net to target"    "${net_amount} ($(echo "scale=6; ${net_amount} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Receiver"         "$receiver_input"

  prompt_confirm "Proceed with stake?" || return

  # Snapshot target-side baseline so we can poll for delivery after stake
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

  info "Running stake instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/stake.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --target-chain-id "$peer_chain_id" \
    --amount "$amount" \
    --receiver "$receiver_input"

  local stake_rc=$?
  if (( stake_rc == 0 )); then
    append_log "[svm/stake] target=${target} program=${program_id} peerChainId=${peer_chain_id} amount=${amount} sourceFee=${source_fee} targetFee=${target_fee} netAmount=${net_amount} receiver=${receiver_input}"
    success "Stake submitted. Receiver should see ~${net_amount} (raw USDC) on ${peer_name} once the relayer finishes."

    if [[ -n "$target_chain_key" && -n "$target_usdc" && -n "$baseline_bal" ]]; then
      if prompt_confirm "Wait for the relayer to deliver ~${net_amount} (raw USDC) to ${receiver_for_target} on ${peer_name}?"; then
        poll_target_balance "$target_chain_key" "$target_usdc" "$receiver_for_target" \
          "$baseline_bal" "$net_amount" 300 10 || true
      fi
    fi
  else
    error "Stake failed (see logs above)."
  fi
}
