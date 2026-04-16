#!/usr/bin/env bash
# evm/configure-rate-limits.sh — Configure rate limits on Bridge1024 EVM
# Sourced by bridge.sh; do not execute directly.

op_evm_configure_rate_limits() {
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
  echo -e "  ${BOLD}── Configure Rate Limits on ${chain_name} ──${NC}" >&2
  echo "" >&2

  info "Chain:    $chain_name (ID: $chain_id)"
  info "RPC:      $rpc"
  info "Bridge:   $bridge_addr"

  evm_check_chain_id "$rpc" "$chain_id" || return

  # Check timelock
  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge_addr" "timelockActive()(bool)" 2>/dev/null | xargs) || true
  if [[ "$timelock_active" == "true" ]]; then
    error "Timelock is active. Use schedule/execute flow for configuration changes."; return
  fi

  # Parameters (amounts in raw units — 6 decimals for USDC)
  info "All amounts in USDC raw units (6 decimals, e.g. 10000000000 = 10,000 USDC)"
  echo ""

  local max_per_window window_duration max_single max_stake min_reserve

  max_per_window=$(prompt_input "Max unlock per window (raw)" "10000000000" uint)
  window_duration=$(prompt_input "Window duration (seconds)" "3600" uint)
  max_single=$(prompt_input "Max single unlock (raw)" "5000000000" uint)
  max_stake=$(prompt_input "Max single stake (raw)" "5000000000" uint)
  min_reserve=$(prompt_input "Minimum reserve (raw)" "20000000000" uint)

  print_summary "Rate Limits" \
    "Bridge"           "$bridge_addr" \
    "Chain"            "$chain_name" \
    "Max per window"   "${max_per_window} ($(echo "scale=0; $max_per_window / 1000000" | bc) USDC)" \
    "Window duration"  "${window_duration}s" \
    "Max single"       "${max_single} ($(echo "scale=0; $max_single / 1000000" | bc) USDC)" \
    "Max stake"        "${max_stake} ($(echo "scale=0; $max_stake / 1000000" | bc) USDC)" \
    "Min reserve"      "${min_reserve} ($(echo "scale=0; $min_reserve / 1000000" | bc) USDC)"

  prompt_confirm "Proceed?" || return

  evm_simulate "$rpc" "$bridge_addr" \
    "configureRateLimits(uint64,uint64,uint64,uint64,uint64)" \
    "$max_per_window" "$window_duration" "$max_single" "$max_stake" "$min_reserve" || return

  local output
  output=$(evm_send "$rpc" "$bridge_addr" \
    "configureRateLimits(uint64,uint64,uint64,uint64,uint64)" \
    "$max_per_window" "$window_duration" "$max_single" "$max_stake" "$min_reserve" 2>&1)

  local tx_hash
  tx_hash=$(echo "$output" | grep -i "transactionHash" | awk '{print $NF}') || \
    tx_hash=$(echo "$output" | head -1)

  # Verify via getRateLimitStatus()
  info "Verifying rate limits..."
  local -a rl
  mapfile -t rl < <(evm_read "$rpc" "$bridge_addr" \
    "getRateLimitStatus()(uint64,uint64,uint64,uint64,uint64,uint64,uint64,uint64)" 2>/dev/null)
  print_verification "maxUnlockPerWindow" "$max_per_window" "$(echo "${rl[0]}" | xargs)"
  print_verification "windowDuration"     "$window_duration" "$(echo "${rl[1]}" | xargs)"
  print_verification "maxSingleUnlock"    "$max_single"      "$(echo "${rl[2]}" | xargs)"
  print_verification "maxStakeAmount"     "$max_stake"       "$(echo "${rl[3]}" | xargs)"
  print_verification "minimumReserve"     "$min_reserve"     "$(echo "${rl[4]}" | xargs)"

  append_log "[evm/configureRateLimits] chain=${chain} bridge=${bridge_addr} maxPerWindow=${max_per_window} windowDuration=${window_duration} maxSingle=${max_single} maxStake=${max_stake} minReserve=${min_reserve} tx=${tx_hash:-unknown}"
  print_tx_result "$chain" "${tx_hash:-unknown}"
}
