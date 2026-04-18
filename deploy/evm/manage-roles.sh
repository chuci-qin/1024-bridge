#!/usr/bin/env bash
# evm/manage-roles.sh — Bridge1024 角色轮换 (proposeAdmin / acceptAdmin / setGuardian / setOperator / setRecovery)
# Sourced by bridge.sh; do not execute directly.
#
# 设计要点：
# - 4 个 setX 操作和 proposeAdmin 都受 timelock 保护：合约内部用
#   keccak256(abi.encode("<sigName>", newAddr)) 作为 opHash，
#   timelockActive=false 时直接执行；true 时需先 scheduleOperation 再执行。
# - acceptAdmin 不走 timelock（必须由 pendingAdmin 自己签名调用，新 admin 的私钥
#   通常和当前 EVM_PRIVATE_KEY 不同）。
# - 入口前预读 getBridgeInfo()，做一次本地角色重叠预检，提前阻断会被合约 RoleOverlap 回滚的提议，
#   避免在 timelockActive=true 下白白消耗一次 24h 的调度。

# ── 内部辅助 ────────────────────────────────────────────────────────────

# 读取桥的 5 个角色，写入指定的输出变量名
# 用法：_evm_load_roles "$rpc" "$bridge_addr" admin guardian operator recovery pending
#
# 内部 nameref 必须用唯一前缀 (__lr_*)，否则若调用方传进来的变量名也叫
# `_admin / _guard / ...`（比如 _evm_role_preflight 里就是这么命名的 nameref），
# `local -n _admin="_admin"` 会形成自指引用，bash 报 "circular name reference"，
# 后续 `_admin=...` 赋值落不到真正的目标变量上。
_evm_load_roles() {
  local rpc="$1" bridge="$2"
  local -n __lr_admin="$3" __lr_guard="$4" __lr_oper="$5" __lr_rec="$6" __lr_pending="$7"
  local -a bi
  mapfile -t bi < <(evm_read "$rpc" "$bridge" \
    "getBridgeInfo()(address,address,address,address,address,address,bytes32,uint64,uint64,bool,bool,uint256)")
  __lr_admin=$(echo "${bi[0]}" | xargs)
  __lr_guard=$(echo "${bi[1]}" | xargs)
  __lr_oper=$(echo "${bi[2]}" | xargs)
  __lr_rec=$(echo "${bi[3]}" | xargs)
  __lr_pending=$(echo "${bi[4]}" | xargs)
}

# 角色重叠本地预检（合约层 RoleOverlap 的同义检查）
# 用法：_evm_check_role_overlap "<role_name>" "$new_addr" "$admin" "$guardian" "$operator" "$recovery" "$pending"
# 注意各 setX 函数的合约规则不一致：
#   - proposeAdmin: 不可与 admin/guardian/operator/recovery 之一相等
#   - setGuardian:  不可与 admin/operator/recovery/pendingAdmin 之一相等
#   - setOperator:  不可与 admin/guardian/recovery/pendingAdmin 之一相等
#   - setRecovery:  不可与 admin/guardian/operator/pendingAdmin 之一相等
_evm_check_role_overlap() {
  local op="$1" new="$2" admin="$3" guardian="$4" operator="$5" recovery="$6" pending="$7"
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
      [[ -n "$pending" && "$pending" != "0x0000000000000000000000000000000000000000" \
        && "$new" == "$pending" ]] && conflict="pendingAdmin"
      ;;
    setOperator)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$guardian" ]] && conflict="guardian"
      [[ "$new" == "$recovery" ]] && conflict="recovery"
      [[ -n "$pending" && "$pending" != "0x0000000000000000000000000000000000000000" \
        && "$new" == "$pending" ]] && conflict="pendingAdmin"
      ;;
    setRecovery)
      [[ "$new" == "$admin" ]]    && conflict="admin"
      [[ "$new" == "$guardian" ]] && conflict="guardian"
      [[ "$new" == "$operator" ]] && conflict="operator"
      [[ -n "$pending" && "$pending" != "0x0000000000000000000000000000000000000000" \
        && "$new" == "$pending" ]] && conflict="pendingAdmin"
      ;;
  esac

  if [[ -n "$conflict" ]]; then
    error "Role overlap: ${new} 已经是当前 ${conflict}，合约会回滚 RoleOverlap"
    return 1
  fi
  return 0
}

