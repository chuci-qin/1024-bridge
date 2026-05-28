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

# Compute the auth-binding hash that the bridge's stakeWithAuthorization()
# requires inside auth.authNonce — see Bridge1024.sol lines 1053-1057.
#
# keccak256(abi.encode(
#   "Bridge1024.stakeWithAuth.v1",
#   uint64 localChainId, address bridge,
#   uint64 nonce, uint256 amount, bytes32 receiver, address from
# ))
_evm_compute_auth_nonce() {
  local local_chain_id="$1" bridge="$2" nonce="$3" amount="$4" receiver_b32="$5" from="$6"
  local data
  data=$(cast abi-encode \
    "f(string,uint64,address,uint64,uint256,bytes32,address)" \
    "Bridge1024.stakeWithAuth.v1" "$local_chain_id" "$bridge" \
    "$nonce" "$amount" "$receiver_b32" "$from")
  cast keccak "$data"
}

# Build the EIP-712 digest for USDC's ReceiveWithAuthorization typed data:
#   typeHash =
#     keccak256("ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)")
#   structHash = keccak256(abi.encode(typeHash, from, to, value, validAfter, validBefore, nonce))
#   digest     = keccak256(abi.encodePacked(0x1901, domainSeparator, structHash))
#
# DomainSeparator is read off the USDC contract directly — works regardless of
# chain-specific name/version (native USDC, bridged USDC.e, etc).
_evm_build_eip3009_digest() {
  local rpc="$1" usdc="$2" from="$3" to="$4" value="$5"
  local valid_after="$6" valid_before="$7" auth_nonce="$8"

  local domain_sep
  domain_sep=$(evm_read "$rpc" "$usdc" "DOMAIN_SEPARATOR()(bytes32)" | xargs)
  if [[ -z "$domain_sep" || "$domain_sep" == "0x0000000000000000000000000000000000000000000000000000000000000000" ]]; then
    error "USDC at ${usdc} doesn't expose DOMAIN_SEPARATOR(); cannot build EIP-712 digest"
    return 1
  fi

  local type_hash
  type_hash=$(cast keccak "ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)")

  local struct_data struct_hash
  struct_data=$(cast abi-encode \
    "f(bytes32,address,address,uint256,uint256,uint256,bytes32)" \
    "$type_hash" "$from" "$to" "$value" "$valid_after" "$valid_before" "$auth_nonce")
  struct_hash=$(cast keccak "$struct_data")

  # 0x1901 || DOMAIN_SEPARATOR (32B) || structHash (32B) — keccak256 of concat
  local concat="0x1901${domain_sep#0x}${struct_hash#0x}"
  cast keccak "$concat"
}

# Sign a 32-byte digest with the user-provided EIP-712 signing key.
# Outputs "r|s|v" on stdout (each value 0x-prefixed).
_evm_sign_digest_with_key() {
  local digest="$1" priv_key="$2"
  local sig
  sig=$(cast wallet sign --no-hash --private-key "$priv_key" "$digest" 2>/dev/null)
  if [[ -z "$sig" || ! "$sig" =~ ^0x[0-9a-fA-F]{130}$ ]]; then
    error "cast wallet sign failed or returned an unexpected signature shape"
    return 1
  fi
  # 65-byte sig (r 32B || s 32B || v 1B) — split it cleanly.
  local raw="${sig#0x}"
  local r="0x${raw:0:64}"
  local s="0x${raw:64:64}"
  local v_hex="0x${raw:128:2}"
  local v=$((v_hex))
  echo "${r}|${s}|${v}"
}

op_evm_stake() {
  local chain="$1"

  echo "" >&2
  echo -e "  ${BOLD}── Select Stake Path ──${NC}" >&2

  local idx
  idx=$(prompt_select "Bridge Transfer mode:" \
    "Plain stake (user signs USDC.approve + Bridge.stake)" \
    "Gasless: stakeWithAuthorization (EIP-3009; paymaster pays gas)" \
    "← Back")

  case "$idx" in
    0) _op_evm_stake_plain "$chain" ;;
    1) _op_evm_stake_with_authorization "$chain" ;;
    *) return 0 ;;
  esac
}

