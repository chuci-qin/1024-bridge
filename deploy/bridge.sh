#!/usr/bin/env bash
# bridge.sh — Bridge1024 unified deployment entry point
# Usage: ./deploy/bridge.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

# Source all operation modules
source "$SCRIPT_DIR/evm/deploy.sh"
source "$SCRIPT_DIR/evm/configure.sh"
source "$SCRIPT_DIR/evm/configure-rate-limits.sh"
source "$SCRIPT_DIR/evm/add-relayer.sh"
source "$SCRIPT_DIR/evm/activate-timelock.sh"
source "$SCRIPT_DIR/evm/fund-vault.sh"
source "$SCRIPT_DIR/evm/manage-roles.sh"
source "$SCRIPT_DIR/evm/info.sh"
source "$SCRIPT_DIR/svm/build.sh"
source "$SCRIPT_DIR/svm/deploy.sh"
source "$SCRIPT_DIR/svm/initialize.sh"
source "$SCRIPT_DIR/svm/configure.sh"
source "$SCRIPT_DIR/svm/configure-rate-limits.sh"
source "$SCRIPT_DIR/svm/register-peer.sh"
source "$SCRIPT_DIR/svm/add-relayer.sh"
source "$SCRIPT_DIR/svm/activate-timelock.sh"
source "$SCRIPT_DIR/svm/manage-roles.sh"
source "$SCRIPT_DIR/svm/info.sh"

# ── Main Menu Flow ─────────────────────────────────────────────────────────────

main() {
  print_header
  select_env

  while true; do
    menu_chain_type
  done
}

menu_chain_type() {
  local idx
  idx=$(prompt_select "[$CURRENT_ENV] Select chain type:" "EVM" "SVM" "← Back")

  case "$idx" in
    0) menu_evm_chain ;;
    1) menu_svm_target ;;
    2) select_env ;;
  esac
}

# ── EVM Menu ───────────────────────────────────────────────────────────────────

menu_evm_chain() {
  local chains
  read -ra chains <<< "$(get_evm_chains "$CURRENT_ENV")"

  while true; do
    local display_opts=()
    for c in "${chains[@]}"; do
      local name="${CHAIN_DISPLAY[$c]}"
      local bridge_addr
      bridge_addr=$(read_address ".evm.${c}.bridge")
      if [[ -n "$bridge_addr" ]]; then
        display_opts+=("${name}  (bridge: ${bridge_addr:0:10}...  deployed)")
      else
        display_opts+=("${name}  (not deployed)")
      fi
    done
    display_opts+=("← Back")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/evm] Select chain:" "${display_opts[@]}")

    if [[ "$idx" -ge "${#chains[@]}" ]]; then
      return 0
    fi

    local selected_chain="${chains[$idx]}"
    menu_evm_operation "$selected_chain"
  done
}

menu_evm_operation() {
  local chain="$1"
  local name="${CHAIN_DISPLAY[$chain]}"

  while true; do
    local bridge_addr
    bridge_addr=$(read_address ".evm.${chain}.bridge")

    local ops=()
    if [[ -z "$bridge_addr" ]]; then
      ops+=("Deploy contract")
    else
      ops+=("View contract info")
      ops+=("Deploy contract (redeploy)")
    fi
    ops+=("Configure bridge")
    ops+=("Configure rate limits")
    ops+=("Add relayer")
    ops+=("Activate timelock")
    ops+=("Fund vault")
    ops+=("Manage roles  →")
    ops+=("← Back")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/evm/$name] Select operation:" "${ops[@]}")

    if [[ -z "$bridge_addr" ]]; then
      case "$idx" in
        0) op_evm_deploy "$chain" || true ;;
        1) op_evm_configure "$chain" || true ;;
        2) op_evm_configure_rate_limits "$chain" || true ;;
        3) op_evm_add_relayer "$chain" || true ;;
        4) op_evm_activate_timelock "$chain" || true ;;
        5) op_evm_fund_vault "$chain" || true ;;
        6) menu_evm_roles "$chain" || true ;;
        *) return 0 ;;
      esac
    else
      case "$idx" in
        0) op_evm_info "$chain" || true ;;
        1) op_evm_deploy "$chain" || true ;;
        2) op_evm_configure "$chain" || true ;;
        3) op_evm_configure_rate_limits "$chain" || true ;;
        4) op_evm_add_relayer "$chain" || true ;;
        5) op_evm_activate_timelock "$chain" || true ;;
        6) op_evm_fund_vault "$chain" || true ;;
        7) menu_evm_roles "$chain" || true ;;
        *) return 0 ;;
      esac
    fi
  done
}