# 用 cast 计算 keccak256(abi.encode(opName, newAddr))
# 输出 32-byte 0x... 的 opHash
_evm_compute_op_hash() {
  local op_name="$1" new_addr="$2"
  local data
  data=$(cast abi-encode "f(string,address)" "$op_name" "$new_addr")
  cast keccak "$data"
}

# 通用 timelock-aware 发送：自动处理 schedule / execute 三态。
# 在 admin 是多签（signer != admin）时自动降级为打印 Safe payload + 导出 Safe Tx Builder JSON。
# 用法：_evm_send_role_op rpc bridge admin op_name new_addr fn_sig
#   admin:   合约当前 admin（用于 simulate --from + Safe 模式判定）
#   op_name: 用于 abi.encode 的字符串，如 "setGuardian"
#   fn_sig:  目标函数签名，如 "setGuardian(address)"
_evm_send_role_op() {
  local rpc="$1" bridge="$2" admin="$3" op_name="$4" new_addr="$5" fn_sig="$6"

  local timelock_active
  timelock_active=$(evm_read "$rpc" "$bridge" "timelockActive()(bool)" 2>/dev/null | xargs) || true

  if [[ "$timelock_active" != "true" ]]; then
    info "Timelock 未激活，直接执行 ${fn_sig}"
    evm_send_as "$admin" "$rpc" "$bridge" "$fn_sig" "$new_addr"
    return $?
  fi

  # ── timelock active ──
  local op_hash data eta now
  op_hash=$(_evm_compute_op_hash "$op_name" "$new_addr")
  data=$(cast abi-encode "f(string,address)" "$op_name" "$new_addr")
  eta=$(evm_read "$rpc" "$bridge" "timelockEta(bytes32)(uint64)" "$op_hash" 2>/dev/null | xargs) || eta=0
  now=$(date +%s)

  echo "" >&2
  info "Timelock 已激活"
  info "  opHash: $op_hash"

  if [[ -z "$eta" || "$eta" == "0" ]]; then
    info "状态：未调度"
    local idx
    idx=$(prompt_select "选择操作：" "Schedule（调度，等待 24h 后可执行）" "← Back")
    [[ "$idx" == "0" ]] || return 0

    evm_send_as "$admin" "$rpc" "$bridge" "scheduleOperation(bytes)" "$data"
    local rc=$?
    [[ $rc -eq 0 ]] && success "调度已提交，可执行时间约：$(date -u -d "@$(($(date +%s) + 86400))" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo 'now+24h')"
    return $rc
  fi

  # 已调度：判断是否到期
  local grace=$((48 * 3600))
  local eta_str expire_str
  eta_str=$(date -u -d "@$eta" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "@$eta")
  expire_str=$(date -u -d "@$((eta + grace))" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || echo "@$((eta + grace))")

  info "已调度  ETA:     $eta_str"
  info "        Expires: $expire_str"

  if (( now < eta )); then
    local remain=$((eta - now))
    warn "未到执行时间，还差 $((remain / 3600)) 小时 $((remain % 3600 / 60)) 分钟"
    return 0
  fi
  if (( now > eta + grace )); then
    error "操作已过期（超过 grace period 48h），需要重新调度"
    return 1
  fi

  local idx
  idx=$(prompt_select "状态：可执行。选择操作：" "Execute（执行 ${fn_sig}）" "← Back")
  [[ "$idx" == "0" ]] || return 0

  evm_send_as "$admin" "$rpc" "$bridge" "$fn_sig" "$new_addr"
  return $?
}

