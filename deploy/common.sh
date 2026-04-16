#!/usr/bin/env bash
# common.sh — Bridge1024 deploy shared library
# Source this file; do not execute directly.

set -euo pipefail

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$DEPLOY_DIR/.." && pwd)"
CONFIG_DIR="$DEPLOY_DIR/config"
KEYS_DIR="$DEPLOY_DIR/keys"

# ── Colors ─────────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC} $*" >&2; }
success() { echo -e "${GREEN}[OK]${NC} $*" >&2; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }
fatal()   { error "$@"; exit 1; }

# ── Chain Registry ─────────────────────────────────────────────────────────────
# Fixed data: chain IDs, USDC addresses, explorer URLs, RPC env var names.

declare -A CHAIN_ID=(
  [arbitrum_sepolia]=421614
  [ethereum_sepolia]=11155111
  [base_sepolia]=84532
  [arbitrum]=42161
  [ethereum]=1
  [base]=8453
  [1024_testnet]=91025
  [1024_stablenet]=91026
  [1024_mainnet]=91024
  [solana_devnet]=0
  [solana]=0
)

declare -A CHAIN_USDC_VAR=(
  [arbitrum_sepolia]="USDC_ARBITRUM_SEPOLIA"
  [ethereum_sepolia]="USDC_ETHEREUM_SEPOLIA"
  [base_sepolia]="USDC_BASE_SEPOLIA"
  [arbitrum]="USDC_ARBITRUM"
  [ethereum]="USDC_ETHEREUM"
  [base]="USDC_BASE"
  [solana_devnet]="USDC_SOLANA_DEVNET"
  [solana]="USDC_SOLANA"
  [1024_testnet]="USDC_1024_TESTNET"
  [1024_stablenet]="USDC_1024_STABLENET"
  [1024_mainnet]="USDC_1024_MAINNET"
)

declare -A CHAIN_EXPLORER=(
  [arbitrum_sepolia]="https://sepolia.arbiscan.io"
  [ethereum_sepolia]="https://sepolia.etherscan.io"
  [base_sepolia]="https://sepolia.basescan.org"
  [arbitrum]="https://arbiscan.io"
  [ethereum]="https://etherscan.io"
  [base]="https://basescan.org"
)

declare -A CHAIN_EXPLORER_API_VAR=(
  [arbitrum_sepolia]="ARBISCAN_API_KEY"
  [ethereum_sepolia]="ETHERSCAN_API_KEY"
  [base_sepolia]="BASESCAN_API_KEY"
  [arbitrum]="ARBISCAN_API_KEY"
  [ethereum]="ETHERSCAN_API_KEY"
  [base]="BASESCAN_API_KEY"
)

declare -A CHAIN_VERIFY_URL=(
  [arbitrum_sepolia]="https://api-sepolia.arbiscan.io/api"
  [ethereum_sepolia]="https://api-sepolia.etherscan.io/api"
  [base_sepolia]="https://api-sepolia.basescan.org/api"
  [arbitrum]="https://api.arbiscan.io/api"
  [ethereum]="https://api.etherscan.io/api"
  [base]="https://api.basescan.org/api"
)

# Maps env + chain_key to the RPC env var name
declare -A CHAIN_RPC_VAR=(
  [arbitrum_sepolia]="RPC_ARBITRUM_SEPOLIA"
  [ethereum_sepolia]="RPC_ETHEREUM_SEPOLIA"
  [base_sepolia]="RPC_BASE_SEPOLIA"
  [arbitrum]="RPC_ARBITRUM"
  [ethereum]="RPC_ETHEREUM"
  [base]="RPC_BASE"
  [1024_testnet]="RPC_1024_TESTNET"
  [1024_stablenet]="RPC_1024_STABLENET"
  [1024_mainnet]="RPC_1024_MAINNET"
  [solana_devnet]="RPC_SOLANA_DEVNET"
  [solana]="RPC_SOLANA"
)

# Human-readable chain names
declare -A CHAIN_DISPLAY=(
  [arbitrum_sepolia]="Arbitrum Sepolia"
  [ethereum_sepolia]="Ethereum Sepolia"
  [base_sepolia]="Base Sepolia"
  [arbitrum]="Arbitrum One"
  [ethereum]="Ethereum"
  [base]="Base"
  [1024_testnet]="1024 Testnet"
  [1024_stablenet]="1024 Stablenet"
  [1024_mainnet]="1024 Mainnet"
  [solana_devnet]="Solana Devnet"
  [solana]="Solana Mainnet"
)

