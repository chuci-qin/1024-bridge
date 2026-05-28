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
source "$SCRIPT_DIR/evm/configure-bridge-fee.sh"
source "$SCRIPT_DIR/evm/configure-gasless-fee.sh"
source "$SCRIPT_DIR/evm/add-relayer.sh"
source "$SCRIPT_DIR/evm/rotate-relayer.sh"
source "$SCRIPT_DIR/evm/activate-timelock.sh"
source "$SCRIPT_DIR/evm/fund-vault.sh"
source "$SCRIPT_DIR/evm/fund-relayers.sh"
source "$SCRIPT_DIR/evm/manage-roles.sh"
source "$SCRIPT_DIR/evm/info.sh"
source "$SCRIPT_DIR/evm/stake.sh"
source "$SCRIPT_DIR/evm/withdraw.sh"
source "$SCRIPT_DIR/svm/build.sh"
source "$SCRIPT_DIR/svm/deploy.sh"
source "$SCRIPT_DIR/svm/initialize.sh"
source "$SCRIPT_DIR/svm/configure.sh"
source "$SCRIPT_DIR/svm/configure-rate-limits.sh"
source "$SCRIPT_DIR/svm/configure-peer-fee.sh"
source "$SCRIPT_DIR/svm/configure-peer-rate-limits.sh"
source "$SCRIPT_DIR/svm/configure-bridge-fee.sh"
source "$SCRIPT_DIR/svm/configure-gasless-fee.sh"
source "$SCRIPT_DIR/svm/register-peer.sh"
source "$SCRIPT_DIR/svm/unregister-peer.sh"
source "$SCRIPT_DIR/svm/add-relayer.sh"
source "$SCRIPT_DIR/svm/rotate-relayer.sh"
source "$SCRIPT_DIR/svm/fund-vault.sh"
source "$SCRIPT_DIR/svm/fund-relayers.sh"
source "$SCRIPT_DIR/svm/activate-timelock.sh"
source "$SCRIPT_DIR/svm/manage-roles.sh"
source "$SCRIPT_DIR/svm/info.sh"
source "$SCRIPT_DIR/svm/stake.sh"
source "$SCRIPT_DIR/svm/stake-gasless.sh"
source "$SCRIPT_DIR/svm/withdraw.sh"

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

# Parallel arrays: ops_labels[i] is shown in the menu, ops_handlers[i] is the
# function called when the user picks index i. Both are rebuilt on every render
# so labels reflect current on-chain state (deployed vs not).
menu_evm_operation() {
  local chain="$1"
  local name="${CHAIN_DISPLAY[$chain]}"

  while true; do
    local bridge_addr
    bridge_addr=$(read_address ".evm.${chain}.bridge")

    local ops_labels=() ops_handlers=()
    if [[ -z "$bridge_addr" ]]; then
      ops_labels+=("Deploy contract")              ; ops_handlers+=("op_evm_deploy")
    else
      ops_labels+=("View contract info")           ; ops_handlers+=("op_evm_info")
      ops_labels+=("Deploy contract (redeploy)")   ; ops_handlers+=("op_evm_deploy")
    fi
    ops_labels+=("Configure bridge")                ; ops_handlers+=("op_evm_configure")
    ops_labels+=("Configure rate limits")           ; ops_handlers+=("op_evm_configure_rate_limits")
    ops_labels+=("Configure bridge fee")            ; ops_handlers+=("op_evm_configure_bridge_fee")
    ops_labels+=("Configure gasless fee")           ; ops_handlers+=("op_evm_configure_gasless_fee")
    ops_labels+=("Add relayer")                     ; ops_handlers+=("op_evm_add_relayer")
    ops_labels+=("Rotate relayer")                  ; ops_handlers+=("op_evm_rotate_relayer")
    ops_labels+=("Activate timelock")               ; ops_handlers+=("op_evm_activate_timelock")
    ops_labels+=("Fund vault")                      ; ops_handlers+=("op_evm_fund_vault")
    ops_labels+=("Withdraw")                        ; ops_handlers+=("op_evm_withdraw")
    ops_labels+=("Fund relayers (gas)")             ; ops_handlers+=("op_evm_fund_relayers")
    ops_labels+=("Manage roles  →")                 ; ops_handlers+=("menu_evm_roles")
    if [[ -n "$bridge_addr" ]]; then
      ops_labels+=("Bridge transfer (stake)")       ; ops_handlers+=("op_evm_stake")
    fi
    ops_labels+=("← Back")                          ; ops_handlers+=("__back__")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/evm/$name] Select operation:" "${ops_labels[@]}")

    local handler="${ops_handlers[$idx]}"
    if [[ "$handler" == "__back__" ]]; then
      return 0
    fi
    "$handler" "$chain" || true
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
      local prog_name
      prog_name=$(get_svm_program_name "$t")
      local addr_key
      if [[ "$t" == 1024_* ]]; then
        addr_key=".\"1024\".program_id"
      else
        addr_key=".solana.program_id"
      fi
      local prog_id
      prog_id=$(read_address "$addr_key")
      if [[ -n "$prog_id" ]]; then
        display_opts+=("${name}  [${prog_name}]  (program: ${prog_id:0:10}...  deployed)")
      else
        display_opts+=("${name}  [${prog_name}]  (not deployed)")
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