# ── 共用前置：环境/合约/连接检查 + 加载当前角色 ─────────────────────────
# 成功返回 0，且通过 nameref 输出 rpc / bridge / chain_id / 各角色字段
_evm_role_preflight() {
  local chain="$1"
  local -n _rpc="$2" _bridge="$3"
  local -n _admin="$4" _guard="$5" _oper="$6" _rec="$7" _pending="$8"

  local chain_name="${CHAIN_DISPLAY[$chain]}"
  local chain_id="${CHAIN_ID[$chain]}"
  _rpc=$(get_rpc "$chain")
  if [[ -z "$_rpc" ]]; then error "RPC not configured for $chain_name"; return 1; fi

  _bridge=$(read_address ".evm.${chain}.bridge")
  if [[ -z "$_bridge" ]]; then error "Bridge not deployed on $chain_name."; return 1; fi

  evm_check_chain_id "$_rpc" "$chain_id" || return 1

  _evm_load_roles "$_rpc" "$_bridge" _admin _guard _oper _rec _pending

  echo "" >&2
  info "Bridge:    $_bridge"
  info "Chain:     $chain_name (ID: $chain_id)"
  info "Admin:     $_admin"
  info "Guardian:  $_guard"
  info "Operator:  $_oper"
  info "Recovery:  $_rec"
  if [[ "$_pending" != "0x0000000000000000000000000000000000000000" && -n "$_pending" ]]; then
    info "Pending:   $_pending"
  fi
}

# ── 操作：proposeAdmin ─────────────────────────────────────────────────

op_evm_propose_admin() {
  local chain="$1"
  local rpc bridge admin guardian operator recovery pending
  _evm_role_preflight "$chain" rpc bridge admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Propose New Admin ──${NC}" >&2
  warn "提议生效需要新 admin 用自己的私钥调用 acceptAdmin（两步转移）"

  local new_admin
  new_admin=$(prompt_input "新 admin 地址" "" evm_address) || return 0
  _evm_check_role_overlap "proposeAdmin" "$new_admin" "$admin" "$guardian" "$operator" "$recovery" "$pending" || return 0

  if [[ "$pending" != "0x0000000000000000000000000000000000000000" && -n "$pending" && "$pending" != "$new_admin" ]]; then
    warn "当前已有 pendingAdmin=${pending}，重新提议会覆盖它"
    prompt_confirm "继续？" || return 0
  fi

  print_summary "Propose Admin" \
    "Bridge"     "$bridge" \
    "Old admin"  "$admin" \
    "Pending"    "${pending}" \
    "New admin"  "$new_admin"
  prompt_confirm "Proceed?" || return 0

  local tx
  tx=$(_evm_send_role_op "$rpc" "$bridge" "$admin" "proposeAdmin" "$new_admin" "proposeAdmin(address)") || return 1
  if [[ -n "$tx" ]]; then
    append_log "[evm/proposeAdmin] chain=${chain} bridge=${bridge} newAdmin=${new_admin} tx=${tx}"
    print_tx_result "$chain" "$tx"
  else
    append_log "[evm/proposeAdmin] chain=${chain} bridge=${bridge} newAdmin=${new_admin} status=safe-queued"
  fi
}

# ── 操作：acceptAdmin ──────────────────────────────────────────────────

op_evm_accept_admin() {
  local chain="$1"
  local rpc bridge admin guardian operator recovery pending
  _evm_role_preflight "$chain" rpc bridge admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Accept Admin ──${NC}" >&2

  if [[ "$pending" == "0x0000000000000000000000000000000000000000" || -z "$pending" ]]; then
    warn "当前 pendingAdmin 为空，没有待接受的提议"
    return 0
  fi

  local signer
  signer=$(evm_signer_address)
  info "当前 signer:  ${signer:-<none>}"
  info "pendingAdmin: $pending"

  print_summary "Accept Admin" \
    "Bridge"     "$bridge" \
    "Old admin"  "$admin" \
    "New admin"  "$pending"
  prompt_confirm "Proceed?" || return 0

  # 注意：acceptAdmin 要求 msg.sender == pendingAdmin。
  # 如果 signer != pending（新 admin 也是多签），evm_send_as 会自动降级为 Safe payload。
  local tx
  tx=$(evm_send_as "$pending" "$rpc" "$bridge" "acceptAdmin()") || return 0
  if [[ -n "$tx" ]]; then
    local on_admin on_pending
    on_admin=$(evm_read "$rpc" "$bridge" "admin()(address)" 2>/dev/null | xargs) || true
    on_pending=$(evm_read "$rpc" "$bridge" "pendingAdmin()(address)" 2>/dev/null | xargs) || true
    print_verification "admin"        "$pending"                                          "$on_admin"
    print_verification "pendingAdmin" "0x0000000000000000000000000000000000000000"        "$on_pending"

    write_address ".roles.admin_evm" "$pending"
    append_log "[evm/acceptAdmin] chain=${chain} bridge=${bridge} oldAdmin=${admin} newAdmin=${pending} tx=${tx}"
    print_tx_result "$chain" "$tx"
  else
    append_log "[evm/acceptAdmin] chain=${chain} bridge=${bridge} oldAdmin=${admin} newAdmin=${pending} status=safe-queued"
  fi
}

