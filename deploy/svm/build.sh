#!/usr/bin/env bash
# svm/build.sh — Build bridge1024 Anchor program
# Sourced by bridge.sh; do not execute directly.

op_svm_build() {
  local target="$1"
  local target_name="${CHAIN_DISPLAY[$target]}"

  echo "" >&2
  echo -e "  ${BOLD}── Build bridge1024 program ──${NC}" >&2
  echo "" >&2

  local svm_dir="$PROJECT_ROOT/contracts/svm"

  if [[ ! -d "$svm_dir" ]]; then
    error "SVM contracts directory not found: $svm_dir"; return
  fi

  local keypair="$svm_dir/target/deploy/bridge1024-keypair.json"
  if [[ -f "$keypair" ]]; then
    rm -f "$keypair"
    info "Removed old program keypair, will generate new program ID"
  fi

  info "Building Anchor program..."
  (cd "$svm_dir" && anchor build)

  if [[ -f "$svm_dir/target/deploy/bridge1024.so" ]]; then
    success "Build complete"
    info "Binary: target/deploy/bridge1024.so"
    local new_keypair="$svm_dir/target/deploy/bridge1024-keypair.json"
    if [[ -f "$new_keypair" ]]; then
      local program_id
      program_id=$(solana-keygen pubkey "$new_keypair" 2>/dev/null)
      info "Program ID: $program_id"
    fi
    if [[ -f "$svm_dir/target/idl/bridge1024.json" ]]; then
      info "IDL: target/idl/bridge1024.json"
    fi
  else
    error "Build may have failed — .so not found."
  fi
}