_op_evm_stake_plain() {
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
  echo -e "  ${BOLD}── Bridge Transfer (plain stake) from ${chain_name} ──${NC}" >&2
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

  # EVM contract deducts bridgeFee at stake time; read it directly.
  local bridge_fee
  bridge_fee=$(evm_read "$rpc" "$bridge_addr" "bridgeFee()(uint64)" 2>/dev/null | xargs) || bridge_fee="0"
  info "Bridge fee: ${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

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
  # Contract requires amount > bridgeFee (FeeExceedsAmount on stake)
  if (( bridge_fee > 0 && amount <= bridge_fee )); then
    error "Amount ${amount} <= bridge fee ${bridge_fee}; stake would revert with FeeExceedsAmount."
    return
  fi
  local net_amount=$(( amount - bridge_fee ))

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
    "Bridge fee"     "${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC) — deducted at stake" \
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
  append_log "[evm/stake] chain=${chain} bridge=${bridge_addr} target=${peer_name}(${peer_id}) amount=${amount} bridgeFee=${bridge_fee} netAmount=${net_amount} receiver=${receiver_input} nonce=${nonce} tx=${stake_tx:-unknown}"

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

# ── Gasless: stakeWithAuthorization (EIP-3009) ────────────────────────────────
#
# Flow (mirrors Bridge1024.stakeWithAuthorization in the contract):
#   1. User picks amount + receiver, supplies a *staker* private key separate
#      from the env's paymaster signer (or chooses to use the paymaster as
#      the staker for a self-test).
#   2. We build the auth-binding hash exactly as the contract does:
#        keccak256(abi.encode(
#          "Bridge1024.stakeWithAuth.v1",
#          uint64 localChainId, address bridge,
#          uint64 nonce, uint256 amount, bytes32 receiver, address from
#        ))
#   3. We compute the EIP-712 ReceiveWithAuthorization digest over USDC's
#      DOMAIN_SEPARATOR with that hash inserted as the `nonce` field, and the
#      staker key signs it.
#   4. The env paymaster signer submits `stakeWithAuthorization(...)` — pays
#      gas, holds no USDC.
_op_evm_stake_with_authorization() {
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
  echo -e "  ${BOLD}── Bridge Transfer (gasless / EIP-3009) from ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  # Read bridge state — abort early on common misconfigurations so the user
  # doesn't burn a tx that would revert.
  local usdc_addr peer_id peer_contract paused local_id bridge_fee gasless_fee max_stake
  usdc_addr=$(evm_read "$rpc" "$bridge_addr" "usdcContract()(address)" 2>/dev/null | xargs) || true
  peer_id=$(evm_read "$rpc" "$bridge_addr" "peerChainId()(uint64)" 2>/dev/null | xargs) || true
  peer_contract=$(evm_read "$rpc" "$bridge_addr" "peerContract()(bytes32)" 2>/dev/null | xargs) || true
  paused=$(evm_read "$rpc" "$bridge_addr" "paused()(bool)" 2>/dev/null | xargs) || true
  local_id=$(evm_read "$rpc" "$bridge_addr" "localChainId()(uint64)" 2>/dev/null | xargs) || true
  bridge_fee=$(evm_read "$rpc" "$bridge_addr" "bridgeFee()(uint64)" 2>/dev/null | xargs) || bridge_fee="0"
  gasless_fee=$(evm_read "$rpc" "$bridge_addr" "gaslessFee()(uint64)" 2>/dev/null | xargs) || gasless_fee="0"
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
  if [[ "$gasless_fee" == "0" ]]; then
    error "gaslessFee == 0 → gasless path is disabled (GaslessDisabled). Run 'Configure gasless fee' first."
    return
  fi

  local peer_key peer_name peer_kind
  peer_key=$(_evm_stake_chain_key_by_id "$peer_id")
  peer_name="${CHAIN_DISPLAY[$peer_key]:-ID:$peer_id}"
  peer_kind=$(_evm_stake_kind_for_chain_id "$peer_id")

  info "USDC:        $usdc_addr"
  info "Target:      $peer_name (chain ID: $peer_id, kind: $peer_kind)"
  info "Bridge fee:  ${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"
  info "Gasless fee: ${gasless_fee} ($(echo "scale=6; ${gasless_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  # Paymaster (env signer) submits the tx; staker signs the EIP-3009 auth.
  local paymaster
  paymaster=$(evm_signer_address) || paymaster=""
  if [[ -z "$paymaster" ]]; then error "Cannot determine paymaster (env signer) address"; return; fi
  info "Paymaster:   $paymaster (pays gas)"

  # Staker key — separate by default; allow same-as-paymaster for self-tests
  local staker_key=""
  local default_pk="${BRIDGE_STAKER_PRIVATE_KEY:-}"
  if [[ -z "$default_pk" ]]; then
    # Fall back to a private-key file (testnet-friendly)
    if [[ -n "${BRIDGE_STAKER_PRIVATE_KEY_PATH:-}" && -f "${BRIDGE_STAKER_PRIVATE_KEY_PATH}" ]]; then
      default_pk=$(jq -r '.private_key // empty' "${BRIDGE_STAKER_PRIVATE_KEY_PATH}" 2>/dev/null)
      [[ -z "$default_pk" ]] && default_pk=$(tr -d '[:space:]' < "${BRIDGE_STAKER_PRIVATE_KEY_PATH}")
    fi
  fi

  if [[ -n "$default_pk" ]]; then
    if prompt_confirm "Use BRIDGE_STAKER_PRIVATE_KEY from env / config for the staker?"; then
      staker_key="$default_pk"
    fi
  fi
  if [[ -z "$staker_key" ]]; then
    if prompt_confirm "Use paymaster's signer key as staker too (self-test only)?"; then
      staker_key=$(_resolve_evm_private_key)
      if [[ -z "$staker_key" ]]; then
        error "Paymaster is using a keystore/Ledger; cannot reuse as staker. Set BRIDGE_STAKER_PRIVATE_KEY in env."
        return
      fi
    else
      staker_key=$(prompt_input "Staker raw private key (0x-prefixed, signs EIP-3009)" "" )
      [[ -z "$staker_key" ]] && return 0
      # Validate format
      if [[ ! "$staker_key" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
        error "Invalid private key format (need 0x + 64 hex chars)."
        return
      fi
    fi
  fi

  local staker_addr
  staker_addr=$(cast wallet address --private-key "$staker_key" 2>/dev/null)
  if [[ -z "$staker_addr" ]]; then error "Failed to derive address from staker key."; return; fi
  info "Staker:      $staker_addr (signs EIP-3009 / holds USDC)"

  local staker_bal
  staker_bal=$(evm_read "$rpc" "$usdc_addr" "balanceOf(address)(uint256)" "$staker_addr" 2>/dev/null | xargs) || staker_bal="0"
  info "Staker USDC: ${staker_bal} ($(echo "scale=6; ${staker_bal:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  echo "" >&2

  local amount
  amount=$(prompt_input "Stake amount (raw USDC, 6 decimals)" "" uint) || return 0
  if [[ "$amount" == "0" ]]; then error "Amount must be > 0"; return; fi
  if [[ "$staker_bal" != "0" ]] && (( amount > staker_bal )); then
    error "Amount $amount exceeds staker's USDC balance $staker_bal"; return
  fi
  if [[ "$max_stake" != "0" ]] && (( amount > max_stake )); then
    error "Amount $amount exceeds maxStakeAmount $max_stake (StakeAmountExceeded)."; return
  fi
  local total_fee=$(( bridge_fee + gasless_fee ))
  if (( total_fee > 0 )) && (( amount <= total_fee )); then
    error "Amount ${amount} <= total fee ${total_fee}; stake would revert FeeExceedsAmount."
    return
  fi
  local net_amount=$(( amount - total_fee ))

  # Receiver formatting (same as plain stake)
  local receiver_input receiver_hex
  if [[ "$peer_kind" == "svm" ]]; then
    local default_receiver=""
    if [[ -n "${SVM_KEYPAIR_PATH:-}" && -f "${SVM_KEYPAIR_PATH}" ]]; then
      default_receiver=$(solana-keygen pubkey "$SVM_KEYPAIR_PATH" 2>/dev/null || echo "")
    fi
    receiver_input=$(prompt_input "Receiver on ${peer_name} (SVM base58 pubkey)" "$default_receiver" svm_pubkey) || return 0
    receiver_hex=$(_svm_b58_to_hex64 "$receiver_input")
    if [[ -z "$receiver_hex" || ${#receiver_hex} -ne 64 ]]; then
      error "Failed to decode SVM pubkey (need python3 + base58 module)"; return
    fi
  else
    receiver_input=$(prompt_input "Receiver on ${peer_name} (EVM address)" "$staker_addr" evm_address) || return 0
    local addr_no_prefix="${receiver_input#0x}"
    receiver_hex=$(printf '%064s' "$addr_no_prefix" | tr ' ' '0')
  fi
  local receiver_bytes32="0x${receiver_hex}"

  # Bridge nonce (uint64) — random non-zero
  local nonce
  nonce=$(od -An -N8 -tu8 < /dev/urandom | tr -d ' \n')
  if [[ -z "$nonce" || "$nonce" == "0" ]]; then nonce=1; fi

  # EIP-3009 validity window — 1h validBefore by default
  local now valid_after valid_before
  now=$(date +%s)
  valid_after=0
  valid_before=$(( now + 3600 ))

  # Step A: compute auth-binding hash (used as EIP-3009 authNonce)
  local auth_nonce
  auth_nonce=$(_evm_compute_auth_nonce "$local_id" "$bridge_addr" "$nonce" "$amount" "$receiver_bytes32" "$staker_addr")
  if [[ -z "$auth_nonce" ]]; then
    error "Failed to compute auth-binding hash"; return
  fi

  # Step B: compute the EIP-712 digest over USDC's ReceiveWithAuthorization
  local digest
  digest=$(_evm_build_eip3009_digest "$rpc" "$usdc_addr" "$staker_addr" "$bridge_addr" "$amount" "$valid_after" "$valid_before" "$auth_nonce") || return

  # Step C: staker signs the digest (cast --no-hash so we sign the digest as-is)
  local sig
  sig=$(_evm_sign_digest_with_key "$digest" "$staker_key") || return
  local r="${sig%%|*}"
  local rest="${sig#*|}"
  local s="${rest%%|*}"
  local v="${rest#*|}"

  print_summary "Bridge Transfer (Gasless Stake)" \
    "Bridge"         "$bridge_addr" \
    "Source"         "$chain_name (chain ID: $chain_id)" \
    "Target"         "$peer_name (chain ID: $peer_id)" \
    "USDC"           "$usdc_addr" \
    "Paymaster"      "$paymaster" \
    "Staker (from)"  "$staker_addr" \
    "Amount (debit)" "${amount} ($(echo "scale=6; $amount / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Bridge fee"     "${bridge_fee} ($(echo "scale=6; ${bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Gasless fee"    "${gasless_fee} ($(echo "scale=6; ${gasless_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Net to target"  "${net_amount} ($(echo "scale=6; ${net_amount} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Receiver"       "${receiver_input}" \
    "Receiver32"     "${receiver_bytes32}" \
    "Nonce"          "$nonce" \
    "EIP-712 digest" "$digest" \
    "Auth nonce"     "$auth_nonce" \
    "validAfter"     "$valid_after" \
    "validBefore"    "$valid_before (≈$(date -u -d "@${valid_before}" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "?"))"

  prompt_confirm "Proceed with gasless stake?" || return

  # Baseline (for poll) — same logic as the plain-stake path
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

  # Step D: build the StakeAuthorization tuple and send
  # tuple (address,uint256,uint256,bytes32,uint8,bytes32,bytes32) = (from, validAfter, validBefore, authNonce, v, r, s)
  local auth_tuple
  auth_tuple="(${staker_addr},${valid_after},${valid_before},${auth_nonce},${v},${r},${s})"

  evm_simulate "$rpc" "$bridge_addr" \
    "stakeWithAuthorization(uint64,uint256,bytes32,(address,uint256,uint256,bytes32,uint8,bytes32,bytes32))" \
    "$nonce" "$amount" "$receiver_bytes32" "$auth_tuple" || return

  local stake_out stake_tx
  stake_out=$(evm_send "$rpc" "$bridge_addr" \
    "stakeWithAuthorization(uint64,uint256,bytes32,(address,uint256,uint256,bytes32,uint8,bytes32,bytes32))" \
    "$nonce" "$amount" "$receiver_bytes32" "$auth_tuple" 2>&1)
  stake_tx=$(evm_extract_tx_hash "$stake_out")

  echo "" >&2
  print_tx_result "$chain" "$stake_tx"
  info "Gasless Staked event emitted with nonce=${nonce}."
  append_log "[evm/stakeWithAuthorization] chain=${chain} bridge=${bridge_addr} target=${peer_name}(${peer_id}) staker=${staker_addr} paymaster=${paymaster} amount=${amount} bridgeFee=${bridge_fee} gaslessFee=${gasless_fee} netAmount=${net_amount} receiver=${receiver_input} nonce=${nonce} tx=${stake_tx:-unknown}"

  if [[ -n "$target_chain_key" && -n "$target_usdc" && -n "$baseline_bal" ]]; then
    if prompt_confirm "Wait for the relayer to deliver ~${net_amount} (raw USDC) to ${receiver_input} on ${peer_name}?"; then
      poll_target_balance "$target_chain_key" "$target_usdc" "$receiver_input" \
        "$baseline_bal" "$net_amount" 300 10 || true
    fi
  else
    info "Watch the relayer logs / ${peer_name} explorer to confirm the unlock."
  fi
}