# Which EVM chains belong to each environment
get_evm_chains() {
  local env="$1"
  case "$env" in
    testnet|stablenet) echo "arbitrum_sepolia ethereum_sepolia base_sepolia" ;;
    mainnet)           echo "arbitrum ethereum base" ;;
  esac
}

# Which SVM targets belong to each environment
get_svm_targets() {
  local env="$1"
  case "$env" in
    testnet)    echo "1024_testnet solana_devnet" ;;
    stablenet)  echo "1024_stablenet solana_devnet" ;;
    mainnet)    echo "1024_mainnet solana" ;;
  esac
}

# The 1024 chain key for an environment
get_1024_chain_key() {
  local env="$1"
  echo "1024_${env}"
}

# Get the addresses.json key for an EVM chain (used in jq paths)
get_evm_addr_key() {
  local chain="$1"
  echo "$chain"
}

get_chain_id()      { echo "${CHAIN_ID[$1]:-}"; }
get_usdc_address() {
  local chain="$1"
  local var="${CHAIN_USDC_VAR[$chain]:-}"
  [[ -z "$var" ]] && return 1
  echo "${!var:-}"
}
get_explorer_url()  { echo "${CHAIN_EXPLORER[$1]:-}"; }

get_rpc() {
  local chain="$1"
  local var="${CHAIN_RPC_VAR[$chain]:-}"
  [[ -z "$var" ]] && return 1
  echo "${!var:-}"
}

# ── Environment Selection & Config Loading ─────────────────────────────────────

CURRENT_ENV=""
ADDRESSES_FILE=""

select_env() {
  while true; do
    echo ""
    echo -e "  ${BOLD}Select environment:${NC}"
    echo "    1) testnet"
    echo "    2) stablenet"
    echo "    3) mainnet"
    echo ""
    local choice
    read -rp "  > " choice
    case "$choice" in
      1) CURRENT_ENV="testnet"; break ;;
      2) CURRENT_ENV="stablenet"; break ;;
      3) CURRENT_ENV="mainnet"; break ;;
      *) error "Invalid choice: $choice" ;;
    esac
  done

  if [[ "$CURRENT_ENV" == "mainnet" ]]; then
    require_mainnet_confirm
  fi

  load_config "$CURRENT_ENV"
}

require_mainnet_confirm() {
  echo ""
  echo -e "  ${RED}${BOLD}⚠  WARNING: You selected MAINNET — this operates on REAL funds.${NC}"
  echo ""
  local confirm
  read -rp "  Type YES to confirm: " confirm
  [[ "$confirm" == "YES" ]] || fatal "Aborted."
}

