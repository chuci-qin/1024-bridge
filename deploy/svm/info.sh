#!/usr/bin/env bash
# svm/info.sh — Display on-chain bridge1024 / bridge1024_hub program state (SVM)
# Sourced by bridge.sh; do not execute directly.
#
# Program-kind aware:
#   - hub:  reads PeerConfig PDAs for every candidate peer chain and lists
#           them under "Peers".
#   - leaf: BridgeState carries the single peer inline; we print it as a
#           one-element list and also surface bridge_fee + gasless_fee from
#           the top-level fields read-state.ts now exposes for leaf programs.

# Collect all possible peer chain IDs in the current environment.
# Used as read-state.ts --peer-chain-ids (only registered PeerConfigs come back).
_svm_peer_chain_ids() {
  local target="$1"
  local ids=()
  local c
  for c in $(get_evm_chains "$CURRENT_ENV"); do
    ids+=("${CHAIN_ID[$c]}")
  done
  for c in $(get_svm_targets "$CURRENT_ENV"); do
    [[ "$c" == "$target" ]] && continue
    ids+=("${CHAIN_ID[$c]}")
  done
  local IFS=,
  echo "${ids[*]}"
}

# Reverse-lookup a chain ID into a display name (falls back to "ID:<n>").
_svm_chain_name_by_id() {
  local target_id="$1"
  local c
  for c in "${!CHAIN_ID[@]}"; do
    if [[ "${CHAIN_ID[$c]}" == "$target_id" ]]; then
      echo "${CHAIN_DISPLAY[$c]:-$c}"
      return
    fi
  done
  echo "ID:${target_id}"
}

# Classify a chain ID as EVM or SVM-pubkey. 1024 (91024-91026) + Solana (101/103)
# use 32-byte pubkeys as peer_contract; other chains right-align a 20B address.
_svm_kind_for_chain_id() {
  local cid="$1"
  case "$cid" in
    91024|91025|91026|101|103) echo "svm" ;;
    *) echo "evm" ;;
  esac
}

