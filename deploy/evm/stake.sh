#!/usr/bin/env bash
# evm/stake.sh — Stake USDC on EVM bridge to trigger a cross-chain transfer
# Sourced by bridge.sh; do not execute directly.

# Look up a chain key from a numeric chain ID.
# Returns the key (e.g. "1024_testnet") or empty string if unknown.
_evm_stake_chain_key_by_id() {
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

# Classify a chain ID as evm or svm — 1024 chains and Solana use 32-byte pubkeys
# as receiver, every other chain we know about is EVM (right-aligned 20B).
_evm_stake_kind_for_chain_id() {
  local cid="$1"
  case "$cid" in
    91024|91025|91026|101|103) echo "svm" ;;
    *) echo "evm" ;;
  esac
}

# Convert an SVM base58 pubkey into a 64-char hex string (no 0x prefix).
# Falls back to empty on failure; caller decides what to do.
_svm_b58_to_hex64() {
  local pk="$1"
  python3 -c "import base58,sys; sys.stdout.write(base58.b58decode('$pk').hex())" 2>/dev/null
}

op_evm_stake() {
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
  echo -e "  ${BOLD}── Bridge Transfer from ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  # Read peer + USDC config from the bridge so we know where this transfer goes
  local usdc_addr peer_id peer_contract paused max_stake
  usdc_addr=$(evm_read "$rpc" "$bridge_addr" "usdcContract()(address)" 2>/dev/null | xargs) || true
  peer_id=$(evm_read "$rpc" "$bridge_addr" "peerChainId()(uint64)" 2>/dev/null | xargs) || true
  peer_contract=$(evm_read "$rpc" "$bridge_addr" "peerContract()(bytes32)" 2>/dev/null | xargs) || true
  paused=$(evm_read "$rpc" "$bridge_addr" "paused()(bool)" 2>/dev/null | xargs) || true
  max_stake=$(evm_read "$rpc" "$bridge_addr" "maxStakeAmount()(uint64)" 2>/dev/null | xargs) || max_stake="0"

  if [[ -z "$usdc_addr" || "$usdc_addr" == "0x0000000000000000000000000000000000000000" ]]; then
    error "USDC not configured on this bridge. Run 'Configure bridge' first."; return
  fi
  if [[ -z "$peer_id" || "$peer_id" == "0" ]]; then
    error "Peer chain not configured on this bridge. Run 'Configure bridge' first."; return
  fi
  if [[ "$paused" == "true" ]]; then
    error "Bridge is paused. Cannot stake."; return
  fi

  local peer_key peer_name peer_kind
  peer_key=$(_evm_stake_chain_key_by_id "$peer_id")
  if [[ -n "$peer_key" ]]; then
    peer_name="${CHAIN_DISPLAY[$peer_key]}"
  else
    peer_name="ID:${peer_id}"
  fi
  peer_kind=$(_evm_stake_kind_for_chain_id "$peer_id")

  info "USDC:     $usdc_addr"
  info "Target:   $peer_name (chain ID: $peer_id, kind: $peer_kind)"

  # The EVM contract has no fee logic — the bridge fee for *this* leg is
  # actually deducted at unlock time on the 1024 hub, using the hub's
  # PeerConfig.bridge_fee for our EVM chain ID. Look it up so we can show
  # the user how much the receiver will really get.
  local hub_fee
  hub_fee=$(hub_fee_for_peer_chain_id "$chain_id")
  info "1024 hub fee for ${chain_name}: ${hub_fee} ($(echo "scale=6; ${hub_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  # Show signer + balances so the user can sanity-check before staking
  local signer
  signer=$(evm_signer_address) || signer=""
  if [[ -z "$signer" ]]; then error "Cannot determine signer address"; return; fi
  info "Signer:   $signer"

  local signer_bal allowance
  signer_bal=$(evm_read "$rpc" "$usdc_addr" "balanceOf(address)(uint256)" "$signer" 2>/dev/null | xargs) || signer_bal="0"
  allowance=$(evm_read "$rpc" "$usdc_addr" "allowance(address,address)(uint256)" "$signer" "$bridge_addr" 2>/dev/null | xargs) || allowance="0"
  info "Your USDC balance: ${signer_bal} ($(echo "scale=6; ${signer_bal:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "Current allowance: ${allowance}"
  if [[ "$max_stake" != "0" ]]; then
    info "Per-tx max stake:  ${max_stake} ($(echo "scale=6; ${max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  fi

  echo "" >&2

  local amount
  amount=$(prompt_input "Stake amount (raw USDC, 6 decimals)" "" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi
  if [[ "$signer_bal" != "0" ]] && (( amount > signer_bal )); then
    error "Amount $amount exceeds your USDC balance $signer_bal"; return
  fi
  if [[ "$max_stake" != "0" ]] && (( amount > max_stake )); then
    error "Amount $amount exceeds maxStakeAmount $max_stake (StakeAmountExceeded)."
    return
  fi
  # Hub will reject if event_amount (= amount - hub_fee) ends up <= 0
  if (( amount <= hub_fee )); then
    error "Amount ${amount} <= 1024 hub fee ${hub_fee}; the unlock would revert with FeeExceedsAmount."
    return
  fi
  local net_amount=$(( amount - hub_fee ))

  # Receiver: format depends on target chain kind. Sensible default for SVM
  # targets is the configured SVM admin keypair (so a self-test "just works").
  local receiver_input receiver_hex
  if [[ "$peer_kind" == "svm" ]]; then
    local default_receiver=""
    if [[ -n "${SVM_KEYPAIR_PATH:-}" && -f "${SVM_KEYPAIR_PATH}" ]]; then
      default_receiver=$(solana-keygen pubkey "$SVM_KEYPAIR_PATH" 2>/dev/null || echo "")
    fi
    receiver_input=$(prompt_input "Receiver on ${peer_name} (SVM base58 pubkey)" "$default_receiver" svm_pubkey) || return 0
    receiver_hex=$(_svm_b58_to_hex64 "$receiver_input")
    if [[ -z "$receiver_hex" || ${#receiver_hex} -ne 64 ]]; then
      error "Failed to decode SVM pubkey to 32 bytes (need python3 + base58 module)"; return
    fi
  else
    receiver_input=$(prompt_input "Receiver on ${peer_name} (EVM address)" "$signer" evm_address) || return 0
    local addr_no_prefix="${receiver_input#0x}"
    receiver_hex=$(printf '%064s' "$addr_no_prefix" | tr ' ' '0')
  fi
  local receiver_bytes32="0x${receiver_hex}"

  # Random non-zero uint64 nonce (top 64 bits from /dev/urandom)
  local nonce
  nonce=$(od -An -N8 -tu8 < /dev/urandom | tr -d ' \n')
  if [[ -z "$nonce" || "$nonce" == "0" ]]; then nonce=1; fi

  print_summary "Bridge Transfer (Stake)" \
    "Bridge"         "$bridge_addr" \
    "Source"         "$chain_name (chain ID: $chain_id)" \
    "Target"         "$peer_name (chain ID: $peer_id)" \
    "USDC"           "$usdc_addr" \
    "Amount (debit)" "${amount} ($(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "1024 hub fee"   "${hub_fee} ($(echo "scale=6; ${hub_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC) — deducted at unlock" \
    "Net to target"  "${net_amount} ($(echo "scale=6; ${net_amount} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Receiver"       "${receiver_input}" \
    "Receiver32"     "${receiver_bytes32}" \
    "Nonce"          "$nonce"

  prompt_confirm "Proceed with stake?" || return

  # Snapshot the receiver's current USDC balance on the target chain so we
  # can later poll for `baseline + net_amount` after the relayer unlocks.
  local target_chain_key="$peer_key"
  local target_usdc baseline_bal
  if [[ -n "$target_chain_key" ]]; then
    target_usdc=$(resolve_usdc_address "$target_chain_key")
    baseline_bal=$(read_usdc_balance "$target_chain_key" "$target_usdc" "$receiver_input")
    info "Target-side baseline (${peer_name}, USDC ${target_usdc:-?}): ${baseline_bal:-?}"
  else
    target_usdc=""
    baseline_bal=""
    warn "Cannot resolve target chain key for ID ${peer_id}; skipping post-stake polling."
  fi

  # Step 1: ensure USDC allowance covers the amount
  if (( allowance < amount )); then
    info "Approving USDC (current allowance ${allowance} < ${amount})..."
    evm_simulate "$rpc" "$usdc_addr" "approve(address,uint256)(bool)" "$bridge_addr" "$amount" || return
    local approve_out approve_tx
    approve_out=$(evm_send "$rpc" "$usdc_addr" "approve(address,uint256)(bool)" "$bridge_addr" "$amount" 2>&1)
    approve_tx=$(evm_extract_tx_hash "$approve_out")
    success "Approve tx: ${approve_tx}"
  else
    info "Allowance is already sufficient; skipping approve."
  fi

  # Step 2: simulate stake from the user's address before sending
  evm_simulate "$rpc" "$bridge_addr" "stake(uint64,uint256,bytes32)" "$nonce" "$amount" "$receiver_bytes32" || return

  # Step 3: send stake
  local stake_out stake_tx
  stake_out=$(evm_send "$rpc" "$bridge_addr" "stake(uint64,uint256,bytes32)" "$nonce" "$amount" "$receiver_bytes32" 2>&1)
  stake_tx=$(evm_extract_tx_hash "$stake_out")

  echo "" >&2
  print_tx_result "$chain" "$stake_tx"

  info "StakeEvent emitted with nonce=${nonce}."
  append_log "[evm/stake] chain=${chain} bridge=${bridge_addr} target=${peer_name}(${peer_id}) amount=${amount} hubFee=${hub_fee} netAmount=${net_amount} receiver=${receiver_input} nonce=${nonce} tx=${stake_tx:-unknown}"

  # Poll target chain until the receiver actually receives the funds
  if [[ -n "$target_chain_key" && -n "$target_usdc" && -n "$baseline_bal" ]]; then
    if prompt_confirm "Wait for the relayer to deliver ~${net_amount} (raw USDC) to ${receiver_input} on ${peer_name}?"; then
      poll_target_balance "$target_chain_key" "$target_usdc" "$receiver_input" \
        "$baseline_bal" "$net_amount" 300 10 || true
    fi
  else
    info "Watch the relayer logs / ${peer_name} explorer to confirm the unlock."
  fi
}
