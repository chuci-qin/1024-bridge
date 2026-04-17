#!/usr/bin/env bash
# svm/configure-rate-limits.sh — Configure rate limits on bridge1024
# Sourced by bridge.sh; do not execute directly.

op_svm_configure_rate_limits() {
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
  echo -e "  ${BOLD}── Configure Rate Limits on ${target_name} ──${NC}"
  echo ""

  local keypair_path="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$keypair_path" ]]; then
    keypair_path=$(prompt_input "SVM admin keypair path") || return 0
  fi

  info "All amounts in USDC raw units (6 decimals)"
  echo ""

  local max_per_window window_duration max_single min_reserve

  max_per_window=$(prompt_input "Max unlock per window (raw)" "10000000000" uint)
  window_duration=$(prompt_input "Window duration (seconds)" "3600" uint)
  max_single=$(prompt_input "Max single unlock (raw)" "5000000000" uint)
  min_reserve=$(prompt_input "Minimum reserve (raw)" "20000000000" uint)

  print_summary "Rate Limits" \
    "Target"          "$target_name" \
    "Program"         "$program_id" \
    "Max per window"  "${max_per_window} ($(echo "scale=0; ${max_per_window} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Window duration" "${window_duration}s" \
    "Max single"      "${max_single} ($(echo "scale=0; ${max_single} / 1000000" | bc 2>/dev/null || echo "?") USDC)" \
    "Min reserve"     "${min_reserve} ($(echo "scale=0; ${min_reserve} / 1000000" | bc 2>/dev/null || echo "?") USDC)"

  prompt_confirm "Proceed?" || return

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  info "Running configure_rate_limits instruction..."

  npx ts-node "$svm_deploy_dir/src/instructions/configure-rate-limits.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" \
    --max-per-window "$max_per_window" \
    --window-duration "$window_duration" \
    --max-single "$max_single" \
    --min-reserve "$min_reserve"

  if [[ $? -eq 0 ]]; then
    append_log "[svm/configureRateLimits] target=${target} program=${program_id} maxPerWindow=${max_per_window} windowDuration=${window_duration} maxSingle=${max_single} minReserve=${min_reserve}"
    success "Rate limits configured"
  fi
}
