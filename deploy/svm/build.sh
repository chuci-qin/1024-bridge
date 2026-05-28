#!/usr/bin/env bash
# svm/build.sh — Build the Anchor program for the selected SVM target
# Sourced by bridge.sh; do not execute directly.
#
# 1024_* targets build `bridge1024_hub` (multi-peer hub program).
# solana / solana_devnet build `bridge1024` (single-peer leaf, EVM-symmetric).
# Each program has its own keypair/.so/IDL under contracts/svm/target/.

op_svm_build() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"
  local prog
  prog=$(get_svm_program_name "$target")
  local kind
  kind=$(get_svm_program_kind "$target")

  echo "" >&2
  echo -e "  ${BOLD}── Build ${prog} (${kind}) for ${target_name} ──${NC}" >&2
  echo "" >&2

  local svm_dir="$PROJECT_ROOT/contracts/svm"

  if [[ ! -d "$svm_dir" ]]; then
    error "SVM contracts directory not found: $svm_dir"; return
  fi

  local keypair="$svm_dir/target/deploy/${prog}-keypair.json"
  local rotate_id=0

  if [[ -f "$keypair" ]]; then
    # Keep existing keypair by default — rotate only when explicitly asked.
    local existing_id
    existing_id=$(solana-keygen pubkey "$keypair" 2>/dev/null || echo "")
    info "Existing program keypair: ${existing_id:-<unreadable>}"
    if prompt_confirm "Rotate program ID (generate new keypair)?"; then
      rm -f "$keypair"
      rotate_id=1
      info "Removed keypair, will generate new program ID"
    fi
  else
    info "No program keypair found, will generate a new one"
    rotate_id=1
  fi

  # When the keypair changes, generate it first + `anchor keys sync` so
  # declare_id!() points at the new ID before anchor build — otherwise anchor
  # builds once with the old declare_id, syncs, and rebuilds (wasted compile).
  if [[ "$rotate_id" == "1" ]]; then
    mkdir -p "$svm_dir/target/deploy"
    info "Generating new program keypair..."
    solana-keygen new --no-bip39-passphrase --outfile "$keypair" --silent --force >/dev/null 2>&1 \
      || { error "Failed to generate keypair: $keypair"; return; }
    local new_id
    new_id=$(solana-keygen pubkey "$keypair" 2>/dev/null)
    info "New program ID: $new_id"
    info "Syncing declare_id!() to new keypair..."
    (cd "$svm_dir" && anchor keys sync 2>&1 | sed 's/^/  /') \
      || warn "anchor keys sync failed; update lib.rs declare_id!() manually then re-run build"
  fi

  info "Building Anchor program: ${prog}..."
  (cd "$svm_dir" && anchor build -p "$prog") || { error "anchor build failed"; return; }

  local so_path="$svm_dir/target/deploy/${prog}.so"
  local idl_path="$svm_dir/target/idl/${prog}.json"
  if [[ -f "$so_path" ]]; then
    success "Build complete"
    info "Binary: target/deploy/${prog}.so"
    if [[ -f "$keypair" ]]; then
      local program_id
      program_id=$(solana-keygen pubkey "$keypair" 2>/dev/null)
      info "Program ID: $program_id"
      local idl_addr=""
      if [[ -f "$idl_path" ]]; then
        idl_addr=$(jq -r '.address // .metadata.address // ""' "$idl_path" 2>/dev/null)
      fi
      if [[ -n "$idl_addr" && "$idl_addr" != "$program_id" ]]; then
        warn "IDL.address ($idl_addr) != keypair ($program_id) — declare_id!() not synced; init will fail after deploy"
      fi
    fi
    if [[ -f "$idl_path" ]]; then
      info "IDL: target/idl/${prog}.json"
    fi
  else
    error "Build may have failed — ${prog}.so not found."
  fi
}
