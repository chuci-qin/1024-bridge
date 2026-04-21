#!/usr/bin/env bash
# evm/info.sh — Display on-chain Bridge1024 contract state
# Sourced by bridge.sh; do not execute directly.

op_evm_info() {
  local chain="$1"
  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  local rpc
  rpc=$(get_rpc "$chain")
  if [[ -z "$rpc" ]]; then error "RPC not configured for $chain_name"; return; fi

  local bridge_addr
  bridge_addr=$(read_address ".evm.${chain}.bridge")
  if [[ -z "$bridge_addr" ]]; then error "Bridge not deployed on $chain_name."; return; fi

  echo "" >&2
  echo -e "  ${BOLD}── Bridge1024 Info: ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Contract: $bridge_addr"
  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"

  evm_check_chain_id "$rpc" "$chain_id" || return

  echo "" >&2

  # Batch read via getBridgeInfo()
  local -a bi
  mapfile -t bi < <(evm_read "$rpc" "$bridge_addr" \
    "getBridgeInfo()(address,address,address,address,address,address,bytes32,uint64,uint64,bool,bool,uint256)")
  local on_admin on_guardian on_operator on_recovery on_pending_admin
  local on_usdc on_peer_contract on_local_id on_peer_id
  local paused timelock_active relayer_count
  on_admin=$(echo "${bi[0]}" | xargs)
  on_guardian=$(echo "${bi[1]}" | xargs)
  on_operator=$(echo "${bi[2]}" | xargs)
  on_recovery=$(echo "${bi[3]}" | xargs)
  on_pending_admin=$(echo "${bi[4]}" | xargs)
  on_usdc=$(echo "${bi[5]}" | xargs)
  on_peer_contract=$(echo "${bi[6]}" | xargs)
  on_local_id=$(echo "${bi[7]}" | xargs)
  on_peer_id=$(echo "${bi[8]}" | xargs)
  paused=$(echo "${bi[9]}" | xargs)
  timelock_active=$(echo "${bi[10]}" | xargs)
  relayer_count=$(echo "${bi[11]}" | xargs)

  echo -e "  ${BOLD}Roles:${NC}" >&2
  echo "    Admin:      $on_admin" >&2
  echo "    Guardian:   $on_guardian" >&2
  echo "    Operator:   $on_operator" >&2
  echo "    Recovery:   $on_recovery" >&2
  if [[ "$on_pending_admin" != "0x0000000000000000000000000000000000000000" && -n "$on_pending_admin" ]]; then
    echo "    Pending:    $on_pending_admin" >&2
  fi

  # Config
  echo "" >&2
  echo -e "  ${BOLD}Configuration:${NC}" >&2
  echo "    USDC:           $on_usdc" >&2
  echo "    Local chain ID: $on_local_id" >&2
  echo "    Peer chain ID:  $on_peer_id" >&2
  echo "    Peer contract:  $on_peer_contract" >&2
  local on_bridge_fee
  on_bridge_fee=$(evm_read "$rpc" "$bridge_addr" "bridgeFee()(uint64)" 2>/dev/null | xargs) || on_bridge_fee="0"
  echo "    Bridge fee:     ${on_bridge_fee} ($(echo "scale=6; ${on_bridge_fee} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2

  # Timelock
  echo "" >&2
  echo -e "  ${BOLD}Timelock:${NC}" >&2
  echo "    Active: $timelock_active" >&2

  # Rate limits via getRateLimitStatus()
  local -a rl
  mapfile -t rl < <(evm_read "$rpc" "$bridge_addr" \
    "getRateLimitStatus()(uint64,uint64,uint64,uint64,uint64,uint64,uint64,uint64)")
  local rl_max_per_window rl_window_dur rl_max_single rl_max_stake rl_min_reserve
  local rl_win_start rl_win_usage rl_prev_usage
  rl_max_per_window=$(echo "${rl[0]}" | xargs)
  rl_window_dur=$(echo "${rl[1]}" | xargs)
  rl_max_single=$(echo "${rl[2]}" | xargs)
  rl_max_stake=$(echo "${rl[3]}" | xargs)
  rl_min_reserve=$(echo "${rl[4]}" | xargs)
  rl_win_start=$(echo "${rl[5]}" | xargs)
  rl_win_usage=$(echo "${rl[6]}" | xargs)
  rl_prev_usage=$(echo "${rl[7]}" | xargs)
  echo "" >&2
  echo -e "  ${BOLD}Rate Limits:${NC}" >&2
  echo "    Max per window:  ${rl_max_per_window:-0} ($(echo "scale=0; ${rl_max_per_window:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  echo "    Window duration: ${rl_window_dur:-0}s" >&2
  echo "    Max single:      ${rl_max_single:-0} ($(echo "scale=0; ${rl_max_single:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  echo "    Max stake:       ${rl_max_stake:-0} ($(echo "scale=0; ${rl_max_stake:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  echo "    Min reserve:     ${rl_min_reserve:-0} ($(echo "scale=0; ${rl_min_reserve:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  local win_start_str=""
  if [[ -n "${rl_win_start:-}" && "${rl_win_start}" != "0" ]]; then
    win_start_str=$(date -u -d "@${rl_win_start}" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "")
  fi
  if [[ -n "$win_start_str" ]]; then
    echo "    Window start:    ${rl_win_start} (${win_start_str})" >&2
  else
    echo "    Window start:    ${rl_win_start:-0}" >&2
  fi
  echo "    Window usage:    ${rl_win_usage:-0}" >&2
  echo "    Prev usage:      ${rl_prev_usage:-0}" >&2

  # Relayers
  echo "" >&2
  echo -e "  ${BOLD}Relayers:${NC} ${relayer_count:-0}" >&2

  if [[ "${relayer_count:-0}" -gt 0 ]]; then
    local i=0
    while [[ $i -lt $relayer_count ]]; do
      local r
      r=$(evm_read "$rpc" "$bridge_addr" "relayers(uint256)(address)" "$i" | xargs) || true
      echo "    [$i] $r" >&2
      ((i++))
    done
  fi

  # Status
  echo "" >&2
  echo -e "  ${BOLD}Status:${NC}" >&2
  echo "    Paused: $paused" >&2

  # USDC balance
  if [[ "$on_usdc" != "0x0000000000000000000000000000000000000000" && -n "$on_usdc" ]]; then
    local vault_balance
    vault_balance=$(evm_read "$rpc" "$on_usdc" "balanceOf(address)(uint256)" "$bridge_addr" | xargs) || true
    echo "    Vault USDC: ${vault_balance:-0} ($(echo "scale=2; ${vault_balance:-0} / 1000000" | bc 2>/dev/null || echo "?") USDC)" >&2
  fi

  echo "" >&2
}