# ── SVM Menu ───────────────────────────────────────────────────────────────────

menu_svm_target() {
  local targets
  read -ra targets <<< "$(get_svm_targets "$CURRENT_ENV")"

  while true; do
    local display_opts=()
    for t in "${targets[@]}"; do
      local name="${CHAIN_DISPLAY[$t]}"
      local addr_key
      if [[ "$t" == 1024_* ]]; then
        addr_key=".\"1024\".program_id"
      else
        addr_key=".solana.program_id"
      fi
      local prog_id
      prog_id=$(read_address "$addr_key")
      if [[ -n "$prog_id" ]]; then
        display_opts+=("${name}  (program: ${prog_id:0:10}...  deployed)")
      else
        display_opts+=("${name}  (not deployed)")
      fi
    done
    display_opts+=("← Back")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/svm] Select target:" "${display_opts[@]}")

    if [[ "$idx" -ge "${#targets[@]}" ]]; then
      return 0
    fi

    local selected_target="${targets[$idx]}"
    menu_svm_operation "$selected_target"
  done
}

menu_svm_operation() {
  local target="$1"
  local name="${CHAIN_DISPLAY[$target]}"

  while true; do
    local addr_key
    if [[ "$target" == 1024_* ]]; then
      addr_key=".\"1024\".program_id"
    else
      addr_key=".solana.program_id"
    fi
    local prog_id
    prog_id=$(read_address "$addr_key")

    local ops=("Build program")
    if [[ -z "$prog_id" ]]; then
      ops+=("Deploy program")
    else
      ops+=("View program info")
      ops+=("Deploy program (redeploy)")
    fi
    ops+=("Initialize")
    ops+=("Configure")
    ops+=("Configure rate limits")
    ops+=("Register peer")
    ops+=("Add relayer")
    ops+=("Activate timelock")
    ops+=("Manage roles  →")
    ops+=("← Back")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/svm/$name] Select operation:" "${ops[@]}")

    if [[ -z "$prog_id" ]]; then
      case "$idx" in
        0) op_svm_build "$target" || true ;;
        1) op_svm_deploy "$target" || true ;;
        2) op_svm_initialize "$target" || true ;;
        3) op_svm_configure "$target" || true ;;
        4) op_svm_configure_rate_limits "$target" || true ;;
        5) op_svm_register_peer "$target" || true ;;
        6) op_svm_add_relayer "$target" || true ;;
        7) op_svm_activate_timelock "$target" || true ;;
        8) menu_svm_roles "$target" || true ;;
        *) return 0 ;;
      esac
    else
      case "$idx" in
        0) op_svm_build "$target" || true ;;
        1) op_svm_info "$target" || true ;;
        2) op_svm_deploy "$target" || true ;;
        3) op_svm_initialize "$target" || true ;;
        4) op_svm_configure "$target" || true ;;
        5) op_svm_configure_rate_limits "$target" || true ;;
        6) op_svm_register_peer "$target" || true ;;
        7) op_svm_add_relayer "$target" || true ;;
        8) op_svm_activate_timelock "$target" || true ;;
        9) menu_svm_roles "$target" || true ;;
        *) return 0 ;;
      esac
    fi
  done
}

main "$@"
