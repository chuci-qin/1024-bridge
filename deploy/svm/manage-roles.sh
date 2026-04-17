#!/usr/bin/env bash
# svm/manage-roles.sh — bridge1024 角色轮换 (proposeAdmin / acceptAdmin / setGuardian / setOperator / setRecovery)
# Sourced by bridge.sh; do not execute directly.
#
# 设计要点：
# - 4 个 setX 操作和 propose_admin 受 timelock 保护：role-op.ts 会自动根据
#   timelock_active 选择直接执行（未激活）或 schedule/execute 二段流程（激活后）。
# - accept_admin 必须由 pending_admin 自己签名调用，不走 timelock；
#   通常需要先在 .env 切换 SVM_KEYPAIR_PATH 到新 admin keypair 后再运行。
# - 入口前先 fetch BridgeState，做一次本地角色重叠预检，提前阻断会被合约 RoleOverlap 回滚的提议。

# ── 共用工具 ────────────────────────────────────────────────────────────

# 读取 program_id 的 jq 路径
_svm_addr_key() {
  local target="$1"
  if [[ "$target" == 1024_* ]]; then
    echo ".\"1024\".program_id"
  else
    echo ".solana.program_id"
  fi
}

# 通过 ts-node 读取 BridgeState，回填 5 个 nameref
_svm_load_roles() {
  local rpc="$1" program_id="$2" keypair_path="$3"
  local -n _admin="$4" _guard="$5" _oper="$6" _rec="$7" _pending="$8"

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  local out
  out=$(npx ts-node "$svm_deploy_dir/src/instructions/read-state.ts" \
    --rpc-url "$rpc" \
    --keypair "$keypair_path" \
    --program-id "$program_id" 2>/dev/null) || {
    error "无法读取 BridgeState：请确认 program 已部署且 IDL 已编译"
    return 1
  }

  _admin=$(echo "$out" | jq -r '.admin')
  _guard=$(echo "$out" | jq -r '.guardian')
  _oper=$(echo "$out" | jq -r '.operator')
  _rec=$(echo "$out" | jq -r '.recovery')
  _pending=$(echo "$out" | jq -r '.pending')
}

# 角色重叠本地预检（合约层 RoleOverlap 的同义检查）
# 用法：_svm_check_role_overlap "<op>" "$new_addr" "$admin" "$guardian" "$operator" "$recovery" "$pending"
_svm_check_role_overlap() {
  local op="$1" new="$2" admin="$3" guardian="$4" operator="$5" recovery="$6" pending="$7"
  local zero="11111111111111111111111111111111"
  local conflict=""

  case "$op" in
    proposeAdmin)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$guardian" ]] && conflict="guardian"
      [[ "$new" == "$operator" ]] && conflict="operator"
      [[ "$new" == "$recovery" ]] && conflict="recovery"
      ;;
    setGuardian)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$operator" ]] && conflict="operator"
      [[ "$new" == "$recovery" ]] && conflict="recovery"
      [[ "$pending" != "$zero" && "$new" == "$pending" ]] && conflict="pending_admin"
      ;;
    setOperator)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$guardian" ]] && conflict="guardian"
      [[ "$new" == "$recovery" ]] && conflict="recovery"
      [[ "$pending" != "$zero" && "$new" == "$pending" ]] && conflict="pending_admin"
      ;;
    setRecovery)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$guardian" ]] && conflict="guardian"
      [[ "$new" == "$operator" ]] && conflict="operator"
      [[ "$new" == "$pending" && "$pending" != "$zero" ]] && conflict="pending_admin"
      ;;
  esac

  if [[ -n "$conflict" ]]; then
    error "Role overlap: ${new} 已经是当前 ${conflict}，合约会回滚 RoleOverlap"
    return 1
  fi
  return 0
}

# 共用前置：解析 rpc / program_id / keypair_path / 读取角色
# 输出（nameref）：rpc / program_id / keypair_path / admin / guardian / operator / recovery / pending
_svm_role_preflight() {
  local target="$1"
  local -n _rpc="$2" _prog="$3" _kp="$4"
  local -n _admin="$5" _guard="$6" _oper="$7" _rec="$8" _pending="$9"

  local target_name="${CHAIN_DISPLAY[$target]}"
  _rpc=$(get_rpc "$target")
  if [[ -z "$_rpc" ]]; then error "RPC not configured for $target_name"; return 1; fi

  local addr_key
  addr_key=$(_svm_addr_key "$target")
  _prog=$(read_address "$addr_key")
  if [[ -z "$_prog" ]]; then error "Program not deployed on $target_name."; return 1; fi

  _kp="${SVM_KEYPAIR_PATH:-}"
  if [[ -z "$_kp" ]]; then
    _kp=$(prompt_input "SVM admin keypair path") || return 1
  fi

  _svm_load_roles "$_rpc" "$_prog" "$_kp" _admin _guard _oper _rec _pending || return 1

  echo "" >&2
  info "Program:   $_prog"
  info "Target:    $target_name"
  info "Admin:     $_admin"
  info "Guardian:  $_guard"
  info "Operator:  $_oper"
  info "Recovery:  $_rec"
  if [[ "$_pending" != "11111111111111111111111111111111" ]]; then
    info "Pending:   $_pending"
  fi
}

# 通用 timelock-aware 调度/执行：直接委托给 role-op.ts --mode auto
_svm_run_role_op() {
  local rpc="$1" prog="$2" kp="$3" op="$4" target_pubkey="$5"

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  npx ts-node "$svm_deploy_dir/src/instructions/role-op.ts" \
    --rpc-url "$rpc" \
    --keypair "$kp" \
    --program-id "$prog" \
    --op "$op" \
    --target "$target_pubkey" \
    --mode auto
}