# ── 操作：setGuardian / setOperator / setRecovery ───────────────────────

_op_evm_set_role() {
  local chain="$1" role="$2" fn_sig="$3" op_name="$4" json_key="$5"

  local rpc bridge admin guardian operator recovery pending
  _evm_role_preflight "$chain" rpc bridge admin guardian operator recovery pending || return 0

  echo "" >&2
  echo -e "  ${BOLD}── Set ${role} ──${NC}" >&2

  local default_addr
  case "$role" in
    Guardian) default_addr="$guardian" ;;
    Operator) default_addr="$operator" ;;
    Recovery) default_addr="$recovery" ;;
  esac

  local new_addr
  new_addr=$(prompt_input "新 ${role} 地址" "" evm_address) || return 0
  if [[ "${new_addr,,}" == "${default_addr,,}" ]]; then
    warn "新地址与当前 ${role} 相同，无需变更"
    return 0
  fi
  _evm_check_role_overlap "$op_name" "$new_addr" "$admin" "$guardian" "$operator" "$recovery" "$pending" || return 0

  print_summary "Set ${role}" \
    "Bridge"     "$bridge" \
    "Old ${role}" "$default_addr" \
    "New ${role}" "$new_addr"
  prompt_confirm "Proceed?" || return 0

  local tx
  tx=$(_evm_send_role_op "$rpc" "$bridge" "$admin" "$op_name" "$new_addr" "$fn_sig") || return 1
  if [[ -n "$tx" ]]; then
    write_address "$json_key" "$new_addr"
    append_log "[evm/${op_name}] chain=${chain} bridge=${bridge} new=${new_addr} tx=${tx}"
    print_tx_result "$chain" "$tx"
  else
    append_log "[evm/${op_name}] chain=${chain} bridge=${bridge} new=${new_addr} status=safe-queued"
  fi
}

op_evm_set_guardian() { _op_evm_set_role "$1" "Guardian" "setGuardian(address)" "setGuardian" ".roles.guardian_evm"; }
op_evm_set_operator() { _op_evm_set_role "$1" "Operator" "setOperator(address)" "setOperator" ".roles.operator_evm"; }
op_evm_set_recovery() { _op_evm_set_role "$1" "Recovery" "setRecovery(address)" "setRecovery" ".roles.recovery_evm"; }

# ── 子菜单 ─────────────────────────────────────────────────────────────

menu_evm_roles() {
  local chain="$1"
  local name="${CHAIN_DISPLAY[$chain]}"
  while true; do
    local idx
    idx=$(prompt_select "[$CURRENT_ENV/evm/$name/roles] Select role operation:" \
      "Propose new admin" \
      "Accept admin (signer = pendingAdmin)" \
      "Set guardian" \
      "Set operator" \
      "Set recovery" \
      "← Back")
    case "$idx" in
      0) op_evm_propose_admin "$chain" || true ;;
      1) op_evm_accept_admin  "$chain" || true ;;
      2) op_evm_set_guardian  "$chain" || true ;;
      3) op_evm_set_operator  "$chain" || true ;;
      4) op_evm_set_recovery  "$chain" || true ;;
      *) return 0 ;;
    esac
  done
}