# Parallel arrays + program-kind filtering. The hub program (bridge1024_hub)
# and the leaf program (bridge1024) expose disjoint sets of admin ops; this
# function builds the menu from the right set on each render.
menu_svm_operation() {
  local target="$1"
  local name="${CHAIN_DISPLAY[$target]}"
  local kind
  kind=$(get_svm_program_kind "$target")

  while true; do
    local addr_key
    if [[ "$target" == 1024_* ]]; then
      addr_key=".\"1024\".program_id"
    else
      addr_key=".solana.program_id"
    fi
    local prog_id
    prog_id=$(read_address "$addr_key")

    local ops_labels=() ops_handlers=()
    ops_labels+=("Build program")                    ; ops_handlers+=("op_svm_build")
    if [[ -z "$prog_id" ]]; then
      ops_labels+=("Deploy program")                 ; ops_handlers+=("op_svm_deploy")
    else
      ops_labels+=("View program info")              ; ops_handlers+=("op_svm_info")
      ops_labels+=("Deploy program (redeploy)")      ; ops_handlers+=("op_svm_deploy")
    fi
    ops_labels+=("Initialize")                       ; ops_handlers+=("op_svm_initialize")
    ops_labels+=("Configure")                        ; ops_handlers+=("op_svm_configure")
    ops_labels+=("Configure rate limits")            ; ops_handlers+=("op_svm_configure_rate_limits")

    if [[ "$kind" == "hub" ]]; then
      # Hub: multi-peer ops + per-peer rate limits
      ops_labels+=("Configure peer fee")             ; ops_handlers+=("op_svm_configure_peer_fee")
      ops_labels+=("Configure peer rate limits")     ; ops_handlers+=("op_svm_configure_peer_rate_limits")
      ops_labels+=("Register peer")                  ; ops_handlers+=("op_svm_register_peer")
      ops_labels+=("Unregister peer")                ; ops_handlers+=("op_svm_unregister_peer")
    else
      # Leaf: single-peer, plus global bridge_fee + gasless_fee + gasless stake
      ops_labels+=("Configure bridge fee")           ; ops_handlers+=("op_svm_configure_bridge_fee")
      ops_labels+=("Configure gasless fee")          ; ops_handlers+=("op_svm_configure_gasless_fee")
    fi

    ops_labels+=("Add relayer")                      ; ops_handlers+=("op_svm_add_relayer")
    ops_labels+=("Rotate relayer")                   ; ops_handlers+=("op_svm_rotate_relayer")

    if [[ -n "$prog_id" ]]; then
      ops_labels+=("Fund vault")                     ; ops_handlers+=("op_svm_fund_vault")
      ops_labels+=("Withdraw")                       ; ops_handlers+=("op_svm_withdraw")
    fi
    ops_labels+=("Fund relayers (gas)")              ; ops_handlers+=("op_svm_fund_relayers")
    ops_labels+=("Activate timelock")                ; ops_handlers+=("op_svm_activate_timelock")
    ops_labels+=("Manage roles  →")                  ; ops_handlers+=("menu_svm_roles")

    if [[ -n "$prog_id" ]]; then
      ops_labels+=("Bridge transfer (stake)")        ; ops_handlers+=("op_svm_stake")
      if [[ "$kind" == "leaf" ]]; then
        ops_labels+=("Bridge transfer (gasless stake)") ; ops_handlers+=("op_svm_stake_gasless")
      fi
    fi
    ops_labels+=("← Back")                           ; ops_handlers+=("__back__")

    local idx
    idx=$(prompt_select "[$CURRENT_ENV/svm/$name (${kind})] Select operation:" "${ops_labels[@]}")

    local handler="${ops_handlers[$idx]}"
    if [[ "$handler" == "__back__" ]]; then
      return 0
    fi
    "$handler" "$target" || true
  done
}

main "$@"