# ── 操作：propose_admin ────────────────────────────────────────────────

op_svm_propose_admin() {
  local target="$1"
  local rpc prog kp admin guardian operator recovery pending
  _svm_role_preflight "$target" rpc prog kp admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Propose New Admin ──${NC}" >&2
  warn "提议生效需要新 admin 用自己的 keypair 调用 accept_admin（两步转移）"

  local new_admin
  new_admin=$(prompt_input "新 admin pubkey" "" svm_pubkey) || return 0
  _svm_check_role_overlap "proposeAdmin" "$new_admin" "$admin" "$guardian" "$operator" "$recovery" "$pending" || return 0

  if [[ "$pending" != "11111111111111111111111111111111" && "$pending" != "$new_admin" ]]; then
    warn "当前已有 pending_admin=${pending}，重新提议会覆盖它"
    prompt_confirm "继续？" || return 0
  fi

  print_summary "Propose Admin" \
    "Program"   "$prog" \
    "Old admin" "$admin" \
    "Pending"   "$pending" \
    "New admin" "$new_admin"
  prompt_confirm "Proceed?" || return 0

  if _svm_run_role_op "$rpc" "$prog" "$kp" "proposeAdmin" "$new_admin"; then
    append_log "[svm/proposeAdmin] target=${target} program=${prog} newAdmin=${new_admin}"
    success "Done"
  fi
}

# ── 操作：accept_admin ─────────────────────────────────────────────────

op_svm_accept_admin() {
  local target="$1"
  local rpc prog kp admin guardian operator recovery pending
  _svm_role_preflight "$target" rpc prog kp admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Accept Admin ──${NC}" >&2

  if [[ "$pending" == "11111111111111111111111111111111" ]]; then
    warn "当前 pending_admin 为空，没有待接受的提议"
    return 0
  fi

  local signer
  signer=$(solana-keygen pubkey "$kp" 2>/dev/null) || { error "无法读取 keypair pubkey: $kp"; return 0; }
  info "当前 signer: $signer"
  info "pending_admin: $pending"
  if [[ "$signer" != "$pending" ]]; then
    error "当前 keypair 对应的 pubkey 不是 pending_admin，无法 accept_admin"
    info "请在 config/${CURRENT_ENV}/.env 切换 SVM_KEYPAIR_PATH 到新 admin keypair 后重试"
    return 0
  fi

  print_summary "Accept Admin" \
    "Program"   "$prog" \
    "Old admin" "$admin" \
    "New admin" "$signer"
  prompt_confirm "Proceed?" || return 0

  local svm_deploy_dir="$DEPLOY_DIR/svm"
  if npx ts-node "$svm_deploy_dir/src/instructions/accept-admin.ts" \
    --rpc-url "$rpc" \
    --keypair "$kp" \
    --program-id "$prog"; then
    write_address ".roles.admin_svm" "$signer"
    append_log "[svm/acceptAdmin] target=${target} program=${prog} oldAdmin=${admin} newAdmin=${signer}"
    success "Done"
  fi
}

# ── 操作：set_guardian / set_operator / set_recovery ────────────────────

_op_svm_set_role() {
  local target="$1" role_label="$2" op_name="$3" json_key="$4"

  local rpc prog kp admin guardian operator recovery pending
  _svm_role_preflight "$target" rpc prog kp admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Set ${role_label} ──${NC}" >&2

  local current
  case "$role_label" in
    Guardian) current="$guardian" ;;
    Operator) current="$operator" ;;
    Recovery) current="$recovery" ;;
  esac

  local new_addr
  new_addr=$(prompt_input "新 ${role_label} pubkey" "" svm_pubkey) || return 0
  if [[ "$new_addr" == "$current" ]]; then
    warn "新地址与当前 ${role_label} 相同，无需变更"
    return 0
  fi
  _svm_check_role_overlap "$op_name" "$new_addr" "$admin" "$guardian" "$operator" "$recovery" "$pending" || return 0

  print_summary "Set ${role_label}" \
    "Program"     "$prog" \
    "Old ${role_label}" "$current" \
    "New ${role_label}" "$new_addr"
  prompt_confirm "Proceed?" || return 0

  if _svm_run_role_op "$rpc" "$prog" "$kp" "$op_name" "$new_addr"; then
    write_address "$json_key" "$new_addr"
    append_log "[svm/${op_name}] target=${target} program=${prog} new=${new_addr}"
    success "Done"
  fi
}

op_svm_set_guardian() { _op_svm_set_role "$1" "Guardian" "setGuardian" ".roles.guardian_svm"; }
op_svm_set_operator() { _op_svm_set_role "$1" "Operator" "setOperator" ".roles.operator_svm"; }
op_svm_set_recovery() { _op_svm_set_role "$1" "Recovery" "setRecovery" ".roles.recovery_svm"; }

# ── 子菜单 ─────────────────────────────────────────────────────────────

menu_svm_roles() {
  local target="$1"
  local name="${CHAIN_DISPLAY[$target]}"
  while true; do
    local idx
    idx=$(prompt_select "[$CURRENT_ENV/svm/$name/roles] Select role operation:" \
      "Propose new admin" \
      "Accept admin (signer = pending_admin)" \
      "Set guardian" \
      "Set operator" \
      "Set recovery" \
      "← Back")
    case "$idx" in
      0) op_svm_propose_admin "$target" || true ;;
      1) op_svm_accept_admin  "$target" || true ;;
      2) op_svm_set_guardian  "$target" || true ;;
      3) op_svm_set_operator  "$target" || true ;;
      4) op_svm_set_recovery  "$target" || true ;;
      *) return 0 ;;
    esac
  done
}