load_config() {
  local env="$1"
  CURRENT_ENV="$env"
  ADDRESSES_FILE="$CONFIG_DIR/$env/addresses.json"

  # Load global RPC config
  local global_env="$CONFIG_DIR/.env"
  if [[ -f "$global_env" ]]; then
    set -a; source "$global_env"; set +a
  else
    warn "Global config not found: $global_env (copy from .env.example)"
  fi

  # Load environment-specific signing config
  local env_file="$CONFIG_DIR/$env/.env"
  if [[ -f "$env_file" ]]; then
    set -a; source "$env_file"; set +a
  else
    warn "Environment config not found: $env_file (copy from .env.example)"
  fi

  # Resolve relative paths (relative to config/{env}/ dir)
  if [[ -n "${SVM_KEYPAIR_PATH:-}" && ! "$SVM_KEYPAIR_PATH" = /* ]]; then
    SVM_KEYPAIR_PATH="$CONFIG_DIR/$env/$SVM_KEYPAIR_PATH"
  fi
  if [[ -n "${EVM_PRIVATE_KEY_PATH:-}" && ! "$EVM_PRIVATE_KEY_PATH" = /* ]]; then
    EVM_PRIVATE_KEY_PATH="$CONFIG_DIR/$env/$EVM_PRIVATE_KEY_PATH"
  fi

  info "Loaded config for ${BOLD}$env${NC}"
}

# ── Addresses JSON I/O ─────────────────────────────────────────────────────────

read_address() {
  local jq_path="$1"
  local file="${2:-$ADDRESSES_FILE}"
  if [[ -f "$file" ]]; then
    jq -r "$jq_path // empty" "$file" 2>/dev/null || echo ""
  else
    echo ""
  fi
}

write_address() {
  local jq_path="$1"
  local value="$2"
  local file="${3:-$ADDRESSES_FILE}"
  local tmp
  tmp=$(mktemp)
  jq "$jq_path = \"$value\"" "$file" > "$tmp" && mv "$tmp" "$file"
}

read_relayers() {
  local file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  if [[ -f "$file" ]]; then
    jq -r '.[] | .name' "$file" 2>/dev/null
  fi
}

get_relayer_field() {
  local name="$1" field="$2"
  local file="$CONFIG_DIR/$CURRENT_ENV/relayers.json"
  jq -r ".[] | select(.name == \"$name\") | .$field // empty" "$file"
}

# ── Deployment Log ─────────────────────────────────────────────────────────────

append_log() {
  local msg="$1"
  local log_file="$CONFIG_DIR/$CURRENT_ENV/deploy.log"
  local ts
  ts=$(date -u '+%Y-%m-%d %H:%M:%S')
  echo "[$ts] $msg" >> "$log_file"
}

# ── Prompts ────────────────────────────────────────────────────────────────────

prompt_input() {
  local label="$1"
  local default="${2:-}"
  local validate="${3:-}"   # optional: evm_address | svm_pubkey | uint | hex64 | bytes32
  local result

  while true; do
    echo "" >&2
    if [[ -n "$default" ]]; then
      echo -e "  ${label}" >&2
      echo -e "  ${DIM}default: ${default}  (enter to accept)${NC}" >&2
      read -rp "  > " result
      result="${result:-$default}"
    else
      echo -e "  ${label} ${DIM}(enter to cancel)${NC}" >&2
      read -rp "  > " result
      if [[ -z "$result" ]]; then
        warn "Cancelled."
        return 1
      fi
    fi

    if [[ -n "$validate" && -n "$result" ]]; then
      case "$validate" in
        evm_address)
          if [[ ! "$result" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
            error "Invalid EVM address: $result"
            continue
          fi ;;
        svm_pubkey)
          if [[ ! "$result" =~ ^[1-9A-HJ-NP-Za-km-z]{32,44}$ ]]; then
            error "Invalid SVM public key: $result"
            continue
          fi ;;
        uint)
          if [[ ! "$result" =~ ^[0-9]+$ ]]; then
            error "Invalid number: $result"
            continue
          fi ;;
        hex64)
          if [[ ! "$result" =~ ^[0-9a-fA-F]{64}$ ]]; then
            error "Invalid hex (need 64 chars, no 0x prefix): $result"
            continue
          fi ;;
        bytes32)
          if [[ ! "$result" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
            error "Invalid bytes32: $result"
            continue
          fi ;;
      esac
    fi
    break
  done
  echo "$result"
}

prompt_select() {
  local label="$1"
  shift
  local options=("$@")

  echo "" >&2
  echo -e "  ${BOLD}${label}${NC}" >&2
  local i=1
  for opt in "${options[@]}"; do
    echo "    ${i}) ${opt}" >&2
    ((i++))
  done
  echo "" >&2
  local choice
  read -rp "  > " choice

  if [[ -z "$choice" ]]; then
    echo "$(( ${#options[@]} - 1 ))"
  elif [[ "$choice" -ge 1 && "$choice" -le "${#options[@]}" ]] 2>/dev/null; then
    echo "$((choice - 1))"
  else
    error "Invalid choice: $choice"
    prompt_select "$label" "${options[@]}"
  fi
}

prompt_confirm() {
  local msg="$1"
  local answer
  echo "" >&2
  read -rp "  ${msg} [y/N]: " answer
  [[ "$answer" =~ ^[Yy]$ ]]
}

# Prompt for an address, with optional auto-generation (disabled on mainnet)
prompt_address_or_gen() {
  local label="$1"
  local chain_type="$2"   # evm or svm
  local role="$3"          # guardian, operator, recovery
  local default="${4:-}"

  while true; do
    echo "" >&2
    echo -e "  ${label}" >&2
    if [[ -n "$default" ]]; then
      echo -e "  ${DIM}default: ${default}  (enter to accept)${NC}" >&2
    elif [[ "$CURRENT_ENV" != "mainnet" ]]; then
      echo -e "  ${DIM}enter to auto-generate${NC}" >&2
    fi

    local input
    read -rp "  > " input

    if [[ -z "$input" ]]; then
      if [[ -n "$default" ]]; then
        echo "$default"
        return
      elif [[ "$CURRENT_ENV" != "mainnet" ]]; then
        info "Auto-generating $role key..."
        if [[ "$chain_type" == "evm" ]]; then
          gen_evm_key "$role"
        else
          gen_svm_key "$role"
        fi
        return
      else
        error "Address required for mainnet. Cannot auto-generate."
        continue
      fi
    fi

    # Validate manual input
    if [[ "$chain_type" == "evm" ]]; then
      if [[ ! "$input" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        error "Invalid EVM address: $input"
        continue
      fi
    else
      if [[ ! "$input" =~ ^[1-9A-HJ-NP-Za-km-z]{32,44}$ ]]; then
        error "Invalid SVM public key: $input"
        continue
      fi
    fi

    echo "$input"
    return
  done
}

# ── Key Generation ─────────────────────────────────────────────────────────────

gen_evm_key() {
  local role="$1"
  local key_file="$KEYS_DIR/$CURRENT_ENV/${role}-evm.key"
  mkdir -p "$KEYS_DIR/$CURRENT_ENV"

  local output
  output=$(cast wallet new 2>&1)
  local address private_key
  address=$(echo "$output" | grep -i 'address' | head -1 | awk '{print $NF}')
  private_key=$(echo "$output" | grep -i 'private key' | head -1 | awk '{print $NF}')

  echo "$private_key" > "$key_file"
  chmod 600 "$key_file"

  success "Generated EVM key for ${role}: ${address}"
  info "Private key saved to: ${key_file}"
  echo "$address"
}

gen_svm_key() {
  local role="$1"
  local key_file="$KEYS_DIR/$CURRENT_ENV/${role}-svm.json"
  mkdir -p "$KEYS_DIR/$CURRENT_ENV"

  solana-keygen new --no-bip39-passphrase --outfile "$key_file" --force --silent >/dev/null 2>&1
  chmod 600 "$key_file"

  local pubkey
  pubkey=$(solana-keygen pubkey "$key_file" 2>/dev/null)

  success "Generated SVM key for ${role}: ${pubkey}"
  info "Keypair saved to: ${key_file}"
  echo "$pubkey"
}

# ── Validation ─────────────────────────────────────────────────────────────────

validate_evm_address() {
  local addr="$1"
  if [[ ! "$addr" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
    error "Invalid EVM address: $addr"
    return 1
  fi
}

validate_svm_pubkey() {
  local pubkey="$1"
  if [[ ! "$pubkey" =~ ^[1-9A-HJ-NP-Za-km-z]{32,44}$ ]]; then
    error "Invalid SVM public key: $pubkey"
    return 1
  fi
}

# ── EVM Helpers ────────────────────────────────────────────────────────────────

# Resolve EVM private key: from EVM_PRIVATE_KEY directly, or from EVM_PRIVATE_KEY_PATH (JSON with .private_key field)
_resolve_evm_private_key() {
  if [[ -n "${EVM_PRIVATE_KEY:-}" ]]; then
    echo "${EVM_PRIVATE_KEY}"
    return
  fi
  if [[ -n "${EVM_PRIVATE_KEY_PATH:-}" ]]; then
    local file="$EVM_PRIVATE_KEY_PATH"
    [[ -f "$file" ]] || fatal "EVM_PRIVATE_KEY_PATH not found: $file"
    local key
    key=$(jq -r '.private_key // empty' "$file" 2>/dev/null)
    if [[ -z "$key" ]]; then
      key=$(tr -d '[:space:]' < "$file")
    fi
    echo "$key"
    return
  fi
  echo ""
}

# Build cast/forge signing flags from environment config
evm_sign_flags() {
  if [[ -n "${EVM_LEDGER:-}" && "${EVM_LEDGER}" == "true" ]]; then
    echo "--ledger"
  elif [[ -n "${EVM_KEYSTORE_PATH:-}" ]]; then
    echo "--keystore ${EVM_KEYSTORE_PATH}"
  else
    local pk
    pk=$(_resolve_evm_private_key)
    if [[ -n "$pk" ]]; then
      echo "--private-key ${pk}"
    else
      fatal "No EVM signing method configured. Set EVM_PRIVATE_KEY_PATH, EVM_PRIVATE_KEY, EVM_KEYSTORE_PATH, or EVM_LEDGER in config/$CURRENT_ENV/.env"
    fi
  fi
}

# Simulate a contract call via cast call before sending
evm_simulate() {
  local rpc="$1" contract="$2" sig="$3"
  shift 3
  local args=("$@")

  info "Simulating: ${sig}..."
  local from_addr
  from_addr=$(evm_signer_address) || true
  local from_flag=()
  if [[ -n "$from_addr" ]]; then
    from_flag=(--from "$from_addr")
  fi
  local result
  if result=$(cast call --rpc-url "$rpc" "${from_flag[@]}" "$contract" "$sig" "${args[@]}" 2>&1); then
    success "Simulation passed"
    return 0
  else
    error "Simulation failed: $result"
    return 1
  fi
}

# Read a value from a deployed contract
evm_read() {
  local rpc="$1" contract="$2" sig="$3"
  shift 3
  cast call --rpc-url "$rpc" "$contract" "$sig" "$@" 2>/dev/null | sed 's/ \[.*\]$//'
}

# Send a transaction
evm_send() {
  local rpc="$1" contract="$2" sig="$3"
  shift 3
  local sign_flags
  sign_flags=$(evm_sign_flags)

  info "Sending tx: ${sig}..."
  # shellcheck disable=SC2086
  cast send --rpc-url "$rpc" $sign_flags "$contract" "$sig" "$@"
}

# Get signer address from configured signing method
evm_signer_address() {
  if [[ -n "${EVM_LEDGER:-}" && "${EVM_LEDGER}" == "true" ]]; then
    cast wallet address --ledger 2>/dev/null
  elif [[ -n "${EVM_KEYSTORE_PATH:-}" ]]; then
    cast wallet address --keystore "${EVM_KEYSTORE_PATH}" 2>/dev/null
  else
    local pk
    pk=$(_resolve_evm_private_key)
    if [[ -n "$pk" ]]; then
      cast wallet address --private-key "${pk}" 2>/dev/null
    else
      echo ""
    fi
  fi
}

# Check that the connected chain ID matches expectations
evm_check_chain_id() {
  local rpc="$1" expected_id="$2"
  local actual
  actual=$(cast chain-id --rpc-url "$rpc" 2>/dev/null || echo "")
  if [[ -z "$actual" ]]; then
    error "Cannot connect to RPC: $rpc"
    return 1
  fi
  if [[ "$actual" != "$expected_id" ]]; then
    error "Chain ID mismatch: expected $expected_id, got $actual"
    return 1
  fi
  success "Chain ID: $actual"
}

evm_check_balance() {
  local rpc="$1"
  local signer
  signer=$(evm_signer_address)
  if [[ -z "$signer" ]]; then
    warn "Cannot determine signer address to check balance"
    return 0
  fi
  local balance
  balance=$(cast balance --ether --rpc-url "$rpc" "$signer" 2>/dev/null || echo "unknown")
  info "Signer:   $signer"
  info "Balance:  ${balance} ETH"
}

# ── Display ────────────────────────────────────────────────────────────────────

print_header() {
  echo ""
  echo -e "  ${BOLD}╔═══════════════════════════════════╗${NC}"
  echo -e "  ${BOLD}║     Bridge1024 Deploy Tool        ║${NC}"
  echo -e "  ${BOLD}╚═══════════════════════════════════╝${NC}"
  echo ""
}

print_summary() {
  local title="$1"
  shift
  echo ""
  echo -e "  ${BOLD}── ${title} ──${NC}"
  while [[ $# -ge 2 ]]; do
    printf "  %-24s %s\n" "$1:" "$2"
    shift 2
  done
  echo ""
}

print_verification() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    echo -e "  ${GREEN}✓${NC} ${label}: ${actual}"
  else
    echo -e "  ${RED}✗${NC} ${label}: expected ${expected}, got ${actual}"
  fi
}

print_tx_result() {
  local chain_key="$1" tx_hash="$2"
  local explorer
  explorer=$(get_explorer_url "$chain_key")
  echo ""
  success "Transaction hash: ${tx_hash}"
  if [[ -n "$explorer" ]]; then
    info "Explorer: ${explorer}/tx/${tx_hash}"
  fi
}