op_svm_info() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local target_id="${CHAIN_ID[$target]}"
  local rpc
  rpc=$(get_rpc "$target")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $target_name"; return; fi

  local kind
  kind=$(get_svm_program_kind "$target")
  local prog_name
  prog_name=$(get_svm_program_name "$target")

  local addr_key
  if [[ "$target" == 1024_* ]]; then
    addr_key=".\"1024\".program_id"
  else
    addr_key=".solana.program_id"
  fi
  local program_id
  program_id=$(read_address "$addr_key")
  if [[ -z "$program_id" ]]; then error "Program not deployed on $target_name."; return; fi

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM signer keypair path (read-only)") || return 0
  fi
  if [[ ! -f "$keypair_path" ]]; then error "Keypair file not found: $keypair_path"; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── ${prog_name} Info: ${target_name} (${kind}) ──${NC}" >&2
  echo "" >&2

  info "Program:   $program_id"
  info "Kind:      $kind (${prog_name})"
  info "Target:    $target_name (ID: $target_id)"
  info "RPC:       $rpc"

  local peer_ids
  peer_ids=$(_svm_peer_chain_ids "$target")

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  # IMPORTANT: do not merge stderr into stdout — npm warnings on stderr would
  # corrupt the JSON. stderr passes through to the terminal; stdout must stay
  # clean JSON. As a fallback we also pick only the last line that starts with '{'.
  local out
  out=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --program-kind "$kind" \
    --peer-chain-ids "$peer_ids") || {
    error "Failed to read program state (see stderr above)"
    return
  }
  out=$(echo "$out" | grep -E '^\{' | tail -n 1)
  if [[ -z "$out" ]]; then error "read-state.ts returned empty/non-JSON output"; return; fi

  # ── PDAs ──
  local bs_pda vault_pda vault_ata vault_bal
  bs_pda=$(echo "$out" | jq -r '.bridgeStatePda')
  vault_pda=$(echo "$out" | jq -r '.vaultPda')
  vault_ata=$(echo "$out" | jq -r '.vaultAta')
  vault_bal=$(echo "$out" | jq -r '.vaultBalance')

  echo "" >&2
  echo -e "  ${BOLD}PDAs:${NC}" >&2
  echo "    BridgeState: $bs_pda" >&2
  echo "    Vault:       $vault_pda" >&2
  if [[ -n "$vault_ata" && "$vault_ata" != "null" ]]; then
    echo "    Vault ATA:   $vault_ata" >&2
  fi

  # ── Roles ──
  local r_admin r_guard r_oper r_rec r_pending
  r_admin=$(echo "$out" | jq -r '.admin')
  r_guard=$(echo "$out" | jq -r '.guardian')
  r_oper=$(echo "$out"  | jq -r '.operator')
  r_rec=$(echo "$out"   | jq -r '.recovery')
  r_pending=$(echo "$out" | jq -r '.pending')
  echo "" >&2
  echo -e "  ${BOLD}Roles:${NC}" >&2
  echo "    Admin:      $r_admin" >&2
  echo "    Guardian:   $r_guard" >&2
  echo "    Operator:   $r_oper" >&2
  echo "    Recovery:   $r_rec" >&2
  if [[ "$r_pending" != "11111111111111111111111111111111" && -n "$r_pending" ]]; then
    echo "    Pending:    $r_pending" >&2
  fi

  # ── Configuration ──
  local usdc local_id vbump
  usdc=$(echo "$out"     | jq -r '.usdcMint')
  local_id=$(echo "$out" | jq -r '.localChainId')
  vbump=$(echo "$out"    | jq -r '.vaultBump')
  echo "" >&2
  echo -e "  ${BOLD}Configuration:${NC}" >&2
  echo "    USDC mint:      $usdc" >&2
  echo "    Local chain ID: $local_id" >&2
  echo "    Vault bump:     $vbump" >&2

  if [[ "$kind" == "leaf" ]]; then
    local leaf_peer_chain leaf_peer_contract leaf_bridge_fee leaf_gasless_fee leaf_max_stake
    leaf_peer_chain=$(echo "$out"     | jq -r '.peerChainId // "0"')
    leaf_peer_contract=$(echo "$out"  | jq -r '.peerContract // ""')
    leaf_bridge_fee=$(echo "$out"     | jq -r '.bridgeFee // "0"')
    leaf_gasless_fee=$(echo "$out"    | jq -r '.gaslessFee // "0"')
    leaf_max_stake=$(echo "$out"      | jq -r '.maxStakeAmount // "0"')
    echo "    Peer chain ID:  $leaf_peer_chain" >&2
    echo "    Peer contract:  $leaf_peer_contract" >&2
    echo "    Bridge fee:     ${leaf_bridge_fee} ($(echo "scale=6; ${leaf_bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
    echo "    Gasless fee:    ${leaf_gasless_fee} ($(echo "scale=6; ${leaf_gasless_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
    if [[ "$leaf_gasless_fee" == "0" ]]; then
      echo "                    (gasless path DISABLED)" >&2
    fi
    echo "    Max stake:      ${leaf_max_stake} ($(echo "scale=0; ${leaf_max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  fi

  # ── Timelock & Status ──
  local tl_active paused
  tl_active=$(echo "$out" | jq -r '.timelockActive')
  paused=$(echo "$out"    | jq -r '.isPaused')
  echo "" >&2
  echo -e "  ${BOLD}Timelock:${NC}" >&2
  echo "    Active: $tl_active" >&2
  echo "" >&2
  echo -e "  ${BOLD}Status:${NC}" >&2
  echo "    Paused: $paused" >&2
  if [[ -n "$vault_bal" && "$vault_bal" != "null" ]]; then
    echo "    Vault USDC: ${vault_bal} ($(echo "scale=2; ${vault_bal} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  fi

  # ── Global rate limits ──
  local rl_max rl_dur rl_single rl_min rl_ws rl_wu rl_pu
  rl_max=$(echo "$out"    | jq -r '.maxUnlockPerWindow')
  rl_dur=$(echo "$out"    | jq -r '.windowDuration')
  rl_single=$(echo "$out" | jq -r '.maxSingleUnlock')
  rl_min=$(echo "$out"    | jq -r '.minimumReserve')
  rl_ws=$(echo "$out"     | jq -r '.currentWindowStart')
  rl_wu=$(echo "$out"     | jq -r '.currentWindowUsage')
  rl_pu=$(echo "$out"     | jq -r '.previousWindowUsage')
  echo "" >&2
  if [[ "$kind" == "leaf" ]]; then
    echo -e "  ${BOLD}Rate Limits (single layer, on BridgeState):${NC}" >&2
  else
    echo -e "  ${BOLD}Global Rate Limits:${NC}" >&2
  fi
  echo "    Max per window:  ${rl_max} ($(echo "scale=0; ${rl_max} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  echo "    Window duration: ${rl_dur}s" >&2
  echo "    Max single:      ${rl_single} ($(echo "scale=0; ${rl_single} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  echo "    Min reserve:     ${rl_min} ($(echo "scale=0; ${rl_min} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  local ws_str=""
  if [[ -n "$rl_ws" && "$rl_ws" != "0" ]]; then
    ws_str=$(date -u -d "@${rl_ws}" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "")
  fi
  if [[ -n "$ws_str" ]]; then
    echo "    Window start:    ${rl_ws} (${ws_str})" >&2
  else
    echo "    Window start:    ${rl_ws}" >&2
  fi
  echo "    Window usage:    ${rl_wu}" >&2
  echo "    Prev usage:      ${rl_pu}" >&2

  # ── Relayers ──
  local relayer_count
  relayer_count=$(echo "$out" | jq -r '.relayers | length')
  echo "" >&2
  echo -e "  ${BOLD}Relayers:${NC} ${relayer_count}" >&2
  if [[ "$relayer_count" -gt 0 ]]; then
    local i=0
    while [[ $i -lt $relayer_count ]]; do
      local r
      r=$(echo "$out" | jq -r ".relayers[$i]")
      echo "    [$i] $r" >&2
      ((i++))
    done
  fi

  # ── Peers ──
  # Hub: scan of PeerConfig PDAs (one entry per registered chain).
  # Leaf: synthesized one-element list from BridgeState (or empty if not configured).
  local peer_count
  peer_count=$(echo "$out" | jq -r '.peers | length')
  echo "" >&2
  if [[ "$kind" == "leaf" ]]; then
    echo -e "  ${BOLD}Peer (single, inline on BridgeState):${NC} ${peer_count}" >&2
  else
    echo -e "  ${BOLD}Peers:${NC} ${peer_count}" >&2
  fi
  if [[ "$peer_count" -gt 0 ]]; then
    local i=0
    while [[ $i -lt $peer_count ]]; do
      local p_chain p_contract_hex p_contract_evm p_contract_svm p_fee p_max_stake p_max_win p_dur p_max_single p_ws p_wu p_pu
      p_chain=$(echo "$out"        | jq -r ".peers[$i].chainId")
      p_contract_hex=$(echo "$out" | jq -r ".peers[$i].peerContract")
      p_contract_evm=$(echo "$out" | jq -r ".peers[$i].peerContractEvm // empty")
      p_contract_svm=$(echo "$out" | jq -r ".peers[$i].peerContractSvm")
      p_fee=$(echo "$out"          | jq -r ".peers[$i].bridgeFee")
      p_max_stake=$(echo "$out"  | jq -r ".peers[$i].maxStakeAmount")
      p_max_win=$(echo "$out"    | jq -r ".peers[$i].maxUnlockPerWindow")
      p_dur=$(echo "$out"        | jq -r ".peers[$i].windowDuration")
      p_max_single=$(echo "$out" | jq -r ".peers[$i].maxSingleUnlock")
      p_ws=$(echo "$out"         | jq -r ".peers[$i].currentWindowStart")
      p_wu=$(echo "$out"         | jq -r ".peers[$i].currentWindowUsage")
      p_pu=$(echo "$out"         | jq -r ".peers[$i].previousWindowUsage")

      local kind2 chain_name peer_pretty
      kind2=$(_svm_kind_for_chain_id "$p_chain")
      chain_name=$(_svm_chain_name_by_id "$p_chain")
      if [[ "$kind2" == "evm" && -n "$p_contract_evm" ]]; then
        peer_pretty="$p_contract_evm"
      else
        peer_pretty="$p_contract_svm"
      fi

      echo "    ─ ${chain_name} (chain ID: ${p_chain})" >&2
      echo "        Peer contract:   $peer_pretty" >&2
      echo "        Bridge fee:      ${p_fee} ($(echo "scale=6; ${p_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
      echo "        Max stake:       ${p_max_stake} ($(echo "scale=0; ${p_max_stake} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
      if [[ "$kind" == "hub" ]]; then
        echo "        Max per window:  ${p_max_win} ($(echo "scale=0; ${p_max_win} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
        echo "        Window duration: ${p_dur}s" >&2
        echo "        Max single:      ${p_max_single} ($(echo "scale=0; ${p_max_single} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
        local p_ws_str=""
        if [[ -n "$p_ws" && "$p_ws" != "0" ]]; then
          p_ws_str=$(date -u -d "@${p_ws}" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "")
        fi
        if [[ -n "$p_ws_str" ]]; then
          echo "        Window start:    ${p_ws} (${p_ws_str})" >&2
        else
          echo "        Window start:    ${p_ws}" >&2
        fi
        echo "        Window usage:    ${p_wu}" >&2
        echo "        Prev usage:      ${p_pu}" >&2
      fi
      ((i++))
    done
  fi

  echo "" >&2
}
