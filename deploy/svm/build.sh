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
  local rotate_id=0

  if [[ -f "$keypair" ]]; then
    # 默认保留现有 keypair；用户显式要换才重新生成
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

  # 需要新 keypair 时，先用 solana-keygen 直接生成 + anchor keys sync 把 declare_id!()
  # 写好，再 anchor build。这样只需要 build 一次（否则 anchor build 会先用旧 declare_id 编一次，
  # sync 完再编一次，浪费一轮编译）。
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
      || warn "anchor keys sync 失败；请手动更新 lib.rs 的 declare_id!() 再重 build"
  fi

  info "Building Anchor program..."
  (cd "$svm_dir" && anchor build) || { error "anchor build failed"; return; }

  if [[ -f "$svm_dir/target/deploy/bridge1024.so" ]]; then
    success "Build complete"
    info "Binary: target/deploy/bridge1024.so"
    if [[ -f "$keypair" ]]; then
      local program_id
      program_id=$(solana-keygen pubkey "$keypair" 2>/dev/null)
      info "Program ID: $program_id"
      local idl_addr=""
      if [[ -f "$svm_dir/target/idl/bridge1024.json" ]]; then
        idl_addr=$(jq -r '.address // .metadata.address // ""' "$svm_dir/target/idl/bridge1024.json" 2>/dev/null)
      fi
      if [[ -n "$idl_addr" && "$idl_addr" != "$program_id" ]]; then
        warn "IDL.address ($idl_addr) != keypair ($program_id) — declare_id!() 未同步，部署后 init 会失败"
      fi
    fi
    if [[ -f "$svm_dir/target/idl/bridge1024.json" ]]; then
      info "IDL: target/idl/bridge1024.json"
    fi
  else
    error "Build may have failed — .so not found."
  fi
}
